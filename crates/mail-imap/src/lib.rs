#![forbid(unsafe_code)]
//! `imap` (BUILD.md section 3): the transport trait, command wrappers, capability detection
//! and server quirks. Never touches the database.
//!
//! Everything above this crate talks to a server only through [`ImapConnector`] and
//! [`ImapTransport`], so the `async-imap` backed implementation ([`ImapClient`]) and the
//! in-memory fake (`fake` feature, used by tests) are interchangeable.

mod error;
mod net;
mod parse;
mod roles;
mod session;
mod transport;
mod types;
pub mod utf7;

#[cfg(any(test, feature = "fake"))]
pub mod fake;

pub use error::{ErrorClass, ImapError};
pub use net::{hex, parse_fingerprint};
pub use roles::assign_roles;
pub use session::{ImapClient, ImapSession, HEADER_FIELDS};
pub use transport::{ConnectConfig, Credentials, ImapConnector, ImapTransport, Secret};
pub use types::{
    AppendUid, BodyPart, Capabilities, CopyUid, FetchedHeader, Flag, FlagSync, FlagUpdate,
    HeaderFetch, IdleEvent, ImapDate, MailboxInfo, MailboxStatus, SearchKey, Security, SelectInfo,
    SpecialUse, StoreOp, UidSet,
};

use std::time::Duration;

/// Timeout for normal commands (section 12).
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// Timeout for large fetches (section 12).
pub const LARGE_FETCH_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// TCP keepalive interval on every connection (section 12).
pub const TCP_KEEPALIVE: Duration = Duration::from_secs(60);
/// IDLE is re-issued after this long (section 6, step 9).
pub const IDLE_REISSUE: Duration = Duration::from_secs(25 * 60);
/// Read timeout while in IDLE: 30 min 30 s (section 12).
pub const IDLE_READ_TIMEOUT: Duration = Duration::from_secs(30 * 60 + 30);
/// Maximum UIDs per command (section 9).
pub const MAX_UIDS_PER_COMMAND: usize = 500;
/// Header fetch batch size (section 6, step 4).
pub const FETCH_BATCH: usize = 100;
/// Connect timeout (TCP, TLS and greeting) used unless a caller sets another.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
