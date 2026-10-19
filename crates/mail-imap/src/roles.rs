//! Folder roles (section 5, step 5): SPECIAL-USE first, then name heuristics.
//!
//! A role the server marks with SPECIAL-USE is never overridden. For every role still
//! missing, the first folder whose name matches a well-known name gets it, at most one per
//! role. Names are matched case-insensitively on the top-level name or on a direct child of
//! INBOX (servers such as Courier put every folder under `INBOX.`).

use crate::types::{MailboxInfo, SpecialUse};

/// Well-known names per role, in order of preference.
const NAMES: &[(SpecialUse, &[&str])] = &[
    (SpecialUse::Drafts, &["drafts", "draft"]),
    (
        SpecialUse::Sent,
        &["sent", "sent items", "sent messages", "sent mail"],
    ),
    (SpecialUse::Archive, &["archive", "archives"]),
    (
        SpecialUse::Junk,
        &["junk", "spam", "junk e-mail", "junk email", "bulk mail"],
    ),
    (
        SpecialUse::Trash,
        &["trash", "deleted items", "deleted messages", "bin"],
    ),
    (SpecialUse::All, &["all mail"]),
];

/// Fills in `special_use` from names where the server gave no role. INBOX is always the
/// inbox, whatever its case.
pub fn assign_roles(folders: &mut [MailboxInfo]) {
    for f in folders.iter_mut() {
        if f.path.eq_ignore_ascii_case("INBOX") {
            f.special_use = Some(SpecialUse::Inbox);
        }
    }
    for (role, names) in NAMES {
        if folders.iter().any(|f| f.special_use == Some(*role)) {
            continue;
        }
        let pick = names.iter().find_map(|name| {
            folders.iter().position(|f| {
                f.special_use.is_none() && f.selectable() && short_name(f).as_deref() == Some(*name)
            })
        });
        if let Some(i) = pick {
            folders[i].special_use = Some(*role);
        }
    }
}

/// The lowercase name to match: the whole path at top level, or the leaf under INBOX.
fn short_name(f: &MailboxInfo) -> Option<String> {
    let lower = f.path.to_lowercase();
    let Some(delim) = f.delimiter else {
        return Some(lower);
    };
    let mut parts = lower.split(delim);
    let first = parts.next()?;
    match (parts.next(), parts.next()) {
        (None, _) => Some(first.to_string()),
        (Some(leaf), None) if first == "inbox" => Some(leaf.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(path: &str, role: Option<SpecialUse>) -> MailboxInfo {
        MailboxInfo {
            path: path.into(),
            delimiter: Some('/'),
            attributes: Vec::new(),
            special_use: role,
            status: None,
        }
    }

    fn role_of(list: &[MailboxInfo], path: &str) -> Option<SpecialUse> {
        list.iter().find(|f| f.path == path)?.special_use
    }

    #[test]
    fn special_use_wins_and_names_fill_gaps() {
        let mut list = vec![
            folder("inbox", None),
            folder("Sent", None),
            folder("Sent Items", Some(SpecialUse::Sent)),
            folder("Deleted Items", None),
            folder("Spam", None),
            folder("Projects/Archive", None),
            folder("Drafts", None),
        ];
        assign_roles(&mut list);
        assert_eq!(role_of(&list, "inbox"), Some(SpecialUse::Inbox));
        assert_eq!(role_of(&list, "Sent Items"), Some(SpecialUse::Sent));
        assert_eq!(
            role_of(&list, "Sent"),
            None,
            "server role is not overridden"
        );
        assert_eq!(role_of(&list, "Deleted Items"), Some(SpecialUse::Trash));
        assert_eq!(role_of(&list, "Spam"), Some(SpecialUse::Junk));
        assert_eq!(role_of(&list, "Drafts"), Some(SpecialUse::Drafts));
        assert_eq!(
            role_of(&list, "Projects/Archive"),
            None,
            "nested names don't count"
        );
    }

    #[test]
    fn children_of_inbox_count_and_one_folder_per_role() {
        let mut list = vec![
            MailboxInfo {
                delimiter: Some('.'),
                ..folder("INBOX.Trash", None)
            },
            MailboxInfo {
                delimiter: Some('.'),
                ..folder("INBOX.Bin", None)
            },
            MailboxInfo {
                attributes: vec!["\\Noselect".into()],
                ..folder("Archive", None)
            },
        ];
        assign_roles(&mut list);
        assert_eq!(role_of(&list, "INBOX.Trash"), Some(SpecialUse::Trash));
        assert_eq!(role_of(&list, "INBOX.Bin"), None);
        assert_eq!(role_of(&list, "Archive"), None, "not selectable");
    }
}
