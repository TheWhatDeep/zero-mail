//! Threading (section 10). Built locally per account, across folders:
//!
//! 1. On Gmail a thread is `X-GM-THRID`; nothing else is computed.
//! 2. Elsewhere, union-find over Message-ID, In-Reply-To and References: two messages share
//!    a thread if they share any ID in that set. `message_refs` holds every message's set
//!    (SPEC_GAPS G74), so a new message finds its threads with a few index lookups, even
//!    when its parent is missing or arrives later. Merging relabels the smaller thread.
//! 3. Subject grouping within 14 days only when the setting is on (default off).
//! 4. A message without a Message-ID already carries a synthetic one (sync, section 10).

use std::collections::{BTreeSet, HashMap};

use rusqlite::{params, OptionalExtension, Transaction};

use crate::error::Result;
use crate::types::{AccountId, MessageId, NewMessage, ThreadId};

/// Setting key: also group by normalized subject within 14 days (section 10, rule 3).
pub const SUBJECT_MERGE_SETTING: &str = "threads.subject_merge";
/// How close two messages must be to group by subject.
const SUBJECT_WINDOW_SECS: i64 = 14 * 86_400;

/// The IDs a message links by: its own Message-ID, In-Reply-To and References, once each.
pub(crate) fn link_ids(m: &NewMessage) -> Vec<String> {
    let mut ids = BTreeSet::new();
    ids.insert(m.message_id.clone());
    if let Some(irt) = &m.in_reply_to {
        ids.insert(irt.clone());
    }
    if let Some(refs) = &m.refs {
        ids.extend(refs.split_whitespace().map(str::to_string));
    }
    ids.retain(|id| !id.is_empty());
    ids.into_iter().collect()
}

/// A subject without `Re:`, `Fwd:` and `Fw:` prefixes, lowercased, whitespace collapsed.
pub fn subject_norm(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let cut = ["re:", "fwd:", "fw:"]
            .iter()
            .find(|p| lower.starts_with(*p))
            .map(|p| p.len());
        match cut {
            Some(n) => s = s[n..].trim_start(),
            None => break,
        }
    }
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(crate) fn subject_merge_on(tx: &Transaction) -> Result<bool> {
    let v: Option<String> = tx
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1",
            [SUBJECT_MERGE_SETTING],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.as_deref() == Some("true"))
}

/// The thread a new message joins, merging every thread it links (rules 1 to 3).
pub(crate) fn assign(
    tx: &Transaction,
    account_id: AccountId,
    m: &NewMessage,
    ids: &[String],
    subject_merge: bool,
) -> Result<ThreadId> {
    let norm = subject_norm(&m.subject);
    if let Some(thrid) = m.gm_thrid {
        let existing: Option<ThreadId> = tx
            .prepare_cached(
                "SELECT thread_id FROM messages WHERE account_id = ?1 AND gm_thrid = ?2 LIMIT 1",
            )?
            .query_row(params![account_id, thrid], |r| r.get(0))
            .optional()?;
        return match existing {
            Some(t) => {
                touch(tx, t, m.date_utc)?;
                Ok(t)
            }
            None => new_thread(tx, account_id, m.date_utc, &norm),
        };
    }
    let mut found: BTreeSet<ThreadId> = BTreeSet::new();
    {
        let mut by_ref = tx.prepare_cached(
            "SELECT DISTINCT msg.thread_id FROM message_refs AS r
             JOIN messages AS msg ON msg.id = r.message_id
             WHERE r.account_id = ?1 AND r.ref = ?2 AND msg.gm_thrid IS NULL",
        )?;
        for id in ids {
            let rows = by_ref.query_map(params![account_id, id], |r| r.get(0))?;
            for t in rows {
                found.insert(t?);
            }
        }
    }
    if subject_merge && !norm.is_empty() {
        let mut by_subject = tx.prepare_cached(
            "SELECT t.id FROM threads AS t
             WHERE t.account_id = ?1 AND t.subject_norm = ?2
               AND abs(t.last_date_utc - ?3) <= ?4
               AND EXISTS (SELECT 1 FROM messages WHERE thread_id = t.id AND gm_thrid IS NULL)",
        )?;
        let rows = by_subject.query_map(
            params![account_id, norm, m.date_utc, SUBJECT_WINDOW_SECS],
            |r| r.get(0),
        )?;
        for t in rows {
            found.insert(t?);
        }
    }
    let thread = match found.len() {
        0 => return new_thread(tx, account_id, m.date_utc, &norm),
        1 => found.into_iter().next().unwrap_or_default(),
        _ => merge(tx, &found.into_iter().collect::<Vec<_>>())?,
    };
    touch(tx, thread, m.date_utc)?;
    Ok(thread)
}

/// Records a stored message's link IDs.
pub(crate) fn record(
    tx: &Transaction,
    account_id: AccountId,
    message: MessageId,
    ids: &[String],
) -> Result<()> {
    let mut insert = tx.prepare_cached(
        "INSERT OR IGNORE INTO message_refs (account_id, ref, message_id) VALUES (?1, ?2, ?3)",
    )?;
    for id in ids {
        insert.execute(params![account_id, id, message])?;
    }
    Ok(())
}

fn new_thread(
    tx: &Transaction,
    account_id: AccountId,
    date: i64,
    subject_norm: &str,
) -> Result<ThreadId> {
    crate::queries::insert_thread(tx, account_id, date, subject_norm)
}

fn touch(tx: &Transaction, thread: ThreadId, date: i64) -> Result<()> {
    tx.prepare_cached("UPDATE threads SET last_date_utc = max(last_date_utc, ?2) WHERE id = ?1")?
        .execute(params![thread, date])?;
    Ok(())
}

/// Merges threads into the one with the most messages (the smaller ones are relabelled,
/// rule 2). A mute on any of them carries over.
fn merge(tx: &Transaction, threads: &[ThreadId]) -> Result<ThreadId> {
    let mut sizes = Vec::with_capacity(threads.len());
    for t in threads {
        let n: i64 = tx
            .prepare_cached("SELECT count(*) FROM messages WHERE thread_id = ?1")?
            .query_row([t], |r| r.get(0))?;
        sizes.push((n, *t));
    }
    // Most messages first; on a tie, the oldest thread.
    sizes.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let keep = sizes[0].1;
    for (_, t) in &sizes[1..] {
        tx.prepare_cached("UPDATE messages SET thread_id = ?1 WHERE thread_id = ?2")?
            .execute(params![keep, t])?;
        tx.prepare_cached(
            "INSERT OR IGNORE INTO muted_threads (account_id, thread_id)
             SELECT account_id, ?1 FROM muted_threads WHERE thread_id = ?2",
        )?
        .execute(params![keep, t])?;
        tx.prepare_cached(
            "UPDATE threads SET last_date_utc =
                 max(last_date_utc, (SELECT last_date_utc FROM threads WHERE id = ?2))
             WHERE id = ?1",
        )?
        .execute(params![keep, t])?;
        tx.prepare_cached("DELETE FROM threads WHERE id = ?1")?
            .execute([t])?;
    }
    Ok(keep)
}

/// One-time pass for databases from before `message_refs` (one thread per message until
/// M9, SPEC_GAPS G36): records every message's link IDs and joins linked messages, in
/// memory per account, then drops threads left without messages.
pub(crate) fn rethread_all(tx: &Transaction) -> Result<()> {
    struct Row {
        id: MessageId,
        account: AccountId,
        thread: ThreadId,
        gmail: bool,
        ids: Vec<String>,
    }
    let rows: Vec<Row> = {
        let mut stmt = tx.prepare(
            "SELECT id, account_id, thread_id, gm_thrid IS NOT NULL, message_id, in_reply_to, refs
             FROM messages ORDER BY id",
        )?;
        let mapped = stmt.query_map([], |r| {
            let mut ids: BTreeSet<String> = BTreeSet::new();
            ids.insert(r.get::<_, String>(4)?);
            if let Some(irt) = r.get::<_, Option<String>>(5)? {
                ids.insert(irt);
            }
            if let Some(refs) = r.get::<_, Option<String>>(6)? {
                ids.extend(refs.split_whitespace().map(str::to_string));
            }
            ids.retain(|i| !i.is_empty());
            Ok(Row {
                id: r.get(0)?,
                account: r.get(1)?,
                thread: r.get(2)?,
                gmail: r.get(3)?,
                ids: ids.into_iter().collect(),
            })
        })?;
        mapped.collect::<rusqlite::Result<_>>()?
    };
    for row in &rows {
        record(tx, row.account, row.id, &row.ids)?;
    }
    // Union-find over (account, link ID); Gmail threads stay as X-GM-THRID made them.
    let mut parent: Vec<usize> = (0..rows.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut first_with: HashMap<(AccountId, &str), usize> = HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        if row.gmail {
            continue;
        }
        for id in &row.ids {
            match first_with.get(&(row.account, id.as_str())) {
                Some(&j) => {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        // The older message's root wins, so a thread keeps its first id.
                        parent[a.max(b)] = a.min(b);
                    }
                }
                None => {
                    first_with.insert((row.account, id.as_str()), i);
                }
            }
        }
    }
    let mut relabel = tx.prepare("UPDATE messages SET thread_id = ?1 WHERE id = ?2")?;
    for i in 0..rows.len() {
        let root = find(&mut parent, i);
        if root != i && rows[i].thread != rows[root].thread {
            relabel.execute(params![rows[root].thread, rows[i].id])?;
        }
    }
    tx.execute(
        "UPDATE threads SET
             last_date_utc = coalesce((SELECT max(date_utc) FROM messages WHERE thread_id = threads.id), last_date_utc),
             subject_norm = CASE WHEN subject_norm = '' THEN coalesce(
                 (SELECT subject FROM messages WHERE thread_id = threads.id ORDER BY date_utc LIMIT 1), '')
                 ELSE subject_norm END",
        [],
    )?;
    tx.execute(
        "DELETE FROM threads WHERE NOT EXISTS (SELECT 1 FROM messages WHERE thread_id = threads.id)
             AND NOT EXISTS (SELECT 1 FROM muted_threads WHERE thread_id = threads.id)",
        [],
    )?;
    // The subjects copied above are raw; normalize them.
    let subjects: Vec<(ThreadId, String)> = {
        let mut stmt = tx.prepare("SELECT id, subject_norm FROM threads")?;
        let mapped = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        mapped.collect::<rusqlite::Result<_>>()?
    };
    let mut set = tx.prepare("UPDATE threads SET subject_norm = ?2 WHERE id = ?1")?;
    for (id, s) in subjects {
        let norm = subject_norm(&s);
        if norm != s {
            set.execute(params![id, norm])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database from before M9 (one thread per message, G36) is threaded once at open.
    #[test]
    fn old_databases_are_threaded_once() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        crate::schema::migrate_with(&mut conn, &crate::schema::MIGRATIONS[..2]).unwrap();
        conn.execute_batch(
            "INSERT INTO accounts VALUES (1, 'n', 'e', 'h', 993, 'tls', 'h', 465, 'tls', 'u',
                 'password', '', NULL, 90, 'auto', '', '', 1, 0);
             INSERT INTO folders VALUES (1, 1, 'INBOX', '/', 'inbox', 1, 1, NULL, 1, 0, 0, 0, 0);
             INSERT INTO threads VALUES (1, 1, 100, ''), (2, 1, 200, ''), (3, 1, 300, '');
             INSERT INTO messages (id, account_id, folder_id, uid, message_id, in_reply_to, refs,
                 thread_id, subject, from_name, from_addr, to_json, cc_json, reply_to_json,
                 date_utc, internal_utc, size, flags, has_attach, preview, body_state) VALUES
             (1, 1, 1, 1, 'a@x', NULL, NULL, 1, 'Budget', '', '', '[]', '[]', '[]', 100, 100, 1, 0, 0, '', 'none'),
             (2, 1, 1, 2, 'c@x', 'b@x', 'a@x b@x', 2, 'Re: Budget', '', '', '[]', '[]', '[]', 200, 200, 1, 0, 0, '', 'none'),
             (3, 1, 1, 3, 'z@x', NULL, NULL, 3, 'Other', '', '', '[]', '[]', '[]', 300, 300, 1, 0, 0, '', 'none');",
        )
        .unwrap();
        let before = crate::schema::migrate(&mut conn).unwrap();
        assert!(before < crate::schema::THREADING_VERSION);
        let tx = conn.transaction().unwrap();
        rethread_all(&tx).unwrap();
        tx.commit().unwrap();
        let thread = |id: i64| -> i64 {
            conn.query_row("SELECT thread_id FROM messages WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert_eq!(thread(1), 1);
        assert_eq!(thread(2), 1, "joined through References");
        assert_eq!(thread(3), 3);
        let threads: Vec<(i64, i64, String)> = conn
            .prepare("SELECT id, last_date_utc, subject_norm FROM threads ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            threads,
            vec![
                (1, 200, "budget".to_string()),
                (3, 300, "other".to_string())
            ]
        );
        let refs: i64 = conn
            .query_row("SELECT count(*) FROM message_refs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(refs, 5, "a; c, b, a; z");
        let window: i64 = conn
            .query_row("SELECT window_start FROM folders WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            window, 100,
            "the oldest cached message, as Load older measured it"
        );
    }

    #[test]
    fn subjects_normalize() {
        assert_eq!(subject_norm("Re: RE: Fwd:  Budget   Q3 "), "budget q3");
        assert_eq!(subject_norm("FW: Plans"), "plans");
        assert_eq!(subject_norm("Reunion"), "reunion");
        assert_eq!(subject_norm(""), "");
    }
}
