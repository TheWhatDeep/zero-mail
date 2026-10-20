//! SQL for sending (section 7): the outbox, drafts, identities and contacts.
//!
//! An outbox row is written before any network call and moves through the section 7
//! states; `envelope_json` holds the SMTP envelope with what the outbox needs afterwards
//! (the draft, the original of a reply). Drafts keep their content as JSON and the UID of
//! their uploaded copy. Contacts count uses: received mail counts 1, sent mail more
//! (section 10).

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde_json::Value;

use crate::error::{Result, StoreError};
use crate::types::*;

const OUTBOX_COLUMNS: &str = "id, account_id, identity_id, envelope_json, state, send_after_utc,
    attempts, last_error, created_at";

fn outbox_row(r: &Row) -> rusqlite::Result<(OutboxItem, String)> {
    let state: String = r.get(4)?;
    Ok((
        OutboxItem {
            id: r.get(0)?,
            account_id: r.get(1)?,
            identity_id: r.get(2)?,
            envelope: Value::Null,
            state: OutboxState::from_sql(&state).unwrap_or(OutboxState::Failed),
            send_after_utc: r.get(5)?,
            attempts: r.get(6)?,
            last_error: r.get(7)?,
            created_at: r.get(8)?,
        },
        r.get(3)?,
    ))
}

fn finish(row: (OutboxItem, String)) -> Result<OutboxItem> {
    let (mut item, envelope) = row;
    item.envelope = serde_json::from_str(&envelope)?;
    Ok(item)
}

pub(crate) fn insert_outbox(tx: &Transaction, o: &NewOutbox) -> Result<OutboxId> {
    tx.execute(
        "INSERT INTO outbox (account_id, identity_id, raw_mime, envelope_json, state,
             send_after_utc, attempts, last_error, created_at)
         VALUES (?1, ?2, ?3, ?4, 'queued', ?5, 0, '', ?6)",
        params![
            o.account_id,
            o.identity_id,
            o.raw_mime,
            o.envelope.to_string(),
            o.send_after_utc,
            o.created_at
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

pub(crate) fn get_outbox(conn: &Connection, id: OutboxId) -> Result<OutboxItem> {
    let sql = format!("SELECT {OUTBOX_COLUMNS} FROM outbox WHERE id = ?1");
    finish(conn.query_row(&sql, [id], outbox_row)?)
}

pub(crate) fn outbox_raw(conn: &Connection, id: OutboxId) -> Result<Vec<u8>> {
    Ok(
        conn.query_row("SELECT raw_mime FROM outbox WHERE id = ?1", [id], |r| {
            r.get(0)
        })?,
    )
}

/// An account's outbox, newest first (`outbox.list`, SPEC_GAPS G69).
pub(crate) fn list_outbox(conn: &Connection, account_id: AccountId) -> Result<Vec<OutboxItem>> {
    let sql = format!("SELECT {OUTBOX_COLUMNS} FROM outbox WHERE account_id = ?1 ORDER BY id DESC");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([account_id], outbox_row)?;
    rows.map(|r| finish(r?)).collect()
}

/// Items whose time has come: queued or waiting to retry, `send_after_utc <= now`.
pub(crate) fn due_outbox(conn: &Connection, now: i64) -> Result<Vec<OutboxId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM outbox WHERE state IN ('queued', 'retry_wait') AND send_after_utc <= ?1
         ORDER BY send_after_utc, id",
    )?;
    let rows = stmt.query_map([now], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The earliest time an item becomes due, if any is waiting.
pub(crate) fn next_due(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT min(send_after_utc) FROM outbox WHERE state IN ('queued', 'retry_wait')",
        [],
        |r| r.get(0),
    )?)
}

/// Moves a due item to `sending`, durably, before any network call (section 7). `false`
/// when it is no longer due (undone, discarded, already claimed).
pub(crate) fn claim_outbox(tx: &Transaction, id: OutboxId, now: i64) -> Result<bool> {
    Ok(tx.execute(
        "UPDATE outbox SET state = 'sending'
         WHERE id = ?1 AND state IN ('queued', 'retry_wait') AND send_after_utc <= ?2",
        params![id, now],
    )? == 1)
}

pub(crate) fn set_outbox(
    tx: &Transaction,
    id: OutboxId,
    state: OutboxState,
    send_after: Option<i64>,
    attempt: bool,
    error: Option<&str>,
) -> Result<()> {
    let n = tx.execute(
        "UPDATE outbox SET state = ?2,
             send_after_utc = coalesce(?3, send_after_utc),
             attempts = attempts + ?4,
             last_error = coalesce(?5, last_error)
         WHERE id = ?1",
        params![id, state.as_sql(), send_after, i64::from(attempt), error],
    )?;
    match n {
        0 => Err(StoreError::NotFound),
        _ => Ok(()),
    }
}

/// Retry (section 7: the user retries a failed or `unknown` item), or Send now on a
/// scheduled one (section 18.3): queued now, with a fresh retry schedule.
pub(crate) fn retry_outbox(tx: &Transaction, id: OutboxId, now: i64) -> Result<bool> {
    Ok(tx.execute(
        "UPDATE outbox SET state = 'queued', send_after_utc = ?2, attempts = 0, last_error = ''
         WHERE id = ?1 AND state IN ('failed', 'unknown', 'queued')",
        params![id, now],
    )? == 1)
}

/// Undo before the send: only a queued item can be taken back (section 7).
pub(crate) fn undo_outbox(tx: &Transaction, id: OutboxId) -> Result<bool> {
    Ok(tx.execute(
        "DELETE FROM outbox WHERE id = ?1 AND state = 'queued'",
        [id],
    )? == 1)
}

pub(crate) fn delete_outbox(tx: &Transaction, id: OutboxId) -> Result<()> {
    tx.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
    Ok(())
}

/// The raw message is dropped once nothing needs it (the Sent copy is filed or not
/// wanted), so sent history stays small.
pub(crate) fn clear_outbox_raw(tx: &Transaction, id: OutboxId) -> Result<()> {
    tx.execute("UPDATE outbox SET raw_mime = x'' WHERE id = ?1", [id])?;
    Ok(())
}

/// A crash left items `sending`: they become `unknown` and are never retried (section 7).
pub(crate) fn sending_to_unknown(tx: &Transaction) -> Result<Vec<OutboxId>> {
    let ids: Vec<OutboxId> = {
        let mut stmt = tx.prepare_cached("SELECT id FROM outbox WHERE state = 'sending'")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    tx.execute(
        "UPDATE outbox SET state = 'unknown' WHERE state = 'sending'",
        [],
    )?;
    Ok(ids)
}

/// Items not yet through: queued, sending or waiting to retry (`pending-count`).
pub(crate) fn outbox_pending(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT count(*) FROM outbox WHERE state IN ('queued', 'sending', 'retry_wait')",
        [],
        |r| r.get(0),
    )?)
}

const DRAFT_COLUMNS: &str = "id, account_id, imap_uid, content_json, updated_at, uploaded_at";

fn draft_row(r: &Row) -> rusqlite::Result<(Draft, String)> {
    Ok((
        Draft {
            id: r.get(0)?,
            account_id: r.get(1)?,
            imap_uid: r.get(2)?,
            content: Value::Null,
            updated_at: r.get(4)?,
            uploaded_at: r.get(5)?,
        },
        r.get(3)?,
    ))
}

pub(crate) fn insert_draft(
    tx: &Transaction,
    account_id: AccountId,
    content: &Value,
    now: i64,
) -> Result<DraftId> {
    tx.execute(
        "INSERT INTO drafts (account_id, imap_uid, content_json, updated_at, uploaded_at)
         VALUES (?1, NULL, ?2, ?3, NULL)",
        params![account_id, content.to_string(), now],
    )?;
    Ok(tx.last_insert_rowid())
}

pub(crate) fn get_draft(conn: &Connection, id: DraftId) -> Result<Draft> {
    let sql = format!("SELECT {DRAFT_COLUMNS} FROM drafts WHERE id = ?1");
    let (mut d, content) = conn.query_row(&sql, [id], draft_row)?;
    d.content = serde_json::from_str(&content)?;
    Ok(d)
}

pub(crate) fn update_draft(tx: &Transaction, id: DraftId, content: &Value, now: i64) -> Result<()> {
    match tx.execute(
        "UPDATE drafts SET content_json = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, content.to_string(), now],
    )? {
        0 => Err(StoreError::NotFound),
        _ => Ok(()),
    }
}

/// Records the uploaded copy's UID (section 7, Drafts).
pub(crate) fn set_draft_uploaded(
    tx: &Transaction,
    id: DraftId,
    uid: Option<u32>,
    now: i64,
) -> Result<()> {
    tx.execute(
        "UPDATE drafts SET imap_uid = ?2, uploaded_at = ?3 WHERE id = ?1",
        params![id, uid, now],
    )?;
    Ok(())
}

pub(crate) fn delete_draft(tx: &Transaction, id: DraftId) -> Result<()> {
    tx.execute("DELETE FROM drafts WHERE id = ?1", [id])?;
    Ok(())
}

pub(crate) fn get_identity(conn: &Connection, id: IdentityId) -> Result<Identity> {
    Ok(conn.query_row(
        "SELECT id, account_id, name, address, reply_to, is_default FROM identities WHERE id = ?1",
        [id],
        |r| {
            Ok(Identity {
                id: r.get(0)?,
                account_id: r.get(1)?,
                identity: NewIdentity {
                    name: r.get(2)?,
                    address: r.get(3)?,
                    reply_to: r.get(4)?,
                    is_default: r.get(5)?,
                },
            })
        },
    )?)
}

/// Adds or edits an identity. Making one the default takes it from the others; the first
/// identity is always the default (section 5: every account has one).
pub(crate) fn save_identity(
    tx: &Transaction,
    account_id: AccountId,
    id: Option<IdentityId>,
    i: &NewIdentity,
) -> Result<IdentityId> {
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM identities WHERE account_id = ?1",
        [account_id],
        |r| r.get(0),
    )?;
    let default = i.is_default || count == 0 || (count == 1 && id.is_some());
    if default {
        tx.execute(
            "UPDATE identities SET is_default = 0 WHERE account_id = ?1",
            [account_id],
        )?;
    }
    let id = match id {
        Some(id) => {
            let n = tx.execute(
                "UPDATE identities SET name = ?3, address = ?4, reply_to = ?5, is_default = ?6
                 WHERE id = ?1 AND account_id = ?2",
                params![id, account_id, i.name, i.address, i.reply_to, default],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound);
            }
            id
        }
        None => crate::queries::insert_identity(
            tx,
            account_id,
            &NewIdentity {
                is_default: default,
                ..i.clone()
            },
        )?,
    };
    ensure_default(tx, account_id)?;
    Ok(id)
}

/// Deletes an identity; the last one cannot go. A deleted default passes to the oldest.
pub(crate) fn delete_identity(tx: &Transaction, id: IdentityId) -> Result<()> {
    let account: AccountId = tx.query_row(
        "SELECT account_id FROM identities WHERE id = ?1",
        [id],
        |r| r.get(0),
    )?;
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM identities WHERE account_id = ?1",
        [account],
        |r| r.get(0),
    )?;
    if count <= 1 {
        return Err(StoreError::Invalid(
            "an account keeps at least one identity",
        ));
    }
    let used: i64 = tx.query_row(
        "SELECT count(*) FROM outbox WHERE identity_id = ?1",
        [id],
        |r| r.get(0),
    )?;
    if used > 0 {
        return Err(StoreError::Invalid(
            "the identity has messages in the outbox",
        ));
    }
    tx.execute("DELETE FROM identities WHERE id = ?1", [id])?;
    ensure_default(tx, account)
}

fn ensure_default(tx: &Transaction, account_id: AccountId) -> Result<()> {
    let has: Option<i64> = tx
        .query_row(
            "SELECT id FROM identities WHERE account_id = ?1 AND is_default = 1",
            [account_id],
            |r| r.get(0),
        )
        .optional()?;
    if has.is_none() {
        tx.execute(
            "UPDATE identities SET is_default = 1 WHERE id =
                 (SELECT min(id) FROM identities WHERE account_id = ?1)",
            [account_id],
        )?;
    }
    Ok(())
}

/// Counts uses of addresses (section 10: filled from From, To and Cc of mail sent or
/// received). A known address keeps its name unless a new non-empty one comes.
pub(crate) fn bump_contacts(
    tx: &Transaction,
    account_id: AccountId,
    people: &[(String, String)],
    weight: u32,
    at: i64,
) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO contacts (account_id, address, name, last_used_utc, use_count)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (account_id, address) DO UPDATE SET
             name = CASE WHEN excluded.name != '' THEN excluded.name ELSE name END,
             last_used_utc = max(last_used_utc, excluded.last_used_utc),
             use_count = use_count + excluded.use_count",
    )?;
    for (addr, name) in people {
        let addr = addr.trim();
        if addr.is_empty() || !addr.contains('@') {
            continue;
        }
        stmt.execute(params![account_id, addr, name.trim(), at, weight])?;
    }
    Ok(())
}

/// Up to `limit` contacts whose address or name starts with `prefix` (a name word counts),
/// by use count, then recency (section 10).
pub(crate) fn suggest_contacts(
    conn: &Connection,
    account_id: AccountId,
    prefix: &str,
    limit: u32,
) -> Result<Vec<Contact>> {
    let escaped: String = prefix
        .trim()
        .chars()
        .flat_map(|c| match c {
            '%' | '_' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    if escaped.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT address, name, last_used_utc, use_count FROM contacts
         WHERE account_id = ?1
           AND (address LIKE ?2 || '%' ESCAPE '\\' OR name LIKE ?2 || '%' ESCAPE '\\'
                OR name LIKE '% ' || ?2 || '%' ESCAPE '\\')
         ORDER BY use_count DESC, last_used_utc DESC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![account_id, escaped, limit], |r| {
        Ok(Contact {
            address: r.get(0)?,
            name: r.get(1)?,
            last_used_utc: r.get(2)?,
            use_count: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
