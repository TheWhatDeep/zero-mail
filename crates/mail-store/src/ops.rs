//! SQL for the pending-op queue (`pending_ops`, section 9) and folder settings.
//!
//! An op's local effect is an [`Overlay`] on the flags of its rows: a star adds bit 1, a
//! pending move or delete adds bit 5, which hides the rows in every view (invariant 2;
//! SPEC_GAPS G59). The overlay is applied when the op is queued, laid again after every
//! server pull while the op is queued or running, and undone from the flags saved at queue
//! time when the op is cancelled or fails. `args_json` holds `{overlay, before, args,
//! result, group}`; only `overlay`, `before` and `group` mean anything to the store.

use rusqlite::{params, Connection, Row, Transaction};
use serde_json::{json, Value};

use crate::error::{Result, StoreError};
use crate::types::*;

const HIDDEN: i64 = Flags::DELETED as i64;

fn op_row(r: &Row) -> rusqlite::Result<(PendingOp, String, String)> {
    Ok((
        PendingOp {
            id: r.get(0)?,
            account_id: r.get(1)?,
            folder_id: r.get(2)?,
            uids: Vec::new(),
            kind: r.get(4)?,
            args: Value::Null,
            result: Value::Null,
            group: 0,
            overlay: Overlay::default(),
            state: OpState::from_sql(&r.get::<_, String>(6)?).unwrap_or(OpState::Queued),
            attempts: r.get(7)?,
            last_error: r.get(8)?,
            created_at: r.get(9)?,
        },
        r.get(3)?,
        r.get(5)?,
    ))
}

const OP_COLUMNS: &str =
    "id, account_id, folder_id, uids_json, kind, args_json, state, attempts, last_error, created_at";

fn finish(row: (PendingOp, String, String)) -> Result<PendingOp> {
    let (mut op, uids, args) = row;
    op.uids = serde_json::from_str(&uids)?;
    let all: Value = serde_json::from_str(&args)?;
    op.overlay = serde_json::from_value(all["overlay"].clone()).unwrap_or_default();
    op.args = all["args"].clone();
    op.result = all["result"].clone();
    op.group = all["group"].as_i64().unwrap_or(op.id);
    Ok(op)
}

fn select_ops(conn: &Connection, filter: &str, value: i64) -> Result<Vec<PendingOp>> {
    let sql = format!("SELECT {OP_COLUMNS} FROM pending_ops WHERE {filter} ORDER BY id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([value], op_row)?;
    rows.map(|r| finish(r?)).collect()
}

/// Recomputes a folder's counters from its rows, as `finish_folder_sync` does.
pub(crate) fn recount(tx: &Transaction, folder_id: FolderId) -> Result<()> {
    tx.execute(
        "UPDATE folders SET
             total = (SELECT count(*) FROM messages WHERE folder_id = ?1 AND (flags & ?2) = 0),
             unread = (SELECT count(*) FROM messages
                       WHERE folder_id = ?1 AND (flags & ?2) = 0 AND (flags & ?3) = 0)
         WHERE id = ?1",
        params![folder_id, HIDDEN, i64::from(Flags::SEEN)],
    )?;
    Ok(())
}

fn flags_of(tx: &Transaction, folder_id: FolderId, uids: &[u32]) -> Result<Vec<(u32, u8)>> {
    let mut stmt =
        tx.prepare_cached("SELECT flags FROM messages WHERE folder_id = ?1 AND uid = ?2")?;
    let mut out = Vec::with_capacity(uids.len());
    for uid in uids {
        let mut rows = stmt.query(params![folder_id, uid])?;
        if let Some(r) = rows.next()? {
            out.push((*uid, r.get(0)?));
        }
    }
    Ok(out)
}

fn lay(tx: &Transaction, folder_id: FolderId, uids: &[u32], overlay: Overlay) -> Result<()> {
    if overlay.is_empty() {
        return Ok(());
    }
    let mut stmt = tx.prepare_cached(
        "UPDATE messages SET flags = (flags | ?3) & ~?4 WHERE folder_id = ?1 AND uid = ?2",
    )?;
    for uid in uids {
        stmt.execute(params![folder_id, uid, overlay.add, overlay.remove])?;
    }
    Ok(())
}

/// Queues ops and applies their overlays, in one transaction. Every op after the first
/// joins the first one's group, so one Undo reverses them all.
pub(crate) fn queue_ops(tx: &Transaction, ops: &[NewOp], now: i64) -> Result<Vec<OpId>> {
    let mut ids = Vec::with_capacity(ops.len());
    let mut group: Option<OpId> = None;
    for op in ops {
        let before = flags_of(tx, op.folder_id, &op.uids)?;
        let args = json!({
            "overlay": op.overlay,
            "before": before,
            "args": op.args,
            "group": group,
        });
        tx.execute(
            "INSERT INTO pending_ops (account_id, folder_id, uids_json, kind, args_json, state,
                 attempts, last_error, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'queued', 0, '', ?6)",
            params![
                op.account_id,
                op.folder_id,
                serde_json::to_string(&op.uids)?,
                op.kind,
                args.to_string(),
                now
            ],
        )?;
        let id = tx.last_insert_rowid();
        group.get_or_insert(id);
        ids.push(id);
        lay(tx, op.folder_id, &op.uids, op.overlay)?;
        recount(tx, op.folder_id)?;
    }
    Ok(ids)
}

/// Applies a folder change locally and queues its op, in one transaction. The op's
/// `folder_id` is the folder concerned (the new row for a create). Returns (folder, op).
pub(crate) fn queue_folder_op(
    tx: &Transaction,
    change: &FolderChange,
    op: &NewOp,
    now: i64,
) -> Result<(FolderId, OpId)> {
    let folder_id = match change {
        FolderChange::Create(f) => crate::queries::insert_folder(tx, f)?,
        FolderChange::Rename { folder_id, to } => {
            rename_folder(tx, *folder_id, to)?;
            *folder_id
        }
        FolderChange::Delete { folder_id } => {
            crate::queries::delete_folder(tx, *folder_id)?;
            *folder_id
        }
    };
    let op = NewOp {
        folder_id,
        ..op.clone()
    };
    let ids = queue_ops(tx, std::slice::from_ref(&op), now)?;
    Ok((folder_id, ids.first().copied().unwrap_or_default()))
}

pub(crate) fn get_op(conn: &Connection, id: OpId) -> Result<PendingOp> {
    select_ops(conn, "id = ?1", id)?
        .into_iter()
        .next()
        .ok_or(StoreError::NotFound)
}

/// An account's ops that are queued or running, oldest first.
pub(crate) fn account_ops(conn: &Connection, account_id: AccountId) -> Result<Vec<PendingOp>> {
    select_ops(conn, "account_id = ?1 AND state != 'done'", account_id)
}

/// The ops of one Undo group.
pub(crate) fn group_ops(conn: &Connection, group: OpId) -> Result<Vec<PendingOp>> {
    let ops = select_ops(conn, "id >= ?1", group)?;
    Ok(ops.into_iter().filter(|o| o.group == group).collect())
}

/// Lays every queued or running op of a folder over its rows again, after a server pull
/// replaced their flags (section 6, step 5).
pub(crate) fn relay(tx: &Transaction, folder_id: FolderId) -> Result<()> {
    let sql = format!(
        "SELECT {OP_COLUMNS} FROM pending_ops WHERE folder_id = ?1 AND state != 'done' ORDER BY id"
    );
    let ops: Vec<PendingOp> = {
        let mut stmt = tx.prepare_cached(&sql)?;
        let rows = stmt.query_map([folder_id], op_row)?;
        rows.map(|r| finish(r?)).collect::<Result<_>>()?
    };
    for op in &ops {
        lay(tx, op.folder_id, &op.uids, op.overlay)?;
    }
    if !ops.is_empty() {
        recount(tx, folder_id)?;
    }
    Ok(())
}

/// Restores the overlay's bits to what they were when the op was queued.
fn unlay(tx: &Transaction, op_id: OpId) -> Result<PendingOp> {
    let (op, args) = {
        let sql = format!("SELECT {OP_COLUMNS} FROM pending_ops WHERE id = ?1");
        let row = tx.query_row(&sql, [op_id], op_row)?;
        let args: Value = serde_json::from_str(&row.2)?;
        (finish(row)?, args)
    };
    let mask = op.overlay.add | op.overlay.remove;
    if mask != 0 {
        let before: Vec<(u32, u8)> =
            serde_json::from_value(args["before"].clone()).unwrap_or_default();
        let mut stmt = tx.prepare_cached(
            "UPDATE messages SET flags = (flags & ~?3) | (?4 & ?3) WHERE folder_id = ?1 AND uid = ?2",
        )?;
        for (uid, flags) in before {
            stmt.execute(params![op.folder_id, uid, mask, flags])?;
        }
        recount(tx, op.folder_id)?;
    }
    Ok(op)
}

/// Cancels a queued op: its overlay comes off and the row goes. A running or finished op
/// is left alone and `false` returned.
pub(crate) fn cancel_op(tx: &Transaction, op_id: OpId) -> Result<bool> {
    let state: String = tx.query_row(
        "SELECT state FROM pending_ops WHERE id = ?1",
        [op_id],
        |r| r.get(0),
    )?;
    if state != "queued" {
        return Ok(false);
    }
    unlay(tx, op_id)?;
    tx.execute("DELETE FROM pending_ops WHERE id = ?1", [op_id])?;
    Ok(true)
}

/// A permanent failure: the overlay comes off and the op is dropped (section 9).
pub(crate) fn fail_op(tx: &Transaction, op_id: OpId) -> Result<PendingOp> {
    let op = unlay(tx, op_id)?;
    tx.execute("DELETE FROM pending_ops WHERE id = ?1", [op_id])?;
    Ok(op)
}

pub(crate) fn set_op_state(
    tx: &Transaction,
    op_id: OpId,
    state: OpState,
    result: Option<&Value>,
    error: Option<&str>,
) -> Result<()> {
    let args: String = tx.query_row(
        "SELECT args_json FROM pending_ops WHERE id = ?1",
        [op_id],
        |r| r.get(0),
    )?;
    let mut all: Value = serde_json::from_str(&args)?;
    if let Some(result) = result {
        all["result"] = result.clone();
    }
    tx.execute(
        "UPDATE pending_ops SET state = ?2, args_json = ?3,
             attempts = attempts + CASE WHEN ?4 IS NULL THEN 0 ELSE 1 END,
             last_error = coalesce(?4, last_error)
         WHERE id = ?1",
        params![op_id, state.as_sql(), all.to_string(), error],
    )?;
    Ok(())
}

pub(crate) fn delete_op(tx: &Transaction, op_id: OpId) -> Result<()> {
    tx.execute("DELETE FROM pending_ops WHERE id = ?1", [op_id])?;
    Ok(())
}

/// Ops not yet finished, over every account (the `pending-count` event).
pub(crate) fn pending_count(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT count(*) FROM pending_ops WHERE state != 'done'",
        [],
        |r| r.get(0),
    )?)
}

/// Finished ops kept for Undo are dropped once their window passed.
pub(crate) fn purge_done(tx: &Transaction, before: i64) -> Result<usize> {
    Ok(tx.execute(
        "DELETE FROM pending_ops WHERE state = 'done' AND created_at < ?1",
        [before],
    )?)
}

/// A running op left behind by a crash runs again.
pub(crate) fn requeue_running(tx: &Transaction, account_id: AccountId) -> Result<()> {
    tx.execute(
        "UPDATE pending_ops SET state = 'queued' WHERE account_id = ?1 AND state = 'running'",
        [account_id],
    )?;
    Ok(())
}

pub(crate) fn find_folder(
    conn: &Connection,
    account_id: AccountId,
    path: &str,
) -> Result<FolderId> {
    Ok(conn.query_row(
        "SELECT id FROM folders WHERE account_id = ?1 AND path = ?2",
        params![account_id, path],
        |r| r.get(0),
    )?)
}

/// Sets a folder's role; another folder of the account with that role loses it (section 5,
/// step 5: the user can override each role).
pub(crate) fn set_role(
    tx: &Transaction,
    folder_id: FolderId,
    role: Option<SpecialUse>,
) -> Result<()> {
    let account: AccountId = tx.query_row(
        "SELECT account_id FROM folders WHERE id = ?1",
        [folder_id],
        |r| r.get(0),
    )?;
    if let Some(role) = role {
        tx.execute(
            "UPDATE folders SET special_use = NULL WHERE account_id = ?1 AND special_use = ?2",
            params![account, role.as_sql()],
        )?;
    }
    tx.execute(
        "UPDATE folders SET special_use = ?2 WHERE id = ?1",
        params![folder_id, role.map(SpecialUse::as_sql)],
    )?;
    Ok(())
}

pub(crate) fn set_sync(tx: &Transaction, folder_id: FolderId, on: bool) -> Result<()> {
    match tx.execute(
        "UPDATE folders SET sync_enabled = ?2 WHERE id = ?1",
        params![folder_id, on],
    )? {
        0 => Err(StoreError::NotFound),
        _ => Ok(()),
    }
}

pub(crate) fn set_watch(tx: &Transaction, folder_id: FolderId, on: bool) -> Result<()> {
    match tx.execute(
        "UPDATE folders SET watch_idle = ?2 WHERE id = ?1",
        params![folder_id, on],
    )? {
        0 => Err(StoreError::NotFound),
        _ => Ok(()),
    }
}

/// Renames a folder and its children locally (a folder rename moves its subtree).
pub(crate) fn rename_folder(tx: &Transaction, folder_id: FolderId, to: &str) -> Result<()> {
    let (account, from, delimiter): (AccountId, String, String) = tx.query_row(
        "SELECT account_id, path, delimiter FROM folders WHERE id = ?1",
        [folder_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    tx.execute(
        "UPDATE folders SET path = ?2 WHERE id = ?1",
        params![folder_id, to],
    )?;
    if !delimiter.is_empty() {
        let prefix = format!("{from}{delimiter}");
        let children: Vec<(FolderId, String)> = {
            let mut stmt = tx.prepare_cached(
                "SELECT id, path FROM folders WHERE account_id = ?1 AND substr(path, 1, length(?2)) = ?2",
            )?;
            let rows = stmt.query_map(params![account, prefix], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, path) in children {
            let rest = &path[prefix.len()..];
            tx.execute(
                "UPDATE folders SET path = ?2 WHERE id = ?1",
                params![id, format!("{to}{delimiter}{rest}")],
            )?;
        }
    }
    Ok(())
}

/// UIDs of a folder's visible unread messages (Mark all read).
pub(crate) fn unread_uids(conn: &Connection, folder_id: FolderId) -> Result<Vec<u32>> {
    let mut stmt = conn.prepare_cached(
        "SELECT uid FROM messages WHERE folder_id = ?1 AND (flags & ?2) = 0 AND (flags & ?3) = 0
         ORDER BY uid",
    )?;
    let rows = stmt.query_map(params![folder_id, HIDDEN, i64::from(Flags::SEEN)], |r| {
        r.get(0)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// UIDs of a folder's visible messages (Empty Trash).
pub(crate) fn visible_uids(conn: &Connection, folder_id: FolderId) -> Result<Vec<u32>> {
    let mut stmt = conn.prepare_cached(
        "SELECT uid FROM messages WHERE folder_id = ?1 AND (flags & ?2) = 0 ORDER BY uid",
    )?;
    let rows = stmt.query_map(params![folder_id, HIDDEN], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The oldest INTERNALDATE cached in a folder: where its sync window starts (G51, G62).
pub(crate) fn oldest_internal(conn: &Connection, folder_id: FolderId) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT min(internal_utc) FROM messages WHERE folder_id = ?1",
        [folder_id],
        |r| r.get(0),
    )?)
}

/// UIDs in a folder whose Message-ID is one of `ids` (Undo without COPYUID, section 9).
pub(crate) fn uids_by_message_id(
    conn: &Connection,
    folder_id: FolderId,
    ids: &[String],
) -> Result<Vec<u32>> {
    let mut stmt =
        conn.prepare_cached("SELECT uid FROM messages WHERE folder_id = ?1 AND message_id = ?2")?;
    let mut out = Vec::new();
    for id in ids {
        let rows = stmt.query_map(params![folder_id, id], |r| r.get(0))?;
        for uid in rows {
            out.push(uid?);
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}
