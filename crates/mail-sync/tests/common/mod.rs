#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mail_auth::{AuthError, AuthMethod, Login, LoginRequest, LoginSource, Secret};
use mail_imap::fake::{FakeMessage, FakeServer, FakeTransport};
use mail_imap::{ConnectConfig, Credentials, ImapConnector, ImapError};
use mail_store::*;
use mail_sync::{spawn_account, AccountHandle, AccountState, Events, SyncDeps, SyncEvent};
use tokio::sync::mpsc;

pub const DAY: i64 = 86_400;

/// A scratch directory under the OS temp dir, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zero-mail-sync-{tag}-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn db(&self) -> PathBuf {
        self.0.join("mail.db")
    }

    pub fn store(&self) -> Store {
        Store::open(&self.db()).unwrap()
    }

    pub fn raw(&self) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(self.db()).unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        conn
    }

    pub fn count(&self, sql: &str) -> i64 {
        self.raw().query_row(sql, [], |r| r.get(0)).unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Passwords by username, standing in for the keychain.
#[derive(Debug, Default, Clone)]
pub struct Logins(Arc<Mutex<BTreeMap<String, String>>>);

impl Logins {
    pub fn set(&self, username: &str, password: &str) {
        self.0
            .lock()
            .unwrap()
            .insert(username.into(), password.into());
    }
}

impl LoginSource for Logins {
    async fn login(&self, request: &LoginRequest) -> Result<Login, AuthError> {
        let username = request.username.as_str();
        let password = self
            .0
            .lock()
            .unwrap()
            .get(username)
            .cloned()
            .ok_or(AuthError::MissingSecret)?;
        Ok(Login {
            username: username.into(),
            secret: Secret::new(password),
            method: AuthMethod::Password,
        })
    }
}

/// Wraps a fake server and records the tokio time of every connect attempt.
#[derive(Debug, Clone)]
pub struct Recording {
    pub server: FakeServer,
    pub attempts: Arc<Mutex<Vec<tokio::time::Instant>>>,
}

impl Recording {
    pub fn new(server: FakeServer) -> Self {
        Self {
            server,
            attempts: Arc::default(),
        }
    }

    pub fn attempts(&self) -> Vec<tokio::time::Instant> {
        self.attempts.lock().unwrap().clone()
    }
}

impl ImapConnector for Recording {
    type Transport = FakeTransport;

    async fn probe(&self, config: &ConnectConfig) -> Result<mail_imap::Capabilities, ImapError> {
        self.server.probe(config).await
    }

    async fn connect(
        &self,
        config: &ConnectConfig,
        credentials: &Credentials,
    ) -> Result<FakeTransport, ImapError> {
        self.attempts
            .lock()
            .unwrap()
            .push(tokio::time::Instant::now());
        self.server.connect(config, credentials).await
    }
}

pub fn new_account(email: &str) -> NewAccount {
    NewAccount {
        name: "Test".into(),
        email: email.into(),
        imap_host: "imap.test.local".into(),
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: "smtp.test.local".into(),
        smtp_port: 465,
        smtp_security: Security::Tls,
        username: email.into(),
        auth_kind: AuthKind::Password,
        oauth_client_id: String::new(),
        pinned_cert_sha256: None,
        sync_days: 90,
        save_sent_copy: SaveSentCopy::Auto,
        signature_html: String::new(),
        signature_text: String::new(),
        enabled: true,
        created_at: 0,
    }
}

pub async fn add_account(store: &Store, email: &str) -> AccountId {
    store
        .add_account(
            new_account(email),
            NewIdentity {
                name: "Test".into(),
                address: email.into(),
                reply_to: None,
                is_default: true,
            },
        )
        .await
        .unwrap()
        .0
}

pub fn msg(n: u32, date: i64) -> FakeMessage {
    FakeMessage::simple(
        &format!("m{n}@test.local"),
        &format!("Sender {n} <s{n}@test.local>"),
        "me@test.local",
        &format!("Subject {n}"),
        &format!("Body of message {n}\r\n> quoted\r\n-- \r\nsig"),
        date,
    )
}

/// Collects events and waits for the ones a test expects.
pub struct Watch {
    rx: mpsc::UnboundedReceiver<SyncEvent>,
    pub seen: Vec<SyncEvent>,
    /// The latest AccountState event per account.
    states: BTreeMap<AccountId, SyncEvent>,
}

impl Watch {
    pub fn new() -> (Events, Self) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Events::new(tx),
            Self {
                rx,
                seen: Vec::new(),
                states: BTreeMap::new(),
            },
        )
    }

    /// The next event matching `pred`. Waits at most `limit` of tokio time when given.
    pub async fn until(
        &mut self,
        limit: Option<Duration>,
        mut pred: impl FnMut(&SyncEvent) -> bool,
    ) -> SyncEvent {
        loop {
            let next = match limit {
                Some(limit) => tokio::time::timeout(limit, self.rx.recv())
                    .await
                    .expect("event did not arrive in time"),
                None => self.rx.recv().await,
            };
            let event = next.expect("event channel closed");
            self.record(&event);
            if pred(&event) {
                return event;
            }
        }
    }

    fn record(&mut self, event: &SyncEvent) {
        self.seen.push(event.clone());
        if let SyncEvent::AccountState { account_id, .. } = event {
            self.states.insert(*account_id, event.clone());
        }
    }

    /// Waits until the account's latest state is `state` (now, or by a later event).
    pub async fn state(&mut self, account: AccountId, state: AccountState) -> SyncEvent {
        self.state_within(account, state, Some(Duration::from_secs(20)))
            .await
    }

    /// As [`Watch::state`]; `None` waits without a timer, for paused-time tests.
    pub async fn state_within(
        &mut self,
        account: AccountId,
        state: AccountState,
        limit: Option<Duration>,
    ) -> SyncEvent {
        if let Some(e @ SyncEvent::AccountState { state: s, .. }) = self.states.get(&account) {
            if *s == state {
                return e.clone();
            }
        }
        self.until(limit, |e| {
            matches!(e, SyncEvent::AccountState { account_id, state: s, .. } if *account_id == account && *s == state)
        })
        .await
    }

    /// Everything received so far, without waiting.
    pub fn drain(&mut self) -> Vec<SyncEvent> {
        while let Ok(e) = self.rx.try_recv() {
            self.record(&e);
        }
        self.seen.clone()
    }
}

pub fn spawn<C: ImapConnector + 'static>(
    store: &Store,
    connector: C,
    logins: &Logins,
    events: &Events,
    account_id: AccountId,
) -> AccountHandle {
    spawn_account(
        SyncDeps {
            store: store.clone(),
            connector: Arc::new(connector),
            logins: Arc::new(logins.clone()),
            events: events.clone(),
        },
        account_id,
    )
}

pub async fn folder_id(store: &Store, account: AccountId, path: &str) -> FolderId {
    store
        .list_folders(account)
        .await
        .unwrap()
        .into_iter()
        .find(|f| f.path == path)
        .unwrap()
        .id
}

/// Every visible row of a folder, newest first, by walking keyset pages.
pub async fn rows(store: &Store, folder: FolderId) -> Vec<ListRow> {
    let mut out = Vec::new();
    let mut cursor = None;
    loop {
        let page = store.folder_page(folder, cursor, 100).await.unwrap();
        out.extend(page.rows);
        match page.next {
            Some(c) => cursor = Some(c),
            None => return out,
        }
    }
}

/// Polls `cond` until it holds, at most 20 s of real time.
pub async fn eventually(mut cond: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !cond() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "condition not reached"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
