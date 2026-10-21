//! T30, local half: on a 100,000-message fixture,
//! `from:alice has:attachment after:2026-01-01 budget` returns the right messages, and its
//! first page arrives within 300 ms (section 14). The Search server too half runs on the
//! fake transport in mail-core's tests.
#![cfg(test)]

use std::time::{Duration, Instant};

use mail_search::{local, parse, Context};
use mail_store::*;

const ROWS: u32 = 100_000;
const BUDGET: Duration = Duration::from_millis(300);
/// 2025-06-01 and 2026-01-01, UTC.
const START: i64 = 1_748_736_000;
const NEW_YEAR: i64 = 1_767_225_600;

struct Spec {
    date: i64,
    alice: bool,
    attach: bool,
    budget: bool,
}

fn spec(n: u32) -> Spec {
    Spec {
        // About 500 days, a few minutes apart.
        date: START + i64::from(n) * 431,
        alice: n.is_multiple_of(7),
        attach: n.is_multiple_of(3),
        budget: n.is_multiple_of(5),
    }
}

fn message(n: u32, thread: ThreadId) -> NewMessage {
    let s = spec(n);
    let (name, addr) = if s.alice {
        ("Alice Smith", "alice@example.test")
    } else {
        ("Bob Jones", "bob@example.test")
    };
    NewMessage {
        uid: n + 1,
        gm_msgid: None,
        gm_thrid: None,
        message_id: format!("{n}@fixture.test"),
        in_reply_to: None,
        refs: None,
        gm_labels: None,
        list_id: None,
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        thread_id: thread,
        subject: if s.budget {
            format!("Quarterly budget review {n}")
        } else {
            format!("Status update {n}")
        },
        from: Address {
            name: name.into(),
            addr: addr.into(),
        },
        to: vec![Address {
            name: "Me".into(),
            addr: "me@example.test".into(),
        }],
        cc: Vec::new(),
        reply_to: Vec::new(),
        date_utc: s.date,
        internal_utc: s.date,
        size: 2048,
        flags: Flags::default(),
        has_attach: s.attach,
        preview: String::new(),
        body_state: BodyState::None,
        attachments: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t30_search_100k_within_300_ms() {
    let dir = std::env::temp_dir().join(format!("zero-mail-t30-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Store::open(&dir.join("mail.db")).unwrap();
    let account = store
        .insert_account(NewAccount {
            name: "Me".into(),
            email: "me@example.test".into(),
            imap_host: "imap.example.test".into(),
            imap_port: 993,
            imap_security: Security::Tls,
            smtp_host: "smtp.example.test".into(),
            smtp_port: 465,
            smtp_security: Security::Tls,
            username: "me".into(),
            auth_kind: AuthKind::Password,
            oauth_client_id: String::new(),
            pinned_cert_sha256: None,
            sync_days: 90,
            save_sent_copy: SaveSentCopy::Auto,
            signature_html: String::new(),
            signature_text: String::new(),
            enabled: true,
            created_at: 0,
        })
        .await
        .unwrap();
    let folder = |path: &str, special_use| NewFolder {
        account_id: account,
        path: path.into(),
        delimiter: "/".into(),
        special_use,
        sync_enabled: true,
        watch_idle: false,
    };
    let inbox = store
        .insert_folder(folder("INBOX", Some(SpecialUse::Inbox)))
        .await
        .unwrap();
    let trash = store
        .insert_folder(folder("Trash", Some(SpecialUse::Trash)))
        .await
        .unwrap();
    let thread = store
        .insert_thread(account, 0, String::new())
        .await
        .unwrap();
    for start in (0..ROWS).step_by(1_000) {
        let batch = (start..start + 1_000).map(|n| message(n, thread)).collect();
        store.insert_messages(inbox, batch).await.unwrap();
    }
    // A match in Trash is out of scope (section 10, Scope).
    let mut in_trash = message(ROWS + 1, thread);
    in_trash.from = Address {
        name: "Alice".into(),
        addr: "alice@example.test".into(),
    };
    in_trash.has_attach = true;
    in_trash.subject = "budget in the trash".into();
    in_trash.date_utc = NEW_YEAR + 86_400;
    store.insert_messages(trash, vec![in_trash]).await.unwrap();

    let mut expected: Vec<(i64, u32)> = (0..ROWS)
        .filter(|n| {
            let s = spec(*n);
            s.alice && s.attach && s.budget && s.date >= NEW_YEAR
        })
        .map(|n| (spec(n).date, n + 1))
        .collect();
    expected.sort_unstable_by(|a, b| b.cmp(a));
    assert!(expected.len() > 500, "the fixture has many matches");

    let folders = store.list_folders(account).await.unwrap();
    let ctx = Context {
        accounts: vec![(account, "me@example.test".into())],
        folders,
        account: Some(account),
        now: NEW_YEAR + 300 * 86_400,
    };
    let query = parse("from:alice has:attachment after:2026-01-01 budget");
    let sql = local(&query, &ctx);

    // Warm the page cache once, then measure the first page (section 14, measured warm).
    store.query_page(sql.clone(), None, 100).await.unwrap();
    let started = Instant::now();
    let first = store.query_page(sql.clone(), None, 100).await.unwrap();
    let took = started.elapsed();
    println!("T30: first page in {took:?}");
    assert!(took <= BUDGET, "first page in {took:?}");

    // Every page together is exactly the expected set, newest first.
    let mut got: Vec<(i64, u32)> = Vec::new();
    let mut page = first;
    loop {
        for r in &page.rows {
            let m = store.get_message(r.id).await.unwrap();
            assert_eq!(m.folder_id, inbox);
            got.push((r.date_utc, m.uid));
        }
        match page.next {
            Some(c) => page = store.query_page(sql.clone(), Some(c), 100).await.unwrap(),
            None => break,
        }
    }
    assert_eq!(got, expected);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
