//! What the UI receives from the M4 commands (section 3): folder trees, list pages and the
//! reader's display model. Plain data, serialized for IPC; no secrets, no SQL.

use mail_store::{
    Address, BodyState, Cursor, Flags, Folder, ListRow, MessageRecord, NewAttachment, SpecialUse,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
            }
        })
        .collect()
}

/// `list.page` scope (section 3). M4 lists folders; the other scopes arrive in M9.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    Folder { folder_id: i64 },
    Unified,
    Search { query: String },
    Thread { thread_id: i64 },
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageView {
    pub rows: Vec<RowView>,
    pub next: Option<CursorView>,
}

/// How the reader shows the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    /// Plain text from `mail-body` (M4; HTML arrives in M5).
    Plain,
    /// Cut at the 1 MB cap (section 12, Very large message).
    PlainTruncated,
    /// Not cached yet; it is being fetched, and `store-changed` follows.
    Loading,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentView {
    pub part_id: String,
    pub filename: String,
    pub mime: String,
    pub size: u64,
    pub is_inline: bool,
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
    /// `mail-body://localhost/<id>?images=0`. The UI maps it to the platform's form.
    pub body_url: String,
    pub attachments: Vec<AttachmentView>,
    /// Remote images arrive with the HTML pipeline in M5; plain text has none.
    pub remote_images: &'static str,
}

pub fn message_view(m: &MessageRecord, attachments: &[NewAttachment]) -> MessageView {
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
        body_kind: match m.body_state {
            BodyState::Full => BodyKind::Plain,
            BodyState::Text => BodyKind::PlainTruncated,
            BodyState::None => BodyKind::Loading,
        },
        body_url: format!("mail-body://localhost/{}?images=0", m.id),
        attachments: attachments
            .iter()
            .filter(|a| !a.is_inline)
            .map(|a| AttachmentView {
                part_id: a.part_id.clone(),
                filename: a.filename.clone(),
                mime: a.mime.clone(),
                size: a.size,
                is_inline: a.is_inline,
            })
            .collect(),
        remote_images: "none",
    }
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
