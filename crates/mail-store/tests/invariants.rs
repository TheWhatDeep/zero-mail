//! Section 4 invariants, the FTS index, and the writer and read pool.
#![cfg(test)]

mod common;

use common::*;
use mail_store::*;

fn attachment(part: &str) -> NewAttachment {
    NewAttachment {
        part_id: part.into(),
        filename: "a.pdf".into(),
        mime: "application/pdf".into(),
        size: 10,
        content_id: None,
        is_inline: false,
    }
}

fn body(text: &str) -> Body {
    Body {
        text_plain: text.into(),
        text_html: format!("<p>{text}</p>"),
        fetched_at: 1,
    }
}

/// A folder of `n` messages, each with a body and an attachment, plus a pending op.
async fn filled(
    store: &Store,
    account: AccountId,
    path: &str,
    thread: ThreadId,
    n: u32,
) -> FolderId {
    let folder_id = store.insert_folder(folder(account, path)).await.unwrap();
    let batch = (1..=n)
        .map(|uid| {
            let mut m = message(uid, i64::from(uid), thread);
            m.has_attach = true;
            m.attachments = vec![attachment("2")];
            m
        })
        .collect();
    let ids = store.insert_messages(folder_id, batch).await.unwrap();
    for id in ids {
        store
            .put_body(id, body("quarterly budget"), BodyState::Full)
            .await
            .unwrap();
    }
    folder_id
}

fn rows_in(conn: &rusqlite::Connection, folder_id: FolderId) -> [i64; 4] {
    let q = |sql: &str| conn.query_row(sql, [folder_id], |r| r.get(0)).unwrap();
    [
        q("SELECT count(*) FROM messages WHERE folder_id = ?1"),
        q("SELECT count(*) FROM bodies AS b JOIN messages AS m ON m.id = b.message_id WHERE m.folder_id = ?1"),
        q("SELECT count(*) FROM attachments AS a JOIN messages AS m ON m.id = a.message_id WHERE m.folder_id = ?1"),
        q("SELECT count(*) FROM pending_ops WHERE folder_id = ?1"),
    ]
}

fn add_pending_op(conn: &rusqlite::Connection, account: AccountId, folder_id: FolderId) {
    conn.execute(
        "INSERT INTO pending_ops (account_id, folder_id, uids_json, kind, args_json, state,
             attempts, last_error, created_at)
         VALUES (?1, ?2, '[1]', 'flags', '{}', 'queued', 0, '', 0)",
        [account, folder_id],
    )
    .unwrap();
}

#[tokio::test]
async fn uidvalidity_purge_empties_only_that_folder_in_one_transaction() {
    let dir = TempDir::new("purge");
    let store = Store::open(&dir.db()).unwrap();
    let (account, _, thread) = seeded(&store).await;
    let a = filled(&store, account, "A", thread, 20).await;
    let b = filled(&store, account, "B", thread, 5).await;
    let conn = raw(&dir.db());
    add_pending_op(&conn, account, a);
    add_pending_op(&conn, account, b);
    assert_eq!(rows_in(&conn, a), [20, 20, 20, 1]);

    store.purge_folder(a, 777).await.unwrap();

    assert_eq!(
        rows_in(&conn, a),
        [0, 0, 0, 0],
        "messages, bodies, attachments, ops of A"
    );
    assert_eq!(rows_in(&conn, b), [5, 5, 5, 1], "B untouched");
    let (uidvalidity, uidnext, modseq, synced): (i64, i64, Option<i64>, i64) = conn
        .query_row(
            "SELECT uidvalidity, uidnext, highestmodseq, synced_at FROM folders WHERE id = ?1",
            [a],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!((uidvalidity, uidnext, modseq, synced), (777, 0, None, 0));
    assert!(fts_consistent(&conn));

    // Repopulating reuses the UIDs without conflict.
    store
        .insert_messages(a, vec![message(1, 1, thread)])
        .await
        .unwrap();
}

#[tokio::test]
async fn failed_purge_leaves_everything_in_place() {
    let dir = TempDir::new("purge-atomic");
    let store = Store::open(&dir.db()).unwrap();
    let (account, _, thread) = seeded(&store).await;
    let a = filled(&store, account, "A", thread, 3).await;
    let conn = raw(&dir.db());
    add_pending_op(&conn, account, a);
    // Inject a failure into the purge's last step, after the messages are already deleted.
    conn.execute_batch(
        "CREATE TRIGGER fail_ops BEFORE DELETE ON pending_ops BEGIN SELECT RAISE(ABORT, 'injected'); END;",
    )
    .unwrap();

    assert!(store.purge_folder(a, 9).await.is_err());

    assert_eq!(rows_in(&conn, a), [3, 3, 3, 1], "nothing was purged");
    let uidvalidity: i64 = conn
        .query_row("SELECT uidvalidity FROM folders WHERE id = ?1", [a], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(uidvalidity, 0, "the new UIDVALIDITY was not stored");
    assert!(fts_consistent(&conn));
    assert!(matches!(
        store.purge_folder(a + 100, 9).await,
        Err(StoreError::NotFound)
    ));
}

#[tokio::test]
async fn deleting_a_folder_cascades() {
    let dir = TempDir::new("folder-cascade");
    let store = Store::open(&dir.db()).unwrap();
    let (account, _, thread) = seeded(&store).await;
    let a = filled(&store, account, "A", thread, 4).await;
    store.delete_folder(a).await.unwrap();
    let conn = raw(&dir.db());
    assert_eq!(rows_in(&conn, a)[..3], [0, 0, 0]);
    assert_eq!(count(&conn, "SELECT count(*) FROM bodies"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM attachments"), 0);
    assert!(fts_consistent(&conn));
}

#[tokio::test]
async fn deleting_an_account_removes_all_its_rows() {
    let dir = TempDir::new("account-cascade");
    let store = Store::open(&dir.db()).unwrap();
    let (account, _, thread) = seeded(&store).await;
    filled(&store, account, "A", thread, 4).await;
    let other = store
        .insert_account(common::account("other@example.test"))
        .await
        .unwrap();
    let other_thread = store.insert_thread(other, 0, String::new()).await.unwrap();
    let kept = filled(&store, other, "INBOX", other_thread, 2).await;

    store.delete_account(account).await.unwrap();

    let conn = raw(&dir.db());
    for table in ["folders", "threads", "messages"] {
        let left: i64 = conn
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE account_id = ?1"),
                [account],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "{table}");
    }
    assert_eq!(rows_in(&conn, kept)[..3], [2, 2, 2]);
    assert!(fts_consistent(&conn));
}

#[tokio::test]
async fn deleted_messages_are_hidden_from_lists_and_search() {
    let dir = TempDir::new("deleted");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, thread) = seeded(&store).await;
    let mut gone = message(2, 20, thread);
    gone.flags = Flags(Flags::DELETED | Flags::SEEN);
    let ids = store
        .insert_messages(
            inbox,
            vec![message(1, 10, thread), gone, message(3, 30, thread)],
        )
        .await
        .unwrap();

    let page = store.folder_page(inbox, None, 10).await.unwrap();
    let shown: Vec<MessageId> = page.rows.iter().map(|r| r.id).collect();
    assert_eq!(shown, vec![ids[2], ids[0]]);

    let found = store.search_page("alice".into(), None, 10).await.unwrap();
    assert!(found.rows.iter().all(|r| r.id != ids[1]));
    assert_eq!(found.rows.len(), 2);
}

#[tokio::test]
async fn duplicate_uid_in_a_folder_is_rejected_atomically() {
    let dir = TempDir::new("dup");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, thread) = seeded(&store).await;
    store
        .insert_messages(inbox, vec![message(1, 1, thread)])
        .await
        .unwrap();
    let batch = vec![message(2, 2, thread), message(1, 1, thread)];
    assert!(store.insert_messages(inbox, batch).await.is_err());
    let page = store.folder_page(inbox, None, 10).await.unwrap();
    assert_eq!(
        page.rows.len(),
        1,
        "the failed batch rolled back as a whole"
    );
}

#[tokio::test]
async fn delete_by_uid_removes_bodies_and_keeps_fts_consistent() {
    let dir = TempDir::new("expunge");
    let store = Store::open(&dir.db()).unwrap();
    let (account, _, thread) = seeded(&store).await;
    let a = filled(&store, account, "A", thread, 5).await;
    assert_eq!(store.delete_messages(a, vec![2, 4, 99]).await.unwrap(), 2);
    let conn = raw(&dir.db());
    assert_eq!(rows_in(&conn, a)[..3], [3, 3, 3]);
    assert!(fts_consistent(&conn));
}

#[tokio::test]
async fn fts_indexes_headers_and_bodies() {
    let dir = TempDir::new("fts");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, thread) = seeded(&store).await;
    let mut m = message(1, 1, thread);
    m.subject = "Réunion au café".into();
    m.cc = vec![Address {
        name: "Zoë Müller".into(),
        addr: "zoe@example.test".into(),
    }];
    let id = store.insert_messages(inbox, vec![m]).await.unwrap()[0];
    let hits = |q: &str| {
        let store = store.clone();
        let q = q.to_string();
        async move { store.search_page(q, None, 10).await.unwrap().rows.len() }
    };

    // Diacritics removed (section 10), on subject and on address names.
    assert_eq!(hits("cafe").await, 1);
    assert_eq!(hits("reunion").await, 1);
    assert_eq!(hits("recipients:muller").await, 1);
    assert_eq!(hits("recipients:zoe").await, 1);
    // Addresses are indexed too.
    assert_eq!(hits("recipients:\"zoe@example.test\"").await, 1);
    // Prefix matching on the last token.
    assert_eq!(hits("reun*").await, 1);
    // No body yet.
    assert_eq!(hits("budget").await, 0);

    store
        .put_body(id, body("the budget draft"), BodyState::Full)
        .await
        .unwrap();
    assert_eq!(hits("body:budget").await, 1);
    store
        .put_body(id, body("revised numbers"), BodyState::Full)
        .await
        .unwrap();
    assert_eq!(hits("budget").await, 0, "old body text left the index");
    assert_eq!(hits("revised").await, 1);
    assert!(fts_consistent(&raw(&dir.db())));

    // Clearing the body cache (invariant 7) keeps the header entry.
    raw(&dir.db())
        .execute("DELETE FROM bodies WHERE message_id = ?1", [id])
        .unwrap();
    assert_eq!(hits("revised").await, 0);
    assert_eq!(hits("cafe").await, 1);
    assert!(fts_consistent(&raw(&dir.db())));
}

#[tokio::test]
async fn header_updates_reindex() {
    let dir = TempDir::new("fts-update");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, thread) = seeded(&store).await;
    store
        .insert_messages(inbox, vec![message(1, 1, thread)])
        .await
        .unwrap();
    let conn = raw(&dir.db());
    conn.execute("UPDATE messages SET subject = 'Invoice 42'", [])
        .unwrap();
    conn.execute("UPDATE messages SET flags = 1", []).unwrap();
    assert!(fts_consistent(&conn));
    let found = store.search_page("invoice".into(), None, 10).await.unwrap();
    assert_eq!(found.rows.len(), 1);
}

#[tokio::test]
async fn list_rows_carry_their_columns() {
    let dir = TempDir::new("row");
    let store = Store::open(&dir.db()).unwrap();
    let (account, inbox, thread) = seeded(&store).await;
    let mut m = message(7, 1_760_000_000, thread);
    m.flags = Flags(Flags::SEEN | Flags::FLAGGED);
    m.has_attach = true;
    m.preview = "Hello".into();
    let expected_to = m.to.clone();
    let id = store.insert_messages(inbox, vec![m]).await.unwrap()[0];
    let page = store.folder_page(inbox, None, 1).await.unwrap();
    assert_eq!(page.next, None);
    let row = &page.rows[0];
    assert_eq!(
        (row.id, row.account_id, row.folder_id, row.thread_id),
        (id, account, inbox, thread)
    );
    assert_eq!(row.subject, "Subject 7");
    assert_eq!(row.from.addr, "alice@example.test");
    assert_eq!(row.to, expected_to);
    assert_eq!(row.date_utc, 1_760_000_000);
    assert!(row.flags.contains(Flags::FLAGGED) && !row.flags.contains(Flags::DELETED));
    assert!(row.has_attach);
    assert_eq!(row.preview, "Hello");
}

#[tokio::test]
async fn zero_limit_is_rejected() {
    let dir = TempDir::new("limit");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, _) = seeded(&store).await;
    assert!(matches!(
        store.folder_page(inbox, None, 0).await,
        Err(StoreError::Invalid(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_run_concurrently_with_the_writer() {
    let dir = TempDir::new("concurrency");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, thread) = seeded(&store).await;

    let writer = {
        let store = store.clone();
        tokio::spawn(async move {
            for batch in 0..20u32 {
                let msgs = (1..=100)
                    .map(|i| message(batch * 100 + i, i64::from(i), thread))
                    .collect();
                store.insert_messages(inbox, msgs).await.unwrap();
            }
        })
    };
    let mut readers = Vec::new();
    for _ in 0..16 {
        let store = store.clone();
        readers.push(tokio::spawn(async move {
            for _ in 0..10 {
                let page = store.folder_page(inbox, None, 50).await.unwrap();
                // Each read sees whole batches only: a batch is one transaction.
                assert!(page.rows.len() == 50 || page.rows.len() % 100 == 0);
            }
        }));
    }
    writer.await.unwrap();
    for r in readers {
        r.await.unwrap();
    }
    let all = store.folder_page(inbox, None, 5000).await.unwrap();
    assert_eq!(all.rows.len(), 2000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_reads_return_their_connection() {
    let dir = TempDir::new("cancel");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, thread) = seeded(&store).await;
    store
        .insert_messages(inbox, vec![message(1, 1, thread)])
        .await
        .unwrap();

    // Start and drop many reads (search typing cancels queries, section 10).
    for _ in 0..50 {
        let read = store.folder_page(inbox, None, 10);
        let _ = tokio::time::timeout(std::time::Duration::from_micros(1), read).await;
    }
    // Every pooled connection is still usable.
    let mut all = Vec::new();
    for _ in 0..READERS * 3 {
        let store = store.clone();
        all.push(tokio::spawn(async move {
            store.folder_page(inbox, None, 10).await
        }));
    }
    for r in all {
        assert_eq!(r.await.unwrap().unwrap().rows.len(), 1);
    }
}

#[tokio::test]
async fn writes_after_reopen_see_previous_commits() {
    let dir = TempDir::new("durable");
    {
        let store = Store::open(&dir.db()).unwrap();
        let (_, inbox, thread) = seeded(&store).await;
        store
            .insert_messages(inbox, vec![message(1, 1, thread)])
            .await
            .unwrap();
    }
    let conn = raw(&dir.db());
    assert_eq!(count(&conn, "SELECT count(*) FROM messages"), 1);
    let sync: i64 = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
        .map(|s| i64::from(s == "ok"))
        .unwrap();
    assert_eq!(sync, 1);
}
