#![forbid(unsafe_code)]
//! `mail-core` (BUILD.md section 16): wires the module crates into one API. It is the only
//! crate `app` imports.
//!
//! - The [`AppError`] contract (section 3, SPEC_GAPS G18) and account CRUD ([`Accounts`]), M2.
//! - [`Core`]: the commands, actors and events the app wires up, M4.

pub mod accounts;
mod core;
mod error;
pub mod views;

pub use crate::core::{
    AccountView, Core, CoreConfig, CoreEvent, EventSink, StateName, QUICK_CHECK_DELAY,
    STORE_CHANGED_DEBOUNCE,
};
pub use mail_auth::HttpClient;
pub use mail_imap::ImapClient;
pub use mail_render::BodyResponse;
pub use mail_secrets::Keychain;
pub use mail_smtp::SmtpClient;

pub use accounts::{
    AccountConfig, AccountSummary, Accounts, AuthKind, Candidate, ConfigSource, Endpoint, Security,
    TestReport, ONLINE_LOOKUPS,
};
pub use error::{AppError, ErrorCode};
