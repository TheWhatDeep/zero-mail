//! The seam between the client and an IMAP server. `sync`, `ops`, `outbox` and `search` only
//! ever see these traits.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use crate::types::{
    AppendUid, Capabilities, CopyUid, FetchedHeader, Flag, FlagSync, HeaderFetch, IdleEvent,
    MailboxInfo, SearchKey, Security, SelectInfo, StoreOp, UidSet,
};
use crate::ImapError;

/// A secret that never prints. `Debug` and `Display` show `[redacted]`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The only way to read the value. Call sites are easy to audit.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// How to log in (section 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credentials {
    /// `AUTHENTICATE PLAIN` over TLS, falling back to `LOGIN`.
    Password { username: String, password: Secret },
    /// `AUTHENTICATE XOAUTH2` with an in-memory access token.
    XOAuth2 {
        username: String,
        access_token: Secret,
    },
}

/// Where and how to connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
    /// SHA-256 of the pinned leaf certificate (section 5, Certificates).
    pub pinned_cert_sha256: Option<[u8; 32]>,
    pub connect_timeout: Duration,
}

/// Opens authenticated connections. One connector per account.
pub trait ImapConnector: Send + Sync {
    type Transport: ImapTransport;

    /// TLS (or required STARTTLS) and the capabilities advertised before login, then
    /// LOGOUT. Used to offer a login method during setup (section 5, step 3).
    fn probe(
        &self,
        config: &ConnectConfig,
    ) -> impl Future<Output = Result<Capabilities, ImapError>> + Send;

    /// TLS (or required STARTTLS), then login. Fails with a `NeedsAttention` class error on a
    /// bad login or certificate.
    fn connect(
        &self,
        config: &ConnectConfig,
        credentials: &Credentials,
    ) -> impl Future<Output = Result<Self::Transport, ImapError>> + Send;
}

/// One authenticated IMAP connection. Every method applies the timeouts from section 12 in
/// the real implementation; UIDs refer to the currently selected folder.
pub trait ImapTransport: Send {
    /// Capabilities as advertised after login.
    fn capabilities(&self) -> &Capabilities;

    /// `LIST` (with `RETURN (SPECIAL-USE STATUS ...)` where supported).
    fn list(&mut self) -> impl Future<Output = Result<Vec<MailboxInfo>, ImapError>> + Send;

    /// `SELECT`, read-write.
    fn select(&mut self, path: &str) -> impl Future<Output = Result<SelectInfo, ImapError>> + Send;

    /// `UID SEARCH`. Returns matching UIDs in ascending order.
    fn uid_search(
        &mut self,
        key: &SearchKey,
    ) -> impl Future<Output = Result<Vec<u32>, ImapError>> + Send;

    /// Header fetch for at most [`crate::FETCH_BATCH`] UIDs.
    fn uid_fetch_headers(
        &mut self,
        uids: &UidSet,
        options: HeaderFetch,
    ) -> impl Future<Output = Result<Vec<FetchedHeader>, ImapError>> + Send;

    /// `UID FETCH ... (FLAGS)`, with `CHANGEDSINCE` when `changed_since` is set (CONDSTORE)
    /// and `VANISHED` when the server supports QRESYNC.
    fn uid_fetch_flags(
        &mut self,
        uids: &UidSet,
        changed_since: Option<u64>,
    ) -> impl Future<Output = Result<FlagSync, ImapError>> + Send;

    /// `UID FETCH 1:up_to (UID FLAGS MODSEQ) (CHANGEDSINCE m VANISHED)`: every flag change
    /// since `changed_since` among UIDs up to `up_to`, and with QRESYNC the UIDs that
    /// vanished. One short command however many messages are cached (section 6, steps 5
    /// and 6). Fails with `Unsupported("CONDSTORE")` when not advertised.
    fn uid_fetch_changes(
        &mut self,
        up_to: u32,
        changed_since: u64,
    ) -> impl Future<Output = Result<FlagSync, ImapError>> + Send;

    /// `UID FETCH uid BODY.PEEK[part]<0.max_bytes>`. An empty `part` fetches the whole raw
    /// message (used only by Save source and forward-as-attachment).
    fn uid_fetch_part(
        &mut self,
        uid: u32,
        part: &str,
        max_bytes: Option<u64>,
    ) -> impl Future<Output = Result<Vec<u8>, ImapError>> + Send;

    /// `UID STORE uids (+|-)FLAGS.SILENT (flags)`.
    fn uid_store(
        &mut self,
        uids: &UidSet,
        op: StoreOp,
        flags: &[Flag],
    ) -> impl Future<Output = Result<(), ImapError>> + Send;

    /// `UID MOVE`. Fails with `Unsupported("MOVE")` when not advertised.
    fn uid_move(
        &mut self,
        uids: &UidSet,
        dest: &str,
    ) -> impl Future<Output = Result<Option<CopyUid>, ImapError>> + Send;

    /// `UID COPY`. Returns `COPYUID` when the server supports UIDPLUS.
    fn uid_copy(
        &mut self,
        uids: &UidSet,
        dest: &str,
    ) -> impl Future<Output = Result<Option<CopyUid>, ImapError>> + Send;

    /// `UID EXPUNGE` of exactly these UIDs. Fails with `Unsupported("UIDPLUS")` when not
    /// advertised.
    fn uid_expunge(&mut self, uids: &UidSet) -> impl Future<Output = Result<(), ImapError>> + Send;

    /// Bare `EXPUNGE`: removes every `\Deleted` message in the folder. Callers must check
    /// `UID SEARCH DELETED` first (section 9).
    fn expunge(&mut self) -> impl Future<Output = Result<(), ImapError>> + Send;

    /// `APPEND` a raw message. Returns `APPENDUID` when the server supports UIDPLUS.
    fn append(
        &mut self,
        folder: &str,
        flags: &[Flag],
        raw: &[u8],
    ) -> impl Future<Output = Result<Option<AppendUid>, ImapError>> + Send;

    fn create(&mut self, path: &str) -> impl Future<Output = Result<(), ImapError>> + Send;

    fn rename(
        &mut self,
        from: &str,
        to: &str,
    ) -> impl Future<Output = Result<(), ImapError>> + Send;

    fn delete(&mut self, path: &str) -> impl Future<Output = Result<(), ImapError>> + Send;

    /// `IDLE` on the selected folder until news arrives or `max_wait` passes. Without the
    /// IDLE capability this fails with `Unsupported("IDLE")` and the actor polls instead.
    fn idle(
        &mut self,
        max_wait: Duration,
    ) -> impl Future<Output = Result<IdleEvent, ImapError>> + Send;

    fn noop(&mut self) -> impl Future<Output = Result<(), ImapError>> + Send;

    fn logout(self) -> impl Future<Output = Result<(), ImapError>> + Send
    where
        Self: Sized;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_print() {
        let creds = Credentials::Password {
            username: "u@example.test".into(),
            password: Secret::new("hunter2"),
        };
        let shown = format!("{creds:?} {}", Secret::new("hunter2"));
        assert!(!shown.contains("hunter2"));
        assert!(shown.contains("[redacted]"));
    }
}
