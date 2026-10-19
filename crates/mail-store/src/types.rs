//! Row types for the store API. All times are UTC epoch seconds (invariant 6).

use serde::{Deserialize, Serialize};

pub type AccountId = i64;
pub type FolderId = i64;
pub type ThreadId = i64;
pub type MessageId = i64;

/// Transport security for an endpoint. There is no plaintext option (section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Tls,
    StartTls,
}

impl Security {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            Self::Tls => "tls",
            Self::StartTls => "starttls",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    Password,
    OauthGoogle,
}

impl AuthKind {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::OauthGoogle => "oauth_google",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveSentCopy {
    Auto,
    On,
    Off,
}

impl SaveSentCopy {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

/// A folder role (`folders.special_use`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialUse {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
    All,
}

impl SpecialUse {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Sent => "sent",
            Self::Drafts => "drafts",
            Self::Trash => "trash",
            Self::Junk => "junk",
            Self::Archive => "archive",
            Self::All => "all",
        }
    }
}

/// How much of a message body is cached (`messages.body_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyState {
    None,
    Text,
    Full,
}

impl BodyState {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Text => "text",
            Self::Full => "full",
        }
    }
}

/// `messages.flags`: what the user sees, server flags with pending ops on top (invariant 2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags(pub u8);

impl Flags {
    pub const SEEN: u8 = 1 << 0;
    pub const FLAGGED: u8 = 1 << 1;
    pub const ANSWERED: u8 = 1 << 2;
    pub const DRAFT: u8 = 1 << 3;
    pub const FORWARDED: u8 = 1 << 4;
    /// `\Deleted` on the server. Such messages are hidden in every view (invariant 8).
    pub const DELETED: u8 = 1 << 5;

    pub fn contains(self, bit: u8) -> bool {
        self.0 & bit == bit
    }
}

/// One mailbox in an address list. `to_json`, `cc_json` and `reply_to_json` hold JSON arrays
/// of `[name, addr]` pairs: no keys, so the FTS index sees only names and addresses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "(String, String)", into = "(String, String)")]
pub struct Address {
    pub name: String,
    pub addr: String,
}

impl From<(String, String)> for Address {
    fn from((name, addr): (String, String)) -> Self {
        Self { name, addr }
    }
}

impl From<Address> for (String, String) {
    fn from(a: Address) -> Self {
        (a.name, a.addr)
    }
}

/// Columns of a new `accounts` row. No secrets: the keychain holds them by account id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAccount {
    pub name: String,
    pub email: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: Security,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: Security,
    pub username: String,
    pub auth_kind: AuthKind,
    /// The user's own OAuth client ID (section 5); empty for password accounts.
    pub oauth_client_id: String,
    pub pinned_cert_sha256: Option<String>,
    pub sync_days: u32,
    pub save_sent_copy: SaveSentCopy,
    pub signature_html: String,
    pub signature_text: String,
    pub enabled: bool,
    pub created_at: i64,
}

/// A folder as `LIST` reports it, before it is first selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFolder {
    pub account_id: AccountId,
    pub path: String,
    /// Hierarchy delimiter; empty when the server reports `NIL`.
    pub delimiter: String,
    pub special_use: Option<SpecialUse>,
    pub sync_enabled: bool,
    pub watch_idle: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAttachment {
    pub part_id: String,
    pub filename: String,
    pub mime: String,
    pub size: u64,
    pub content_id: Option<String>,
    pub is_inline: bool,
}

/// A message header row as sync writes it, with its attachment metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    pub uid: u32,
    pub gm_msgid: Option<i64>,
    pub gm_thrid: Option<i64>,
    pub message_id: String,
    pub in_reply_to: Option<String>,
    pub refs: Option<String>,
    pub gm_labels: Option<Vec<String>>,
    pub thread_id: ThreadId,
    pub subject: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub date_utc: i64,
    pub internal_utc: i64,
    pub size: u64,
    pub flags: Flags,
    pub has_attach: bool,
    pub preview: String,
    pub body_state: BodyState,
    pub attachments: Vec<NewAttachment>,
}

/// Cached body parts of one message. HTML is stored unsanitized (invariant 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    pub text_plain: String,
    pub text_html: String,
    pub fetched_at: i64,
}

/// Keyset pagination cursor: the `(date, id)` of the last row already shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub date_utc: i64,
    pub id: MessageId,
}

/// One message row in a list (section 11, middle pane).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    pub id: MessageId,
    pub account_id: AccountId,
    pub folder_id: FolderId,
    pub thread_id: ThreadId,
    pub subject: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub date_utc: i64,
    pub flags: Flags,
    pub has_attach: bool,
    pub preview: String,
}

/// A page of rows, newest first, and the cursor for the next page (`None` at the end).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub rows: Vec<ListRow>,
    pub next: Option<Cursor>,
}
