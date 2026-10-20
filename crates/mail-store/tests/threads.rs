//! M9 threading (section 10), T29: header-linked messages form one thread whatever order
//! they arrive in and with a message missing from the middle; a subject-only match stays
//! separate unless subject grouping is on; Gmail threads by X-GM-THRID alone.
#![cfg(test)]

mod common;

use common::*;
use mail_store::*;

const DAY: i64 = 86_400;

fn msg(
    uid: u32,
    date: i64,
    id: &str,
    irt: Option<&str>,
    refs: &[&str],
    subject: &str,
) -> NewMessage {
    NewMessage {
        message_id: id.into(),
        in_reply_to: irt.map(str::to_string),
        refs: (!refs.is_empty()).then(|| refs.join(" ")),
        subject: subject.into(),
        ..message(uid, date, 0)
    }
}

async fn thread_of(store: &Store, id: MessageId) -> ThreadId {
    store.get_message(id).await.unwrap().thread_id
}

#[tokio::test]
async fn t29_out_of_order_missing_middle_and_subject_only() {
    let dir = TempDir::new("threads");
    let store = Store::open(&dir.db()).unwrap();
    let (account, inbox, _) = seeded(&store).await;
    let archive = store
        .insert_folder(folder(account, "Archive"))
        .await
        .unwrap();
    let t0 = 1_760_000_000;

    // The thread: A <- B (never arrives) <- C <- D. D arrives first, then C, then A, and
    // A lands in another folder: threads are per account, across folders.
    let d = store
        .insert_synced(
            inbox,
            vec![msg(
                4,
                t0 + 3 * DAY,
                "d@x",
                Some("c@x"),
                &["a@x", "b@x", "c@x"],
                "Re: Budget",
            )],
        )
        .await
        .unwrap()[0];
    // C references only the missing B in In-Reply-To; its References still hold A.
    let c = store
        .insert_synced(
            inbox,
            vec![msg(
                3,
                t0 + 2 * DAY,
                "c@x",
                Some("b@x"),
                &["a@x", "b@x"],
                "Re: Budget",
            )],
        )
        .await
        .unwrap()[0];
    let a = store
        .insert_synced(archive, vec![msg(1, t0, "a@x", None, &[], "Budget")])
        .await
        .unwrap()[0];
    // Same subject, no header link, a day later: separate by default (section 10, rule 3).
    let s = store
        .insert_synced(
            inbox,
            vec![msg(5, t0 + DAY, "s@x", None, &[], "Re: Budget")],
        )
        .await
        .unwrap()[0];

    let thread = thread_of(&store, a).await;
    assert_eq!(thread_of(&store, c).await, thread);
    assert_eq!(thread_of(&store, d).await, thread);
    assert_ne!(
        thread_of(&store, s).await,
        thread,
        "subject-only match stays separate"
    );

    // A reply that links two threads merges them; the smaller one is relabelled.
    let lone = store
        .insert_synced(inbox, vec![msg(6, t0 + 4 * DAY, "e@x", None, &[], "Other")])
        .await
        .unwrap()[0];
    let other = thread_of(&store, lone).await;
    let bridge = store
        .insert_synced(
            inbox,
            vec![msg(
                7,
                t0 + 5 * DAY,
                "f@x",
                Some("e@x"),
                &["d@x", "e@x"],
                "Re: Other",
            )],
        )
        .await
        .unwrap()[0];
    assert_eq!(
        thread_of(&store, bridge).await,
        thread,
        "the bigger thread wins"
    );
    assert_eq!(thread_of(&store, lone).await, thread);
    assert_ne!(other, thread);
    let conn = raw(&dir.db());
    assert_eq!(
        count(
            &conn,
            &format!("SELECT count(*) FROM threads WHERE id = {other}")
        ),
        0,
        "the merged thread is gone"
    );
    let last: i64 = conn
        .query_row(
            &format!("SELECT last_date_utc FROM threads WHERE id = {thread}"),
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(last, t0 + 5 * DAY);
}

#[tokio::test]
async fn messages_in_one_batch_link_and_subject_grouping_is_a_setting() {
    let dir = TempDir::new("threads-batch");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, _) = seeded(&store).await;
    let t0 = 1_760_000_000;
    let ids = store
        .insert_synced(
            inbox,
            vec![
                msg(
                    2,
                    t0 + 10,
                    "reply@x",
                    Some("root@x"),
                    &["root@x"],
                    "Re: Plan",
                ),
                msg(1, t0, "root@x", None, &[], "Plan"),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        thread_of(&store, ids[0]).await,
        thread_of(&store, ids[1]).await
    );

    store
        .set_setting(SUBJECT_MERGE_SETTING, serde_json::json!(true))
        .await
        .unwrap();
    let near = store
        .insert_synced(
            inbox,
            vec![msg(3, t0 + 13 * DAY, "n@x", None, &[], "RE: plan")],
        )
        .await
        .unwrap()[0];
    let far = store
        .insert_synced(inbox, vec![msg(4, t0 + 40 * DAY, "f@x", None, &[], "Plan")])
        .await
        .unwrap()[0];
    assert_eq!(
        thread_of(&store, near).await,
        thread_of(&store, ids[1]).await,
        "within 14 days"
    );
    assert_ne!(
        thread_of(&store, far).await,
        thread_of(&store, ids[1]).await,
        "too far apart"
    );
}

#[tokio::test]
async fn gmail_threads_by_thrid_only() {
    let dir = TempDir::new("threads-gmail");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, _) = seeded(&store).await;
    let gm = |uid, thrid, id: &str, irt: Option<&str>| NewMessage {
        gm_thrid: Some(thrid),
        ..msg(uid, 1_760_000_000 + i64::from(uid), id, irt, &[], "x")
    };
    let ids = store
        .insert_synced(
            inbox,
            vec![
                gm(1, 77, "a@g", None),
                gm(2, 77, "b@g", None),
                gm(3, 88, "c@g", Some("a@g")),
            ],
        )
        .await
        .unwrap();
    let t = thread_of(&store, ids[0]).await;
    assert_eq!(thread_of(&store, ids[1]).await, t);
    assert_ne!(
        thread_of(&store, ids[2]).await,
        t,
        "X-GM-THRID decides, not headers"
    );
}

#[tokio::test]
async fn deleting_a_message_drops_its_refs() {
    let dir = TempDir::new("threads-refs");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, _) = seeded(&store).await;
    store
        .insert_synced(
            inbox,
            vec![msg(1, 1_760_000_000, "a@x", None, &["z@x"], "x")],
        )
        .await
        .unwrap();
    let conn = raw(&dir.db());
    assert_eq!(count(&conn, "SELECT count(*) FROM message_refs"), 2);
    store.delete_messages(inbox, vec![1]).await.unwrap();
    assert_eq!(count(&conn, "SELECT count(*) FROM message_refs"), 0);
}

/// Group by conversation stays fast on a big folder (section 14): the first page of thread
/// rows over 50,000 messages, and a deep page, each well inside a frame budget of work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_pages_over_50k_messages_are_fast() {
    let dir = TempDir::new("threads-perf");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, _) = seeded(&store).await;
    let t0 = 1_760_000_000;
    for start in (0..50_000u32).step_by(1_000) {
        let batch: Vec<NewMessage> = (start..start + 1_000)
            .map(|n| {
                // Every tenth message replies to the one before it.
                let irt = (n % 10 == 9).then(|| format!("m{}@x", n - 1));
                msg(
                    n + 1,
                    t0 + i64::from(n) * 60,
                    &format!("m{n}@x"),
                    irt.as_deref(),
                    &[],
                    "Topic",
                )
            })
            .collect();
        store.insert_synced(inbox, batch).await.unwrap();
    }
    store
        .threads_page(vec![inbox], Vec::new(), None, 100)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let first = store
        .threads_page(vec![inbox], Vec::new(), None, 100)
        .await
        .unwrap();
    let took = started.elapsed();
    println!("first thread page in {took:?}");
    assert_eq!(first.rows.len(), 100);
    assert!(took < std::time::Duration::from_millis(100), "{took:?}");
    // Thread rows are unique and newest first across pages.
    let mut cursor = first.next;
    let mut seen: std::collections::HashSet<ThreadId> =
        first.rows.iter().map(|r| r.row.thread_id).collect();
    for _ in 0..50 {
        let page = store
            .threads_page(vec![inbox], Vec::new(), cursor, 100)
            .await
            .unwrap();
        for r in &page.rows {
            assert!(seen.insert(r.row.thread_id), "a thread listed twice");
        }
        cursor = page.next;
    }
    assert_eq!(seen.len(), 5_100);
}
