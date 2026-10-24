//! Sync engine acceptance tests on the fake transport (BUILD.md section 15: the fake powers
//! unit tests of the sync procedure). Real time, multi-threaded runtime.
#![cfg(test)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use mail_imap::fake::{FakeServer, Op};
use mail_imap::{Flag, ImapError, SpecialUse};
use mail_store::Flags;
use mail_sync::{AccountState, SyncError, SyncEvent};

const USER: &str = "me@test.local";
const PASS: &str = "pw";

fn server(caps: &[&str]) -> FakeServer {
    let s = FakeServer::new(caps);
    s.add_user(USER, PASS);
    s
}

fn logins() -> Logins {
    let l = Logins::default();
    l.set(USER, PASS);
    l
}

/// Folders in the order they were first selected, on any connection.
fn selects(s: &FakeServer) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in s.calls() {
        if let (Op::Select, Some(f)) = (c.op, c.folder) {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t01_first_sync_takes_the_window_inbox_first_and_caps_prefetch() {
    let dir = TempDir::new("t01");
    let store = dir.store();
    let account = add_account(&store, USER).await;
    let s = server(&["IMAP4rev1", "IDLE", "UIDPLUS", "SPECIAL-USE"]);
    s.add_folder("Archive", Some(SpecialUse::Archive));
    s.add_folder("Sent", Some(SpecialUse::Sent));
    s.add_folder("Drafts", Some(SpecialUse::Drafts));
    s.add_folder("Projects", None);
    let now = now();
    // 300 INBOX messages: 250 within the last 90 days (newest has the highest UID), 50 older.
    for n in 1..=300u32 {
        let age = if n <= 50 {
            200 * DAY
        } else {
            i64::from(300 - n) * 3600
        };
        s.deliver("INBOX", msg(n, now - age)).unwrap();
    }
    s.deliver("Archive", msg(1000, now - DAY)).unwrap();

    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);

    watch.state(account, AccountState::Idle).await;
    let inbox = folder_id(&store, account, "INBOX").await;

    // Usable before sync ends: INBOX rows were committed and announced batch by batch,
    // and INBOX was finished before any other folder was touched.
    let progress: Vec<(u32, u32)> = watch
        .seen
        .iter()
        .filter_map(|e| match e {
            SyncEvent::SyncProgress {
                folder_id,
                fetched,
                total,
                ..
            } if *folder_id == inbox => Some((*fetched, *total)),
            _ => None,
        })
        .collect();
    assert_eq!(progress, vec![(100, 250), (200, 250), (250, 250)]);
    let first_change = watch
        .seen
        .iter()
        .position(
            |e| matches!(e, SyncEvent::StoreChanged { folder_id: Some(f), .. } if *f == inbox),
        )
        .unwrap();
    let first_idle = watch
        .seen
        .iter()
        .position(|e| {
            matches!(
                e,
                SyncEvent::AccountState {
                    state: AccountState::Idle,
                    ..
                }
            )
        })
        .unwrap();
    assert!(
        first_change < first_idle,
        "INBOX rows announced before sync ends"
    );
    let calls = s.calls();
    let last_inbox_fetch = calls
        .iter()
        .rposition(|c| {
            c.op == Op::FetchHeaders && c.folder.as_deref() == Some("INBOX") && c.uids.len() == 50
        })
        .unwrap();
    let first_other_select = calls
        .iter()
        .position(|c| c.op == Op::Select && c.folder.as_deref() != Some("INBOX"))
        .unwrap();
    assert!(
        last_inbox_fetch < first_other_select,
        "INBOX headers before any other folder"
    );

    let listed = rows(&store, inbox).await;
    assert_eq!(listed.len(), 250, "headers for the last 90 days only");
    assert!(listed.iter().all(|r| r.date_utc >= now - 90 * DAY - 60));
    assert_eq!(listed[0].subject, "Subject 300", "newest first");
    let order: Vec<String> = selects(&s).into_iter().take(5).collect();
    assert_eq!(
        order,
        vec!["INBOX", "Drafts", "Sent", "Archive", "Projects"]
    );

    // Prefetch: bodies for the newest 200 messages of INBOX only, never more.
    let body_count = format!(
        "SELECT count(*) FROM bodies b JOIN messages m ON m.id = b.message_id WHERE m.folder_id = {inbox}"
    );
    eventually(|| dir.count(&body_count) >= 200).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(dir.count(&body_count), 200);
    let oldest_with_body: i64 = dir.raw().query_row(
        &format!("SELECT min(m.uid) FROM bodies b JOIN messages m ON m.id = b.message_id WHERE m.folder_id = {inbox}"),
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(
        oldest_with_body, 101,
        "exactly the newest 200 (UIDs 101..300)"
    );
    let preview = rows(&store, inbox).await[0].preview.clone();
    assert_eq!(
        preview, "Body of message 300",
        "quotes and signature dropped"
    );
    assert_eq!(
        dir.count(&format!(
            "SELECT count(*) FROM messages WHERE folder_id = {inbox} AND body_state = 'full'"
        )),
        200
    );
    let f = store.get_folder(inbox).await.unwrap();
    assert!(f.synced_at > 0 && f.uidvalidity == 1 && f.total == 250 && f.unread == 250);
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t02_uidvalidity_change_purges_and_repopulates() {
    let dir = TempDir::new("t02");
    let store = dir.store();
    let account = add_account(&store, USER).await;
    let s = server(&["IMAP4rev1", "IDLE", "CONDSTORE"]);
    let now = now();
    for n in 1..=5 {
        s.deliver("INBOX", msg(n, now - i64::from(n) * 60)).unwrap();
    }
    s.expunge_external("INBOX", 2).unwrap();
    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Idle).await;
    let inbox = folder_id(&store, account, "INBOX").await;
    assert_eq!(store.cached_uids(inbox).await.unwrap(), vec![1, 3, 4, 5]);

    // The server rebuilds the mailbox: new UIDVALIDITY, renumbered from 1.
    s.change_uidvalidity("INBOX", 99).unwrap();
    let reset = watch
        .until(Some(Duration::from_secs(20)), |e| {
            matches!(e, SyncEvent::FolderReset { .. })
        })
        .await;
    assert_eq!(
        reset,
        SyncEvent::FolderReset {
            account_id: account,
            folder_id: inbox
        }
    );
    watch.state(account, AccountState::Idle).await;
    assert_eq!(
        store.cached_uids(inbox).await.unwrap(),
        vec![1, 2, 3, 4],
        "no stale rows"
    );
    assert_eq!(s.uids("INBOX"), vec![1, 2, 3, 4]);
    let f = store.get_folder(inbox).await.unwrap();
    assert_eq!(f.uidvalidity, 99);
    assert_eq!(f.total, 4);
    let subjects: Vec<String> = rows(&store, inbox)
        .await
        .into_iter()
        .map(|r| r.subject)
        .collect();
    assert_eq!(
        subjects,
        vec!["Subject 1", "Subject 3", "Subject 4", "Subject 5"]
    );
    handle.shutdown().await;
}

async fn t04_with(caps: &[&str]) {
    let dir = TempDir::new("t04");
    let store = dir.store();
    let account = add_account(&store, USER).await;
    let s = server(caps);
    let now = now();
    for n in 1..=3 {
        s.deliver("INBOX", msg(n, now - 60)).unwrap();
    }
    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Idle).await;
    let inbox = folder_id(&store, account, "INBOX").await;
    assert_eq!(rows(&store, inbox).await.len(), 3);

    s.expunge_external("INBOX", 2).unwrap();
    let start = Instant::now();
    loop {
        watch
            .until(
                Some(Duration::from_secs(20)),
                |e| matches!(e, SyncEvent::StoreChanged { folder_id: Some(f), .. } if *f == inbox),
            )
            .await;
        // The row goes at step 6 and the counter follows at step 7: wait for both.
        if store.cached_uids(inbox).await.unwrap() == vec![1, 3]
            && store.get_folder(inbox).await.unwrap().total == 2
        {
            break;
        }
    }
    assert!(start.elapsed() < Duration::from_secs(2));
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t04_expunge_elsewhere_removes_the_row_with_qresync() {
    t04_with(&["IMAP4rev1", "IDLE", "QRESYNC"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t04_expunge_elsewhere_removes_the_row_on_plain_imap() {
    t04_with(&["IMAP4rev1", "IDLE"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t05_delivered_mail_is_in_the_list_within_two_seconds() {
    let dir = TempDir::new("t05");
    let store = dir.store();
    let account = add_account(&store, USER).await;
    let s = server(&["IMAP4rev1", "IDLE", "UIDPLUS", "CONDSTORE"]);
    s.deliver("INBOX", msg(1, now() - 60)).unwrap();
    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Idle).await;
    let inbox = folder_id(&store, account, "INBOX").await;
    // Let the IDLE connection settle.
    eventually(|| s.calls().iter().any(|c| c.op == Op::Idle)).await;

    let start = Instant::now();
    s.deliver("INBOX", msg(2, now())).unwrap();
    loop {
        watch
            .until(
                Some(Duration::from_secs(5)),
                |e| matches!(e, SyncEvent::StoreChanged { folder_id: Some(f), .. } if *f == inbox),
            )
            .await;
        if rows(&store, inbox).await.len() == 2 {
            break;
        }
    }
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(rows(&store, inbox).await[0].subject, "Subject 2");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t07_a_kill_between_batches_resumes_without_duplicates() {
    let dir = TempDir::new("t07");
    let s = server(&["IMAP4rev1", "IDLE", "UIDPLUS"]);
    let now = now();
    for n in 1..=350 {
        s.deliver("INBOX", msg(n, now - i64::from(n))).unwrap();
    }
    let account;
    {
        let store = dir.store();
        account = add_account(&store, USER).await;
        // The second header batch never completes; the process is killed while it waits.
        s.fail_after(Op::FetchHeaders, 1, ImapError::Timeout);
        let (events, mut watch) = Watch::new();
        let handle = spawn(&store, s.clone(), &logins(), &events, account);
        watch.state(account, AccountState::Backoff).await;
        handle.abort();
    }
    assert_eq!(
        dir.count("SELECT count(*) FROM messages"),
        100,
        "one batch committed"
    );

    let store = dir.store();
    store.integrity_check().await.unwrap();
    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Idle).await;
    let inbox = folder_id(&store, account, "INBOX").await;
    assert_eq!(dir.count("SELECT count(*) FROM messages"), 350);
    assert_eq!(dir.count("SELECT count(DISTINCT uid) FROM messages"), 350);
    assert_eq!(
        store.cached_uids(inbox).await.unwrap(),
        (1..=350).collect::<Vec<_>>()
    );
    // FTS5's check on a connection other than the store's writer can report corruption
    // while the writer changes the index (mail-store tests/fts_concurrency.rs), so the
    // checks wait for the resumed sync to finish its prefetch: the newest 200 have bodies.
    eventually(|| {
        dir.count(
            "SELECT count(*) FROM (SELECT body_state FROM messages
             ORDER BY date_utc DESC, id DESC LIMIT 200) WHERE body_state = 'none'",
        ) == 0
    })
    .await;
    store.integrity_check().await.unwrap();
    let fts = dir.raw().execute(
        "INSERT INTO message_fts(message_fts, rank) VALUES ('integrity-check', 1)",
        [],
    );
    assert!(fts.is_ok(), "FTS index agrees with its content: {fts:?}");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t09_revoked_password_needs_attention_and_the_other_account_keeps_syncing() {
    let dir = TempDir::new("t09");
    let store = dir.store();
    let alice_server = FakeServer::new(&["IMAP4rev1", "IDLE"]);
    alice_server.add_user("alice@test.local", "a-pw");
    let bob_server = FakeServer::new(&["IMAP4rev1", "IDLE"]);
    bob_server.add_user("bob@test.local", "b-pw");
    let logins = Logins::default();
    logins.set("alice@test.local", "a-pw");
    logins.set("bob@test.local", "b-pw");
    let alice = add_account(&store, "alice@test.local").await;
    let bob = add_account(&store, "bob@test.local").await;
    let (events, mut watch) = Watch::new();
    let ha = spawn(&store, alice_server.clone(), &logins, &events, alice);
    let hb = spawn(&store, bob_server.clone(), &logins, &events, bob);
    watch.state(alice, AccountState::Idle).await;
    watch.state(bob, AccountState::Idle).await;
    eventually(|| alice_server.calls().iter().any(|c| c.op == Op::Idle)).await;

    // Revoke Alice's app password, then drop her connection (the server says BYE).
    alice_server.set_password("alice@test.local", "revoked");
    alice_server.clear_calls();
    alice_server.fail_next(Op::Idle, ImapError::Bye);
    alice_server.deliver("INBOX", msg(1, now())).unwrap();

    let event = watch.state(alice, AccountState::NeedsAttention).await;
    let SyncEvent::AccountState { last_error, .. } = event else {
        unreachable!()
    };
    assert_eq!(last_error, Some(SyncError::Imap(ImapError::AuthFailed)));
    // Zero retries: one login after the drop, then nothing, however long we wait.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let connects = alice_server
        .calls()
        .iter()
        .filter(|c| c.op == Op::Connect)
        .count();
    assert_eq!(connects, 1);

    // Bob keeps syncing.
    let bob_inbox = folder_id(&store, bob, "INBOX").await;
    bob_server.deliver("INBOX", msg(7, now())).unwrap();
    loop {
        watch
            .until(Some(Duration::from_secs(5)), |e| {
                matches!(e, SyncEvent::StoreChanged { folder_id: Some(f), .. } if *f == bob_inbox)
            })
            .await;
        if rows(&store, bob_inbox).await.len() == 1 {
            break;
        }
    }

    // The user fixes the password: Retry connects again.
    alice_server.set_password("alice@test.local", "a-pw");
    ha.retry();
    watch.state(alice, AccountState::Idle).await;
    ha.shutdown().await;
    hb.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flags_from_other_clients_are_pulled_with_and_without_condstore() {
    for caps in [
        &["IMAP4rev1", "IDLE", "CONDSTORE"][..],
        &["IMAP4rev1", "IDLE"][..],
    ] {
        let dir = TempDir::new("flags");
        let store = dir.store();
        let account = add_account(&store, USER).await;
        let s = server(caps);
        for n in 1..=3 {
            s.deliver("INBOX", msg(n, now() - 60)).unwrap();
        }
        let (events, mut watch) = Watch::new();
        let handle = spawn(&store, s.clone(), &logins(), &events, account);
        watch.state(account, AccountState::Idle).await;
        let inbox = folder_id(&store, account, "INBOX").await;

        s.set_flags("INBOX", 2, vec![Flag::Seen, Flag::Flagged])
            .unwrap();
        s.set_flags("INBOX", 3, vec![Flag::Deleted]).unwrap();
        loop {
            watch
                .until(Some(Duration::from_secs(5)), |e| {
                    matches!(e, SyncEvent::StoreChanged { folder_id: Some(f), .. } if *f == inbox)
                })
                .await;
            // Rows change at step 5 and counters at step 7: wait for both. \Deleted rows
            // are hidden and not counted.
            let r = rows(&store, inbox).await;
            let f = store.get_folder(inbox).await.unwrap();
            if r.len() == 2
                && r.iter()
                    .any(|m| m.flags == Flags(Flags::SEEN | Flags::FLAGGED))
                && (f.total, f.unread) == (2, 1)
            {
                break;
            }
        }
        handle.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_an_uncached_message_fetches_it_and_caps_large_parts() {
    let dir = TempDir::new("open");
    let store = dir.store();
    let account = add_account(&store, USER).await;
    let s = server(&["IMAP4rev1", "IDLE"]);
    let now = now();
    // A 1.5 MB text part: never prefetched; opened, it is cut at 1 MB.
    let big_text = "x".repeat(1_500_000);
    s.deliver(
        "INBOX",
        mail_imap::fake::FakeMessage::simple("big@t", "a@t", USER, "Big", &big_text, now - 30),
    )
    .unwrap();
    // 201 newer messages push the first one out of the prefetch cap as well.
    for n in 1..=201 {
        s.deliver("INBOX", msg(n, now - 20 + i64::from(n) / 100))
            .unwrap();
    }
    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Idle).await;
    let inbox = folder_id(&store, account, "INBOX").await;
    let big: i64 = dir
        .raw()
        .query_row("SELECT id FROM messages WHERE uid = 1", [], |r| r.get(0))
        .unwrap();
    eventually(|| dir.count("SELECT count(*) FROM bodies") == 200).await;
    assert_eq!(
        store.get_message(big).await.unwrap().body_state,
        mail_store::BodyState::None
    );

    handle.fetch_body(big);
    eventually(|| {
        dir.count(&format!(
            "SELECT count(*) FROM bodies WHERE message_id = {big}"
        )) == 1
    })
    .await;
    let m = store.get_message(big).await.unwrap();
    assert_eq!(m.body_state, mail_store::BodyState::Text, "cut at 1 MB");
    let body = store.get_body(big).await.unwrap().unwrap();
    assert_eq!(body.text_plain.len(), 1024 * 1024);
    assert!(rows(&store, inbox).await.len() == 202);
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_qresync_resync_is_one_flag_command_without_search_all() {
    let dir = TempDir::new("qresync");
    let store = dir.store();
    let account = add_account(&store, USER).await;
    let s = server(&["IMAP4rev1", "IDLE", "QRESYNC"]);
    for n in 1..=600 {
        s.deliver("INBOX", msg(n, now() - 60)).unwrap();
    }
    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Idle).await;
    let inbox = folder_id(&store, account, "INBOX").await;
    s.clear_calls();
    s.set_flags("INBOX", 10, vec![Flag::Seen]).unwrap();
    loop {
        watch
            .until(
                Some(Duration::from_secs(10)),
                |e| matches!(e, SyncEvent::StoreChanged { folder_id: Some(f), .. } if *f == inbox),
            )
            .await;
        if store.get_folder(inbox).await.unwrap().unread == 599 {
            break;
        }
    }
    let calls = s.calls();
    let worker: Vec<_> = calls.iter().filter(|c| c.op != Op::Idle).collect();
    // Each resync of the 600 cached messages is one flag command, not batches of 500.
    let syncs = worker
        .iter()
        .filter(|c| c.op == Op::Search && c.arg.starts_with("UidFrom"))
        .count();
    assert!(syncs >= 1);
    assert_eq!(
        worker.iter().filter(|c| c.op == Op::FetchFlags).count(),
        syncs
    );
    assert!(
        !worker.iter().any(|c| c.op == Op::Search && c.arg == "All"),
        "VANISHED replaces SEARCH ALL"
    );
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folders_gone_from_list_are_purged_and_gmail_all_mail_starts_unsynced() {
    let dir = TempDir::new("folders");
    let store = dir.store();
    let account = add_account(&store, USER).await;
    let s = server(&["IMAP4rev1", "IDLE", "SPECIAL-USE", "X-GM-EXT-1", "UIDPLUS"]);
    s.add_folder("[Gmail]/All Mail", Some(SpecialUse::All));
    s.add_folder("Old", None);
    s.deliver("Old", msg(1, now() - 60)).unwrap();
    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Idle).await;
    let folders = store.list_folders(account).await.unwrap();
    let all = folders
        .iter()
        .find(|f| f.path == "[Gmail]/All Mail")
        .unwrap();
    assert!(
        !all.sync_enabled,
        "All Mail duplicates every message (section 9)"
    );
    assert!(!selects(&s).contains(&"[Gmail]/All Mail".to_string()));
    let old = folder_id(&store, account, "Old").await;
    assert_eq!(rows(&store, old).await.len(), 1);
    handle.shutdown().await;

    // "Old" is deleted on the server; the next session's LIST purges it.
    let mut t = mail_imap::ImapConnector::connect(
        &s,
        &mail_imap::ConnectConfig {
            host: "x".into(),
            port: 1,
            security: mail_imap::Security::Tls,
            pinned_cert_sha256: None,
            connect_timeout: Duration::from_secs(1),
        },
        &mail_imap::Credentials::Password {
            username: USER.into(),
            password: mail_imap::Secret::new(PASS),
        },
    )
    .await
    .unwrap();
    mail_imap::ImapTransport::delete(&mut t, "Old")
        .await
        .unwrap();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Connecting).await;
    watch.state(account, AccountState::Idle).await;
    assert!(store
        .list_folders(account)
        .await
        .unwrap()
        .iter()
        .all(|f| f.path != "Old"));
    assert_eq!(
        dir.count(&format!(
            "SELECT count(*) FROM messages WHERE folder_id = {old}"
        )),
        0
    );
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_connections_are_capped_at_five_per_account() {
    let dir = TempDir::new("cap");
    let store = dir.store();
    let account = add_account(&store, USER).await;
    let s = server(&["IMAP4rev1", "IDLE"]);
    for name in ["A", "B", "C", "D"] {
        s.add_folder(name, None);
    }
    // First session creates the folder rows; then the user watches all four.
    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Idle).await;
    handle.shutdown().await;
    dir.raw()
        .execute(
            "UPDATE folders SET watch_idle = 1 WHERE path IN ('A','B','C','D')",
            [],
        )
        .unwrap();
    s.clear_calls();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Connecting).await;
    watch.state(account, AccountState::Idle).await;
    eventually(|| s.calls().iter().filter(|c| c.op == Op::Idle).count() >= 4).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let connects = s.calls().iter().filter(|c| c.op == Op::Connect).count();
    assert_eq!(connects, 5, "worker + INBOX + 3 watched");
    let idling: std::collections::BTreeSet<String> = s
        .calls()
        .into_iter()
        .filter(|c| c.op == Op::Idle)
        .filter_map(|c| c.folder)
        .collect();
    assert_eq!(idling.len(), 4);
    assert!(idling.contains("INBOX"));
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_and_open_folder_sync_on_request() {
    let dir = TempDir::new("refresh");
    let store = dir.store();
    let account = add_account(&store, USER).await;
    let s = server(&["IMAP4rev1", "IDLE"]);
    s.add_folder("Work", None);
    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Idle).await;
    let work = folder_id(&store, account, "Work").await;

    // Mail in a non-IDLE folder arrives only on request.
    s.deliver("Work", msg(1, now())).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(rows(&store, work).await.is_empty());
    handle.refresh(work);
    eventually(|| {
        dir.count(&format!(
            "SELECT count(*) FROM messages WHERE folder_id = {work}"
        )) == 1
    })
    .await;

    s.deliver("Work", msg(2, now())).unwrap();
    handle.open_folder(Some(work));
    eventually(|| {
        dir.count(&format!(
            "SELECT count(*) FROM messages WHERE folder_id = {work}"
        )) == 2
    })
    .await;
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_the_account_closes_it_and_missing_secrets_need_attention() {
    let dir = TempDir::new("disable");
    let store = dir.store();
    let account = add_account(&store, USER).await;
    let s = server(&["IMAP4rev1", "IDLE"]);
    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, s.clone(), &logins(), &events, account);
    watch.state(account, AccountState::Idle).await;
    store.set_account_enabled(account, false).await.unwrap();
    handle.account_changed();
    watch.state(account, AccountState::Disabled).await;
    store.set_account_enabled(account, true).await.unwrap();
    handle.account_changed();
    watch.state(account, AccountState::Idle).await;
    handle.shutdown().await;

    let other = add_account(&store, "nobody@test.local").await;
    let handle = spawn(&store, s.clone(), &logins(), &events, other);
    let event = watch.state(other, AccountState::NeedsAttention).await;
    assert!(matches!(
        event,
        SyncEvent::AccountState {
            last_error: Some(SyncError::Auth(_)),
            ..
        }
    ));
    handle.shutdown().await;
}
