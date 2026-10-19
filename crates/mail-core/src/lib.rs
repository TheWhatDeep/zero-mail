#![forbid(unsafe_code)]
//! `mail-core` (BUILD.md section 16): wires the module crates into one API. It is the only
//! crate `app` imports.
//!
//! M2: the [`AppError`] contract (section 3, SPEC_GAPS G18) and account CRUD ([`Accounts`]).

pub mod accounts;
mod error;

pub use accounts::{
    AccountConfig, AccountSummary, Accounts, AuthKind, Candidate, ConfigSource, Endpoint, Security,
    TestReport, ONLINE_LOOKUPS,
};
pub use error::{AppError, ErrorCode};
