//! What the UI receives from the M4 commands (section 3): folder trees, list pages and the
//! reader's display model. Plain data, serialized for IPC; no secrets, no SQL.

use mail_store::{
    Address, AttachmentRecord, BodyState, Cursor, Flags, Folder, ListRow, MessageRecord, SpecialUse,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AddressView {
    pub name: String,
    pub addr: String,
}

impl From<&Address> for AddressView {
    fn from(a: &Address) -> Self {
        Self {
            name: a.name.clone(),
            addr: a.addr.clone(),
        }
    }
}

fn addresses(list: &[Address]) -> Vec<AddressView> {
    list.iter().map(AddressView::from).collect()
}

/// A folder role (`folders.special_use`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Inbox,
    Drafts,
    Sent,
    Archive,
    Junk,
    Trash,
    All,
}

impl From<Role> for SpecialUse {
    fn from(r: Role) -> Self {
        match r {
            Role::Inbox => Self::Inbox,
            Role::Drafts => Self::Drafts,
            Role::Sent => Self::Sent,
            Role::Archive => Self::Archive,
            Role::Junk => Self::Junk,
            Role::Trash => Self::Trash,
            Role::All => Self::All,
        }
    }
}

impl From<SpecialUse> for Role {
    fn from(s: SpecialUse) -> Self {
        match s {
            SpecialUse::Inbox => Self::Inbox,
            SpecialUse::Drafts => Self::Drafts,
            SpecialUse::Sent => Self::Sent,
            SpecialUse::Archive => Self::Archive,
            SpecialUse::Junk => Self::Junk,
            SpecialUse::Trash => Self::Trash,
            SpecialUse::All => Self::All,
        }
    }
}

/// One folder of `folder.list`, in display order, with its depth in the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderView {
    pub id: i64,
    pub account_id: i64,
    pub path: String,
    /// The last path segment.
    pub name: String,
    /// How many listed ancestors it has.
    pub depth: u32,
    pub role: Option<Role>,
    pub unread: u32,
    pub total: u32,
    pub sync_enabled: bool,
    pub watch_idle: bool,
    /// A full sync has completed once; before that an empty folder shows skeleton rows
    /// (section 11, States).
    pub synced: bool,
    /// Gmail's Important and Starred while their sync is off: the sidebar omits them
    /// (section 9; SPEC_GAPS G61).
    pub hidden: bool,
}

/// Gmail's system folders live under `[Gmail]/` (or `[Google Mail]/`); the ones without a
/// role there are the virtual Important and Starred (G61).
pub fn gmail_virtual_hidden(f: &Folder) -> bool {
    !f.sync_enabled
        && f.special_use.is_none()
        && (f.path.starts_with("[Gmail]/") || f.path.starts_with("[Google Mail]/"))
}

/// Folder list order (section 9): Inbox, Drafts, Sent, Archive, Junk, Trash pinned at the
/// top, then every other folder alphabetically, children under their parent.
pub fn folder_views(folders: &[Folder]) -> Vec<FolderView> {
    let rank = |f: &Folder| match f.special_use {
        Some(SpecialUse::Inbox) => 0,
        Some(SpecialUse::Drafts) => 1,
        Some(SpecialUse::Sent) => 2,
        Some(SpecialUse::Archive) => 3,
        Some(SpecialUse::Junk) => 4,
        Some(SpecialUse::Trash) => 5,
        _ => 6,
    };
    let segments = |f: &Folder| -> Vec<String> {
        match f.delimiter.chars().next() {
            Some(d) => f.path.split(d).map(str::to_lowercase).collect(),
            None => vec![f.path.to_lowercase()],
        }
    };
    let mut sorted: Vec<&Folder> = folders.iter().collect();
    sorted.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| segments(a).cmp(&segments(b)))
    });
    sorted
        .iter()
        .map(|f| {
            let pinned = rank(f) < 6;
            let delim = f.delimiter.chars().next();
            let (depth, name) = match delim {
                Some(d) if !pinned => {
                    let parts: Vec<&str> = f.path.split(d).collect();
                    // Count the ancestors that are themselves listed folders.
                    let depth = (1..parts.len())
                        .filter(|i| {
                            let prefix = parts[..*i].join(&d.to_string());
                            folders.iter().any(|o| o.path == prefix)
                        })
                        .count();
                    let name = if depth == 0 {
                        f.path.clone()
                    } else {
                        parts.last().map(|s| (*s).to_string()).unwrap_or_default()
                    };
                    (depth, name)
                }
                _ => (0, f.path.clone()),
            };
            FolderView {
                id: f.id,
                account_id: f.account_id,
                path: f.path.clone(),
                name: if f.special_use == Some(SpecialUse::Inbox) {
                    "Inbox".into()
                } else {
                    name
                },
                depth: u32::try_from(depth).unwrap_or(0),
                role: f.special_use.map(Role::from),
                unread: f.unread,
                total: f.total,
                sync_enabled: f.sync_enabled,
                watch_idle: f.watch_idle,
                synced: f.synced_at > 0,
                hidden: gmail_virtual_hidden(f),
            }
        })
        .collect()
}

/// `list.page` scope (section 3). A thread takes the open folder, for the Trash, Junk and
/// Drafts rule (section 10, rule 6; SPEC_GAPS G77).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    Folder {
        folder_id: i64,
    },
    Unified,
    Search {
        query: String,
        /// `None`: every account (section 10, Scope).
        #[serde(default)]
        account_id: Option<i64>,
    },
    Thread {
        thread_id: i64,
        #[serde(default)]
        folder_id: Option<i64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorView {
    pub date_utc: i64,
    pub id: i64,
}

impl From<CursorView> for Cursor {
    fn from(c: CursorView) -> Self {
        Cursor {
            date_utc: c.date_utc,
            id: c.id,
        }
    }
}

impl From<Cursor> for CursorView {
    fn from(c: Cursor) -> Self {
        Self {
            date_utc: c.date_utc,
            id: c.id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ListRequest {
    pub scope: Scope,
    pub cursor: Option<CursorView>,
    pub limit: u32,
    #[serde(default)]
    pub group_by_thread: bool,
}

/// One list row (section 11, middle pane).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowView {
    pub id: i64,
    pub account_id: i64,
    pub folder_id: i64,
    pub thread_id: i64,
    pub subject: String,
    pub from: AddressView,
    pub to: Vec<AddressView>,
    pub date_utc: i64,
    pub unread: bool,
    pub flagged: bool,
    pub has_attach: bool,
    pub preview: String,
    /// With Group by conversation: the thread this row stands for (section 10, rule 5).
    pub thread: Option<ThreadInfo>,
}

/// A thread row's counts and up to 3 participant names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThreadInfo {
    pub messages: u32,
    pub unread: u32,
    pub participants: Vec<String>,
}

impl From<&ListRow> for RowView {
    fn from(r: &ListRow) -> Self {
        Self {
            id: r.id,
            account_id: r.account_id,
            folder_id: r.folder_id,
            thread_id: r.thread_id,
            subject: r.subject.clone(),
            from: AddressView::from(&r.from),
            to: addresses(&r.to),
            date_utc: r.date_utc,
            unread: !r.flags.contains(Flags::SEEN),
            flagged: r.flags.contains(Flags::FLAGGED),
            has_attach: r.has_attach,
            preview: r.preview.clone(),
            thread: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageView {
    pub rows: Vec<RowView>,
    pub next: Option<CursorView>,
    /// Search only, first page: messages in scope whose body is not indexed yet; the UI
    /// offers Search server too (section 10).
    pub unindexed: Option<u32>,
}

/// How the reader shows the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    /// Sanitized HTML from `mail-body` (section 8).
    Html,
    /// Plain text from `mail-body`.
    Plain,
    /// Not cached yet; it is being fetched, and `store-changed` follows.
    Loading,
    /// The body could not be fetched or read: Could not display this message, with Save
    /// source (section 12).
    Unreadable,
}

/// Remote images in the HTML body (section 8): none, blocked (the banner shows), or
/// allowed for this message or its sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteImages {
    None,
    Blocked,
    Allowed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentView {
    pub id: i64,
    /// The name as the sender meant it (encoded words decoded).
    pub filename: String,
    pub mime: String,
    pub size: u64,
    /// Open asks first: an executable type (section 8).
    pub executable: bool,
}

/// What `message.open` knows about the body beyond the header row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyInfo {
    pub has_html: bool,
    pub remote_images: RemoteImages,
    pub unreadable: bool,
    pub can_archive: bool,
    pub delete_is_permanent: bool,
}

/// `message.open`'s display model (section 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageView {
    pub id: i64,
    pub account_id: i64,
    pub folder_id: i64,
    pub subject: String,
    pub from: AddressView,
    pub to: Vec<AddressView>,
    pub cc: Vec<AddressView>,
    pub reply_to: Vec<AddressView>,
    pub date_utc: i64,
    pub unread: bool,
    pub flagged: bool,
    pub body_kind: BodyKind,
    /// A text part was cut at 1 MB: Message too large, with Save source (section 12).
    pub truncated: bool,
    /// The body has an HTML part, so the plain-text toggle applies (SPEC_GAPS G53).
    pub has_html: bool,
    /// `mail-body://localhost/<id>?images=0|1`. The UI maps it to the platform's form and
    /// adds `&plain=1` for the toggle.
    pub body_url: String,
    pub attachments: Vec<AttachmentView>,
    pub remote_images: RemoteImages,
    /// The account has an archive folder (section 9: otherwise Archive is hidden).
    pub can_archive: bool,
    /// Delete here is permanent and asks first (section 9).
    pub delete_is_permanent: bool,
    /// Gmail only (section 18.1): one chip per label; `None` elsewhere, and the label UI
    /// is hidden.
    pub labels: Option<Vec<LabelView>>,
    /// Gmail's `\Important`, shown as a read-only badge.
    pub important: bool,
}

/// A folder's display name: Inbox, or the last segment of its path.
pub fn folder_name(f: &Folder) -> String {
    if f.special_use == Some(SpecialUse::Inbox) {
        return "Inbox".into();
    }
    match f.delimiter.as_str() {
        "" => f.path.clone(),
        d => f.path.rsplit(d).next().unwrap_or(&f.path).to_string(),
    }
}

/// A label chip: the folder the label is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LabelView {
    pub folder_id: i64,
    pub name: String,
}

pub fn message_view(
    m: &MessageRecord,
    attachments: &[AttachmentRecord],
    body: BodyInfo,
) -> MessageView {
    let images = u8::from(body.remote_images == RemoteImages::Allowed);
    MessageView {
        id: m.id,
        account_id: m.account_id,
        folder_id: m.folder_id,
        subject: m.subject.clone(),
        from: AddressView::from(&m.from),
        to: addresses(&m.to),
        cc: addresses(&m.cc),
        reply_to: addresses(&m.reply_to),
        date_utc: m.date_utc,
        unread: !m.flags.contains(Flags::SEEN),
        flagged: m.flags.contains(Flags::FLAGGED),
        body_kind: match (m.body_state, body.unreadable, body.has_html) {
            (BodyState::None, true, _) => BodyKind::Unreadable,
            (BodyState::None, false, _) => BodyKind::Loading,
            (_, _, true) => BodyKind::Html,
            (_, _, false) => BodyKind::Plain,
        },
        truncated: m.body_state == BodyState::Text,
        has_html: body.has_html,
        body_url: format!("mail-body://localhost/{}?images={images}", m.id),
        attachments: attachments
            .iter()
            .filter(|a| !a.attachment.is_inline)
            .map(|a| {
                let name = mail_render::display_name(&a.attachment.filename);
                AttachmentView {
                    id: a.id,
                    executable: mail_render::is_executable(&name),
                    filename: name,
                    mime: a.attachment.mime.clone(),
                    size: a.attachment.size,
                }
            })
            .collect(),
        remote_images: body.remote_images,
        can_archive: body.can_archive,
        delete_is_permanent: body.delete_is_permanent,
        labels: None,
        important: false,
    }
}

/// `message.set_flags` values: what to set; a field left out is unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct FlagChange {
    pub seen: Option<bool>,
    pub flagged: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: i64, path: &str, role: Option<SpecialUse>) -> Folder {
        Folder {
            id,
            account_id: 1,
            path: path.into(),
            delimiter: "/".into(),
            special_use: role,
            uidvalidity: 1,
            uidnext: 1,
            highestmodseq: None,
            sync_enabled: true,
            watch_idle: false,
            unread: 0,
            total: 0,
            synced_at: 0,
            window_start: 0,
        }
    }

    #[test]
    fn special_folders_first_then_a_tree() {
        let folders = vec![
            folder(1, "Work/Clients", None),
            folder(2, "[Gmail]/Trash", Some(SpecialUse::Trash)),
            folder(3, "Work", None),
            folder(4, "INBOX", Some(SpecialUse::Inbox)),
            folder(5, "Work Notes", None),
            folder(6, "Sent", Some(SpecialUse::Sent)),
            folder(7, "Archive", Some(SpecialUse::Archive)),
            folder(8, "Orphan/Child", None),
            folder(9, "Junk", Some(SpecialUse::Junk)),
            folder(10, "Drafts", Some(SpecialUse::Drafts)),
        ];
        let v = folder_views(&folders);
        let order: Vec<(&str, u32)> = v.iter().map(|f| (f.name.as_str(), f.depth)).collect();
        assert_eq!(
            order,
            vec![
                ("Inbox", 0),
                ("Drafts", 0),
                ("Sent", 0),
                ("Archive", 0),
                ("Junk", 0),
                ("[Gmail]/Trash", 0),
                ("Orphan/Child", 0),
                ("Work", 0),
                ("Clients", 1),
                ("Work Notes", 0),
            ]
        );
        assert_eq!(v[0].role, Some(Role::Inbox));
    }

    #[test]
    fn list_requests_parse_scopes() {
        let r: ListRequest = serde_json::from_value(serde_json::json!({
            "scope": {"kind": "folder", "folder_id": 7},
            "cursor": {"date_utc": 10, "id": 3},
            "limit": 100
        }))
        .unwrap();
        assert_eq!(r.scope, Scope::Folder { folder_id: 7 });
        assert!(!r.group_by_thread);
        let r: ListRequest = serde_json::from_value(serde_json::json!({
            "scope": {"kind": "unified"}, "cursor": null, "limit": 50, "group_by_thread": true
        }))
        .unwrap();
        assert_eq!(r.scope, Scope::Unified);
    }
}
