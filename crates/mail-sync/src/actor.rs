//! The per-account actor (section 6): states, connections, the worker queue and IDLE.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use mail_auth::{Login, LoginRequest, LoginSource};
use mail_imap::{
    BodyPart, ConnectConfig, Credentials, ErrorClass, HeaderFetch, IdleEvent, ImapConnector,
    ImapError, ImapTransport, Security, UidSet, CONNECT_TIMEOUT, FETCH_BATCH, IDLE_REISSUE,
};
use mail_store::{
    Account, AccountId, Folder, FolderId, MessageId, PrefetchItem, SpecialUse, Store,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, Interval, MissedTickBehavior};

use crate::backoff::{jitter, Backoff, HEALTHY, THROTTLE_PAUSE};
use crate::folder::{sync_folder, sync_folder_list, sync_order, Ctx};
use crate::{
    body, now, AccountState, Events, SyncError, SyncEvent, MAX_WATCHED, OPEN_FOLDER_INTERVAL,
    POLL_INTERVAL, PREFETCH_COUNT,
};

/// What every actor needs. Cheap to clone.
#[derive(Debug)]
pub struct SyncDeps<C, L> {
    pub store: Store,
    pub connector: Arc<C>,
    pub logins: Arc<L>,
    pub events: Events,
}

impl<C, L> Clone for SyncDeps<C, L> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            connector: Arc::clone(&self.connector),
            logins: Arc::clone(&self.logins),
            events: self.events.clone(),
        }
    }
}

/// Where a job the user waits for sends its outcome. Dropped unanswered when the account is
/// offline or the session ends, which the caller sees as a closed channel.
struct Reply<T>(Option<oneshot::Sender<Result<T, SyncError>>>);

impl<T> Reply<T> {
    fn send(&mut self, result: Result<T, SyncError>) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(result);
        }
    }
}

impl<T> std::fmt::Debug for Reply<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reply")
    }
}

#[derive(Debug)]
enum Command {
    Refresh(FolderId),
    FetchBody(MessageId, Reply<()>),
    /// A part's decoded bytes; an empty part id is the raw message source.
    FetchPart(MessageId, String, Reply<Vec<u8>>),
    /// New ops were queued for this folder (`None`: folder ops).
    OpsQueued(Option<FolderId>),
    /// Load older: 90 more days (`folder.extend_window`).
    Extend(FolderId),
    /// Search server too (section 10): the number of hits stored.
    Search(ServerSearch, Reply<usize>),
    OpenFolder(Option<FolderId>),
    AccountChanged,
    Retry,
    Shutdown,
}

/// Controls one running actor.
#[derive(Debug)]
pub struct AccountHandle {
    account_id: AccountId,
    commands: mpsc::UnboundedSender<Command>,
    task: JoinHandle<()>,
    caps: Arc<std::sync::Mutex<Option<mail_imap::Capabilities>>>,
}

impl AccountHandle {
    pub fn account_id(&self) -> AccountId {
        self.account_id
    }

    /// The capabilities of the last connection (APPENDLIMIT for the size check, section 7).
    pub fn capabilities(&self) -> Option<mail_imap::Capabilities> {
        self.caps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// `folder.refresh`: sync the folder now, ahead of background work (priority 1).
    pub fn refresh(&self, folder_id: FolderId) {
        let _ = self.commands.send(Command::Refresh(folder_id));
    }

    /// The user opened a message whose body is not cached: fetch it ahead of everything
    /// else (section 6, step 8). The receiver gets the outcome, or closes when the account
    /// is offline.
    pub fn fetch_body(&self, message_id: MessageId) -> oneshot::Receiver<Result<(), SyncError>> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .commands
            .send(Command::FetchBody(message_id, Reply(Some(tx))));
        rx
    }

    /// One body part, transfer-decoded (attachments, inline images), or with an empty
    /// `part_id` the raw message source (Save source), fetched ahead of background work.
    /// The receiver closes unanswered when the account is offline.
    pub fn fetch_part(
        &self,
        message_id: MessageId,
        part_id: &str,
    ) -> oneshot::Receiver<Result<Vec<u8>, SyncError>> {
        let (tx, rx) = oneshot::channel();
        let _ = self.commands.send(Command::FetchPart(
            message_id,
            part_id.to_string(),
            Reply(Some(tx)),
        ));
        rx
    }

    /// Ops were queued: push them ahead of folder syncs (worker queue priority 2).
    pub fn ops_queued(&self, folder_id: Option<FolderId>) {
        let _ = self.commands.send(Command::OpsQueued(folder_id));
    }

    /// `folder.extend_window`: fetch the 90 days before the folder's oldest cached message.
    pub fn extend_window(&self, folder_id: FolderId) {
        let _ = self.commands.send(Command::Extend(folder_id));
    }

    /// Search server too (section 10): runs the search on the server and header-fetches
    /// the hits that are not cached into the store, newest first, at most `cap`. The
    /// receiver gets how many hits are stored, or closes when the account is offline.
    pub fn server_search(
        &self,
        search: ServerSearch,
    ) -> oneshot::Receiver<Result<usize, SyncError>> {
        let (tx, rx) = oneshot::channel();
        let _ = self.commands.send(Command::Search(search, Reply(Some(tx))));
        rx
    }

    /// The folder open in the UI: it syncs now and every 5 minutes while open.
    pub fn open_folder(&self, folder_id: Option<FolderId>) {
        let _ = self.commands.send(Command::OpenFolder(folder_id));
    }

    /// The account row changed (settings or the enabled switch): reconnect with it.
    pub fn account_changed(&self) {
        let _ = self.commands.send(Command::AccountChanged);
    }

    /// The user fixed a login or certificate problem: leave `NeedsAttention` and connect.
    pub fn retry(&self) {
        let _ = self.commands.send(Command::Retry);
    }

    /// Stops after the job in progress and closes every connection.
    pub async fn shutdown(self) {
        let _ = self.commands.send(Command::Shutdown);
        let _ = self.task.await;
    }

    /// Stops at once, as a crash or kill would. Committed batches stay committed.
    pub fn abort(self) {
        self.task.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

/// Starts the actor for one account.
pub fn spawn_account<C, L>(deps: SyncDeps<C, L>, account_id: AccountId) -> AccountHandle
where
    C: ImapConnector + 'static,
    L: LoginSource,
{
    let (tx, rx) = mpsc::unbounded_channel();
    let caps: Arc<std::sync::Mutex<Option<mail_imap::Capabilities>>> = Arc::default();
    let actor = Actor {
        deps,
        account_id,
        commands: rx,
        open_folder: None,
        shown: None,
        caps: Arc::clone(&caps),
    };
    AccountHandle {
        account_id,
        commands: tx,
        task: tokio::spawn(actor.run()),
        caps,
    }
}

struct Actor<C, L> {
    deps: SyncDeps<C, L>,
    account_id: AccountId,
    commands: mpsc::UnboundedReceiver<Command>,
    open_folder: Option<FolderId>,
    shown: Option<AccountState>,
    caps: Arc<std::sync::Mutex<Option<mail_imap::Capabilities>>>,
}

enum Exit {
    Shutdown,
    /// Re-read the account and connect again.
    Reload,
    Failed {
        error: SyncError,
        /// The connection had been up for at least 60 s.
        healthy: bool,
    },
}

enum Wake {
    Shutdown,
    Go,
}

/// Worker queue priorities (section 6): 1 user requests, 2 pending ops, 3 folder sync,
/// 4 prefetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Priority {
    User = 0,
    Ops = 1,
    Sync = 2,
    Prefetch = 3,
}

#[derive(Debug)]
enum Job {
    Sync(FolderId),
    Prefetch(FolderId),
    Body {
        folder_id: FolderId,
        item: PrefetchItem,
        structure: Box<BodyPart>,
    },
    /// A body the user is waiting for.
    Open(MessageId, Reply<()>),
    /// A part (or the source) the user is waiting for.
    Part(MessageId, String, Reply<Vec<u8>>),
    /// Push one folder's message ops.
    Ops(FolderId),
    /// Push the account's folder ops (create, rename, delete).
    FolderOps,
    /// Load older for one folder.
    Extend(FolderId),
    /// A server search the user is waiting for.
    Search(ServerSearch, Reply<usize>),
}

/// What Search server too sends (section 10): `UID SEARCH` with `key` on `folders`, or on
/// Gmail `X-GM-RAW` with `gmail_raw` on All Mail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSearch {
    pub folders: Vec<FolderId>,
    pub key: Option<mail_imap::SearchKey>,
    pub gmail_raw: Option<String>,
    pub cap: usize,
}

impl Job {
    fn same(&self, other: &Job) -> bool {
        match (self, other) {
            (Job::Sync(a), Job::Sync(b)) | (Job::Prefetch(a), Job::Prefetch(b)) => a == b,
            (Job::Body { item: a, .. }, Job::Body { item: b, .. }) => a.message_id == b.message_id,
            (Job::Open(a, _), Job::Open(b, _)) => a == b,
            (Job::Part(a, p, _), Job::Part(b, q, _)) => a == b && p == q,
            (Job::Ops(a), Job::Ops(b)) | (Job::Extend(a), Job::Extend(b)) => a == b,
            (Job::FolderOps, Job::FolderOps) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Default)]
struct Queue {
    lanes: [VecDeque<Job>; 4],
}

impl Queue {
    /// Adds a job once. A job already queued at a lower priority moves up.
    fn push(&mut self, priority: Priority, job: Job) {
        let p = priority as usize;
        for (lane, jobs) in self.lanes.iter_mut().enumerate() {
            if let Some(pos) = jobs.iter().position(|j| j.same(&job)) {
                if lane <= p {
                    return;
                }
                jobs.remove(pos);
                break;
            }
        }
        self.lanes[p].push_back(job);
    }

    fn pop(&mut self) -> Option<Job> {
        self.lanes.iter_mut().find_map(VecDeque::pop_front)
    }

    fn has_sync(&self) -> bool {
        self.lanes
            .iter()
            .flatten()
            .any(|j| matches!(j, Job::Sync(_)))
    }
}

enum IdleMsg {
    Changed(FolderId),
    Failed(ImapError),
}

/// IDLE connections of one session; they close when the session ends.
struct IdleTasks(Vec<JoinHandle<()>>);

impl Drop for IdleTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

fn emit_state(
    events: &Events,
    shown: &mut Option<AccountState>,
    account_id: AccountId,
    state: AccountState,
    last_error: Option<SyncError>,
    retry_in: Option<Duration>,
) {
    if *shown == Some(state) && last_error.is_none() {
        return;
    }
    *shown = Some(state);
    tracing::info!(account_id, state = ?state, retry_in = ?retry_in, "account state");
    events.send(SyncEvent::AccountState {
        account_id,
        state,
        last_error,
        retry_in,
    });
}

fn connect_config(account: &Account) -> ConnectConfig {
    let a = &account.config;
    ConnectConfig {
        host: a.imap_host.clone(),
        port: a.imap_port,
        security: match a.imap_security {
            mail_store::Security::Tls => Security::Tls,
            mail_store::Security::StartTls => Security::StartTls,
        },
        pinned_cert_sha256: a
            .pinned_cert_sha256
            .as_deref()
            .and_then(mail_imap::parse_fingerprint),
        connect_timeout: CONNECT_TIMEOUT,
    }
}

/// What the login source needs to know about an account.
pub fn login_request(account: &Account) -> LoginRequest {
    let a = &account.config;
    LoginRequest {
        account_id: account.id,
        username: a.username.clone(),
        method: match a.auth_kind {
            mail_store::AuthKind::Password => mail_auth::AuthMethod::Password,
            mail_store::AuthKind::OauthGoogle => mail_auth::AuthMethod::OauthGoogle,
        },
        client_id: a.oauth_client_id.clone(),
    }
}

fn credentials(login: &Login) -> Credentials {
    let secret = mail_imap::Secret::new(login.secret.expose());
    match login.method {
        mail_auth::AuthMethod::Password => Credentials::Password {
            username: login.username.clone(),
            password: secret,
        },
        mail_auth::AuthMethod::OauthGoogle => Credentials::XOAuth2 {
            username: login.username.clone(),
            access_token: secret,
        },
    }
}

fn ticker(period: Duration) -> Interval {
    let mut t = tokio::time::interval_at(Instant::now() + period, period);
    t.set_missed_tick_behavior(MissedTickBehavior::Delay);
    t
}

impl<C, L> Actor<C, L>
where
    C: ImapConnector + 'static,
    L: LoginSource,
{
    fn state(&mut self, state: AccountState, error: Option<SyncError>, retry_in: Option<Duration>) {
        emit_state(
            &self.deps.events,
            &mut self.shown,
            self.account_id,
            state,
            error,
            retry_in,
        );
    }

    async fn run(mut self) {
        let mut backoff = Backoff::default();
        let mut reconnected_after_bye = false;
        loop {
            let account = match self.deps.store.get_account(self.account_id).await {
                Ok(a) => a,
                // The account was removed.
                Err(mail_store::StoreError::NotFound) => return,
                Err(e) => {
                    let delay = backoff.next_delay(jitter());
                    self.state(AccountState::Backoff, Some(e.into()), Some(delay));
                    if let Wake::Shutdown = self.pause(delay).await {
                        return;
                    }
                    continue;
                }
            };
            if !account.config.enabled {
                self.state(AccountState::Disabled, None, None);
                if let Wake::Shutdown = self.wait_for_user().await {
                    return;
                }
                continue;
            }
            self.state(AccountState::Connecting, None, None);
            match self.session(&account).await {
                Exit::Shutdown => return,
                Exit::Reload => {}
                Exit::Failed { error, healthy } => {
                    if healthy {
                        backoff.reset();
                        reconnected_after_bye = false;
                    }
                    match error.class() {
                        // A bad login or certificate: nothing retries until the user acts.
                        ErrorClass::NeedsAttention | ErrorClass::Permanent => {
                            self.state(AccountState::NeedsAttention, Some(error), None);
                            if let Wake::Shutdown = self.wait_for_user().await {
                                return;
                            }
                            backoff.reset();
                        }
                        ErrorClass::Throttle => {
                            self.state(AccountState::Backoff, Some(error), Some(THROTTLE_PAUSE));
                            if let Wake::Shutdown = self.pause(THROTTLE_PAUSE).await {
                                return;
                            }
                        }
                        ErrorClass::Transient => {
                            // BYE or a closed socket: reconnect once at once (section 12).
                            if error == SyncError::Imap(ImapError::Bye) && !reconnected_after_bye {
                                reconnected_after_bye = true;
                                continue;
                            }
                            let delay = backoff.next_delay(jitter());
                            self.state(AccountState::Backoff, Some(error), Some(delay));
                            if let Wake::Shutdown = self.pause(delay).await {
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Waits out a delay. A user change or Retry ends it early.
    async fn pause(&mut self, delay: Duration) -> Wake {
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => return Wake::Go,
                cmd = self.commands.recv() => match cmd {
                    None | Some(Command::Shutdown) => return Wake::Shutdown,
                    Some(Command::AccountChanged | Command::Retry) => return Wake::Go,
                    Some(Command::OpenFolder(f)) => self.open_folder = f,
                    // Offline: the waiting caller's channel closes; queued ops wait in the
                    // store for the next connection.
                    Some(
                        Command::Refresh(_)
                        | Command::FetchBody(..)
                        | Command::FetchPart(..)
                        | Command::OpsQueued(_)
                        | Command::Extend(_)
                        | Command::Search(..),
                    ) => {}
                },
            }
        }
    }

    /// Waits for the user: Retry, or a change to the account.
    async fn wait_for_user(&mut self) -> Wake {
        loop {
            match self.commands.recv().await {
                None | Some(Command::Shutdown) => return Wake::Shutdown,
                Some(Command::AccountChanged | Command::Retry) => return Wake::Go,
                Some(Command::OpenFolder(f)) => self.open_folder = f,
                Some(
                    Command::Refresh(_)
                    | Command::FetchBody(..)
                    | Command::FetchPart(..)
                    | Command::OpsQueued(_)
                    | Command::Extend(_)
                    | Command::Search(..),
                ) => {}
            }
        }
    }

    /// One connected session: connect, pull every enabled folder in order, then wait for
    /// news and requests until something fails or the user stops it.
    async fn session(&mut self, account: &Account) -> Exit {
        let account_id = self.account_id;
        let login = match self.deps.logins.login(&login_request(account)).await {
            Ok(login) => login,
            Err(e) => {
                return Exit::Failed {
                    error: e.into(),
                    healthy: false,
                }
            }
        };
        let config = connect_config(account);
        let creds = credentials(&login);
        let mut worker = match self.deps.connector.connect(&config, &creds).await {
            Ok(t) => t,
            Err(e) => {
                return Exit::Failed {
                    error: e.into(),
                    healthy: false,
                }
            }
        };
        let connected = Instant::now();
        let failed = |error: SyncError| Exit::Failed {
            error,
            healthy: connected.elapsed() >= HEALTHY,
        };
        let caps = worker.capabilities().clone();
        *self
            .caps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(caps.clone());
        self.state(AccountState::Syncing, None, None);

        let deps = self.deps.clone();
        let ctx = Ctx {
            store: &deps.store,
            events: &deps.events,
            account_id,
            sync_days: account.config.sync_days,
            caps: &caps,
        };
        // Push first (section 6): every op queued while offline, ahead of the pulls. Folder
        // ops go before LIST, which would otherwise drop a folder created here or bring back
        // one deleted here.
        if let Err(e) = deps.store.requeue_running_ops(account_id).await {
            return failed(e.into());
        }
        let _ = deps
            .store
            .purge_done_ops(now() - crate::UNDO_WINDOW_SECS)
            .await;
        if let Err(e) =
            crate::ops::run_folder_ops_account(&mut worker, &deps.store, &deps.events, account_id)
                .await
        {
            return failed(e);
        }
        let mut folders = match sync_folder_list(&mut worker, &ctx).await {
            Ok(f) => f,
            Err(e) => return failed(e),
        };
        let mut queue = Queue::default();
        match deps.store.account_ops(account_id).await {
            Ok(ops) => {
                for op in ops {
                    if crate::ops::is_folder_op(&op) {
                        queue.push(Priority::Ops, Job::FolderOps);
                    } else {
                        queue.push(Priority::Ops, Job::Ops(op.folder_id));
                    }
                }
            }
            Err(e) => return failed(e.into()),
        }
        for id in sync_order(&folders) {
            queue.push(Priority::Sync, Job::Sync(id));
        }
        let inbox = folders
            .iter()
            .find(|f| f.special_use == Some(SpecialUse::Inbox))
            .map(|f| f.id);

        let (idle_tx, mut idle_rx) = mpsc::unbounded_channel();
        let mut idle = IdleTasks(Vec::new());
        let mut waiting_started = false;
        let mut inbox_done = inbox.is_none();
        let mut selected: Option<FolderId> = None;
        let mut poll = ticker(POLL_INTERVAL);
        let mut open_tick = ticker(OPEN_FOLDER_INTERVAL);

        loop {
            while let Ok(cmd) = self.commands.try_recv() {
                if let Some(exit) = self.on_command(cmd, &mut queue, &folders, &mut open_tick) {
                    return exit;
                }
            }
            while let Ok(msg) = idle_rx.try_recv() {
                match msg {
                    IdleMsg::Changed(f) => queue.push(Priority::Sync, Job::Sync(f)),
                    IdleMsg::Failed(e) => return failed(e.into()),
                }
            }

            // IDLE connections open once INBOX headers are in (section 6: INBOX is visible
            // before any other folder is touched; its news must not wait for the rest).
            if inbox_done && !waiting_started {
                waiting_started = true;
                if caps.idle {
                    let watched = folders
                        .iter()
                        .filter(|f| f.watch_idle && f.sync_enabled && Some(f.id) != inbox)
                        .take(MAX_WATCHED);
                    for f in inbox
                        .and_then(|id| folders.iter().find(|f| f.id == id))
                        .into_iter()
                        .chain(watched)
                    {
                        idle.0.push(spawn_idle(
                            Arc::clone(&deps.connector),
                            config.clone(),
                            creds.clone(),
                            f.id,
                            f.path.clone(),
                            idle_tx.clone(),
                        ));
                    }
                }
            }

            if let Some(job) = queue.pop() {
                let syncing = matches!(job, Job::Sync(_)) || queue.has_sync();
                self.state(
                    if syncing {
                        AccountState::Syncing
                    } else {
                        AccountState::Idle
                    },
                    None,
                    None,
                );
                let was_inbox = matches!(job, Job::Sync(id) if Some(id) == inbox);
                match run_job(
                    job,
                    &mut worker,
                    &ctx,
                    &mut selected,
                    &mut queue,
                    &mut folders,
                )
                .await
                {
                    Ok(()) => inbox_done |= was_inbox,
                    // One folder or message failed; the connection is fine.
                    Err(e) if e.class() == ErrorClass::Permanent => {
                        tracing::warn!(account_id, error = ?e, "sync job skipped");
                        inbox_done |= was_inbox;
                    }
                    Err(e) => return failed(e),
                }
                continue;
            }

            self.state(AccountState::Idle, None, None);
            let polling = waiting_started && !caps.idle && inbox.is_some();
            let open = self.open_folder;
            tokio::select! {
                biased;
                cmd = self.commands.recv() => match cmd {
                    None => return Exit::Shutdown,
                    Some(cmd) => {
                        if let Some(exit) = self.on_command(cmd, &mut queue, &folders, &mut open_tick) {
                            return exit;
                        }
                    }
                },
                msg = idle_rx.recv() => match msg {
                    Some(IdleMsg::Changed(f)) => queue.push(Priority::Sync, Job::Sync(f)),
                    Some(IdleMsg::Failed(e)) => return failed(e.into()),
                    None => {}
                },
                _ = poll.tick(), if polling => {
                    if let Some(id) = inbox {
                        queue.push(Priority::Sync, Job::Sync(id));
                    }
                }
                _ = open_tick.tick(), if open.is_some() => {
                    if let Some(id) = open {
                        queue.push(Priority::Sync, Job::Sync(id));
                    }
                }
            }
        }
    }

    fn on_command(
        &mut self,
        cmd: Command,
        queue: &mut Queue,
        folders: &[Folder],
        open_tick: &mut Interval,
    ) -> Option<Exit> {
        let ours = |id: FolderId| folders.iter().any(|f| f.id == id);
        match cmd {
            Command::Shutdown => return Some(Exit::Shutdown),
            Command::AccountChanged => return Some(Exit::Reload),
            Command::Retry => {}
            Command::Refresh(id) => {
                if ours(id) {
                    queue.push(Priority::User, Job::Sync(id));
                }
            }
            Command::FetchBody(id, reply) => queue.push(Priority::User, Job::Open(id, reply)),
            Command::FetchPart(id, part, reply) => {
                queue.push(Priority::User, Job::Part(id, part, reply));
            }
            Command::OpsQueued(Some(id)) => queue.push(Priority::Ops, Job::Ops(id)),
            Command::OpsQueued(None) => queue.push(Priority::Ops, Job::FolderOps),
            Command::Extend(id) => {
                if ours(id) {
                    queue.push(Priority::User, Job::Extend(id));
                }
            }
            Command::Search(search, reply) => {
                queue.push(Priority::User, Job::Search(search, reply));
            }
            Command::OpenFolder(f) => {
                self.open_folder = f;
                open_tick.reset();
                if let Some(id) = f.filter(|id| ours(*id)) {
                    queue.push(Priority::User, Job::Sync(id));
                }
            }
        }
        None
    }
}

/// One IDLE connection on `path` (section 6, step 9): re-issued every 25 minutes, and any
/// news is reported for the worker to sync. Ends at the first error.
fn spawn_idle<C: ImapConnector + 'static>(
    connector: Arc<C>,
    config: ConnectConfig,
    creds: Credentials,
    folder_id: FolderId,
    path: String,
    tx: mpsc::UnboundedSender<IdleMsg>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = idle_loop(&*connector, &config, &creds, folder_id, &path, &tx).await {
            let _ = tx.send(IdleMsg::Failed(e));
        }
    })
}

async fn idle_loop<C: ImapConnector>(
    connector: &C,
    config: &ConnectConfig,
    creds: &Credentials,
    folder_id: FolderId,
    path: &str,
    tx: &mpsc::UnboundedSender<IdleMsg>,
) -> Result<(), ImapError> {
    let mut conn = connector.connect(config, creds).await?;
    conn.select(path).await?;
    // Changes between the worker's last sync of this folder and this SELECT would never
    // be announced; one catch-up sync closes that gap.
    let _ = tx.send(IdleMsg::Changed(folder_id));
    loop {
        if conn.idle(IDLE_REISSUE).await? == IdleEvent::Changed {
            let _ = tx.send(IdleMsg::Changed(folder_id));
        }
    }
}

/// Selects `folder` on the worker if another folder is selected. A UIDVALIDITY change
/// queues a sync instead, and the caller skips its job.
async fn ensure_selected<T: ImapTransport>(
    t: &mut T,
    folder: &Folder,
    selected: &mut Option<FolderId>,
    queue: &mut Queue,
) -> Result<bool, SyncError> {
    if *selected == Some(folder.id) {
        return Ok(true);
    }
    *selected = None;
    let info = t.select(&folder.path).await?;
    *selected = Some(folder.id);
    if info.uidvalidity != folder.uidvalidity {
        queue.push(Priority::Sync, Job::Sync(folder.id));
        return Ok(false);
    }
    Ok(true)
}

/// Queues a sync of each folder an op touched, when it is one of the account's synced
/// folders.
fn queue_followups(queue: &mut Queue, folders: &[Folder], sync: &[FolderId]) {
    for id in sync {
        if folders.iter().any(|f| f.id == *id && f.sync_enabled) {
            queue.push(Priority::Sync, Job::Sync(*id));
        }
    }
}

async fn run_job<T: ImapTransport>(
    job: Job,
    t: &mut T,
    ctx: &Ctx<'_>,
    selected: &mut Option<FolderId>,
    queue: &mut Queue,
    folders: &mut Vec<Folder>,
) -> Result<(), SyncError> {
    match job {
        Job::Sync(folder_id) => {
            *selected = None;
            let follow = sync_folder(t, ctx, folder_id).await?;
            *selected = Some(folder_id);
            queue_followups(queue, folders, &follow);
            queue.push(Priority::Prefetch, Job::Prefetch(folder_id));
        }
        Job::Ops(folder_id) => {
            let folder = ctx.store.get_folder(folder_id).await?;
            if !ensure_selected(t, &folder, selected, queue).await? {
                return Ok(());
            }
            let follow =
                crate::ops::run_folder_ops(t, ctx.store, ctx.events, ctx.account_id, &folder)
                    .await?;
            queue_followups(queue, folders, &follow.sync);
        }
        Job::FolderOps => {
            if crate::ops::run_folder_ops_account(t, ctx.store, ctx.events, ctx.account_id).await? {
                *selected = None;
                *folders = sync_folder_list(t, ctx).await?;
                for f in folders
                    .iter()
                    .filter(|f| f.sync_enabled && f.synced_at == 0)
                {
                    queue.push(Priority::Sync, Job::Sync(f.id));
                }
            }
        }
        Job::Extend(folder_id) => {
            *selected = None;
            crate::folder::extend_window(t, ctx, folder_id).await?;
            *selected = Some(folder_id);
        }
        Job::Prefetch(folder_id) => {
            let folder = ctx.store.get_folder(folder_id).await?;
            if !ensure_selected(t, &folder, selected, queue).await? {
                return Ok(());
            }
            let items = ctx
                .store
                .prefetch_candidates(folder_id, PREFETCH_COUNT)
                .await?;
            for batch in items.chunks(FETCH_BATCH) {
                let uids = UidSet::new(batch.iter().map(|i| i.uid));
                let headers = t.uid_fetch_headers(&uids, HeaderFetch::default()).await?;
                for item in batch {
                    if let Some(h) = headers.iter().find(|h| h.uid == item.uid) {
                        queue.push(
                            Priority::Prefetch,
                            Job::Body {
                                folder_id,
                                item: *item,
                                structure: Box::new(h.body.clone()),
                            },
                        );
                    }
                }
            }
        }
        Job::Body {
            folder_id,
            item,
            structure,
        } => {
            let folder = ctx.store.get_folder(folder_id).await?;
            if !ensure_selected(t, &folder, selected, queue).await? {
                return Ok(());
            }
            match body::fetch_body(t, item.uid, &structure, now(), body::Purpose::Prefetch).await {
                Ok(Some((body, state))) => {
                    let preview = body::preview(&body.text_plain);
                    ctx.store
                        .store_body(item.message_id, body, state, preview)
                        .await?;
                    ctx.events.send(SyncEvent::BodyStored {
                        account_id: ctx.account_id,
                        message_id: item.message_id,
                    });
                    ctx.events.send(SyncEvent::StoreChanged {
                        account_id: ctx.account_id,
                        folder_id: Some(folder_id),
                    });
                }
                // Too large to prefetch (section 12); opened on demand later.
                Ok(None) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Job::Open(message_id, mut reply) => {
            let result = open_body(t, ctx, selected, queue, message_id).await;
            reply.send(result.clone());
            result?;
        }
        Job::Search(search, mut reply) => {
            let result = server_search(t, ctx, selected, folders, &search).await;
            let failed = result.as_ref().err().cloned();
            reply.send(result);
            if let Some(e) = failed {
                return Err(e);
            }
        }
        Job::Part(message_id, part_id, mut reply) => {
            let result = fetch_part(t, ctx, selected, queue, message_id, &part_id).await;
            let failed = result.as_ref().err().cloned();
            reply.send(result);
            if let Some(e) = failed {
                return Err(e);
            }
        }
    }
    Ok(())
}

/// Search server too (section 10): each folder's hits, newest first, until `cap`; the ones
/// not cached are header-fetched into the store. Hits older than a folder's sync window
/// stay outside it (SPEC_GAPS G75). A folder never synced (Gmail's All Mail) is searched
/// all the same.
async fn server_search<T: ImapTransport>(
    t: &mut T,
    ctx: &Ctx<'_>,
    selected: &mut Option<FolderId>,
    folders: &[Folder],
    search: &ServerSearch,
) -> Result<usize, SyncError> {
    use mail_imap::SearchKey;
    let targets: Vec<(&Folder, SearchKey)> = match (&search.gmail_raw, ctx.caps.gmail) {
        (Some(raw), true) => folders
            .iter()
            .filter(|f| f.special_use == Some(mail_store::SpecialUse::All))
            .map(|f| (f, SearchKey::GmailRaw(raw.clone())))
            .collect(),
        _ => match &search.key {
            Some(key) => folders
                .iter()
                .filter(|f| search.folders.contains(&f.id))
                .map(|f| (f, key.clone()))
                .collect(),
            None => Vec::new(),
        },
    };
    let mut remaining = search.cap;
    let mut stored = 0;
    for (folder, key) in targets {
        if remaining == 0 {
            break;
        }
        *selected = None;
        let info = t.select(&folder.path).await?;
        *selected = Some(folder.id);
        if folder.uidvalidity == 0 {
            ctx.store
                .set_uidvalidity(folder.id, info.uidvalidity)
                .await?;
        } else if folder.uidvalidity != info.uidvalidity {
            // A changed mailbox: its regular sync purges it first.
            continue;
        }
        let mut hits = t.uid_search(&key).await?;
        hits.sort_unstable_by(|a, b| b.cmp(a));
        hits.dedup();
        hits.truncate(remaining);
        remaining -= hits.len();
        let known: std::collections::HashSet<u32> = ctx
            .store
            .cached_uids(folder.id)
            .await?
            .into_iter()
            .collect();
        let missing: Vec<u32> = hits.into_iter().filter(|u| !known.contains(u)).collect();
        for batch in missing.chunks(FETCH_BATCH) {
            let headers = t
                .uid_fetch_headers(
                    &UidSet::new(batch.iter().copied()),
                    HeaderFetch {
                        gmail: ctx.caps.gmail,
                    },
                )
                .await?;
            let rows: Vec<_> = headers
                .iter()
                .map(|h| {
                    crate::convert::new_message(h, ctx.account_id, folder.id, now(), ctx.caps.gmail)
                })
                .collect();
            stored += rows.len();
            if !rows.is_empty() {
                ctx.store.insert_synced(folder.id, rows).await?;
            }
        }
        ctx.events.send(SyncEvent::StoreChanged {
            account_id: ctx.account_id,
            folder_id: Some(folder.id),
        });
    }
    Ok(stored)
}

/// Fetches and stores the body of a message the user opened.
async fn open_body<T: ImapTransport>(
    t: &mut T,
    ctx: &Ctx<'_>,
    selected: &mut Option<FolderId>,
    queue: &mut Queue,
    message_id: MessageId,
) -> Result<(), SyncError> {
    let message = ctx.store.get_message(message_id).await?;
    if message.account_id != ctx.account_id || message.body_state != mail_store::BodyState::None {
        return Ok(());
    }
    let folder = ctx.store.get_folder(message.folder_id).await?;
    if !ensure_selected(t, &folder, selected, queue).await? {
        return Err(SyncError::Gone);
    }
    let headers = t
        .uid_fetch_headers(&UidSet::single(message.uid), HeaderFetch::default())
        .await?;
    let Some(h) = headers.into_iter().next() else {
        return Err(SyncError::Gone);
    };
    if let Some((body, state)) =
        body::fetch_body(t, message.uid, &h.body, now(), body::Purpose::Open).await?
    {
        let preview = body::preview(&body.text_plain);
        ctx.store
            .store_body(message_id, body, state, preview)
            .await?;
        ctx.events.send(SyncEvent::BodyStored {
            account_id: ctx.account_id,
            message_id,
        });
        ctx.events.send(SyncEvent::StoreChanged {
            account_id: ctx.account_id,
            folder_id: Some(folder.id),
        });
    }
    Ok(())
}

/// One part, transfer-decoded, or with an empty `part_id` the raw source. The part's
/// encoding comes from a fresh BODYSTRUCTURE, since the store keeps no encoding. Attachment
/// bytes are fetched only here, on demand (section 8, invariant 3).
async fn fetch_part<T: ImapTransport>(
    t: &mut T,
    ctx: &Ctx<'_>,
    selected: &mut Option<FolderId>,
    queue: &mut Queue,
    message_id: MessageId,
    part_id: &str,
) -> Result<Vec<u8>, SyncError> {
    let message = ctx.store.get_message(message_id).await?;
    if message.account_id != ctx.account_id {
        return Err(SyncError::Gone);
    }
    let folder = ctx.store.get_folder(message.folder_id).await?;
    if !ensure_selected(t, &folder, selected, queue).await? {
        return Err(SyncError::Gone);
    }
    if part_id.is_empty() {
        return Ok(t.uid_fetch_part(message.uid, "", None).await?);
    }
    let headers = t
        .uid_fetch_headers(&UidSet::single(message.uid), HeaderFetch::default())
        .await?;
    let Some(h) = headers.into_iter().next() else {
        return Err(SyncError::Gone);
    };
    let encoding = h
        .body
        .leaves()
        .iter()
        .find(|p| p.part_id == part_id)
        .map(|p| p.encoding.clone())
        .ok_or(SyncError::Gone)?;
    let raw = t.uid_fetch_part(message.uid, part_id, None).await?;
    Ok(body::decode_transfer(&raw, encoding.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_job(id: i64) -> Job {
        Job::Body {
            folder_id: 1,
            item: PrefetchItem {
                message_id: id,
                uid: 1,
            },
            structure: Box::new(BodyPart::text_plain(1)),
        }
    }

    #[test]
    fn queue_orders_by_priority_and_deduplicates() {
        let mut q = Queue::default();
        q.push(Priority::Prefetch, body_job(1));
        q.push(Priority::Sync, Job::Sync(1));
        q.push(Priority::Sync, Job::Sync(2));
        q.push(Priority::Sync, Job::Sync(1));
        q.push(Priority::Prefetch, body_job(1));
        q.push(Priority::User, Job::Sync(2));
        assert!(q.has_sync());
        let order: Vec<String> = std::iter::from_fn(|| q.pop())
            .map(|j| format!("{j:?}"))
            .collect();
        assert_eq!(order.len(), 3);
        assert!(order[0].starts_with("Sync(2)"), "{order:?}");
        assert!(order[1].starts_with("Sync(1)"));
        assert!(order[2].starts_with("Body"));
        assert!(!q.has_sync());
    }
}
