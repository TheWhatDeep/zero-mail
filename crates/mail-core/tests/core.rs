//! The M4 facade on the fakes: an account added through `Core` syncs, and the commands the
//! UI calls (account.list, folder.list, list.page, message.open, mail-body) see the result.
#![cfg(test)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{config, FakeHttp, TempDir};
use mail_core::views::{BodyKind, ListRequest, Role, Scope};
use mail_core::{Core, CoreConfig, CoreEvent, ErrorCode, EventSink, StateName};
use mail_imap::fake::{FakeMessage, FakeServer};
use mail_secrets::{MemoryStore, SecretStore};
use mail_smtp::fake::FakeSmtp;

const USER: &str = "alice@test.local";
const PASS: &str = "alice-pw";

type TestCore = Core<FakeServer, FakeSmtp, FakeHttp>;

struct Setup {
    dir: TempDir,
    imap: FakeServer,
    events: Arc<Mutex<Vec<CoreEvent>>>,
    core: Arc<TestCore>,
}

async fn start(dir: TempDir, imap: FakeServer) -> Setup {
    let smtp = FakeSmtp::default();
    smtp.add_user(USER, PASS);
    let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
    let log = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |e| log.lock().unwrap().push(e));
    let core = Core::start(
        CoreConfig {
            data_dir: dir.path(),
            secrets: Arc::new(MemoryStore::default()) as Arc<dyn SecretStore>,
            imap: imap.clone(),
            smtp,
            http: FakeHttp::default(),
        },
        sink,
    )
    .await
    .unwrap();
    Setup {
        dir,
        imap,
        events,
        core,
    }
}

async fn until(s: &Setup, mut pred: impl FnMut(&CoreEvent) -> bool) {
    let start = std::time::Instant::now();
    loop {
        if s.events.lock().unwrap().iter().any(&mut pred) {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "event not seen");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn server() -> FakeServer {
    let s = FakeServer::new(&["IMAP4rev1", "IDLE", "SPECIAL-USE"]);
    s.add_user(USER, PASS);
    s.add_folder("Sent", Some(mail_imap::SpecialUse::Sent));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for n in 1..=3 {
        s.deliver(
            "INBOX",
            FakeMessage::simple(
                &format!("m{n}@t"),
                "Bob <bob@test.local>",
                USER,
                &format!("Hello {n}"),
                &format!("<b>not html</b> & line {n}"),
                now - 60 * i64::from(n),
            ),
        )
        .unwrap();
    }
    s
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_added_account_syncs_and_the_reader_commands_see_it() {
    let s = start(TempDir::new("core"), server()).await;
    assert!(s.core.account_list().await.unwrap().is_empty());

    let summary = s.core.account_add(config(USER, USER, PASS)).await.unwrap();
    let id = summary.id;
    until(&s, |e| {
        matches!(e, CoreEvent::AccountState { account_id, state: StateName::Idle, .. } if *account_id == id)
    })
    .await;
    let accounts = s.core.account_list().await.unwrap();
    assert_eq!(accounts[0].state, StateName::Idle);
    let json = serde_json::to_value(&accounts[0]).unwrap();
    assert_eq!(json["email"], USER, "summary fields are flattened");
    assert_eq!(json["state"], "idle");

    let folders = s.core.folder_list(id).await.unwrap();
    assert_eq!(folders[0].role, Some(Role::Inbox));
    assert_eq!(folders[0].name, "Inbox");
    assert_eq!(folders[1].role, Some(Role::Sent));
    let inbox = folders[0].id;
    assert_eq!(folders[0].total, 3);

    let page = s
        .core
        .list_page(ListRequest {
            scope: Scope::Folder { folder_id: inbox },
            cursor: None,
            limit: 2,
            group_by_thread: false,
        })
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.rows[0].subject, "Hello 1");
    assert!(page.rows[0].unread);
    let rest = s
        .core
        .list_page(ListRequest {
            scope: Scope::Folder { folder_id: inbox },
            cursor: page.next,
            limit: 2,
            group_by_thread: false,
        })
        .await
        .unwrap();
    assert_eq!(rest.rows.len(), 1);
    assert!(rest.next.is_none());

    // Bodies are prefetched; the reader gets plain text through mail-body.
    let message = page.rows[0].id;
    let mut view = s.core.message_open(message).await.unwrap();
    let start = std::time::Instant::now();
    while view.body_kind == BodyKind::Loading {
        assert!(start.elapsed() < Duration::from_secs(20));
        tokio::time::sleep(Duration::from_millis(20)).await;
        view = s.core.message_open(message).await.unwrap();
    }
    assert_eq!(view.body_kind, BodyKind::Plain);
    assert_eq!(view.from.addr, "bob@test.local");
    assert_eq!(
        view.body_url,
        format!("mail-body://localhost/{message}?images=0")
    );
    let body = s.core.mail_body(message, "images=0").await.unwrap();
    assert_eq!(body.status, 200);
    let html = String::from_utf8(body.body).unwrap();
    assert!(html.contains("&lt;b&gt;not html&lt;/b&gt; &amp; line 1"));
    assert!(body.csp.starts_with("default-src 'none'"));
    assert_eq!(s.core.mail_body(999, "images=0").await.unwrap().status, 404);

    // Unified, search and thread scopes arrive in M9.
    let err = s
        .core
        .list_page(ListRequest {
            scope: Scope::Unified,
            cursor: None,
            limit: 10,
            group_by_thread: false,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);

    // T04, the Core half: a message expunged elsewhere is gone for message.open.
    s.imap.expunge_external("INBOX", 1).unwrap();
    until(
        &s,
        |e| matches!(e, CoreEvent::StoreChanged { folder_id: Some(f), .. } if *f == inbox),
    )
    .await;
    let start = std::time::Instant::now();
    loop {
        match s.core.message_open(message).await {
            Err(e) => {
                assert_eq!(e.code, ErrorCode::NotFound);
                break;
            }
            Ok(_) => {
                assert!(start.elapsed() < Duration::from_secs(20));
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }

    s.core.account_remove(id).await.unwrap();
    assert!(s.core.account_list().await.unwrap().is_empty());
    s.core.shutdown().await;
    drop(s.dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupt_database_is_moved_aside_and_rebuilt_with_a_warning() {
    let dir = TempDir::new("corrupt");
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(dir.path().join("mail.db"), b"this is not a database at all").unwrap();
    let s = start(dir, server()).await;
    until(&s, |e| {
        matches!(e, CoreEvent::StorageWarning { kind: "db_corrupt" })
    })
    .await;
    let aside: Vec<String> = std::fs::read_dir(s.dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("mail.db.corrupt-"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert!(
        s.core.account_list().await.unwrap().is_empty(),
        "a fresh database"
    );
    s.core.shutdown().await;
    let dir = s.dir;
    drop(s.core);
    // Let aborted tasks drop their store handles.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // A marker from a failed background check rebuilds at the next start.
    std::fs::write(dir.path().join("rebuild-database"), b"x").unwrap();
    let again = start(dir, server()).await;
    until(&again, |e| matches!(e, CoreEvent::StorageWarning { .. })).await;
    assert!(!again.dir.path().join("rebuild-database").exists());
    again.core.shutdown().await;
}
