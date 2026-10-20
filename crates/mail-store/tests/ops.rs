//! The pending-op queue (M6, section 9; SPEC_GAPS G59, G62): overlays laid at queue time,
//! laid again after a server pull, lifted on cancel and on failure from the flags saved at
//! queue time; Undo groups; the pending count; and folder ops that win over LIST.
#![cfg(test)]

mod common;

use common::TempDir;
use mail_store::*;
use serde_json::json;

fn flags_of(store_flags: &[(u32, Flags)], uid: u32) -> u8 {
    store_flags
        .iter()
        .find(|(u, _)| *u == uid)
        .map_or(0, |(_, f)| f.0)
}

async fn flags(store: &Store, folder: FolderId) -> Vec<(u32, Flags)> {
    let page = store.folder_page(folder, None, 100).await.unwrap();
    let mut out = Vec::new();
    for row in page.rows {
        let m = store.get_message(row.id).await.unwrap();
        out.push((m.uid, m.flags));
    }
    out
}

fn op(account_id: AccountId, folder_id: FolderId, uids: Vec<u32>, overlay: Overlay) -> NewOp {
    NewOp {
        account_id,
        folder_id,
        uids,
        kind: "test".into(),
        args: json!({ "x": 1 }),
        overlay,
    }
}

#[tokio::test]
async fn overlays_are_laid_relaid_and_lifted() {
    let dir = TempDir::new("ops-overlay");
    let store = Store::open(&dir.db()).unwrap();
    let (a, folder, thread) = common::seeded(&store).await;
    let mut seen = common::message(7, 100, thread);
    seen.flags = Flags(Flags::SEEN);
    store
        .insert_messages(folder, vec![common::message(1, 10, thread), seen])
        .await
        .unwrap();

    let star = Overlay {
        add: Flags::FLAGGED,
        remove: 0,
    };
    let ids = store
        .queue_ops(
            vec![
                op(a, folder, vec![1, 7], star),
                op(a, folder, vec![7], Overlay::HIDE),
            ],
            1000,
        )
        .await
        .unwrap();
    let f = flags(&store, folder).await;
    assert_eq!(flags_of(&f, 1), Flags::FLAGGED);
    assert!(
        f.iter().all(|(u, _)| *u != 7),
        "a pending move hides the row"
    );
    assert_eq!(store.pending_count().await.unwrap(), 2);
    let group = store.group_ops(ids[0]).await.unwrap();
    assert_eq!(group.len(), 2, "one Undo group");
    assert_eq!(group[1].group, ids[0]);
    assert_eq!(group[0].args["x"], 1);

    // A server pull replaces flags; the overlays go back on top.
    store
        .apply_flags(folder, vec![(1, Flags(0)), (7, Flags(Flags::SEEN))])
        .await
        .unwrap();
    store.relay_ops(folder).await.unwrap();
    let f = flags(&store, folder).await;
    assert_eq!(flags_of(&f, 1), Flags::FLAGGED);
    assert!(f.iter().all(|(u, _)| *u != 7));

    // Cancel (queued): only the overlay's bits return to what they were.
    assert!(store.cancel_op(ids[1]).await.unwrap());
    let f = flags(&store, folder).await;
    assert_eq!(
        flags_of(&f, 7),
        Flags::SEEN | Flags::FLAGGED,
        "seen kept, star still pending"
    );
    // A running op cannot be cancelled; a failed one is reverted.
    store
        .set_op_state(ids[0], OpState::Running, None, None)
        .await
        .unwrap();
    assert!(!store.cancel_op(ids[0]).await.unwrap());
    let failed = store.fail_op(ids[0]).await.unwrap();
    assert_eq!(failed.uids, vec![1, 7]);
    let f = flags(&store, folder).await;
    assert_eq!(flags_of(&f, 1), 0);
    assert_eq!(flags_of(&f, 7), Flags::SEEN);
    assert_eq!(store.pending_count().await.unwrap(), 0);
}

#[tokio::test]
async fn done_ops_keep_their_result_for_undo_then_go() {
    let dir = TempDir::new("ops-done");
    let store = Store::open(&dir.db()).unwrap();
    let (a, folder, thread) = common::seeded(&store).await;
    store
        .insert_messages(folder, vec![common::message(1, 10, thread)])
        .await
        .unwrap();
    let id = store
        .queue_ops(vec![op(a, folder, vec![1], Overlay::HIDE)], 1000)
        .await
        .unwrap()[0];
    store
        .set_op_state(
            id,
            OpState::Done,
            Some(json!({ "copyuid": { "dest": [9] } })),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        store.pending_count().await.unwrap(),
        0,
        "done is not pending"
    );
    let done = store.get_op(id).await.unwrap();
    assert_eq!(done.result["copyuid"]["dest"][0], 9);
    assert!(store.account_ops(a).await.unwrap().is_empty());
    assert_eq!(store.purge_done_ops(1001).await.unwrap(), 1);
    assert!(matches!(store.get_op(id).await, Err(StoreError::NotFound)));
}

#[tokio::test]
async fn pending_folder_ops_win_over_list() {
    let dir = TempDir::new("ops-folders");
    let store = Store::open(&dir.db()).unwrap();
    let a = store
        .insert_account(common::account("me@example.test"))
        .await
        .unwrap();
    let listed = |p: &str| common::folder(a, p);
    store
        .apply_folder_list(a, vec![listed("INBOX"), listed("Old"), listed("Gone")])
        .await
        .unwrap();
    let old = store.find_folder(a, "Old").await.unwrap();
    let gone = store.find_folder(a, "Gone").await.unwrap();
    let folder_op = |kind: &str, args: serde_json::Value| NewOp {
        account_id: a,
        folder_id: 0,
        uids: Vec::new(),
        kind: kind.into(),
        args,
        overlay: Overlay::default(),
    };
    store
        .queue_folder_op(
            FolderChange::Create(listed("New")),
            folder_op("folder_create", json!({ "path": "New" })),
            1,
        )
        .await
        .unwrap();
    store
        .queue_folder_op(
            FolderChange::Rename {
                folder_id: old,
                to: "Renamed".into(),
            },
            folder_op("folder_rename", json!({ "path": "Old", "to": "Renamed" })),
            1,
        )
        .await
        .unwrap();
    store
        .queue_folder_op(
            FolderChange::Delete { folder_id: gone },
            folder_op("folder_delete", json!({ "path": "Gone" })),
            1,
        )
        .await
        .unwrap();
    // The server has not seen any of it yet.
    let after = store
        .apply_folder_list(a, vec![listed("INBOX"), listed("Old"), listed("Gone")])
        .await
        .unwrap();
    let paths: Vec<&str> = after.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["INBOX", "New", "Renamed"]);
    assert_eq!(
        after.iter().find(|f| f.path == "Renamed").unwrap().id,
        old,
        "same row"
    );
}
