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
    /// The Sent copy of an outbox item (section 7; SPEC_GAPS G66).
    pub const APPEND: &str = "append";
    /// Upload a draft's new version and remove the previous one (section 7, Drafts).
    pub const DRAFT_REPLACE: &str = "draft_replace";
    /// Remove an uploaded draft (sent or discarded).
    pub const DRAFT_DELETE: &str = "draft_delete";
    /// Gmail: add a label, `UID COPY` to the label's folder (section 18.1).
    pub const LABEL_ADD: &str = "label_add";
    /// Gmail: remove a label, `\Deleted` and `UID EXPUNGE` of that UID in its folder.
    pub const LABEL_REMOVE: &str = "label_remove";
}

/// The Sent copy of outbox item `outbox_id`, to `sent` (section 7). Whether it is filed is
/// decided when it runs: `save_sent_copy` `auto` is off on servers with `X-GM-EXT-1`.
pub fn append_op(account_id: AccountId, sent: &Folder, outbox_id: i64) -> NewOp {
    NewOp {
        account_id,
        folder_id: sent.id,
        uids: Vec::new(),
        kind: kind::APPEND.into(),
        args: json!({ "outbox_id": outbox_id }),
        overlay: Overlay::default(),
    }
}

/// A draft's new version, already built (`raw_b64`), to `drafts`; the previous uploaded
/// UID is removed after the APPEND (section 7, T18).
pub fn draft_replace_op(
    account_id: AccountId,
    drafts: &Folder,
    draft_id: i64,
    raw_b64: String,
    message_id: &str,
) -> NewOp {
    NewOp {
        account_id,
        folder_id: drafts.id,
        uids: Vec::new(),
        kind: kind::DRAFT_REPLACE.into(),
        args: json!({ "draft_id": draft_id, "raw_b64": raw_b64, "message_id": message_id }),
        overlay: Overlay::default(),
    }
}

/// Removes an uploaded draft copy.
pub fn draft_delete_op(account_id: AccountId, drafts: &Folder, uid: u32) -> NewOp {
    NewOp {
        account_id,
        folder_id: drafts.id,
        uids: vec![uid],
        kind: kind::DRAFT_DELETE.into(),
        args: json!({}),
        overlay: Overlay::default(),
    }
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

/// A keyword that only the server keeps (`$Muted`, section 18.5): set or cleared where the
/// folder's PERMANENTFLAGS take new keywords, skipped elsewhere. No local bits change.
pub fn keyword_op(
    account_id: AccountId,
    folder_id: FolderId,
    uids: Vec<u32>,
    keyword: &str,
    set: bool,
) -> NewOp {
    let flag = [Flag::Keyword(keyword.to_string())];
    let (add, remove): (&[Flag], &[Flag]) = if set { (&flag, &[]) } else { (&[], &flag) };
    let mut op = flags_op(account_id, folder_id, uids, add, remove);
    op.args["keywords_if_allowed"] = json!(true);
    op
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

/// Snooze (section 18.2): a move to the Snoozed folder whose snoozes follow the copies
/// (`rows` are the source rows, in the order of `message_ids`).
pub fn snooze_op(
    account_id: AccountId,
    folder_id: FolderId,
    uids: Vec<u32>,
    snoozed: &Folder,
    message_ids: Vec<String>,
    rows: Vec<i64>,
) -> NewOp {
    let mut op = move_op(account_id, folder_id, uids, snoozed, message_ids);
    op.args["snooze"] = json!(true);
    op.args["rows"] = json!(rows);
    op
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

/// Gmail add label (section 18.1): `UID COPY` from the message's row to the label's folder.
/// Gmail deduplicates by `X-GM-MSGID`, so this adds a label and never a second copy. The row
/// stays as it is.
pub fn label_add_op(
    account_id: AccountId,
    folder_id: FolderId,
    uids: Vec<u32>,
    label: &Folder,
    message_ids: Vec<String>,
) -> NewOp {
    NewOp {
        account_id,
        folder_id,
        uids,
        kind: kind::LABEL_ADD.into(),
        args: json!({ "dest": label.id, "dest_path": label.path, "message_ids": message_ids }),
        overlay: Overlay::default(),
    }
}

/// Gmail remove label (section 18.1): in the label's folder, `\Deleted` and `UID EXPUNGE`
/// of those UIDs only; the row is hidden at once. `restore` is a row of the same message in
/// another folder, which Undo copies back from. With `find`, the UIDs are looked up by
/// Message-ID first (Undo of an add without COPYUID).
pub fn label_remove_op(
    account_id: AccountId,
    label: FolderId,
    uids: Vec<u32>,
    message_ids: Vec<String>,
    restore: Option<(FolderId, u32)>,
    find: bool,
) -> NewOp {
    NewOp {
        account_id,
        folder_id: label,
        uids,
        kind: kind::LABEL_REMOVE.into(),
        args: json!({
            "message_ids": message_ids,
            "restore_folder": restore.map(|r| r.0),
            "restore_uid": restore.map(|r| r.1),
            "find": find,
        }),
        overlay: if find {
            Overlay::default()
        } else {
            Overlay::HIDE
        },
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
    // Folder ops go first: a message op may target a folder created a moment ago (the
    // Snoozed folder on a first snooze, section 18.2). CREATE leaves the selection alone.
    if store
        .account_ops(account_id)
        .await?
        .iter()
        .any(is_folder_op)
    {
        run_folder_ops_account(t, store, events, account_id).await?;
    }
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
                if op.args["snooze"].as_bool() == Some(true) {
                    // The source row goes at this folder's next pull; the snooze must point
                    // at the copy in the Snoozed folder first (SPEC_GAPS G77).
                    follow_snooze(t, store, folder, &op, &result).await?;
                }
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

/// After a snooze move: header-fetches the moved copies in the destination, re-points each
/// snooze at its copy, and selects `folder` again for the ops still to run.
async fn follow_snooze<T: ImapTransport>(
    t: &mut T,
    store: &Store,
    folder: &Folder,
    op: &PendingOp,
    result: &Value,
) -> Result<(), SyncError> {
    let Some(dest_id) = op.args["dest"].as_i64() else {
        return Ok(());
    };
    let dest = store.get_folder(dest_id).await?;
    let info = t.select(&dest.path).await?;
    if dest.uidvalidity == 0 {
        store.set_uidvalidity(dest.id, info.uidvalidity).await?;
    }
    let message_ids: Vec<String> = op.args["message_ids"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let rows: Vec<i64> = op.args["rows"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    let mut uids: Vec<u32> = result["copyuid"]["dest"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_u64)
                .filter_map(|u| u32::try_from(u).ok())
                .collect()
        })
        .unwrap_or_default();
    if uids.is_empty() {
        for id in message_ids.iter().filter(|i| !i.starts_with("synth:")) {
            let key = SearchKey::Header("Message-ID".into(), format!("<{id}>"));
            uids.extend(t.uid_search(&key).await?);
        }
    }
    let known: HashSet<u32> = store.cached_uids(dest.id).await?.into_iter().collect();
    let missing: Vec<u32> = uids.into_iter().filter(|u| !known.contains(u)).collect();
    if !missing.is_empty() {
        let gmail = t.capabilities().gmail;
        let headers = t
            .uid_fetch_headers(&UidSet::new(missing), mail_imap::HeaderFetch { gmail })
            .await?;
        let new: Vec<_> = headers
            .iter()
            .map(|h| convert::new_message(h, op.account_id, dest.id, now(), gmail))
            .collect();
        if !new.is_empty() {
            store.insert_synced(dest.id, new).await?;
        }
    }
    for (old, id) in rows.iter().zip(&message_ids) {
        if let Some(copy) = store.find_by_message_id(dest.id, id.clone()).await? {
            store.repoint_snooze(*old, copy).await?;
        }
    }
    t.select(&folder.path).await?;
    Ok(())
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
    match op.kind.as_str() {
        kind::APPEND => return append_sent(t, store, folder, op).await,
        kind::DRAFT_REPLACE => return replace_draft(t, store, folder, op).await,
        _ => {}
    }
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
            // `$Muted` only where PERMANENTFLAGS take new keywords (section 18.5); the
            // folder is selected again to read them.
            if op.args["keywords_if_allowed"].as_bool() == Some(true)
                && !t.select(&folder.path).await?.allows_new_keywords()
            {
                return Ok(Outcome::Flags);
            }
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
        kind::LABEL_ADD => {
            let dest = op.args["dest_path"]
                .as_str()
                .ok_or(SyncError::Store("label without a folder".into()))?;
            let mut copied = CopyUid {
                uidvalidity: 0,
                source: Vec::new(),
                dest: Vec::new(),
            };
            let mut complete = true;
            for chunk in set.chunks() {
                match t.uid_copy(&chunk, dest).await? {
                    Some(c) => {
                        copied.uidvalidity = c.uidvalidity;
                        copied.source.extend(c.source);
                        copied.dest.extend(c.dest);
                    }
                    None => complete = false,
                }
            }
            let copyuid = (complete && !copied.dest.is_empty()).then(|| {
                json!({ "uidvalidity": copied.uidvalidity, "source": copied.source, "dest": copied.dest })
            });
            Ok(Outcome::Moved(json!({ "copyuid": copyuid })))
        }
        kind::EXPUNGE | kind::DRAFT_DELETE | kind::LABEL_REMOVE => {
            for chunk in set.chunks() {
                t.uid_store(&chunk, StoreOp::Add, &[Flag::Deleted]).await?;
                expunge_exactly(t, &chunk).await?;
            }
            Ok(Outcome::Moved(json!({})))
        }
        _ => Err(SyncError::Store(format!("unknown op kind {}", op.kind))),
    }
}

/// Files the Sent copy (section 7): `APPEND` with `\Seen`, unless the account's
/// `save_sent_copy` says no (`auto` is off where the server advertises `X-GM-EXT-1`, as Gmail
/// files its own copy). The raw message is dropped from the outbox afterwards.
async fn append_sent<T: ImapTransport>(
    t: &mut T,
    store: &Store,
    folder: &Folder,
    op: &PendingOp,
) -> Result<Outcome, SyncError> {
    let outbox_id = op.args["outbox_id"].as_i64().unwrap_or_default();
    let account = store.get_account(op.account_id).await?;
    let wanted = match account.config.save_sent_copy {
        mail_store::SaveSentCopy::On => true,
        mail_store::SaveSentCopy::Off => false,
        mail_store::SaveSentCopy::Auto => !t.capabilities().gmail,
    };
    let raw = match store.outbox_raw(outbox_id).await {
        Ok(raw) => raw,
        Err(mail_store::StoreError::NotFound) => return Ok(Outcome::Dropped),
        Err(e) => return Err(e.into()),
    };
    if wanted && !raw.is_empty() {
        t.append(&folder.path, &[Flag::Seen], &raw).await?;
    }
    store.clear_outbox_raw(outbox_id).await?;
    Ok(if wanted {
        Outcome::Flags
    } else {
        Outcome::Dropped
    })
}

/// Uploads a draft's new version with `\Draft`, then removes the previously uploaded UID,
/// so the server holds one copy (section 7, T18). Without APPENDUID the new copy is found
/// by its Message-ID.
async fn replace_draft<T: ImapTransport>(
    t: &mut T,
    store: &Store,
    folder: &Folder,
    op: &PendingOp,
) -> Result<Outcome, SyncError> {
    let draft_id = op.args["draft_id"].as_i64().unwrap_or_default();
    let draft = match store.get_draft(draft_id).await {
        Ok(d) => d,
        // Sent or discarded meanwhile: its own op removes the uploaded copy.
        Err(mail_store::StoreError::NotFound) => return Ok(Outcome::Dropped),
        Err(e) => return Err(e.into()),
    };
    let raw_b64 = op.args["raw_b64"].as_str().unwrap_or_default();
    let raw = mail_parser::decoders::base64::base64_decode(raw_b64.as_bytes())
        .ok_or_else(|| SyncError::Store("draft could not be decoded".into()))?;
    let appended = t
        .append(&folder.path, &[Flag::Draft, Flag::Seen], &raw)
        .await?;
    let old = draft.imap_uid;
    let new_uid = match appended {
        Some(a) => Some(a.uid),
        None => {
            let id = op.args["message_id"].as_str().unwrap_or_default();
            let key = SearchKey::Header("Message-ID".into(), format!("<{id}>"));
            t.uid_search(&key)
                .await?
                .into_iter()
                .filter(|u| Some(*u) != old)
                .max()
        }
    };
    if let Some(old) = old.filter(|o| Some(*o) != new_uid) {
        let set = UidSet::single(old);
        t.uid_store(&set, StoreOp::Add, &[Flag::Deleted]).await?;
        expunge_exactly(t, &set).await?;
    }
    store
        .set_draft_uploaded(draft_id, new_uid, crate::now())
        .await?;
    Ok(Outcome::Flags)
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
