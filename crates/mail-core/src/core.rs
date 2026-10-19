//! The app facade (section 3): every command `app` exposes in M4, the sync actors, and the
//! events the UI listens to. `app` holds no logic; it calls these methods and forwards
//! [`CoreEvent`]s.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use mail_auth::{HttpGet, KeychainLogins};
use mail_imap::ImapConnector;
use mail_secrets::SecretStore;
use mail_smtp::SmtpCheck;
use mail_store::{AccountId, FolderId, MessageId, Store, StoreError};
use mail_sync::{
    spawn_account, AccountHandle, AccountState, Events, SyncDeps, SyncError, SyncEvent,
};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::accounts::{AccountConfig, AccountSummary, Accounts, Candidate, TestReport};
use crate::error::{AppError, ErrorCode};
use crate::views::{self, ListRequest, MessageView, PageView, RowView, Scope};

/// `store-changed` is debounced to this window (section 3).
pub const STORE_CHANGED_DEBOUNCE: Duration = Duration::from_millis(100);
/// The background integrity check starts this long after startup (SPEC_GAPS G48).
pub const QUICK_CHECK_DELAY: Duration = Duration::from_secs(10);

const DB_FILE: &str = "mail.db";
/// Written when a background check fails; the next start moves the database aside.
const REBUILD_MARKER: &str = "rebuild-database";

/// An account state as the UI names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateName {
    Disabled,
    Connecting,
    Syncing,
    Idle,
    Backoff,
    NeedsAttention,
}

impl From<AccountState> for StateName {
    fn from(s: AccountState) -> Self {
        match s {
            AccountState::Disabled => Self::Disabled,
            AccountState::Connecting => Self::Connecting,
            AccountState::Syncing => Self::Syncing,
            AccountState::Idle => Self::Idle,
            AccountState::Backoff => Self::Backoff,
            AccountState::NeedsAttention => Self::NeedsAttention,
        }
    }
}

fn sync_error(e: &SyncError) -> AppError {
    match e {
        SyncError::Imap(e) => e.clone().into(),
        SyncError::Auth(e) => e.clone().into(),
        SyncError::Store(msg) => AppError::new(ErrorCode::Internal, msg.clone()),
        SyncError::Gone => AppError::new(ErrorCode::NotFound, "gone"),
    }
}

/// `account.list` rows (SPEC_GAPS G46): the summary plus the live state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountView {
    #[serde(flatten)]
    pub summary: AccountSummary,
    pub state: StateName,
    pub last_error: Option<AppError>,
}

/// Events for the UI (section 3, event table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum CoreEvent {
    StoreChanged {
        account_id: AccountId,
        folder_id: Option<FolderId>,
    },
    AccountState {
        account_id: AccountId,
        state: StateName,
        last_error: Option<AppError>,
    },
    SyncProgress {
        account_id: AccountId,
        folder_id: FolderId,
        fetched: u32,
        total: u32,
    },
    FolderReset {
        account_id: AccountId,
        folder_id: FolderId,
    },
    StorageWarning {
        kind: &'static str,
    },
}

impl CoreEvent {
    /// The event name the UI listens for.
    pub fn name(&self) -> &'static str {
        match self {
            Self::StoreChanged { .. } => "store-changed",
            Self::AccountState { .. } => "account-state",
            Self::SyncProgress { .. } => "sync-progress",
            Self::FolderReset { .. } => "folder-reset",
            Self::StorageWarning { .. } => "storage-warning",
        }
    }
}

pub type EventSink = Arc<dyn Fn(CoreEvent) + Send + Sync>;

/// What [`Core::start`] needs. The app passes the real network and keychain; tests pass
/// fakes.
pub struct CoreConfig<I, S, H> {
    pub data_dir: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    pub imap: I,
    pub smtp: S,
    pub http: H,
}

impl<I, S, H> std::fmt::Debug for CoreConfig<I, S, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreConfig")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

type States = Arc<Mutex<HashMap<AccountId, (StateName, Option<AppError>)>>>;

pub struct Core<I, S, H> {
    store: Store,
    accounts: Accounts<I, S, H>,
    deps: SyncDeps<I, KeychainLogins>,
    actors: tokio::sync::Mutex<HashMap<AccountId, AccountHandle>>,
    states: States,
    sink: EventSink,
    /// The delayed integrity check; cancelled at shutdown.
    background: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl<I, S, H> std::fmt::Debug for Core<I, S, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core").finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Moves a database (and its WAL files) aside as `*.corrupt-<time>` (section 12).
fn move_aside(db: &Path) -> std::io::Result<()> {
    let stamp = crate::accounts::now();
    for suffix in ["", "-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", db.display()));
        if from.exists() {
            let to = PathBuf::from(format!("{}.corrupt-{stamp}{suffix}", db.display()));
            std::fs::rename(&from, &to)?;
        }
    }
    Ok(())
}

/// Opens the database. A file SQLite cannot use, or one a background check marked, is moved
/// aside and replaced by a fresh one; the flag says so, for `storage-warning` (G48).
fn open_store(data_dir: &Path) -> Result<(Store, bool), AppError> {
    std::fs::create_dir_all(data_dir).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))?;
    let db = data_dir.join(DB_FILE);
    let marker = data_dir.join(REBUILD_MARKER);
    let io = |e: std::io::Error| AppError::new(ErrorCode::Io, e.to_string());
    let mut rebuilt = false;
    if marker.exists() {
        move_aside(&db).map_err(io)?;
        std::fs::remove_file(&marker).map_err(io)?;
        rebuilt = true;
    }
    match Store::open(&db) {
        Ok(store) => Ok((store, rebuilt)),
        Err(StoreError::Corrupt) => {
            move_aside(&db).map_err(io)?;
            Ok((Store::open(&db)?, true))
        }
        Err(e) => Err(e.into()),
    }
}

impl<I, S, H> Core<I, S, H>
where
    I: ImapConnector + Clone + 'static,
    S: SmtpCheck,
    H: HttpGet,
{
    /// Opens the database (section 12 open path), starts one actor per account and the
    /// event pump, and schedules the background integrity check.
    pub async fn start(
        config: CoreConfig<I, S, H>,
        sink: EventSink,
    ) -> Result<Arc<Self>, AppError> {
        let data_dir = config.data_dir.clone();
        let (store, rebuilt) = tokio::task::spawn_blocking(move || open_store(&data_dir))
            .await
            .map_err(|_| AppError::new(ErrorCode::Internal, "startup task failed"))??;
        let (tx, rx) = mpsc::unbounded_channel();
        let states: States = Arc::default();
        let deps = SyncDeps {
            store: store.clone(),
            connector: Arc::new(config.imap.clone()),
            logins: Arc::new(KeychainLogins::new(Arc::clone(&config.secrets))),
            events: Events::new(tx),
        };
        let core = Arc::new(Self {
            accounts: Accounts::new(
                store.clone(),
                Arc::clone(&config.secrets),
                config.imap,
                config.smtp,
                config.http,
            ),
            store,
            deps,
            actors: tokio::sync::Mutex::default(),
            states: Arc::clone(&states),
            sink: Arc::clone(&sink),
            background: Mutex::default(),
        });
        tokio::spawn(pump(rx, Arc::clone(&sink), states));
        if rebuilt {
            sink(CoreEvent::StorageWarning { kind: "db_corrupt" });
        }
        for account in core.store.list_accounts().await? {
            core.start_actor(account.id).await;
        }
        let checker = core.store.clone();
        let marker = config.data_dir.join(REBUILD_MARKER);
        let check = tokio::spawn(async move {
            tokio::time::sleep(QUICK_CHECK_DELAY).await;
            if let Err(StoreError::Corrupt) = checker.quick_check().await {
                let _ = std::fs::write(&marker, b"quick_check failed\n");
                sink(CoreEvent::StorageWarning { kind: "db_corrupt" });
            }
        });
        lock(&core.background).push(check);
        Ok(core)
    }

    async fn start_actor(&self, account_id: AccountId) {
        let handle = spawn_account(self.deps.clone(), account_id);
        if let Some(old) = self.actors.lock().await.insert(account_id, handle) {
            old.abort();
        }
    }

    async fn with_actor(&self, account_id: AccountId, f: impl FnOnce(&AccountHandle)) {
        if let Some(h) = self.actors.lock().await.get(&account_id) {
            f(h);
        }
    }

    /// `account.list` (G46).
    pub async fn account_list(&self) -> Result<Vec<AccountView>, AppError> {
        let states = lock(&self.states).clone();
        Ok(self
            .accounts
            .list()
            .await?
            .into_iter()
            .map(|summary| {
                let (state, last_error) = states.get(&summary.id).cloned().unwrap_or((
                    if summary.enabled {
                        StateName::Connecting
                    } else {
                        StateName::Disabled
                    },
                    None,
                ));
                AccountView {
                    summary,
                    state,
                    last_error,
                }
            })
            .collect())
    }

    pub async fn account_resolve(&self, email: &str) -> Result<Vec<Candidate>, AppError> {
        self.accounts.resolve(email).await
    }

    pub async fn account_test(&self, config: &AccountConfig) -> Result<TestReport, AppError> {
        self.accounts.test(config).await
    }

    pub async fn account_add(&self, config: AccountConfig) -> Result<AccountSummary, AppError> {
        let summary = self.accounts.add(config).await?;
        self.start_actor(summary.id).await;
        Ok(summary)
    }

    /// Also how the user fixes `NeedsAttention`: the actor reconnects with the new settings.
    pub async fn account_update(
        &self,
        account_id: AccountId,
        config: AccountConfig,
    ) -> Result<AccountSummary, AppError> {
        let summary = self.accounts.update(account_id, config).await?;
        self.with_actor(account_id, AccountHandle::account_changed)
            .await;
        Ok(summary)
    }

    pub async fn account_remove(&self, account_id: AccountId) -> Result<(), AppError> {
        let actor = self.actors.lock().await.remove(&account_id);
        if let Some(actor) = actor {
            actor.shutdown().await;
        }
        lock(&self.states).remove(&account_id);
        self.accounts.remove(account_id).await
    }

    pub async fn account_set_enabled(
        &self,
        account_id: AccountId,
        enabled: bool,
    ) -> Result<(), AppError> {
        self.accounts.set_enabled(account_id, enabled).await?;
        self.with_actor(account_id, AccountHandle::account_changed)
            .await;
        Ok(())
    }

    /// `folder.list`: the folder tree with counts and roles.
    pub async fn folder_list(
        &self,
        account_id: AccountId,
    ) -> Result<Vec<views::FolderView>, AppError> {
        let folders = self.store.list_folders(account_id).await?;
        Ok(views::folder_views(&folders))
    }

    /// `folder.refresh`.
    pub async fn folder_refresh(
        &self,
        account_id: AccountId,
        folder_id: FolderId,
    ) -> Result<(), AppError> {
        self.with_actor(account_id, |h| h.refresh(folder_id)).await;
        Ok(())
    }

    /// `list.page`. A first page (no cursor) also tells the actor the folder is open, so it
    /// syncs now and every 5 minutes (section 6, step 9).
    pub async fn list_page(&self, request: ListRequest) -> Result<PageView, AppError> {
        let folder_id = match request.scope {
            Scope::Folder { folder_id } => folder_id,
            Scope::Unified | Scope::Search { .. } | Scope::Thread { .. } => {
                return Err(AppError::new(ErrorCode::Unsupported, "arrives in M9"));
            }
        };
        if request.group_by_thread {
            return Err(AppError::new(
                ErrorCode::Unsupported,
                "threads arrive in M9",
            ));
        }
        if request.cursor.is_none() {
            let folder = self.store.get_folder(folder_id).await?;
            self.with_actor(folder.account_id, |h| h.open_folder(Some(folder_id)))
                .await;
        }
        let page = self
            .store
            .folder_page(
                folder_id,
                request.cursor.map(Into::into),
                request.limit.clamp(1, 500),
            )
            .await?;
        Ok(PageView {
            rows: page.rows.iter().map(RowView::from).collect(),
            next: page.next.map(Into::into),
        })
    }

    /// `message.open`. An uncached body is fetched ahead of everything else; the reader
    /// shows Loading until `store-changed` brings it.
    pub async fn message_open(&self, message_id: MessageId) -> Result<MessageView, AppError> {
        let message = self.store.get_message(message_id).await?;
        let attachments = self.store.list_attachments(message_id).await?;
        if message.body_state == mail_store::BodyState::None {
            self.with_actor(message.account_id, |h| h.fetch_body(message_id))
                .await;
        }
        Ok(views::message_view(&message, &attachments))
    }

    /// The `mail-body` protocol response.
    pub async fn mail_body(
        &self,
        message_id: MessageId,
        images: bool,
    ) -> Result<mail_render::BodyResponse, AppError> {
        mail_render::mail_body(&self.store, message_id, images)
            .await
            .map_err(|mail_render::RenderError::Store(e)| AppError::from(e))
    }

    /// Stops every actor and background task (app quit).
    pub async fn shutdown(&self) {
        for task in lock(&self.background).drain(..) {
            task.abort();
        }
        let actors: Vec<AccountHandle> = self.actors.lock().await.drain().map(|(_, h)| h).collect();
        for a in actors {
            a.shutdown().await;
        }
    }

    pub fn emit(&self, event: CoreEvent) {
        (self.sink)(event);
    }
}

/// Forwards sync events to the UI. `store-changed` is coalesced per (account, folder) into
/// 100 ms windows: the first change opens a window and every key changed in it is sent once
/// when it closes, so a long sync updates the list at most ten times a second.
async fn pump(mut rx: mpsc::UnboundedReceiver<SyncEvent>, sink: EventSink, states: States) {
    let mut pending: BTreeSet<(AccountId, Option<FolderId>)> = BTreeSet::new();
    let mut deadline: Option<Instant> = None;
    loop {
        let next = match deadline {
            Some(at) => tokio::select! {
                e = rx.recv() => e,
                () = tokio::time::sleep_until(at) => {
                    for (account_id, folder_id) in std::mem::take(&mut pending) {
                        sink(CoreEvent::StoreChanged { account_id, folder_id });
                    }
                    deadline = None;
                    continue;
                }
            },
            None => rx.recv().await,
        };
        let Some(event) = next else {
            for (account_id, folder_id) in std::mem::take(&mut pending) {
                sink(CoreEvent::StoreChanged {
                    account_id,
                    folder_id,
                });
            }
            return;
        };
        match event {
            SyncEvent::StoreChanged {
                account_id,
                folder_id,
            } => {
                pending.insert((account_id, folder_id));
                deadline.get_or_insert_with(|| Instant::now() + STORE_CHANGED_DEBOUNCE);
            }
            SyncEvent::AccountState {
                account_id,
                state,
                last_error,
                ..
            } => {
                let state = StateName::from(state);
                let error = last_error.as_ref().map(sync_error);
                lock(&states).insert(account_id, (state, error.clone()));
                sink(CoreEvent::AccountState {
                    account_id,
                    state,
                    last_error: error,
                });
            }
            SyncEvent::SyncProgress {
                account_id,
                folder_id,
                fetched,
                total,
            } => sink(CoreEvent::SyncProgress {
                account_id,
                folder_id,
                fetched,
                total,
            }),
            SyncEvent::FolderReset {
                account_id,
                folder_id,
            } => sink(CoreEvent::FolderReset {
                account_id,
                folder_id,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn store_changed_is_coalesced_into_100_ms_windows() {
        let (tx, rx) = mpsc::unbounded_channel();
        let seen: Arc<Mutex<Vec<(Instant, CoreEvent)>>> = Arc::default();
        let log = Arc::clone(&seen);
        let sink: EventSink = Arc::new(move |e| lock(&log).push((Instant::now(), e)));
        let start = Instant::now();
        tokio::spawn(pump(rx, sink, Arc::default()));
        for _ in 0..50 {
            tx.send(SyncEvent::StoreChanged {
                account_id: 1,
                folder_id: Some(2),
            })
            .unwrap();
            tx.send(SyncEvent::StoreChanged {
                account_id: 1,
                folder_id: None,
            })
            .unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let seen = lock(&seen).clone();
        // 250 ms of changes: three windows, each sending both keys once.
        assert_eq!(seen.len(), 6, "{seen:?}");
        // Each window opens at the first change after the last flush and closes 100 ms
        // later: flushes at 100, 205 and 310 ms, never closer than 100 ms.
        let flushes: Vec<Duration> = seen.chunks(2).map(|pair| pair[0].0 - start).collect();
        assert_eq!(flushes[0], STORE_CHANGED_DEBOUNCE);
        for w in flushes.windows(2) {
            assert!(w[1] - w[0] >= STORE_CHANGED_DEBOUNCE, "{flushes:?}");
        }
        for pair in seen.chunks(2) {
            assert_eq!(pair[0].0, pair[1].0, "both keys go in the same flush");
        }
        assert!(seen.iter().all(|(_, e)| e.name() == "store-changed"));
    }

    #[tokio::test]
    async fn state_events_are_forwarded_and_remembered() {
        let (tx, rx) = mpsc::unbounded_channel();
        let seen: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
        let log = Arc::clone(&seen);
        let states: States = Arc::default();
        let sink: EventSink = Arc::new(move |e| lock(&log).push(e));
        let pumped = tokio::spawn(pump(rx, sink, Arc::clone(&states)));
        tx.send(SyncEvent::AccountState {
            account_id: 4,
            state: AccountState::NeedsAttention,
            last_error: Some(SyncError::Imap(mail_imap::ImapError::AuthFailed)),
            retry_in: None,
        })
        .unwrap();
        drop(tx);
        pumped.await.unwrap();
        let seen = lock(&seen).clone();
        let json = serde_json::to_value(&seen[0]).unwrap();
        assert_eq!(seen[0].name(), "account-state");
        assert_eq!(json["state"], "needs_attention");
        assert_eq!(json["last_error"]["code"], "auth_failed");
        assert_eq!(lock(&states).get(&4).unwrap().0, StateName::NeedsAttention);
    }
}
