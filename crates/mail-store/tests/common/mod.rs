#![allow(dead_code)]

use mail_store::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

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
            "zero-mail-store-{tag}-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn db(&self) -> PathBuf {
        self.0.join("mail.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A raw connection for inspecting what the store wrote.
pub fn raw(path: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn
}

pub fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// FTS5 integrity check, including agreement with the external content (rank = 1).
pub fn fts_consistent(conn: &rusqlite::Connection) -> bool {
    conn.execute(
        "INSERT INTO message_fts(message_fts, rank) VALUES ('integrity-check', 1)",
        [],
    )
    .is_ok()
}

pub fn account(email: &str) -> NewAccount {
    NewAccount {
        name: "Test".into(),
        email: email.into(),
        imap_host: "127.0.0.1".into(),
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: "127.0.0.1".into(),
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
        created_at: 1_760_000_000,
    }
}

pub fn folder(account_id: AccountId, path: &str) -> NewFolder {
    NewFolder {
        account_id,
        path: path.into(),
        delimiter: "/".into(),
        special_use: (path == "INBOX").then_some(SpecialUse::Inbox),
        sync_enabled: true,
        watch_idle: false,
    }
}

pub fn message(uid: u32, date_utc: i64, thread_id: ThreadId) -> NewMessage {
    NewMessage {
        uid,
        gm_msgid: None,
        gm_thrid: None,
        message_id: format!("<{uid}.{date_utc}@example.test>"),
        in_reply_to: None,
        refs: None,
        gm_labels: None,
        list_id: None,
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        thread_id,
        subject: format!("Subject {uid}"),
        from: Address {
            name: "Alice Example".into(),
            addr: "alice@example.test".into(),
        },
        to: vec![Address {
            name: "Bob".into(),
            addr: "bob@example.test".into(),
        }],
        cc: Vec::new(),
        reply_to: Vec::new(),
        date_utc,
        internal_utc: date_utc,
        size: 1024,
        flags: Flags::default(),
        has_attach: false,
        preview: String::new(),
        body_state: BodyState::None,
        attachments: Vec::new(),
    }
}

/// An account with one INBOX and one thread, ready for messages.
pub async fn seeded(store: &Store) -> (AccountId, FolderId, ThreadId) {
    let account = store
        .insert_account(account("me@example.test"))
        .await
        .unwrap();
    let inbox = store.insert_folder(folder(account, "INBOX")).await.unwrap();
    let thread = store
        .insert_thread(account, 0, String::new())
        .await
        .unwrap();
    (account, inbox, thread)
}
