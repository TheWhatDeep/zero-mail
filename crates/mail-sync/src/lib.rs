#![forbid(unsafe_code)]
//! `sync` (BUILD.md section 3): per-account actors, IDLE, backoff, and applying server
//! state to the store. May call `imap`, `store` and `auth`; never renders HTML.
//!
//! One actor per account ([`spawn_account`]) owns that account's IMAP connections: a
//! worker connection that runs the job queue, an IDLE connection on INBOX, and one IDLE
//! connection per watched folder, at most 5 in all (section 6). It pushes nothing yet
//! (`ops` arrives in M6), pulls every enabled folder in the section 6 order, then waits for
//! news. Everything it learns goes to the store first; the UI hears about it through
//! [`SyncEvent`]s and re-queries (section 3, One data path).

mod actor;
pub mod backoff;
mod body;
mod convert;
mod folder;
pub mod ops;

pub use actor::{login_request, spawn_account, AccountHandle, ServerSearch, SyncDeps};
pub use body::preview;
pub use folder::MUTED_KEYWORD;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mail_auth::AuthError;
use mail_imap::{ErrorClass, ImapError};
use mail_store::{AccountId, FolderId, StoreError};
use tokio::sync::mpsc;

/// Without IDLE, INBOX is polled this often (section 6).
pub const POLL_INTERVAL: Duration = Duration::from_secs(2 * 60);
/// A folder open in the UI syncs this often (section 6, step 9).
pub const OPEN_FOLDER_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Folders the user may watch with their own IDLE connection (section 6).
pub const MAX_WATCHED: usize = 3;
/// Bodies prefetched per folder: its newest messages (section 6, step 8).
pub const PREFETCH_COUNT: u32 = 200;
/// Undo stays available this long after an action (section 9: a 10 s toast).
pub const UNDO_WINDOW_SECS: i64 = 10;
/// Load older adds this many days per click (section 12).
pub const EXTEND_DAYS: i64 = 90;

/// Account states (section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountState {
    Disabled,
    Connecting,
    Syncing,
    Idle,
    Backoff,
    NeedsAttention,
}

/// Why an account left the happy path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    Imap(ImapError),
    Auth(AuthError),
    /// The store refused a write, for example `disk_full` (section 12).
    Store(String),
    /// A folder or message vanished while a job was queued; that job is skipped.
    Gone,
}

impl SyncError {
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Imap(e) => e.class(),
            // Google's token endpoint out of reach: try again later.
            Self::Auth(e) if e.is_transient() => ErrorClass::Transient,
            // No usable credentials: only the user can fix it.
            Self::Auth(_) => ErrorClass::NeedsAttention,
            Self::Store(_) => ErrorClass::Transient,
            Self::Gone => ErrorClass::Permanent,
        }
    }
}

impl From<ImapError> for SyncError {
    fn from(e: ImapError) -> Self {
        Self::Imap(e)
    }
}

impl From<AuthError> for SyncError {
    fn from(e: AuthError) -> Self {
        Self::Auth(e)
    }
}

impl From<StoreError> for SyncError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound => Self::Gone,
            other => Self::Store(other.to_string()),
        }
    }
}

/// What sync tells the rest of the app (section 3, events). `mail-core` forwards these
/// to the UI, debouncing `StoreChanged` to 100 ms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncEvent {
    /// A committed write. `folder_id` is `None` when the folder list itself changed.
    StoreChanged {
        account_id: AccountId,
        folder_id: Option<FolderId>,
    },
    /// Every state change, with the error that caused it and the wait before the next
    /// attempt, when there is one.
    AccountState {
        account_id: AccountId,
        state: AccountState,
        last_error: Option<SyncError>,
        retry_in: Option<Duration>,
    },
    /// During a folder's first sync.
    SyncProgress {
        account_id: AccountId,
        folder_id: FolderId,
        fetched: u32,
        total: u32,
    },
    /// A UIDVALIDITY change purged the folder (SPEC_GAPS G37).
    FolderReset {
        account_id: AccountId,
        folder_id: FolderId,
    },
    /// The number of unfinished ops may have changed (`pending-count`).
    PendingChanged { account_id: AccountId },
    /// New messages arrived in a folder and were stored, after that folder's sync. Not
    /// sent for a folder's first sync. Rules, mute and snooze react to it (section 18), and
    /// notifications (section 11).
    NewMail {
        account_id: AccountId,
        folder_id: FolderId,
        message_ids: Vec<mail_store::MessageId>,
    },
    /// A message's body was stored: rules with body conditions can run (section 18.6).
    BodyStored {
        account_id: AccountId,
        message_id: mail_store::MessageId,
    },
    /// The server refused an op for good; its overlay was reverted (SPEC_GAPS G58).
    /// `op_id` is the Undo group the UI knows.
    OpFailed {
        account_id: AccountId,
        op_id: mail_store::OpId,
        kind: String,
        error: SyncError,
    },
}

/// Where events go. A closed receiver drops them; sync never blocks on the UI.
#[derive(Debug, Clone)]
pub struct Events(mpsc::UnboundedSender<SyncEvent>);

impl Events {
    pub fn new(tx: mpsc::UnboundedSender<SyncEvent>) -> Self {
        Self(tx)
    }

    pub(crate) fn send(&self, event: SyncEvent) {
        let _ = self.0.send(event);
    }
}

/// UTC epoch seconds. Sync does not depend on the local clock for correctness (section
/// 12); this sets only the first-sync window and `synced_at`.
pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
