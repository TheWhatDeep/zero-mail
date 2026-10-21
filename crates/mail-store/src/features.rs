//! Section 18 tables: mute, snooze, rules and templates, and the send-later history mark.

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::error::{Result, StoreError};
use crate::types::{
    AccountId, FolderId, MessageCopy, MessageId, RuleFacts, RuleRecord, Snooze, TemplateRecord,
    ThreadId,
};

const HIDDEN: i64 = crate::types::Flags::DELETED as i64;

// Mute (18.5).

pub(crate) fn mute_thread(tx: &Transaction, account: AccountId, thread: ThreadId) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO muted_threads (account_id, thread_id) VALUES (?1, ?2)",
        params![account, thread],
    )?;
    Ok(())
}

pub(crate) fn unmute_thread(tx: &Transaction, account: AccountId, thread: ThreadId) -> Result<()> {
    tx.execute(
        "DELETE FROM muted_threads WHERE account_id = ?1 AND thread_id = ?2",
        params![account, thread],
    )?;
    Ok(())
}

pub(crate) fn mute_threads_of(
    tx: &Transaction,
    account: AccountId,
    messages: &[MessageId],
) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT OR IGNORE INTO muted_threads (account_id, thread_id)
         SELECT account_id, thread_id FROM messages WHERE id = ?1 AND account_id = ?2",
    )?;
    for m in messages {
        stmt.execute(params![m, account])?;
    }
    Ok(())
}

pub(crate) fn thread_muted(conn: &Connection, thread: ThreadId) -> Result<bool> {
    Ok(conn
        .prepare_cached("SELECT 1 FROM muted_threads WHERE thread_id = ?1")?
        .query_row([thread], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Every visible row of a thread, for flag changes across folders.
pub(crate) fn thread_copies(conn: &Connection, thread: ThreadId) -> Result<Vec<MessageCopy>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, folder_id, uid FROM messages WHERE thread_id = ?1 AND (flags & ?2) = 0
         ORDER BY folder_id, uid",
    )?;
    let rows = stmt.query_map(params![thread, HIDDEN], |r| {
        Ok(MessageCopy {
            id: r.get(0)?,
            folder_id: r.get(1)?,
            uid: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// Snooze (18.2).

fn snooze_row(r: &rusqlite::Row) -> rusqlite::Result<Snooze> {
    Ok(Snooze {
        message_id: r.get(0)?,
        account_id: r.get(1)?,
        due_utc: r.get(2)?,
        origin_folder_id: r.get(3)?,
    })
}

pub(crate) fn add_snooze(tx: &Transaction, s: &Snooze) -> Result<()> {
    tx.execute(
        "INSERT INTO snoozes (message_id, account_id, due_utc, origin_folder_id)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (message_id) DO UPDATE SET due_utc = excluded.due_utc",
        params![s.message_id, s.account_id, s.due_utc, s.origin_folder_id],
    )?;
    Ok(())
}

pub(crate) fn due_snoozes(conn: &Connection, now: i64) -> Result<Vec<Snooze>> {
    let mut stmt = conn.prepare_cached(
        "SELECT message_id, account_id, due_utc, origin_folder_id FROM snoozes
         WHERE due_utc <= ?1 ORDER BY due_utc",
    )?;
    let rows = stmt.query_map([now], snooze_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn get_snooze(conn: &Connection, message: MessageId) -> Result<Option<Snooze>> {
    Ok(conn
        .prepare_cached(
            "SELECT message_id, account_id, due_utc, origin_folder_id FROM snoozes
             WHERE message_id = ?1",
        )?
        .query_row([message], snooze_row)
        .optional()?)
}

/// Snoozes on messages of a thread.
pub(crate) fn thread_snoozes(conn: &Connection, thread: ThreadId) -> Result<Vec<Snooze>> {
    let mut stmt = conn.prepare_cached(
        "SELECT s.message_id, s.account_id, s.due_utc, s.origin_folder_id FROM snoozes AS s
         JOIN messages AS m ON m.id = s.message_id WHERE m.thread_id = ?1",
    )?;
    let rows = stmt.query_map([thread], snooze_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Removes a snooze; `false` when it was already gone (it fired or was undone).
pub(crate) fn delete_snooze(tx: &Transaction, message: MessageId) -> Result<bool> {
    Ok(tx.execute("DELETE FROM snoozes WHERE message_id = ?1", [message])? == 1)
}

/// The snoozed message has a new row (the copy in the Snoozed folder).
pub(crate) fn repoint_snooze(tx: &Transaction, from: MessageId, to: MessageId) -> Result<()> {
    tx.execute(
        "UPDATE snoozes SET message_id = ?2 WHERE message_id = ?1",
        params![from, to],
    )?;
    Ok(())
}

/// A visible row of `folder` with this Message-ID.
pub(crate) fn find_by_message_id(
    conn: &Connection,
    folder: FolderId,
    message_id: &str,
) -> Result<Option<MessageId>> {
    Ok(conn
        .prepare_cached(
            "SELECT id FROM messages WHERE folder_id = ?1 AND message_id = ?2
             AND (flags & ?3) = 0 ORDER BY id DESC LIMIT 1",
        )?
        .query_row(params![folder, message_id, HIDDEN], |r| r.get(0))
        .optional()?)
}

// Rules (18.6).

const RULE_COLUMNS: &str =
    "id, account_id, position, name, enabled, conditions_json, actions_json, stop_processing";

fn rule_row(r: &rusqlite::Row) -> rusqlite::Result<(RuleRecord, String, String)> {
    Ok((
        RuleRecord {
            id: r.get(0)?,
            account_id: r.get(1)?,
            position: r.get(2)?,
            name: r.get(3)?,
            enabled: r.get(4)?,
            conditions: serde_json::Value::Null,
            actions: serde_json::Value::Null,
            stop_processing: r.get(7)?,
        },
        r.get(5)?,
        r.get(6)?,
    ))
}

/// Every rule, in order.
pub(crate) fn list_rules(conn: &Connection) -> Result<Vec<RuleRecord>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {RULE_COLUMNS} FROM rules ORDER BY position, id"
    ))?;
    let rows = stmt.query_map([], rule_row)?;
    let mut out = Vec::new();
    for row in rows {
        let (mut rule, conditions, actions) = row?;
        rule.conditions = serde_json::from_str(&conditions)?;
        rule.actions = serde_json::from_str(&actions)?;
        out.push(rule);
    }
    Ok(out)
}

/// Adds (id 0, at the end) or replaces a rule. Returns its id.
pub(crate) fn save_rule(tx: &Transaction, rule: &RuleRecord) -> Result<i64> {
    let conditions = serde_json::to_string(&rule.conditions)?;
    let actions = serde_json::to_string(&rule.actions)?;
    if rule.id == 0 {
        let position: i64 = tx.query_row(
            "SELECT coalesce(max(position), -1) + 1 FROM rules",
            [],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO rules (account_id, position, name, enabled, conditions_json,
                 actions_json, stop_processing) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                rule.account_id,
                position,
                rule.name,
                rule.enabled,
                conditions,
                actions,
                rule.stop_processing
            ],
        )?;
        return Ok(tx.last_insert_rowid());
    }
    let changed = tx.execute(
        "UPDATE rules SET account_id = ?2, name = ?3, enabled = ?4, conditions_json = ?5,
             actions_json = ?6, stop_processing = ?7 WHERE id = ?1",
        params![
            rule.id,
            rule.account_id,
            rule.name,
            rule.enabled,
            conditions,
            actions,
            rule.stop_processing
        ],
    )?;
    if changed == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(rule.id)
}

pub(crate) fn delete_rule(tx: &Transaction, id: i64) -> Result<()> {
    tx.execute("DELETE FROM rules WHERE id = ?1", [id])?;
    Ok(())
}

/// Puts the rules in the given order; rules not named keep their place after them.
pub(crate) fn reorder_rules(tx: &Transaction, ids: &[i64]) -> Result<()> {
    let mut all: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT id FROM rules ORDER BY position, id")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    all.retain(|id| !ids.contains(id));
    let order: Vec<i64> = ids.iter().copied().chain(all).collect();
    let mut set = tx.prepare("UPDATE rules SET position = ?2 WHERE id = ?1")?;
    for (pos, id) in order.iter().enumerate() {
        set.execute(params![id, i64::try_from(pos).unwrap_or(i64::MAX)])?;
    }
    Ok(())
}

pub(crate) fn rules_applied(conn: &Connection, message: MessageId) -> Result<Vec<i64>> {
    let mut stmt =
        conn.prepare_cached("SELECT rule_id FROM rules_applied WHERE message_id = ?1")?;
    let rows = stmt.query_map([message], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn mark_rules_applied(tx: &Transaction, marks: &[(MessageId, i64)]) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT OR IGNORE INTO rules_applied (message_id, rule_id) VALUES (?1, ?2)",
    )?;
    for (m, r) in marks {
        stmt.execute(params![m, r])?;
    }
    Ok(())
}

/// What rule conditions test, for each message (section 18.6). `body` is `None` until the
/// body is cached.
pub(crate) fn rule_facts(conn: &Connection, ids: &[MessageId]) -> Result<Vec<RuleFacts>> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.id, m.account_id, m.folder_id, m.uid, m.thread_id, m.message_id,
                m.from_name || ' ' || m.from_addr, m.to_json, m.cc_json, m.subject,
                m.list_id, m.has_attach, b.text_plain, m.flags
         FROM messages AS m LEFT JOIN bodies AS b ON b.message_id = m.id WHERE m.id = ?1",
    )?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let row = stmt
            .query_row([id], |r| {
                Ok(RuleFacts {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    folder_id: r.get(2)?,
                    uid: r.get(3)?,
                    thread_id: r.get(4)?,
                    message_id: r.get(5)?,
                    from: r.get(6)?,
                    to: r.get(7)?,
                    cc: r.get(8)?,
                    subject: r.get(9)?,
                    list_id: r.get(10)?,
                    has_attach: r.get(11)?,
                    body: r.get(12)?,
                    hidden: r.get::<_, i64>(13)? & HIDDEN != 0,
                })
            })
            .optional()?;
        out.extend(row);
    }
    Ok(out)
}

// Settings screens (M10).

/// The senders an account loads remote images from (Settings, Privacy; SPEC_GAPS G79).
pub(crate) fn remote_image_senders(conn: &Connection, account: AccountId) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT sender_addr FROM remote_image_allow WHERE account_id = ?1 ORDER BY sender_addr",
    )?;
    let rows = stmt.query_map([account], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn disallow_remote_images(
    tx: &Transaction,
    account: AccountId,
    sender: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM remote_image_allow WHERE account_id = ?1 AND sender_addr = ?2",
        params![account, sender],
    )?;
    Ok(())
}

/// Clear body cache (invariant 7): every cached body goes and messages fetch theirs again
/// when opened; headers stay. Returns how many bodies were dropped.
pub(crate) fn clear_bodies(tx: &Transaction) -> Result<usize> {
    tx.execute(
        "UPDATE messages SET body_state = 'none' WHERE body_state != 'none'",
        [],
    )?;
    Ok(tx.execute("DELETE FROM bodies", [])?)
}

/// A message's List-Unsubscribe and List-Unsubscribe-Post headers (section 18.7).
pub(crate) fn unsubscribe_headers(
    conn: &Connection,
    message: MessageId,
) -> Result<(Option<String>, Option<String>)> {
    Ok(conn.query_row(
        "SELECT list_unsubscribe, list_unsubscribe_post FROM messages WHERE id = ?1",
        [message],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// Unread messages in the INBOX of every enabled account: the window title, tray and
/// taskbar count (section 11). A Gmail message counts once.
pub(crate) fn unread_total(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT count(DISTINCT coalesce(m.gm_msgid, -m.id)) FROM messages AS m
         JOIN folders AS f ON f.id = m.folder_id
         JOIN accounts AS a ON a.id = m.account_id
         WHERE f.special_use = 'inbox' AND a.enabled = 1 AND (m.flags & 1) = 0
           AND (m.flags & ?1) = 0",
        [HIDDEN],
        |r| r.get(0),
    )?)
}

// Templates (18.4).

pub(crate) fn list_templates(conn: &Connection) -> Result<Vec<TemplateRecord>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, name, subject, body_html, body_text, updated_at FROM templates
         ORDER BY lower(name), id",
    )?;
    let rows = stmt.query_map([], template_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn template_row(r: &rusqlite::Row) -> rusqlite::Result<TemplateRecord> {
    Ok(TemplateRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        subject: r.get(2)?,
        body_html: r.get(3)?,
        body_text: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

pub(crate) fn get_template(conn: &Connection, id: i64) -> Result<TemplateRecord> {
    Ok(conn.query_row(
        "SELECT id, name, subject, body_html, body_text, updated_at FROM templates WHERE id = ?1",
        [id],
        template_row,
    )?)
}

/// Adds (id 0) or replaces a template. Returns its id.
pub(crate) fn save_template(tx: &Transaction, t: &TemplateRecord) -> Result<i64> {
    if t.id == 0 {
        tx.execute(
            "INSERT INTO templates (name, subject, body_html, body_text, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![t.name, t.subject, t.body_html, t.body_text, t.updated_at],
        )?;
        return Ok(tx.last_insert_rowid());
    }
    let changed = tx.execute(
        "UPDATE templates SET name = ?2, subject = ?3, body_html = ?4, body_text = ?5,
             updated_at = ?6 WHERE id = ?1",
        params![
            t.id,
            t.name,
            t.subject,
            t.body_html,
            t.body_text,
            t.updated_at
        ],
    )?;
    if changed == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(t.id)
}

pub(crate) fn delete_template(tx: &Transaction, id: i64) -> Result<()> {
    tx.execute("DELETE FROM templates WHERE id = ?1", [id])?;
    Ok(())
}
