//! `ops` (BUILD.md sections 3 and 9): the pending-op queue's server half. Commands queue an
//! op and its local overlay in one transaction (`mail-store`), so the UI changes at once;
//! the account's actor pushes ops before it pulls (section 6, step 3) and whenever new ones
//! are queued. A permanent refusal reverts the overlay and is reported once (`OpFailed`,
//! SPEC_GAPS G58); a transient one leaves the op queued for the next connection.
//!
//! IMAP per section 9: flags with `UID STORE (+|-)FLAGS.SILENT`; moves with `UID MOVE`, or
//! `UID COPY`, `\Deleted` and `UID EXPUNGE` of exactly those UIDs (UIDPLUS). Without UIDPLUS
//! a bare `EXPUNGE` runs only when `UID SEARCH DELETED` shows no other flagged message;
//! otherwise the source stays flagged and hidden. Every command takes at most 500 UIDs.

use std::collections::HashSet;

use mail_imap::{CopyUid, Flag, ImapTransport, SearchKey, StoreOp, UidSet};
use mail_store::{AccountId, Folder, FolderId, NewOp, OpId, OpState, Overlay, PendingOp, Store};
use serde_json::{json, Value};

use crate::{convert, now, Events, SyncError, SyncEvent};

/// `pending_ops.kind` values.
pub mod kind {
    pub const FLAGS: &str = "flags";
    pub const MOVE: &str = "move";
    /// Permanent delete: `\Deleted` and expunge.
    pub const EXPUNGE: &str = "expunge";
    pub const FOLDER_CREATE: &str = "folder_create";
    pub const FOLDER_RENAME: &str = "folder_rename";
    pub const FOLDER_DELETE: &str = "folder_delete";
}

fn flag_names(flags: &[Flag]) -> Vec<String> {
    flags.iter().map(|f| f.as_imap().to_string()).collect()
}

/// Star, read state, Answered or Forwarded: the bits change at once (invariant 2).
pub fn flags_op(
    account_id: AccountId,
    folder_id: FolderId,
    uids: Vec<u32>,
    add: &[Flag],
    remove: &[Flag],
) -> NewOp {
    NewOp {
        account_id,
        folder_id,
        uids,
        kind: kind::FLAGS.into(),
        args: json!({ "add": flag_names(add), "remove": flag_names(remove) }),
        overlay: Overlay {
            add: convert::flags(add).0,
            remove: convert::flags(remove).0,
        },
    }
}

/// Move (also Archive, Delete to Trash and Spam): the rows are hidden at once (G59). The
/// Message-IDs let Undo find the messages again when the server gives no COPYUID.
pub fn move_op(
    account_id: AccountId,
    folder_id: FolderId,
    uids: Vec<u32>,
    dest: &Folder,
    message_ids: Vec<String>,
) -> NewOp {
    NewOp {
        account_id,
        folder_id,
        uids,
        kind: kind::MOVE.into(),
        args: json!({ "dest": dest.id, "dest_path": dest.path, "message_ids": message_ids }),
        overlay: Overlay::HIDE,
    }
}

/// Undo of a move whose COPYUID is unknown: the messages are found in `folder_id` by
/// Message-ID after a resync, then moved back (section 9, Undo).
pub fn move_back_by_message_id(
    account_id: AccountId,
    folder_id: FolderId,
    dest: &Folder,
    message_ids: Vec<String>,
) -> NewOp {
    NewOp {
        account_id,
        folder_id,
        uids: Vec::new(),
        kind: kind::MOVE.into(),
        args: json!({
            "dest": dest.id, "dest_path": dest.path, "message_ids": message_ids, "find": true
        }),
        overlay: Overlay::default(),
    }
}

/// Permanent delete (in Trash or Junk, without a Trash folder, or Empty Trash).
pub fn expunge_op(account_id: AccountId, folder_id: FolderId, uids: Vec<u32>) -> NewOp {
    NewOp {
        account_id,
        folder_id,
        uids,
        kind: kind::EXPUNGE.into(),
        args: json!({}),
        overlay: Overlay::HIDE,
    }
}

/// A folder change; `folder_id` is the local row it concerns (no foreign key, G22).
pub fn folder_op(account_id: AccountId, folder_id: FolderId, kind: &str, args: Value) -> NewOp {
    NewOp {
        account_id,
        folder_id,
        uids: Vec::new(),
        kind: kind.into(),
        args,
        overlay: Overlay::default(),
    }
}

/// Whether an op is about a folder rather than messages in it.
pub fn is_folder_op(op: &PendingOp) -> bool {
    op.kind.starts_with("folder_")
}

/// What running one folder's ops asks of the caller.
#[derive(Debug, Default)]
pub(crate) struct Followup {
    /// Folders to sync: sources (moved rows go) and destinations (they arrive).
    pub sync: Vec<FolderId>,
    /// Something ran.
    pub changed: bool,
}

/// Runs every queued message op of `folder`, which is selected. A transient error stops
/// the run and leaves the op queued; a permanent one reverts it and goes on.
pub(crate) async fn run_folder_ops<T: ImapTransport>(
    t: &mut T,
    store: &Store,
    events: &Events,
    account_id: AccountId,
    folder: &Folder,
) -> Result<Followup, SyncError> {
    let mut follow = Followup::default();
    let ops: Vec<PendingOp> = store
        .account_ops(account_id)
        .await?
        .into_iter()
        .filter(|o| o.folder_id == folder.id && !is_folder_op(o))
        .collect();
    for op in ops {
        follow.changed = true;
        store
            .set_op_state(op.id, OpState::Running, None, None)
            .await?;
        match run_message_op(t, store, folder, &op).await {
            Ok(Outcome::Dropped) => store.delete_op(op.id).await?,
            Ok(Outcome::Flags) => {
                store.delete_op(op.id).await?;
                follow.sync.push(folder.id);
            }
            Ok(Outcome::Moved(result)) => {
                store
                    .set_op_state(op.id, OpState::Done, Some(result), None)
                    .await?;
                follow.sync.push(folder.id);
                if let Some(dest) = op.args["dest"].as_i64() {
                    follow.sync.push(dest);
                }
            }
            Err(e) if e.class() == mail_imap::ErrorClass::Permanent => {
                fail(store, events, &op, e).await?;
            }
            Err(e) => {
                store
                    .set_op_state(op.id, OpState::Queued, None, Some(error_text(&e)))
                    .await?;
                return Err(e);
            }
        }
        events.send(SyncEvent::StoreChanged {
            account_id,
            folder_id: Some(folder.id),
        });
        events.send(SyncEvent::PendingChanged { account_id });
    }
    Ok(follow)
}

fn error_text(e: &SyncError) -> String {
    match e {
        SyncError::Imap(i) => i.to_string(),
        SyncError::Auth(a) => a.to_string(),
        SyncError::Store(s) => s.clone(),
        SyncError::Gone => "gone".into(),
    }
}

/// Reverts a permanently refused op and reports it once (section 9; G58).
async fn fail(
    store: &Store,
    events: &Events,
    op: &PendingOp,
    error: SyncError,
) -> Result<(), SyncError> {
    tracing::warn!(op_id = op.id, kind = %op.kind, error = %error_text(&error), "op refused");
    store.fail_op(op.id).await?;
    events.send(SyncEvent::OpFailed {
        account_id: op.account_id,
        op_id: op.group,
        kind: op.kind.clone(),
        error,
    });
    Ok(())
}

enum Outcome {
    /// Every target vanished from the server: dropped silently (section 6, Conflicts).
    Dropped,
    Flags,
    /// Moved or expunged; the COPYUID result for Undo.
    Moved(Value),
}

fn flags_from(v: &Value) -> Vec<Flag> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(Flag::parse)
                .collect()
        })
        .unwrap_or_default()
}

async fn run_message_op<T: ImapTransport>(
    t: &mut T,
    store: &Store,
    folder: &Folder,
    op: &PendingOp,
) -> Result<Outcome, SyncError> {
    let mut uids = op.uids.clone();
    if op.args["find"].as_bool() == Some(true) {
        // No COPYUID: find the messages again by Message-ID (section 9, Undo). The server
        // is asked, so a destination not synced yet is no obstacle; synthetic IDs
        // (section 10) never reached a server and are looked up locally.
        let ids: Vec<String> = op.args["message_ids"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        for id in &ids {
            if id.starts_with("synth:") {
                continue;
            }
            let key = SearchKey::Header("Message-ID".into(), format!("<{id}>"));
            uids.extend(t.uid_search(&key).await?);
        }
        let local: Vec<String> = ids
            .into_iter()
            .filter(|i| i.starts_with("synth:"))
            .collect();
        if !local.is_empty() {
            uids.extend(store.uids_by_message_id(folder.id, local).await?);
        }
    }
    // Only UIDs still on the server; an op whose targets are all gone is dropped.
    let present: Vec<u32> = if uids.is_empty() {
        Vec::new()
    } else {
        let set = UidSet::new(uids.iter().copied());
        let mut found = Vec::new();
        for chunk in set.chunks() {
            let hits = t.uid_search(&SearchKey::Uid(chunk.clone())).await?;
            found.extend(hits.into_iter().filter(|u| chunk.contains(*u)));
        }
        found
    };
    if present.is_empty() {
        return Ok(Outcome::Dropped);
    }
    let set = UidSet::new(present);
    match op.kind.as_str() {
        kind::FLAGS => {
            let add = flags_from(&op.args["add"]);
            let remove = flags_from(&op.args["remove"]);
            for chunk in set.chunks() {
                if !add.is_empty() {
                    t.uid_store(&chunk, StoreOp::Add, &add).await?;
                }
                if !remove.is_empty() {
                    t.uid_store(&chunk, StoreOp::Remove, &remove).await?;
                }
            }
            Ok(Outcome::Flags)
        }
        kind::MOVE => {
            let dest = op.args["dest_path"]
                .as_str()
                .ok_or(SyncError::Store("move without a destination".into()))?;
            let mut copied = CopyUid {
                uidvalidity: 0,
                source: Vec::new(),
                dest: Vec::new(),
            };
            let mut complete = true;
            for chunk in set.chunks() {
                let result = if t.capabilities().move_ {
                    t.uid_move(&chunk, dest).await?
                } else {
                    let r = t.uid_copy(&chunk, dest).await?;
                    t.uid_store(&chunk, StoreOp::Add, &[Flag::Deleted]).await?;
                    expunge_exactly(t, &chunk).await?;
                    r
                };
                match result {
                    Some(c) => {
                        copied.uidvalidity = c.uidvalidity;
                        copied.source.extend(c.source);
                        copied.dest.extend(c.dest);
                    }
                    None => complete = false,
                }
            }
            let copyuid = (complete && !copied.dest.is_empty()).then(|| {
                json!({
                    "uidvalidity": copied.uidvalidity,
                    "source": copied.source,
                    "dest": copied.dest,
                })
            });
            Ok(Outcome::Moved(json!({ "copyuid": copyuid })))
        }
        kind::EXPUNGE => {
            for chunk in set.chunks() {
                t.uid_store(&chunk, StoreOp::Add, &[Flag::Deleted]).await?;
                expunge_exactly(t, &chunk).await?;
            }
            Ok(Outcome::Moved(json!({})))
        }
        _ => Err(SyncError::Store(format!("unknown op kind {}", op.kind))),
    }
}

/// Removes exactly `uids`, which carry `\Deleted` (section 9, Move fallback): `UID EXPUNGE`
/// with UIDPLUS; without it a bare `EXPUNGE` only when no other message is flagged
/// `\Deleted`, else nothing (the rows stay flagged and hidden).
async fn expunge_exactly<T: ImapTransport>(t: &mut T, uids: &UidSet) -> Result<(), SyncError> {
    if t.capabilities().uidplus {
        t.uid_expunge(uids).await?;
        return Ok(());
    }
    let flagged: HashSet<u32> = t
        .uid_search(&SearchKey::Deleted)
        .await?
        .into_iter()
        .collect();
    if flagged.iter().all(|u| uids.contains(*u)) {
        t.expunge().await?;
    } else {
        tracing::info!("other messages carry \\Deleted; leaving the moved ones flagged");
    }
    Ok(())
}

/// Runs the account's folder ops (create, rename, delete with modified UTF-7 names, done by
/// the transport). Returns whether one ran, so the caller lists folders again.
pub(crate) async fn run_folder_ops_account<T: ImapTransport>(
    t: &mut T,
    store: &Store,
    events: &Events,
    account_id: AccountId,
) -> Result<bool, SyncError> {
    let ops: Vec<PendingOp> = store
        .account_ops(account_id)
        .await?
        .into_iter()
        .filter(is_folder_op)
        .collect();
    let ran = !ops.is_empty();
    for op in ops {
        store
            .set_op_state(op.id, OpState::Running, None, None)
            .await?;
        let path = op.args["path"].as_str().unwrap_or_default().to_string();
        let result = match op.kind.as_str() {
            kind::FOLDER_CREATE => t.create(&path).await,
            kind::FOLDER_RENAME => {
                t.rename(&path, op.args["to"].as_str().unwrap_or_default())
                    .await
            }
            kind::FOLDER_DELETE => t.delete(&path).await,
            _ => Ok(()),
        };
        match result.map_err(SyncError::from) {
            Ok(()) => store.delete_op(op.id).await?,
            Err(e) if e.class() == mail_imap::ErrorClass::Permanent => {
                // Revert the local change: a created row goes, a renamed one gets its name
                // back. A deleted folder comes back with the next LIST.
                match op.kind.as_str() {
                    kind::FOLDER_CREATE => {
                        let _ = store.delete_folder(op.folder_id).await;
                    }
                    kind::FOLDER_RENAME => {
                        let _ = store.rename_folder(op.folder_id, &path).await;
                    }
                    _ => {}
                }
                fail(store, events, &op, e).await?;
            }
            Err(e) => {
                store
                    .set_op_state(op.id, OpState::Queued, None, Some(error_text(&e)))
                    .await?;
                return Err(e);
            }
        }
        events.send(SyncEvent::StoreChanged {
            account_id,
            folder_id: None,
        });
        events.send(SyncEvent::PendingChanged { account_id });
    }
    Ok(ran)
}

/// The Undo group's ops with their state settled: running ones are waited for (at most
/// `wait`), since the actor finishes an op in moments.
pub async fn settled_group(
    store: &Store,
    group: OpId,
    wait: std::time::Duration,
) -> Result<Vec<PendingOp>, SyncError> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let ops = store.group_ops(group).await?;
        if ops.iter().all(|o| o.state != OpState::Running)
            || tokio::time::Instant::now() >= deadline
        {
            return Ok(ops);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// UTC now, for `pending_ops.created_at`.
pub fn timestamp() -> i64 {
    now()
}
