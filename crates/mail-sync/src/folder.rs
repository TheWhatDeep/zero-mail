//! The folder list and the folder sync procedure (section 6, Folder order and steps 1–7).

use std::collections::HashSet;

use mail_imap::{
    assign_roles, Capabilities, HeaderFetch, ImapDate, ImapTransport, SearchKey, UidSet,
    FETCH_BATCH, MAX_UIDS_PER_COMMAND,
};
use mail_store::{AccountId, Folder, FolderId, FolderSyncState, NewFolder, SpecialUse, Store};

use crate::{convert, now, Events, SyncError, SyncEvent};

/// What a folder sync needs from the account.
pub(crate) struct Ctx<'a> {
    pub store: &'a Store,
    pub events: &'a Events,
    pub account_id: AccountId,
    pub sync_days: u32,
    pub caps: &'a Capabilities,
}

fn role(r: mail_imap::SpecialUse) -> SpecialUse {
    match r {
        mail_imap::SpecialUse::Inbox => SpecialUse::Inbox,
        mail_imap::SpecialUse::Sent => SpecialUse::Sent,
        mail_imap::SpecialUse::Drafts => SpecialUse::Drafts,
        mail_imap::SpecialUse::Trash => SpecialUse::Trash,
        mail_imap::SpecialUse::Junk => SpecialUse::Junk,
        mail_imap::SpecialUse::Archive => SpecialUse::Archive,
        mail_imap::SpecialUse::All => SpecialUse::All,
    }
}

/// `LIST`, roles (section 5, step 5), then the store: new folders added, vanished ones
/// purged (section 12). On Gmail, All Mail starts with sync off (section 9).
pub(crate) async fn sync_folder_list<T: ImapTransport>(
    t: &mut T,
    ctx: &Ctx<'_>,
) -> Result<Vec<Folder>, SyncError> {
    let mut listed = t.list().await?;
    assign_roles(&mut listed);
    let before = ctx.store.list_folders(ctx.account_id).await?;
    let rows: Vec<NewFolder> = listed
        .iter()
        .filter(|f| f.selectable())
        .map(|f| {
            let special_use = f.special_use.map(role);
            NewFolder {
                account_id: ctx.account_id,
                path: f.path.clone(),
                delimiter: f.delimiter.map(String::from).unwrap_or_default(),
                special_use,
                // Gmail: All Mail duplicates every message; Important and Starred are
                // virtual and hidden by default (section 9; SPEC_GAPS G61).
                sync_enabled: !(ctx.caps.gmail
                    && (special_use == Some(SpecialUse::All) || gmail_virtual(f))),
                watch_idle: false,
            }
        })
        .collect();
    // Unacknowledged folder ops win over this LIST; the store applies that rule in the
    // same transaction (section 6, Conflicts).
    let after = ctx.store.apply_folder_list(ctx.account_id, rows).await?;
    let paths = |v: &[Folder]| v.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
    if paths(&before) != paths(&after) {
        ctx.events.send(SyncEvent::StoreChanged {
            account_id: ctx.account_id,
            folder_id: None,
        });
    }
    Ok(after)
}

/// Gmail's `\Important` and `\Flagged` (Starred) folders.
fn gmail_virtual(f: &mail_imap::MailboxInfo) -> bool {
    f.attributes
        .iter()
        .any(|a| a.eq_ignore_ascii_case("\\Important") || a.eq_ignore_ascii_case("\\Flagged"))
}

/// First sync and after every reconnect: INBOX, then Drafts, Sent and Archive, then the
/// remaining enabled folders alphabetically (section 6, Folder order).
pub(crate) fn sync_order(folders: &[Folder]) -> Vec<FolderId> {
    let rank = |f: &Folder| match f.special_use {
        Some(SpecialUse::Inbox) => 0,
        Some(SpecialUse::Drafts) => 1,
        Some(SpecialUse::Sent) => 2,
        Some(SpecialUse::Archive) => 3,
        _ => 4,
    };
    let mut enabled: Vec<&Folder> = folders.iter().filter(|f| f.sync_enabled).collect();
    enabled.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });
    enabled.into_iter().map(|f| f.id).collect()
}

/// Section 6, steps 1–7, for one folder. Returns the folders its pushed ops touched, for
/// the caller to sync.
pub(crate) async fn sync_folder<T: ImapTransport>(
    t: &mut T,
    ctx: &Ctx<'_>,
    folder_id: FolderId,
) -> Result<Vec<FolderId>, SyncError> {
    let store = ctx.store;
    let mut folder = store.get_folder(folder_id).await?;

    // 1. SELECT.
    let info = t.select(&folder.path).await?;

    // 2. UIDVALIDITY. The first value is recorded before anything is cached, so a change
    // is always seen; a change purges the folder and makes this a first sync.
    if folder.uidvalidity == 0 {
        store.set_uidvalidity(folder_id, info.uidvalidity).await?;
    } else if folder.uidvalidity != info.uidvalidity {
        store.purge_folder(folder_id, info.uidvalidity).await?;
        ctx.events.send(SyncEvent::FolderReset {
            account_id: ctx.account_id,
            folder_id,
        });
        folder = store.get_folder(folder_id).await?;
    }

    // 3. Push first: this folder's pending ops.
    let follow =
        crate::ops::run_folder_ops(t, ctx.store, ctx.events, ctx.account_id, &folder).await?;

    // 4. New mail, newest first, 100 per batch, one transaction per batch.
    let cached = store.cached_uids(folder_id).await?;
    let first = folder.synced_at == 0;
    let candidates: Vec<u32> = if first {
        let since = now() - i64::from(ctx.sync_days) * 86_400;
        // The window starts here; Load older moves it back (SPEC_GAPS G75).
        store.set_window_start(folder_id, since, false).await?;
        t.uid_search(&SearchKey::Since(ImapDate::from_epoch(since)))
            .await?
    } else {
        let from = cached.last().map_or(1, |max| max.saturating_add(1));
        t.uid_search(&SearchKey::UidFrom(from))
            .await?
            .into_iter()
            .filter(|uid| *uid >= from)
            .collect()
    };
    let known: HashSet<u32> = cached.iter().copied().collect();
    let mut missing: Vec<u32> = candidates
        .into_iter()
        .filter(|uid| !known.contains(uid))
        .collect();
    missing.sort_unstable_by(|a, b| b.cmp(a));
    missing.dedup();
    let total = missing.len();
    let gmail = ctx.caps.gmail;
    let mut fetched = 0;
    for batch in missing.chunks(FETCH_BATCH) {
        let headers = t
            .uid_fetch_headers(&UidSet::new(batch.iter().copied()), HeaderFetch { gmail })
            .await?;
        let rows: Vec<_> = headers
            .iter()
            .map(|h| convert::new_message(h, ctx.account_id, folder_id, now(), gmail))
            .collect();
        if !rows.is_empty() {
            store.insert_synced(folder_id, rows).await?;
        }
        fetched += batch.len();
        ctx.events.send(SyncEvent::StoreChanged {
            account_id: ctx.account_id,
            folder_id: Some(folder_id),
        });
        if first {
            ctx.events.send(SyncEvent::SyncProgress {
                account_id: ctx.account_id,
                folder_id,
                fetched: u32::try_from(fetched).unwrap_or(u32::MAX),
                total: u32::try_from(total).unwrap_or(u32::MAX),
            });
        }
    }

    // 5 and 6. Flags and deletions for what was cached before this run. Rows just fetched
    // came with their flags.
    if !cached.is_empty() {
        let mut deleted_by_vanished = false;
        match (ctx.caps.condstore, folder.highestmodseq) {
            (true, Some(modseq)) => {
                let max = cached.last().copied().unwrap_or(1);
                let delta = t.uid_fetch_changes(max, modseq).await?;
                let updates: Vec<_> = delta
                    .updates
                    .into_iter()
                    .filter(|u| known.contains(&u.uid))
                    .collect();
                apply_flags(store, folder_id, &updates).await?;
                if ctx.caps.qresync {
                    let gone: Vec<u32> = delta
                        .vanished
                        .into_iter()
                        .filter(|uid| known.contains(uid))
                        .collect();
                    store.delete_messages(folder_id, gone).await?;
                    deleted_by_vanished = true;
                }
            }
            _ => {
                for chunk in chunked(&cached) {
                    let full = t.uid_fetch_flags(&chunk, None).await?;
                    apply_flags(store, folder_id, &full.updates).await?;
                }
            }
        }
        if !deleted_by_vanished {
            let on_server: HashSet<u32> =
                t.uid_search(&SearchKey::All).await?.into_iter().collect();
            let gone: Vec<u32> = cached
                .iter()
                .copied()
                .filter(|uid| !on_server.contains(uid))
                .collect();
            if !gone.is_empty() {
                store.delete_messages(folder_id, gone).await?;
            }
        }
    }

    // Pending ops win until acknowledged (section 6, Conflicts): their overlays go back on
    // top of what the server said.
    store.relay_ops(folder_id).await?;

    // 7. Counters and server state, then store-changed.
    store
        .finish_folder_sync(
            folder_id,
            FolderSyncState {
                uidvalidity: info.uidvalidity,
                uidnext: info.uidnext,
                highestmodseq: info.highest_modseq,
                synced_at: now(),
            },
        )
        .await?;
    ctx.events.send(SyncEvent::StoreChanged {
        account_id: ctx.account_id,
        folder_id: Some(folder_id),
    });
    Ok(follow
        .sync
        .into_iter()
        .filter(|f| *f != folder_id)
        .collect())
}

/// Load older (`folder.extend_window`, sections 3 and 12; SPEC_GAPS G51, G62): fetches the
/// headers of the 90 days before the folder's oldest cached message. When that range is
/// empty on the server, it takes the 90 days before the next older message instead, so a
/// quiet stretch never stops it.
pub(crate) async fn extend_window<T: ImapTransport>(
    t: &mut T,
    ctx: &Ctx<'_>,
    folder_id: FolderId,
) -> Result<(), SyncError> {
    let store = ctx.store;
    let folder = store.get_folder(folder_id).await?;
    let info = t.select(&folder.path).await?;
    if folder.uidvalidity != 0 && folder.uidvalidity != info.uidvalidity {
        // A changed mailbox: the regular sync purges and starts over.
        return sync_folder(t, ctx, folder_id).await.map(|_| ());
    }
    // The window start, not the oldest cached message: a server-search hit from years ago
    // is outside the window and must not make Load older skip the years between (G75).
    let start = match folder.window_start {
        0 => store
            .oldest_internal(folder_id)
            .await?
            .unwrap_or_else(|| now() - i64::from(ctx.sync_days) * 86_400),
        w => w,
    };
    let day = 86_400;
    let range = |end: i64| {
        SearchKey::And(vec![
            SearchKey::Since(ImapDate::from_epoch(end - crate::EXTEND_DAYS * day)),
            SearchKey::Before(ImapDate::from_epoch(end + day)),
        ])
    };
    let known: HashSet<u32> = store.cached_uids(folder_id).await?.into_iter().collect();
    let fresh = |uids: Vec<u32>| -> Vec<u32> {
        let mut v: Vec<u32> = uids.into_iter().filter(|u| !known.contains(u)).collect();
        v.sort_unstable_by(|a, b| b.cmp(a));
        v
    };
    let mut missing = fresh(t.uid_search(&range(start)).await?);
    let mut new_start = start - crate::EXTEND_DAYS * day;
    if missing.is_empty() {
        // Nothing in the 90 days: start from the newest older message on the server.
        let older = fresh(
            t.uid_search(&SearchKey::Before(ImapDate::from_epoch(start)))
                .await?,
        );
        if let Some(newest) = older.first() {
            let h = t
                .uid_fetch_headers(&UidSet::single(*newest), HeaderFetch::default())
                .await?;
            if let Some(h) = h.first() {
                missing = fresh(t.uid_search(&range(h.internal_date)).await?);
                new_start = h.internal_date - crate::EXTEND_DAYS * day;
            }
        }
    }
    let gmail = ctx.caps.gmail;
    let total = missing.len();
    let mut fetched = 0;
    for batch in missing.chunks(FETCH_BATCH) {
        let headers = t
            .uid_fetch_headers(&UidSet::new(batch.iter().copied()), HeaderFetch { gmail })
            .await?;
        let rows: Vec<_> = headers
            .iter()
            .map(|h| convert::new_message(h, ctx.account_id, folder_id, now(), gmail))
            .collect();
        if !rows.is_empty() {
            store.insert_synced(folder_id, rows).await?;
        }
        fetched += batch.len();
        ctx.events.send(SyncEvent::SyncProgress {
            account_id: ctx.account_id,
            folder_id,
            fetched: u32::try_from(fetched).unwrap_or(u32::MAX),
            total: u32::try_from(total).unwrap_or(u32::MAX),
        });
        ctx.events.send(SyncEvent::StoreChanged {
            account_id: ctx.account_id,
            folder_id: Some(folder_id),
        });
    }
    store.set_window_start(folder_id, new_start, true).await?;
    store
        .finish_folder_sync(
            folder_id,
            FolderSyncState {
                uidvalidity: info.uidvalidity,
                uidnext: info.uidnext,
                highestmodseq: folder.highestmodseq,
                synced_at: folder.synced_at,
            },
        )
        .await?;
    ctx.events.send(SyncEvent::StoreChanged {
        account_id: ctx.account_id,
        folder_id: Some(folder_id),
    });
    Ok(())
}

fn chunked(uids: &[u32]) -> Vec<UidSet> {
    uids.chunks(MAX_UIDS_PER_COMMAND)
        .map(|c| UidSet::new(c.iter().copied()))
        .collect()
}

async fn apply_flags(
    store: &Store,
    folder_id: FolderId,
    updates: &[mail_imap::FlagUpdate],
) -> Result<(), SyncError> {
    if updates.is_empty() {
        return Ok(());
    }
    let changes = updates
        .iter()
        .map(|u| (u.uid, convert::flags(&u.flags)))
        .collect();
    store.apply_flags(folder_id, changes).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: FolderId, path: &str, role: Option<SpecialUse>, enabled: bool) -> Folder {
        Folder {
            id,
            account_id: 1,
            path: path.into(),
            delimiter: "/".into(),
            special_use: role,
            uidvalidity: 0,
            uidnext: 0,
            highestmodseq: None,
            sync_enabled: enabled,
            watch_idle: false,
            unread: 0,
            total: 0,
            synced_at: 0,
            window_start: 0,
        }
    }

    #[test]
    fn inbox_then_drafts_sent_archive_then_alphabetical() {
        let folders = vec![
            folder(1, "zeta", None, true),
            folder(2, "Archive", Some(SpecialUse::Archive), true),
            folder(3, "Alpha", None, true),
            folder(4, "Sent", Some(SpecialUse::Sent), true),
            folder(5, "INBOX", Some(SpecialUse::Inbox), true),
            folder(6, "Drafts", Some(SpecialUse::Drafts), true),
            folder(7, "All Mail", Some(SpecialUse::All), false),
            folder(8, "beta", None, true),
            folder(9, "Trash", Some(SpecialUse::Trash), true),
        ];
        assert_eq!(sync_order(&folders), vec![5, 6, 4, 2, 3, 8, 9, 1]);
    }
}
