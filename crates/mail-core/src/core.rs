//! The app facade (section 3): every command `app` exposes, the sync actors, the custom
//! protocols and the events the UI listens to. `app` holds no logic; it calls these methods
//! and forwards [`CoreEvent`]s.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use mail_auth::{HttpGet, HttpPost, KeychainLogins, LoginSource};
use mail_imap::ImapConnector;
use mail_render::{
    AttachmentCache, BodyRequest, BodyResponse, CacheKey, RemoteProxy, TempFiles, UrlForm,
};
use mail_secrets::SecretStore;
use mail_smtp::SmtpCheck;
use mail_store::{
    AccountId, AttachmentId, AttachmentRecord, FolderId, MessageId, Store, StoreError,
};
use mail_sync::{
    spawn_account, AccountHandle, AccountState, Events, SyncDeps, SyncError, SyncEvent,
};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::accounts::{AccountConfig, AccountSummary, Accounts, Candidate, TestReport};
use crate::error::{AppError, ErrorCode};
use crate::views::{self, BodyInfo, MessageView, RemoteImages};

/// `store-changed` is debounced to this window (section 3).
pub const STORE_CHANGED_DEBOUNCE: Duration = Duration::from_millis(100);
/// The background integrity check starts this long after startup (SPEC_GAPS G48).
pub const QUICK_CHECK_DELAY: Duration = Duration::from_secs(10);

const DB_FILE: &str = "mail.db";
/// The attachment byte cache (invariant 3; SPEC_GAPS G57).
const CACHE_DIR: &str = "attachments";
/// Attachments the user opens (section 13.5; SPEC_GAPS G57).
const TEMP_DIR: &str = "tmp";
/// Schemes `link.open` hands to the OS (section 8, Links; SPEC_GAPS G52). `mailto` opens
/// compose instead.
const LINK_SCHEMES: &[&str] = &["http", "https", "tel"];
/// Written when a background check fails; the next start moves the database aside.
const REBUILD_MARKER: &str = "rebuild-database";

/// The HTTP the core needs: autoconfig lookups and Google's token endpoint.
pub trait Http: HttpGet + HttpPost + Clone + 'static {}

impl<T: HttpGet + HttpPost + Clone + 'static> Http for T {}

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

pub(crate) fn sync_error(e: &SyncError) -> AppError {
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
    /// Every outbox transition (section 3), plus `undone` and `discarded`.
    OutboxState {
        outbox_id: i64,
        account_id: AccountId,
        state: &'static str,
    },
    /// Unfinished ops and outbox items (section 3).
    PendingCount {
        total: u32,
    },
    /// A queued action the server refused for good; it was reverted (SPEC_GAPS G58).
    OpFailed {
        account_id: AccountId,
        op_id: i64,
        kind: String,
        error: AppError,
    },
    /// A snooze came due and the message went back (section 3, 18.2).
    SnoozeDue {
        account_id: AccountId,
        message_id: MessageId,
    },
    /// A browser sign-in ended: tokens saved and Test connection passed, or why not
    /// (section 3; SPEC_GAPS G71).
    OauthDone {
        account_id: AccountId,
        ok: bool,
        error: Option<AppError>,
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
            Self::PendingCount { .. } => "pending-count",
            Self::OutboxState { .. } => "outbox-state",
            Self::OpFailed { .. } => "op-failed",
            Self::OauthDone { .. } => "oauth-done",
            Self::SnoozeDue { .. } => "snooze-due",
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
    pub(crate) store: Store,
    accounts: Accounts<I, S, H>,
    pub(crate) deps: SyncDeps<I, KeychainLogins<H>>,
    /// The browser sign-in waiting for its redirect, per account (section 5).
    sign_ins: Mutex<HashMap<AccountId, tokio::task::JoinHandle<()>>>,
    /// Message-IDs this app moved into an INBOX or watched folder (Undo, a move back), and
    /// when: arriving there they are not new mail, so rules do not file them again and they
    /// do not notify (section 18.6; SPEC_GAPS G77).
    pub(crate) moved_in: Mutex<HashMap<String, Instant>>,
    /// New messages whose rules wait for the body (section 18.6).
    pub(crate) rules_waiting: Mutex<HashSet<MessageId>>,
    /// Submission for the outbox (section 7).
    pub(crate) smtp: S,
    /// The undo window and upload delay (section 7).
    timing: Mutex<crate::compose::Timing>,
    /// One pending draft upload per draft (section 7, Drafts).
    pub(crate) upload_timers: Mutex<HashMap<mail_store::DraftId, tokio::task::JoinHandle<()>>>,
    /// Wakes the outbox runner when something is queued.
    pub(crate) outbox_wake: Arc<tokio::sync::Notify>,
    /// This core, for tasks that call back into it.
    pub(crate) me: std::sync::OnceLock<std::sync::Weak<Self>>,
    pub(crate) actors: tokio::sync::Mutex<HashMap<AccountId, AccountHandle>>,
    states: States,
    pub(crate) sink: EventSink,
    /// The delayed integrity check; cancelled at shutdown.
    background: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    pub(crate) data_dir: PathBuf,
    form: UrlForm,
    proxy: RemoteProxy,
    cache: AttachmentCache,
    temp: TempFiles,
    /// Load images, for this session only (section 8, Remote images).
    images_once: Mutex<HashSet<MessageId>>,
    /// Bodies the server could not deliver: Could not display this message (section 12).
    unreadable: Arc<Mutex<HashSet<MessageId>>>,
}

impl<I, S, H> std::fmt::Debug for Core<I, S, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core").finish_non_exhaustive()
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
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
    S: SmtpCheck + mail_smtp::SmtpSubmit + Clone + 'static,
    H: crate::core::Http,
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
        let debug = matches!(
            store.get_setting(crate::logging::DEBUG_SETTING).await?,
            Some(serde_json::Value::Bool(true))
        );
        crate::logging::set_debug(debug);
        let temp = TempFiles::new(config.data_dir.join(TEMP_DIR));
        // Files opened in an earlier run are deleted at the next start (section 13.5).
        if let Err(e) = temp.clear() {
            tracing::warn!(error = %e, "could not clear the temp folder");
        }
        let proxy =
            RemoteProxy::new().map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?;
        let (tx, rx) = mpsc::unbounded_channel();
        let states: States = Arc::default();
        let deps = SyncDeps {
            store: store.clone(),
            connector: Arc::new(config.imap.clone()),
            logins: Arc::new(KeychainLogins::new(
                Arc::clone(&config.secrets),
                config.http.clone(),
            )),
            events: Events::new(tx),
        };
        // A crash mid-send leaves `sending`: never retried, the user decides (section 7).
        for id in store.sending_to_unknown().await? {
            tracing::warn!(outbox_id = id, "a send was interrupted; marked unknown");
        }
        let core = Arc::new(Self {
            accounts: Accounts::new(
                store.clone(),
                Arc::clone(&config.secrets),
                config.imap,
                config.smtp.clone(),
                config.http,
            ),
            smtp: config.smtp,
            timing: Mutex::default(),
            upload_timers: Mutex::default(),
            sign_ins: Mutex::default(),
            moved_in: Mutex::default(),
            rules_waiting: Mutex::default(),
            outbox_wake: Arc::default(),
            me: std::sync::OnceLock::new(),
            store,
            deps,
            actors: tokio::sync::Mutex::default(),
            states: Arc::clone(&states),
            sink: Arc::clone(&sink),
            background: Mutex::default(),
            data_dir: config.data_dir.clone(),
            form: UrlForm::native(),
            proxy,
            cache: AttachmentCache::new(
                config.data_dir.join(CACHE_DIR),
                mail_render::files::CACHE_CAP,
            ),
            temp,
            images_once: Mutex::default(),
            unreadable: Arc::default(),
        });
        let (follow_tx, follow_rx) = mpsc::unbounded_channel();
        tokio::spawn(pump(
            rx,
            Arc::clone(&sink),
            states,
            Some(core.store.clone()),
            Some(follow_tx),
        ));
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
        let _ = core.me.set(Arc::downgrade(&core));
        let runner = tokio::spawn(outbox_runner(
            Arc::downgrade(&core),
            Arc::clone(&core.outbox_wake),
        ));
        lock(&core.background).push(runner);
        let features = tokio::spawn(crate::features::runner(Arc::downgrade(&core), follow_rx));
        lock(&core.background).push(features);
        Ok(core)
    }

    /// The undo window and upload delay in use.
    pub fn timing(&self) -> crate::compose::Timing {
        *lock(&self.timing)
    }

    /// Changes them (tests use short ones).
    pub fn set_timing(&self, timing: crate::compose::Timing) {
        *lock(&self.timing) = timing;
        self.outbox_wake.notify_one();
    }

    async fn start_actor(&self, account_id: AccountId) {
        let handle = spawn_account(self.deps.clone(), account_id);
        if let Some(old) = self.actors.lock().await.insert(account_id, handle) {
            old.abort();
        }
    }

    pub(crate) async fn with_actor(&self, account_id: AccountId, f: impl FnOnce(&AccountHandle)) {
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

    /// Removes the account's rows, keychain entries and cached files (section 5).
    pub async fn account_remove(&self, account_id: AccountId) -> Result<(), AppError> {
        let actor = self.actors.lock().await.remove(&account_id);
        if let Some(actor) = actor {
            actor.shutdown().await;
        }
        if let Some(task) = lock(&self.sign_ins).remove(&account_id) {
            task.abort();
        }
        self.deps.logins.forget(account_id);
        lock(&self.states).remove(&account_id);
        self.accounts.remove(account_id).await?;
        let cache = self.cache.clone();
        blocking(move || cache.remove_account(account_id)).await
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

    /// `account.oauth_start`: listens on the loopback redirect and returns the consent URL
    /// for the app to open in the system browser. The result arrives as `oauth-done`: the
    /// code is exchanged, the refresh token goes to the keychain, Test connection runs with
    /// the new access token, and the actor reconnects (section 5; SPEC_GAPS G71). A second
    /// start for the same account replaces the first.
    pub async fn account_oauth_start(&self, account_id: AccountId) -> Result<String, AppError> {
        let account = self.store.get_account(account_id).await?;
        if account.config.auth_kind != mail_store::AuthKind::OauthGoogle {
            return Err(
                AppError::invalid("this account does not use Google sign-in")
                    .for_account(account_id),
            );
        }
        let client = self
            .deps
            .logins
            .google_client(account_id, &account.config.oauth_client_id)
            .await
            .map_err(|e| AppError::from(e).for_account(account_id))?;
        let sign_in = mail_auth::oauth::begin(&client, &account.config.email)
            .await
            .map_err(|e| AppError::from(e).for_account(account_id))?;
        let url = sign_in.url().to_string();
        let me = self.me.get().cloned();
        let logins = Arc::clone(&self.deps.logins);
        let task = tokio::spawn(async move {
            let timeout = mail_auth::oauth::SIGN_IN_TIMEOUT;
            let result = mail_auth::oauth::finish(sign_in, &client, logins.http(), timeout).await;
            if let Some(core) = me.and_then(|w| w.upgrade()) {
                core.oauth_finished(account_id, result).await;
            }
        });
        if let Some(old) = lock(&self.sign_ins).insert(account_id, task) {
            old.abort();
        }
        Ok(url)
    }

    async fn oauth_finished(
        &self,
        account_id: AccountId,
        result: Result<mail_auth::Tokens, mail_auth::AuthError>,
    ) {
        let outcome = self.oauth_outcome(account_id, result).await;
        if let Err(e) = &outcome {
            tracing::warn!(account_id, code = ?e.code, "Google sign-in did not complete");
        }
        self.emit(CoreEvent::OauthDone {
            account_id,
            ok: outcome.is_ok(),
            error: outcome.err(),
        });
        lock(&self.sign_ins).remove(&account_id);
    }

    async fn oauth_outcome(
        &self,
        account_id: AccountId,
        result: Result<mail_auth::Tokens, mail_auth::AuthError>,
    ) -> Result<(), AppError> {
        let err = |e: AppError| e.for_account(account_id);
        let tokens = result.map_err(|e| err(e.into()))?;
        self.deps
            .logins
            .signed_in(account_id, tokens)
            .await
            .map_err(|e| err(e.into()))?;
        // Signed in: the actor leaves NeedsAttention and connects with the new token,
        // whatever the test below finds.
        self.with_actor(account_id, AccountHandle::account_changed)
            .await;
        let account = self.store.get_account(account_id).await?;
        let login = self
            .deps
            .logins
            .login(&mail_sync::login_request(&account))
            .await
            .map_err(|e| err(e.into()))?;
        let report = self.accounts.test_saved(account_id, &login).await?;
        match report.imap_error.or(report.smtp_error) {
            Some(e) => Err(err(e)),
            None => Ok(()),
        }
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

    /// `message.open`. An uncached body is fetched ahead of everything else; the reader
    /// shows Loading until `store-changed` brings it. A body the server cannot deliver turns
    /// into Could not display this message (section 12).
    pub async fn message_open(&self, message_id: MessageId) -> Result<MessageView, AppError> {
        let message = self.store.get_message(message_id).await?;
        let attachments = self.store.list_attachments(message_id).await?;
        let body = self.store.get_body(message_id).await?;
        let unreadable = lock(&self.unreadable).contains(&message_id);
        if message.body_state == mail_store::BodyState::None && !unreadable {
            let rx = self
                .actors
                .lock()
                .await
                .get(&message.account_id)
                .map(|h| h.fetch_body(message_id));
            if let Some(rx) = rx {
                let failed = Arc::clone(&self.unreadable);
                let sink = Arc::clone(&self.sink);
                let (account_id, folder_id) = (message.account_id, message.folder_id);
                tokio::spawn(async move {
                    // Offline or a dropped connection is not a broken message; only a
                    // permanent refusal of this message is.
                    if let Ok(Err(e)) = rx.await {
                        if matches!(e, SyncError::Imap(ref i) if i.class() == mail_imap::ErrorClass::Permanent)
                        {
                            lock(&failed).insert(message_id);
                            sink(CoreEvent::StoreChanged {
                                account_id,
                                folder_id: Some(folder_id),
                            });
                        }
                    }
                });
            }
        }
        let has_html = body
            .as_ref()
            .is_some_and(|b| !b.text_html.trim().is_empty());
        let remote = match &body {
            Some(b) if has_html && mail_render::remote_image_count(b, self.form) > 0 => {
                if self.images_allowed(&message).await? {
                    RemoteImages::Allowed
                } else {
                    RemoteImages::Blocked
                }
            }
            _ => RemoteImages::None,
        };
        let folders = self.store.list_folders(message.account_id).await?;
        let here = folders.iter().find(|f| f.id == message.folder_id);
        let info = BodyInfo {
            has_html,
            remote_images: remote,
            unreadable,
            can_archive: crate::mailbox::archive_folder(&folders)
                .is_some_and(|a| Some(a.id) != here.map(|h| h.id)),
            delete_is_permanent: here
                .is_none_or(|h| crate::mailbox::delete_is_permanent(&folders, h)),
        };
        let mut view = views::message_view(&message, &attachments, info);
        if let Some(stored) = self.store.gm_labels(message_id).await? {
            // The labels are the folders the message is in (section 18.1), read from the
            // rows, so a label just added or removed shows at once.
            let copies = self.store.copies(message_id).await?;
            let mut labels: Vec<views::LabelView> = copies
                .iter()
                .filter_map(|c| folders.iter().find(|f| f.id == c.folder_id))
                .filter(|f| f.special_use != Some(mail_store::SpecialUse::All))
                .map(|f| views::LabelView {
                    folder_id: f.id,
                    name: views::folder_name(f),
                })
                .collect();
            labels.sort_by_key(|l| l.name.to_lowercase());
            labels.dedup_by_key(|l| l.folder_id);
            view.labels = Some(labels);
            view.important = stored.iter().any(|l| l.eq_ignore_ascii_case(r"\Important"));
        }
        Ok(view)
    }

    /// Images load for a message the user allowed once, or whose sender is on the
    /// account's allowlist (section 8).
    async fn images_allowed(&self, message: &mail_store::MessageRecord) -> Result<bool, AppError> {
        if lock(&self.images_once).contains(&message.id) {
            return Ok(true);
        }
        if message.from.addr.is_empty() {
            return Ok(false);
        }
        Ok(self
            .store
            .remote_images_allowed(message.account_id, &message.from.addr)
            .await?)
    }

    /// `images.allow_once`: Load images, for this message and this session.
    pub async fn images_allow_once(&self, message_id: MessageId) -> Result<(), AppError> {
        self.store.get_message(message_id).await?;
        lock(&self.images_once).insert(message_id);
        Ok(())
    }

    /// `images.allow_sender`: Always load from this sender (`remote_image_allow`).
    pub async fn images_allow_sender(&self, message_id: MessageId) -> Result<(), AppError> {
        let message = self.store.get_message(message_id).await?;
        if message.from.addr.is_empty() {
            return Err(AppError::invalid("the message has no sender address"));
        }
        self.store
            .allow_remote_images(message.account_id, &message.from.addr)
            .await?;
        Ok(())
    }

    /// The `mail-body` protocol response for `mail-body://<message>?<query>`. `images=1`
    /// counts only for a message whose images are allowed (SPEC_GAPS G57).
    pub async fn mail_body(
        &self,
        message_id: MessageId,
        query: &str,
    ) -> Result<BodyResponse, AppError> {
        let mut request = BodyRequest::from_query(query);
        if request.images {
            request.images = match self.store.get_message(message_id).await {
                Ok(m) => self.images_allowed(&m).await?,
                Err(StoreError::NotFound) => false,
                Err(e) => return Err(e.into()),
            };
        }
        mail_render::mail_body(&self.store, message_id, request, self.form)
            .await
            .map_err(|mail_render::RenderError::Store(e)| AppError::from(e))
    }

    /// The `mail-cid` protocol response for the path `<message>/<cid>`: an inline image of
    /// that message, images only (section 8, Attachments).
    pub async fn mail_cid(&self, path: &str) -> BodyResponse {
        let path = mail_render::urls::request_path(path);
        let Some((id, cid)) = path.split_once('/') else {
            return mail_render::empty_response(400);
        };
        let (Ok(message_id), cid) = (id.parse::<MessageId>(), mail_render::urls::decode(cid))
        else {
            return mail_render::empty_response(400);
        };
        let Ok(attachments) = self.store.list_attachments(message_id).await else {
            return mail_render::empty_response(404);
        };
        let found = attachments.into_iter().find(|a| {
            a.attachment.mime.starts_with("image/")
                && a.attachment
                    .content_id
                    .as_deref()
                    .is_some_and(|c| c.eq_ignore_ascii_case(&cid))
        });
        let Some(record) = found else {
            return mail_render::empty_response(404);
        };
        match self.attachment_bytes(&record).await {
            Ok(bytes) => mail_render::image_response(&record.attachment.mime, bytes),
            Err(_) => mail_render::empty_response(502),
        }
    }

    /// The `mail-remote` protocol response: the image through the proxy (section 8).
    pub async fn mail_remote(&self, path: &str) -> BodyResponse {
        match self.proxy.fetch(path).await {
            Ok(img) => mail_render::image_response(&img.content_type, img.bytes),
            Err(_) => mail_render::empty_response(404),
        }
    }

    /// A part's bytes, from the cache or fetched on demand through the account's actor
    /// (invariant 3).
    pub(crate) async fn attachment_bytes(
        &self,
        record: &AttachmentRecord,
    ) -> Result<Vec<u8>, AppError> {
        let message = self.store.get_message(record.message_id).await?;
        let folder = self.store.get_folder(message.folder_id).await?;
        let key = CacheKey {
            account_id: message.account_id,
            folder_id: folder.id,
            uidvalidity: folder.uidvalidity,
            uid: message.uid,
            part_id: record.attachment.part_id.clone(),
        };
        let cache = self.cache.clone();
        let lookup = key.clone();
        if let Some(bytes) = blocking(move || Ok(cache.get(&lookup))).await? {
            return Ok(bytes);
        }
        let bytes = self
            .fetch_part(message.account_id, message.id, &record.attachment.part_id)
            .await?;
        let cache = self.cache.clone();
        let stored = bytes.clone();
        if let Err(e) = blocking(move || cache.put(&key, &stored)).await {
            tracing::warn!(code = ?e.code, "attachment not cached");
        }
        Ok(bytes)
    }

    /// One part (or with `""` the raw source) through the account's actor. An account that
    /// is offline or off answers `network`.
    pub(crate) async fn fetch_part_pub(
        &self,
        account_id: AccountId,
        message_id: MessageId,
        part_id: &str,
    ) -> Result<Vec<u8>, AppError> {
        self.fetch_part(account_id, message_id, part_id).await
    }

    async fn fetch_part(
        &self,
        account_id: AccountId,
        message_id: MessageId,
        part_id: &str,
    ) -> Result<Vec<u8>, AppError> {
        let rx = self
            .actors
            .lock()
            .await
            .get(&account_id)
            .map(|h| h.fetch_part(message_id, part_id));
        let offline =
            || AppError::new(ErrorCode::Network, "the account is offline").for_account(account_id);
        match rx {
            None => Err(offline()),
            Some(rx) => match rx.await {
                Ok(Ok(bytes)) => Ok(bytes),
                Ok(Err(e)) => Err(sync_error(&e).for_account(account_id)),
                Err(_) => Err(offline()),
            },
        }
    }

    async fn attachment(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<(AttachmentRecord, String), AppError> {
        let record = self.store.get_attachment(attachment_id).await?;
        let name = mail_render::display_name(&record.attachment.filename);
        Ok((record, name))
    }

    /// The name a Save dialog offers for an attachment (SPEC_GAPS G54, G55).
    pub async fn attachment_file_name(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<String, AppError> {
        let (_, name) = self.attachment(attachment_id).await?;
        Ok(mail_render::safe_name(&name))
    }

    /// `attachment.open`: fetched on demand into a randomly named temp folder (section 8;
    /// 13.5). Returns the file for the app to hand to the OS default app. The UI asks
    /// first for executable types.
    pub async fn attachment_open(&self, attachment_id: AttachmentId) -> Result<PathBuf, AppError> {
        let (record, name) = self.attachment(attachment_id).await?;
        let bytes = self.attachment_bytes(&record).await?;
        let temp = self.temp.clone();
        blocking(move || temp.write(&name, &bytes)).await
    }

    /// `attachment.save` to the path the user chose.
    pub async fn attachment_save(
        &self,
        attachment_id: AttachmentId,
        path: PathBuf,
    ) -> Result<(), AppError> {
        let (record, _) = self.attachment(attachment_id).await?;
        let bytes = self.attachment_bytes(&record).await?;
        blocking(move || std::fs::write(&path, &bytes)).await
    }

    /// The name a Save dialog offers for a message's source: its subject, `.eml`.
    pub async fn source_file_name(&self, message_id: MessageId) -> Result<String, AppError> {
        let message = self.store.get_message(message_id).await?;
        let stem = if message.subject.trim().is_empty() {
            "message"
        } else {
            message.subject.trim()
        };
        let stem: String = stem.chars().take(150).collect();
        Ok(mail_render::safe_name(&format!("{stem}.eml")))
    }

    /// `message.save_source`: the raw message, fetched on demand and never stored
    /// (invariant 3), written to the path the user chose (section 12; SPEC_GAPS G54).
    pub async fn message_save_source(
        &self,
        message_id: MessageId,
        path: PathBuf,
    ) -> Result<(), AppError> {
        let message = self.store.get_message(message_id).await?;
        let bytes = self.fetch_part(message.account_id, message_id, "").await?;
        blocking(move || std::fs::write(&path, &bytes)).await
    }

    /// `link.open`: checks the URL; the app then hands it to the OS (SPEC_GAPS G52).
    pub fn link_check(&self, url: &str) -> Result<String, AppError> {
        let url = url.trim();
        match mail_render::urls::scheme(url) {
            Some(s) if LINK_SCHEMES.contains(&s.as_str()) && url.len() > s.len() + 1 => {
                Ok(url.to_string())
            }
            _ => Err(AppError::invalid(
                "only http, https and tel links open outside the app",
            )),
        }
    }

    /// The log folder `log.open` shows (section 13.8).
    pub fn log_dir(&self) -> PathBuf {
        crate::logging::log_dir(&self.data_dir)
    }

    /// Stops every actor and background task and deletes opened attachments (app quit).
    pub async fn shutdown(&self) {
        for task in lock(&self.background).drain(..) {
            task.abort();
        }
        if let Err(e) = self.temp.clear() {
            tracing::warn!(error = %e, "could not clear the temp folder");
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

/// Unfinished ops and outbox items (`pending-count`, section 3).
pub(crate) async fn pending_total(store: &Store) -> Result<u32, StoreError> {
    Ok(store.pending_count().await? + store.outbox_pending().await?)
}

/// Sends due outbox items every tick, and at once when woken (section 7). Ends with the core.
async fn outbox_runner<I, S, H>(
    core: std::sync::Weak<Core<I, S, H>>,
    wake: Arc<tokio::sync::Notify>,
) where
    I: ImapConnector + Clone + 'static,
    S: SmtpCheck + mail_smtp::SmtpSubmit + Clone + 'static,
    H: crate::core::Http,
{
    loop {
        let tick = match core.upgrade() {
            Some(c) => {
                c.process_outbox().await;
                c.timing().outbox_tick
            }
            None => return,
        };
        tokio::select! {
            () = tokio::time::sleep(tick) => {}
            () = wake.notified() => {}
        }
    }
}

/// Runs file work off the async threads; an I/O failure is `io`.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| AppError::new(ErrorCode::Internal, "file task failed"))?
        .map_err(|e| AppError::new(ErrorCode::Io, e.kind().to_string()))
}

/// Forwards sync events to the UI. `store-changed` is coalesced per (account, folder) into
/// 100 ms windows: the first change opens a window and every key changed in it is sent once
/// when it closes, so a long sync updates the list at most ten times a second.
async fn pump(
    mut rx: mpsc::UnboundedReceiver<SyncEvent>,
    sink: EventSink,
    states: States,
    store: Option<Store>,
    follow: Option<mpsc::UnboundedSender<SyncEvent>>,
) {
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
            SyncEvent::PendingChanged { .. } => {
                if let Some(s) = &store {
                    if let Ok(total) = pending_total(s).await {
                        sink(CoreEvent::PendingCount { total });
                    }
                }
            }
            SyncEvent::OpFailed {
                account_id,
                op_id,
                kind,
                error,
            } => sink(CoreEvent::OpFailed {
                account_id,
                op_id,
                kind,
                error: sync_error(&error).for_account(account_id),
            }),
            // New mail and stored bodies go to the core's own task (rules, mute, snooze).
            e @ (SyncEvent::NewMail { .. } | SyncEvent::BodyStored { .. }) => {
                if let Some(tx) = &follow {
                    let _ = tx.send(e);
                }
            }
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
        tokio::spawn(pump(rx, sink, Arc::default(), None, None));
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
        let pumped = tokio::spawn(pump(rx, sink, Arc::clone(&states), None, None));
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
