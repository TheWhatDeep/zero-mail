//! Queries the sync engine uses (M3): folder lists, first UIDVALIDITY, threaded inserts
//! (SPEC_GAPS G36), flags, folder state, the prefetch cap and bodies with previews.
#![cfg(test)]

mod common;

use common::TempDir;
use mail_store::*;

fn listed(account_id: AccountId, path: &str) -> NewFolder {
    common::folder(account_id, path)
}

async fn account(store: &Store) -> AccountId {
    store
        .insert_account(common::account("me@example.test"))
        .await
        .unwrap()
}

#[tokio::test]
async fn folder_list_adds_new_paths_purges_vanished_ones_and_keeps_known_settings() {
    let dir = TempDir::new("sync-folders");
    let store = Store::open(&dir.db()).unwrap();
    let a = account(&store).await;
    let first = store
        .apply_folder_list(
            a,
            vec![listed(a, "INBOX"), listed(a, "Old"), listed(a, "Work")],
        )
        .await
        .unwrap();
    assert_eq!(
        first.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        vec!["INBOX", "Old", "Work"]
    );
    assert!(first.iter().all(|f| f.uidvalidity == 0 && f.synced_at == 0));
    let old = first[1].id;
    let thread = store.insert_thread(a, 0, String::new()).await.unwrap();
    store
        .insert_messages(old, vec![common::message(1, 10, thread)])
        .await
        .unwrap();

    // "Old" vanished from LIST and "New" appeared. Work's listed role changes are ignored.
    let mut work = listed(a, "Work");
    work.special_use = Some(SpecialUse::Archive);
    let second = store
        .apply_folder_list(a, vec![listed(a, "INBOX"), work, listed(a, "New")])
        .await
        .unwrap();
    assert_eq!(
        second.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        vec!["INBOX", "New", "Work"]
    );
    assert_eq!(second[2].special_use, None);
    assert!(matches!(
        store.get_folder(old).await,
        Err(StoreError::NotFound)
    ));
    let conn = common::raw(&dir.db());
    assert_eq!(common::count(&conn, "SELECT count(*) FROM messages"), 0);
    assert_eq!(store.list_folders(a).await.unwrap(), second);
}

#[tokio::test]
async fn uidvalidity_is_recorded_once() {
    let dir = TempDir::new("sync-uidvalidity");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, _) = common::seeded(&store).await;
    store.set_uidvalidity(inbox, 7).await.unwrap();
    store.set_uidvalidity(inbox, 9).await.unwrap();
    assert_eq!(store.get_folder(inbox).await.unwrap().uidvalidity, 7);
    assert!(matches!(
        store.set_uidvalidity(999, 1).await,
        Err(StoreError::NotFound)
    ));
}

#[tokio::test]
async fn gmail_messages_share_a_thread_by_thrid_and_others_get_their_own() {
    let dir = TempDir::new("sync-threads");
    let store = Store::open(&dir.db()).unwrap();
    let (a, inbox, _) = common::seeded(&store).await;
    let all = store.insert_folder(common::folder(a, "All")).await.unwrap();
    let gm = |uid, date, thrid| NewMessage {
        gm_thrid: Some(thrid),
        ..common::message(uid, date, 0)
    };
    store
        .insert_synced(
            inbox,
            vec![
                gm(1, 100, 55),
                common::message(2, 200, 0),
                common::message(3, 300, 0),
            ],
        )
        .await
        .unwrap();
    store
        .insert_synced(all, vec![gm(1, 400, 55)])
        .await
        .unwrap();
    let conn = common::raw(&dir.db());
    let threads: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT thread_id FROM messages ORDER BY folder_id, uid")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(threads[0], threads[3], "same X-GM-THRID, same thread");
    assert_ne!(threads[1], threads[2], "one thread per non-Gmail message");
    assert_ne!(threads[0], threads[1]);
    let last: i64 = conn
        .query_row(
            "SELECT last_date_utc FROM threads WHERE id = ?1",
            [threads[0]],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(last, 400);
    // The seeded thread plus three new ones.
    assert_eq!(common::count(&conn, "SELECT count(*) FROM threads"), 4);
    assert!(store
        .insert_synced(inbox, vec![common::message(2, 1, 0)])
        .await
        .is_err());
    assert_eq!(store.cached_uids(inbox).await.unwrap(), vec![1, 2, 3]);
}

#[tokio::test]
async fn flags_state_and_counters() {
    let dir = TempDir::new("sync-flags");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, _) = common::seeded(&store).await;
    store
        .insert_synced(
            inbox,
            (1..=4)
                .map(|u| common::message(u, i64::from(u), 0))
                .collect(),
        )
        .await
        .unwrap();
    let seen = Flags(Flags::SEEN);
    let changed = store
        .apply_flags(
            inbox,
            vec![(1, seen), (2, seen), (3, Flags(Flags::DELETED)), (9, seen)],
        )
        .await
        .unwrap();
    assert_eq!(changed, 3, "unknown UIDs are skipped");
    assert_eq!(store.apply_flags(inbox, vec![(1, seen)]).await.unwrap(), 0);
    let state = FolderSyncState {
        uidvalidity: 5,
        uidnext: 10,
        highestmodseq: Some(77),
        synced_at: 1_000,
    };
    store.finish_folder_sync(inbox, state).await.unwrap();
    let f = store.get_folder(inbox).await.unwrap();
    assert_eq!(
        (f.uidvalidity, f.uidnext, f.highestmodseq, f.synced_at),
        (5, 10, Some(77), 1_000)
    );
    assert_eq!(
        (f.total, f.unread),
        (3, 1),
        "\\Deleted rows are not counted"
    );
}

#[tokio::test]
async fn prefetch_is_capped_to_the_newest_and_bodies_set_the_preview() {
    let dir = TempDir::new("sync-prefetch");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, _) = common::seeded(&store).await;
    let ids = store
        .insert_synced(
            inbox,
            (1..=10)
                .map(|u| common::message(u, i64::from(u) * 10, 0))
                .collect(),
        )
        .await
        .unwrap();
    let newest3: Vec<u32> = store
        .prefetch_candidates(inbox, 3)
        .await
        .unwrap()
        .iter()
        .map(|p| p.uid)
        .collect();
    assert_eq!(newest3, vec![10, 9, 8]);
    let body = Body {
        text_plain: "Hello".into(),
        text_html: String::new(),
        fetched_at: 1,
    };
    store
        .store_body(ids[9], body, BodyState::Full, "Hello".into())
        .await
        .unwrap();
    let left: Vec<u32> = store
        .prefetch_candidates(inbox, 3)
        .await
        .unwrap()
        .iter()
        .map(|p| p.uid)
        .collect();
    assert_eq!(left, vec![9, 8], "the cap counts fetched messages too");
    let page = store.folder_page(inbox, None, 1).await.unwrap();
    assert_eq!(page.rows[0].preview, "Hello");
    store.integrity_check().await.unwrap();
}
