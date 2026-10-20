//! Mailbox operations (section 9): the `message.*` and `folder.*` commands that change
//! the server. Each applies locally at once and is queued durably (`ops`, in `mail-sync`);
//! the account's actor pushes it. Undo cancels a queued op or reverses a finished one.

use std::collections::BTreeMap;
use std::time::Duration;

use mail_imap::{Flag, ImapConnector};
use mail_smtp::SmtpCheck;
use mail_store::{
    AccountId, Folder, FolderChange, FolderId, MessageId, NewFolder, NewOp, OpId, OpState,
    SpecialUse,
};
use mail_sync::ops::{self, kind};
use serde_json::{json, Value};

use crate::core::{Core, CoreEvent};
use crate::error::{AppError, ErrorCode};
use crate::views::{FlagChange, Role};

/// Running ops finish in moments; Undo waits this long for one.
const UNDO_WAIT: Duration = Duration::from_secs(5);

/// The folder Archive moves to: the `\Archive` folder, else Gmail's `\All` (section 9).
pub(crate) fn archive_folder(folders: &[Folder]) -> Option<&Folder> {
    folders
        .iter()
        .find(|f| f.special_use == Some(SpecialUse::Archive))
        .or_else(|| {
            folders
                .iter()
                .find(|f| f.special_use == Some(SpecialUse::All))
        })
}

/// Delete from `folder` is permanent: it is Trash or Junk, or there is no Trash.
pub(crate) fn delete_is_permanent(folders: &[Folder], folder: &Folder) -> bool {
    matches!(
        folder.special_use,
        Some(SpecialUse::Trash | SpecialUse::Junk)
    ) || !folders
        .iter()
        .any(|f| f.special_use == Some(SpecialUse::Trash))
}

fn role_folder(folders: &[Folder], role: SpecialUse) -> Option<&Folder> {
    folders.iter().find(|f| f.special_use == Some(role))
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn uids(v: &Value) -> Vec<u32> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_u64)
                .filter_map(|u| u32::try_from(u).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The messages of one source folder in a command.
struct Group {
    account_id: AccountId,
    folder: Folder,
    uids: Vec<u32>,
    message_ids: Vec<String>,
}

impl<I, S, H> Core<I, S, H>
where
    I: ImapConnector + Clone + 'static,
    S: SmtpCheck + mail_smtp::SmtpSubmit + Clone + 'static,
    H: crate::core::Http,
{
    /// Queues ops (for the outbox and drafts); see [`Self::queue`].
    pub(crate) async fn queue_ops(&self, new: Vec<NewOp>) -> Result<OpId, AppError> {
        self.queue(new).await
    }

    /// The command's messages, by source folder.
    async fn groups(&self, message_ids: &[MessageId]) -> Result<Vec<Group>, AppError> {
        let mut by_folder: BTreeMap<FolderId, (AccountId, Vec<u32>, Vec<String>)> = BTreeMap::new();
        for id in message_ids {
            let m = self.store.get_message(*id).await?;
            let entry = by_folder
                .entry(m.folder_id)
                .or_insert_with(|| (m.account_id, Vec::new(), Vec::new()));
            entry.1.push(m.uid);
            entry.2.push(m.message_id);
        }
        if by_folder.is_empty() {
            return Err(AppError::invalid("no messages"));
        }
        let mut groups = Vec::with_capacity(by_folder.len());
        for (folder_id, (account_id, uids, message_ids)) in by_folder {
            groups.push(Group {
                account_id,
                folder: self.store.get_folder(folder_id).await?,
                uids,
                message_ids,
            });
        }
        Ok(groups)
    }

    /// Queues ops in one transaction, tells the actors, and reports the new pending count.
    async fn queue(&self, new: Vec<NewOp>) -> Result<OpId, AppError> {
        // Messages this app moves into an INBOX or watched folder are not new mail there
        // (rules and notifications, SPEC_GAPS G77).
        for op in new.iter().filter(|o| o.kind == kind::MOVE) {
            let Some(dest) = op.args["dest"].as_i64() else {
                continue;
            };
            if let Ok(f) = self.store.get_folder(dest).await {
                if f.special_use == Some(SpecialUse::Inbox) || f.watch_idle {
                    let mut moved = crate::core::lock(&self.moved_in);
                    for id in strings(&op.args["message_ids"]) {
                        moved.insert(id, tokio::time::Instant::now());
                    }
                }
            }
        }
        let _ = self
            .store
            .purge_done_ops(ops::timestamp() - mail_sync::UNDO_WINDOW_SECS)
            .await;
        let targets: Vec<(AccountId, Option<FolderId>, FolderId)> = new
            .iter()
            .map(|o| {
                let folder_op = o.kind.starts_with("folder_");
                (
                    o.account_id,
                    (!folder_op).then_some(o.folder_id),
                    o.folder_id,
                )
            })
            .collect();
        let ids = self.store.queue_ops(new, ops::timestamp()).await?;
        let first = *ids
            .first()
            .ok_or_else(|| AppError::invalid("nothing to do"))?;
        let actors = self.actors.lock().await;
        for (account_id, folder, changed) in targets {
            if let Some(h) = actors.get(&account_id) {
                h.ops_queued(folder);
            }
            (self.sink)(CoreEvent::StoreChanged {
                account_id,
                folder_id: folder.or(Some(changed)),
            });
        }
        drop(actors);
        self.emit_pending().await;
        Ok(first)
    }

    /// A folder op: the local change and the queued op in one transaction, then the actor.
    async fn queue_folder(
        &self,
        account_id: AccountId,
        change: FolderChange,
        op_kind: &str,
        args: Value,
    ) -> Result<(), AppError> {
        let op = ops::folder_op(account_id, 0, op_kind, args);
        self.store
            .queue_folder_op(change, op, ops::timestamp())
            .await?;
        if let Some(h) = self.actors.lock().await.get(&account_id) {
            h.ops_queued(None);
        }
        (self.sink)(CoreEvent::StoreChanged {
            account_id,
            folder_id: None,
        });
        self.emit_pending().await;
        Ok(())
    }

    /// `pending-count`: unfinished ops and outbox items.
    pub(crate) async fn emit_pending(&self) {
        if let Ok(total) = crate::core::pending_total(&self.store).await {
            (self.sink)(CoreEvent::PendingCount { total });
        }
    }

    /// `message.set_flags`: read state and star (section 9).
    pub async fn message_set_flags(
        &self,
        message_ids: &[MessageId],
        change: FlagChange,
    ) -> Result<OpId, AppError> {
        let mut add = Vec::new();
        let mut remove = Vec::new();
        for (value, flag) in [(change.seen, Flag::Seen), (change.flagged, Flag::Flagged)] {
            match value {
                Some(true) => add.push(flag),
                Some(false) => remove.push(flag),
                None => {}
            }
        }
        if add.is_empty() && remove.is_empty() {
            return Err(AppError::invalid("no flag to change"));
        }
        let new = self
            .groups(message_ids)
            .await?
            .into_iter()
            .map(|g| ops::flags_op(g.account_id, g.folder.id, g.uids, &add, &remove))
            .collect();
        self.queue(new).await
    }

    /// `message.mark_all_read`: every unread message of a folder (500 UIDs per command).
    pub async fn message_mark_all_read(&self, folder_id: FolderId) -> Result<OpId, AppError> {
        let folder = self.store.get_folder(folder_id).await?;
        let uids = self.store.unread_uids(folder_id).await?;
        if uids.is_empty() {
            return Err(AppError::invalid("nothing unread"));
        }
        self.queue(vec![ops::flags_op(
            folder.account_id,
            folder_id,
            uids,
            &[Flag::Seen],
            &[],
        )])
        .await
    }

    /// `message.move` to `folder_id` (also Spam and Not spam, section 18).
    pub async fn message_move(
        &self,
        message_ids: &[MessageId],
        folder_id: FolderId,
    ) -> Result<OpId, AppError> {
        let dest = self.store.get_folder(folder_id).await?;
        let groups = self.groups(message_ids).await?;
        let mut new = Vec::new();
        for g in groups {
            if g.account_id != dest.account_id {
                return Err(AppError::invalid("messages move within their account"));
            }
            if g.folder.id != dest.id {
                new.push(ops::move_op(
                    g.account_id,
                    g.folder.id,
                    g.uids,
                    &dest,
                    g.message_ids,
                ));
            }
        }
        if new.is_empty() {
            return Err(AppError::invalid("already in that folder"));
        }
        self.queue(new).await
    }

    /// `message.archive`: to the archive folder; none means the action is unsupported.
    pub async fn message_archive(&self, message_ids: &[MessageId]) -> Result<OpId, AppError> {
        let groups = self.groups(message_ids).await?;
        let mut new = Vec::new();
        for g in groups {
            let folders = self.store.list_folders(g.account_id).await?;
            let dest = archive_folder(&folders)
                .ok_or_else(|| AppError::new(ErrorCode::Unsupported, "no archive folder"))?;
            if dest.id != g.folder.id {
                new.push(ops::move_op(
                    g.account_id,
                    g.folder.id,
                    g.uids,
                    dest,
                    g.message_ids,
                ));
            }
        }
        if new.is_empty() {
            return Err(AppError::invalid("already archived"));
        }
        self.queue(new).await
    }

    /// `message.delete`: to Trash; permanent in Trash or Junk or without a Trash (the UI
    /// asked first). With `folder_id` instead of messages: Empty Trash, permanent.
    pub async fn message_delete(
        &self,
        message_ids: &[MessageId],
        folder_id: Option<FolderId>,
    ) -> Result<OpId, AppError> {
        if let Some(folder_id) = folder_id {
            let folder = self.store.get_folder(folder_id).await?;
            let uids = self.store.visible_uids(folder_id).await?;
            if uids.is_empty() {
                return Err(AppError::invalid("the folder is empty"));
            }
            return self
                .queue(vec![ops::expunge_op(folder.account_id, folder_id, uids)])
                .await;
        }
        let groups = self.groups(message_ids).await?;
        let mut new = Vec::new();
        for g in groups {
            let folders = self.store.list_folders(g.account_id).await?;
            match role_folder(&folders, SpecialUse::Trash) {
                Some(trash) if !delete_is_permanent(&folders, &g.folder) => {
                    new.push(ops::move_op(
                        g.account_id,
                        g.folder.id,
                        g.uids,
                        trash,
                        g.message_ids,
                    ));
                }
                _ => new.push(ops::expunge_op(g.account_id, g.folder.id, g.uids)),
            }
        }
        self.queue(new).await
    }

    /// `message.add_label` (Gmail, section 18.1): `UID COPY` of each message to the label's
    /// folder; Gmail keeps one `X-GM-MSGID`, so the message gains a label, not a copy.
    pub async fn message_add_label(
        &self,
        message_ids: &[MessageId],
        folder_id: FolderId,
    ) -> Result<OpId, AppError> {
        let label = self.store.get_folder(folder_id).await?;
        let mut new = Vec::new();
        for id in message_ids {
            let copies = self.store.copies(*id).await?;
            if copies.is_empty() {
                return Err(AppError::new(
                    ErrorCode::Unsupported,
                    "labels are a Gmail feature",
                ));
            }
            if copies.iter().any(|c| c.folder_id == label.id) {
                continue;
            }
            let m = self.store.get_message(*id).await?;
            if m.account_id != label.account_id {
                return Err(AppError::invalid("labels belong to the message's account"));
            }
            new.push(ops::label_add_op(
                m.account_id,
                m.folder_id,
                vec![m.uid],
                &label,
                vec![m.message_id],
            ));
        }
        if new.is_empty() {
            return Err(AppError::invalid("the label is already on"));
        }
        self.queue(new).await
    }

    /// `message.remove_label` (Gmail, section 18.1): in the label's folder, `\Deleted` and
    /// `UID EXPUNGE` of that UID only. The message stays under its other labels.
    pub async fn message_remove_label(
        &self,
        message_ids: &[MessageId],
        folder_id: FolderId,
    ) -> Result<OpId, AppError> {
        let label = self.store.get_folder(folder_id).await?;
        let mut new = Vec::new();
        for id in message_ids {
            let copies = self.store.copies(*id).await?;
            if copies.is_empty() {
                return Err(AppError::new(
                    ErrorCode::Unsupported,
                    "labels are a Gmail feature",
                ));
            }
            let Some(here) = copies.iter().find(|c| c.folder_id == label.id) else {
                continue;
            };
            let restore = copies
                .iter()
                .find(|c| c.folder_id != label.id)
                .map(|c| (c.folder_id, c.uid));
            if restore.is_none() {
                // The last label: removing it would delete the message.
                return Err(AppError::invalid("a message keeps at least one label"));
            }
            let m = self.store.get_message(here.id).await?;
            new.push(ops::label_remove_op(
                m.account_id,
                label.id,
                vec![m.uid],
                vec![m.message_id],
                restore,
                false,
            ));
        }
        if new.is_empty() {
            return Err(AppError::invalid("the label is not on"));
        }
        self.queue(new).await
    }

    /// `message.undo` (section 9): a queued op is cancelled and its overlay lifted; a
    /// finished move goes back with COPYUID, or by Message-ID after a resync; a flag change
    /// is reversed. A permanent delete cannot be undone.
    pub async fn message_undo(&self, op_id: OpId) -> Result<(), AppError> {
        // An op can start between the look and the cancel; then it is undone once it has
        // finished, as a done one.
        match self.undo_attempt(op_id).await {
            Err(e) if e.code == ErrorCode::Conflict => self.undo_attempt(op_id).await,
            other => other,
        }
    }

    async fn undo_attempt(&self, op_id: OpId) -> Result<(), AppError> {
        let group = match self.store.get_op(op_id).await {
            Ok(op) => op.group,
            // The op failed and was reverted, or it already ran and was purged.
            Err(mail_store::StoreError::NotFound) => {
                return Err(AppError::new(ErrorCode::NotFound, "nothing to undo"))
            }
            Err(e) => return Err(e.into()),
        };
        let settled = ops::settled_group(&self.store, group, UNDO_WAIT)
            .await
            .map_err(|e| crate::core::sync_error(&e))?;
        let mut reverse = Vec::new();
        for op in settled {
            if op.args["snooze"].as_bool() == Some(true) {
                // Undo of a snooze: the message is no longer snoozed, wherever it is now.
                for row in op.args["rows"].as_array().into_iter().flatten() {
                    if let Some(id) = row.as_i64() {
                        self.store.delete_snooze(id).await?;
                    }
                }
                if let Some(dest) = op.args["dest"].as_i64() {
                    for mid in strings(&op.args["message_ids"]) {
                        if let Some(copy) = self.store.find_by_message_id(dest, mid).await? {
                            self.store.delete_snooze(copy).await?;
                        }
                    }
                }
            }
            match op.state {
                OpState::Queued => {
                    if !self.store.cancel_op(op.id).await? {
                        return Err(AppError::new(
                            ErrorCode::Conflict,
                            "the action is being sent",
                        ));
                    }
                    (self.sink)(CoreEvent::StoreChanged {
                        account_id: op.account_id,
                        folder_id: Some(op.folder_id),
                    });
                }
                OpState::Running => {
                    return Err(AppError::new(
                        ErrorCode::Conflict,
                        "the action is being sent",
                    ))
                }
                OpState::Done => match op.kind.as_str() {
                    kind::MOVE => {
                        let source = self.store.get_folder(op.folder_id).await?;
                        let dest_id = op.args["dest"].as_i64().unwrap_or_default();
                        let dest = self.store.get_folder(dest_id).await?;
                        let message_ids: Vec<String> = op.args["message_ids"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let copied: Vec<u32> = op.result["copyuid"]["dest"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_u64)
                                    .filter_map(|u| u32::try_from(u).ok())
                                    .collect()
                            })
                            .unwrap_or_default();
                        reverse.push(if copied.is_empty() {
                            ops::move_back_by_message_id(
                                op.account_id,
                                dest.id,
                                &source,
                                message_ids,
                            )
                        } else {
                            ops::move_op(op.account_id, dest.id, copied, &source, message_ids)
                        });
                    }
                    kind::LABEL_ADD => {
                        let label = op.args["dest"].as_i64().unwrap_or_default();
                        let message_ids = strings(&op.args["message_ids"]);
                        let copied = uids(&op.result["copyuid"]["dest"]);
                        reverse.push(ops::label_remove_op(
                            op.account_id,
                            label,
                            copied.clone(),
                            message_ids,
                            Some((op.folder_id, op.uids.first().copied().unwrap_or_default())),
                            copied.is_empty(),
                        ));
                    }
                    kind::LABEL_REMOVE => {
                        let (Some(from), Some(uid)) = (
                            op.args["restore_folder"].as_i64(),
                            op.args["restore_uid"]
                                .as_u64()
                                .and_then(|u| u32::try_from(u).ok()),
                        ) else {
                            return Err(AppError::new(
                                ErrorCode::Unsupported,
                                "nothing to restore",
                            ));
                        };
                        let label = self.store.get_folder(op.folder_id).await?;
                        reverse.push(ops::label_add_op(
                            op.account_id,
                            from,
                            vec![uid],
                            &label,
                            strings(&op.args["message_ids"]),
                        ));
                    }
                    kind::FLAGS => {
                        let names = |key: &str| -> Vec<Flag> {
                            op.args[key]
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(Value::as_str)
                                        .map(Flag::parse)
                                        .collect()
                                })
                                .unwrap_or_default()
                        };
                        reverse.push(ops::flags_op(
                            op.account_id,
                            op.folder_id,
                            op.uids.clone(),
                            &names("remove"),
                            &names("add"),
                        ));
                    }
                    _ => {
                        return Err(AppError::new(
                            ErrorCode::Unsupported,
                            "a permanent delete cannot be undone",
                        ))
                    }
                },
            }
            let _ = self.store.delete_op(op.id).await;
        }
        if !reverse.is_empty() {
            self.queue(reverse).await?;
        } else {
            self.emit_pending().await;
        }
        Ok(())
    }

    async fn folder_by_path(&self, account_id: AccountId, path: &str) -> Result<Folder, AppError> {
        let id = self.store.find_folder(account_id, path).await?;
        Ok(self.store.get_folder(id).await?)
    }

    /// `folder.create`: the row appears at once; the server `CREATE` is queued.
    pub async fn folder_create(&self, account_id: AccountId, path: &str) -> Result<(), AppError> {
        let path = path.trim();
        if path.is_empty() {
            return Err(AppError::invalid("a folder needs a name"));
        }
        let folders = self.store.list_folders(account_id).await?;
        if folders.iter().any(|f| f.path == path) {
            return Err(AppError::new(ErrorCode::Conflict, "that folder exists"));
        }
        let delimiter = folders
            .iter()
            .map(|f| f.delimiter.clone())
            .find(|d| !d.is_empty())
            .unwrap_or_else(|| "/".into());
        self.queue_folder(
            account_id,
            FolderChange::Create(NewFolder {
                account_id,
                path: path.to_string(),
                delimiter,
                special_use: None,
                sync_enabled: true,
                watch_idle: false,
            }),
            kind::FOLDER_CREATE,
            json!({ "path": path }),
        )
        .await
    }

    /// `folder.rename`: INBOX cannot be renamed.
    pub async fn folder_rename(
        &self,
        account_id: AccountId,
        path: &str,
        to: &str,
    ) -> Result<(), AppError> {
        let to = to.trim();
        let folder = self.folder_by_path(account_id, path).await?;
        if folder.special_use == Some(SpecialUse::Inbox) {
            return Err(AppError::new(
                ErrorCode::Conflict,
                "INBOX cannot be renamed",
            ));
        }
        if to.is_empty() || to == path {
            return Err(AppError::invalid("a new name is needed"));
        }
        if self.store.find_folder(account_id, to).await.is_ok() {
            return Err(AppError::new(ErrorCode::Conflict, "that folder exists"));
        }
        self.queue_folder(
            account_id,
            FolderChange::Rename {
                folder_id: folder.id,
                to: to.to_string(),
            },
            kind::FOLDER_RENAME,
            json!({ "path": path, "to": to }),
        )
        .await
    }

    /// `folder.delete`: special-use folders cannot be deleted (section 9); the UI asked
    /// first. The local row goes at once (its op has no foreign key, G22).
    pub async fn folder_delete(&self, account_id: AccountId, path: &str) -> Result<(), AppError> {
        let folder = self.folder_by_path(account_id, path).await?;
        if folder.special_use.is_some() {
            return Err(AppError::new(
                ErrorCode::Conflict,
                "special folders cannot be deleted",
            ));
        }
        self.queue_folder(
            account_id,
            FolderChange::Delete {
                folder_id: folder.id,
            },
            kind::FOLDER_DELETE,
            json!({ "path": path }),
        )
        .await
    }

    /// `folder.set_role`: the user's override of a role (section 5, step 5).
    pub async fn folder_set_role(
        &self,
        account_id: AccountId,
        path: &str,
        role: Option<Role>,
    ) -> Result<(), AppError> {
        let folder = self.folder_by_path(account_id, path).await?;
        self.store
            .set_folder_role(folder.id, role.map(SpecialUse::from))
            .await?;
        (self.sink)(CoreEvent::StoreChanged {
            account_id,
            folder_id: None,
        });
        Ok(())
    }

    /// `folder.set_sync`: the folder's sync toggle (section 9). The actor reconnects with it.
    pub async fn folder_set_sync(
        &self,
        account_id: AccountId,
        path: &str,
        on: bool,
    ) -> Result<(), AppError> {
        let folder = self.folder_by_path(account_id, path).await?;
        self.store.set_folder_sync(folder.id, on).await?;
        self.folder_settings_changed(account_id).await;
        Ok(())
    }

    /// `folder.set_watch`: at most 3 watched folders besides INBOX (section 6).
    pub async fn folder_set_watch(
        &self,
        account_id: AccountId,
        path: &str,
        on: bool,
    ) -> Result<(), AppError> {
        let folder = self.folder_by_path(account_id, path).await?;
        if on {
            let watched = self
                .store
                .list_folders(account_id)
                .await?
                .iter()
                .filter(|f| {
                    f.watch_idle && f.id != folder.id && f.special_use != Some(SpecialUse::Inbox)
                })
                .count();
            if watched >= mail_sync::MAX_WATCHED {
                return Err(AppError::invalid("at most 3 folders can be watched"));
            }
        }
        self.store.set_folder_watch(folder.id, on).await?;
        self.folder_settings_changed(account_id).await;
        Ok(())
    }

    async fn folder_settings_changed(&self, account_id: AccountId) {
        if let Some(h) = self.actors.lock().await.get(&account_id) {
            h.account_changed();
        }
        (self.sink)(CoreEvent::StoreChanged {
            account_id,
            folder_id: None,
        });
    }

    /// `folder.extend_window`: Load older, 90 more days (sections 3 and 12; G51).
    pub async fn folder_extend_window(
        &self,
        account_id: AccountId,
        folder_id: FolderId,
    ) -> Result<(), AppError> {
        let folder = self.store.get_folder(folder_id).await?;
        if folder.account_id != account_id {
            return Err(AppError::invalid("folder of another account"));
        }
        match self.actors.lock().await.get(&account_id) {
            Some(h) => {
                h.extend_window(folder_id);
                Ok(())
            }
            None => {
                Err(AppError::new(ErrorCode::Network, "the account is offline")
                    .for_account(account_id))
            }
        }
    }
}
