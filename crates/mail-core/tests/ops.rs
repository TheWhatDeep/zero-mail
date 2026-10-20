//! M6 on the fakes: T03, T06, T25 (rows and Archive, SPEC_GAPS G60), T26, T27, T28, plus
//! the op-failed revert (G58), Gmail's hidden folders (G61), Load older (G51, G62), Mark all
//! read, Empty Trash and the folder settings.
#![cfg(test)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{config, FakeHttp, TempDir};
use mail_core::views::{FlagChange, ListRequest, RowView, Scope};
use mail_core::{Core, CoreConfig, CoreEvent, ErrorCode, EventSink, StateName};
use mail_imap::fake::{FakeMessage, FakeServer, Op};
use mail_imap::{Flag, ImapError, SpecialUse};
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
    account: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn msg(subject: &str, age_days: i64) -> FakeMessage {
    FakeMessage::simple(
        &format!("{}@t", subject.replace(' ', "-")),
        "Bob <bob@test.local>",
        USER,
        subject,
        &format!("Body of {subject}"),
        now() - age_days * 86_400 - 60,
    )
}

fn server(caps: &[&str]) -> FakeServer {
    let s = FakeServer::new(caps);
    s.add_user(USER, PASS);
    s.add_folder("Archive", Some(SpecialUse::Archive));
    s.add_folder("Trash", Some(SpecialUse::Trash));
    s.add_folder("Work", None);
    s
}

const FULL: &[&str] = &["IMAP4rev1", "IDLE", "MOVE", "UIDPLUS", "SPECIAL-USE"];

async fn start(imap: FakeServer) -> Setup {
    let dir = TempDir::new("ops");
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
    let account = core.account_add(config(USER, USER, PASS)).await.unwrap().id;
    let s = Setup {
        dir,
        imap,
        events,
        core,
        account,
    };
    s.until_idle().await;
    s
}

async fn wait_for(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let start = Instant::now();
    loop {
        if check().await {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

impl Setup {
    async fn until_idle(&self) {
        let (events, account) = (Arc::clone(&self.events), self.account);
        wait_for("idle", async || {
            events.lock().unwrap().iter().any(|e| {
                matches!(e, CoreEvent::AccountState { account_id, state: StateName::Idle, .. } if *account_id == account)
            })
        })
        .await;
    }

    async fn folder(&self, path: &str) -> i64 {
        self.core
            .folder_list(self.account)
            .await
            .unwrap()
            .iter()
            .find(|f| f.path == path)
            .unwrap()
            .id
    }

    async fn rows(&self, path: &str) -> Vec<RowView> {
        let folder_id = self.folder(path).await;
        self.core
            .list_page(ListRequest {
                scope: Scope::Folder { folder_id },
                cursor: None,
                limit: 500,
                group_by_thread: false,
            })
            .await
            .unwrap()
            .rows
    }

    async fn row(&self, path: &str, subject: &str) -> Option<RowView> {
        self.rows(path)
            .await
            .into_iter()
            .find(|r| r.subject == subject)
    }

    /// Waits until `subject` is listed in `path`, and returns it.
    async fn listed(&self, path: &str, subject: &str) -> RowView {
        let start = Instant::now();
        loop {
            if let Some(r) = self.row(path, subject).await {
                return r;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "{subject} never listed in {path}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn pending_zero_seen(&self) -> bool {
        self.events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|e| match e {
                CoreEvent::PendingCount { total } => Some(*total),
                _ => None,
            })
            == Some(0)
    }

    fn calls(&self, op: Op) -> usize {
        self.imap.calls().iter().filter(|c| c.op == op).count()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t03_remote_read_and_local_star_meet_within_two_seconds() {
    let imap = server(FULL);
    let a = imap.deliver("INBOX", msg("read elsewhere", 1)).unwrap();
    let b = imap.deliver("INBOX", msg("star here", 1)).unwrap();
    let s = start(imap).await;
    let row = s.listed("INBOX", "star here").await;
    assert!(s.listed("INBOX", "read elsewhere").await.unread);

    // Another client marks one read; IDLE brings it.
    let t0 = Instant::now();
    s.imap.set_flags("INBOX", a, vec![Flag::Seen]).unwrap();
    wait_for("remote read seen", async || {
        !s.row("INBOX", "read elsewhere").await.unwrap().unread
    })
    .await;
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());

    // A local star shows at once and reaches the server.
    let t0 = Instant::now();
    s.core
        .message_set_flags(
            &[row.id],
            FlagChange {
                seen: None,
                flagged: Some(true),
            },
        )
        .await
        .unwrap();
    assert!(
        s.row("INBOX", "star here").await.unwrap().flagged,
        "instant"
    );
    wait_for("star on the server", async || {
        s.imap.flags("INBOX", b).unwrap().contains(&Flag::Flagged)
    })
    .await;
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    wait_for("pending back to 0", async || s.pending_zero_seen()).await;
    assert!(
        s.row("INBOX", "star here").await.unwrap().flagged,
        "the server agrees"
    );
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t06_offline_star_move_delete_then_reconnect() {
    let imap = server(FULL);
    for subject in ["one", "two", "three"] {
        imap.deliver("INBOX", msg(subject, 1)).unwrap();
    }
    let s = start(imap).await;
    let (one, two, three) = (
        s.listed("INBOX", "one").await,
        s.listed("INBOX", "two").await,
        s.listed("INBOX", "three").await,
    );
    // Offline: the account's connection is gone.
    s.core.account_set_enabled(s.account, false).await.unwrap();
    let account = s.account;
    let events = Arc::clone(&s.events);
    wait_for("disabled", async || {
        events.lock().unwrap().iter().any(|e| {
            matches!(e, CoreEvent::AccountState { account_id, state: StateName::Disabled, .. } if *account_id == account)
        })
    })
    .await;
    s.imap.clear_calls();
    let work = s.folder("Work").await;
    s.core
        .message_set_flags(
            &[one.id],
            FlagChange {
                seen: None,
                flagged: Some(true),
            },
        )
        .await
        .unwrap();
    s.core.message_move(&[two.id], work).await.unwrap();
    s.core.message_delete(&[three.id], None).await.unwrap();
    // The UI changes at once.
    let inbox = s.rows("INBOX").await;
    assert_eq!(inbox.len(), 1);
    assert!(inbox[0].flagged && inbox[0].subject == "one");
    assert!(s.imap.calls().is_empty(), "nothing reached the server");
    let pending = s.events.lock().unwrap().iter().rev().find_map(|e| match e {
        CoreEvent::PendingCount { total } => Some(*total),
        _ => None,
    });
    assert_eq!(pending, Some(3));

    // Reconnect: pushed before anything is pulled; the server matches; pending returns to 0.
    s.core.account_set_enabled(s.account, true).await.unwrap();
    wait_for("server matches", async || {
        s.imap.uids("INBOX").len() == 1
            && s.imap.uids("Work").len() == 1
            && s.imap.uids("Trash").len() == 1
            && s.imap
                .uids("INBOX")
                .first()
                .and_then(|u| s.imap.flags("INBOX", *u))
                .is_some_and(|f| f.contains(&Flag::Flagged))
    })
    .await;
    wait_for("pending 0", async || s.pending_zero_seen()).await;
    s.listed("Work", "two").await;
    s.listed("Trash", "three").await;
    assert_eq!(s.rows("INBOX").await.len(), 1);
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t26_without_move_and_without_uidplus() {
    // No MOVE: COPY, \Deleted, UID EXPUNGE. Same result as MOVE.
    let imap = server(&["IMAP4rev1", "IDLE", "UIDPLUS", "SPECIAL-USE"]);
    imap.deliver("INBOX", msg("no move", 1)).unwrap();
    let s = start(imap).await;
    let row = s.listed("INBOX", "no move").await;
    s.core
        .message_move(&[row.id], s.folder("Work").await)
        .await
        .unwrap();
    wait_for("moved", async || {
        s.imap.uids("INBOX").is_empty() && s.imap.uids("Work").len() == 1
    })
    .await;
    s.listed("Work", "no move").await;
    assert_eq!(s.calls(Op::Move), 0);
    assert!(s.calls(Op::Copy) >= 1 && s.calls(Op::UidExpunge) >= 1);
    assert_eq!(s.calls(Op::Expunge), 0);
    s.core.shutdown().await;

    // No UIDPLUS, another message already \Deleted: nothing unrelated is expunged; the
    // moved one stays flagged on the server and hidden here.
    let imap = server(&["IMAP4rev1", "IDLE", "SPECIAL-USE"]);
    imap.deliver("INBOX", msg("mine", 1)).unwrap();
    let other = imap
        .deliver(
            "INBOX",
            msg("someone else's", 1).with_flags([Flag::Deleted]),
        )
        .unwrap();
    let s = start(imap).await;
    let row = s.listed("INBOX", "mine").await;
    s.core
        .message_move(&[row.id], s.folder("Work").await)
        .await
        .unwrap();
    wait_for("copied", async || s.imap.uids("Work").len() == 1).await;
    wait_for("op finished", async || s.pending_zero_seen()).await;
    assert_eq!(
        s.calls(Op::Expunge),
        0,
        "no bare EXPUNGE with another \\Deleted message"
    );
    assert!(
        s.imap.uids("INBOX").contains(&other),
        "the unrelated message survives"
    );
    assert_eq!(s.imap.uids("INBOX").len(), 2, "the source stays, flagged");
    assert!(s.row("INBOX", "mine").await.is_none(), "and hidden");
    s.core.shutdown().await;

    // No UIDPLUS and nothing else flagged: a bare EXPUNGE is safe.
    let imap = server(&["IMAP4rev1", "IDLE", "SPECIAL-USE"]);
    imap.deliver("INBOX", msg("alone", 1)).unwrap();
    let s = start(imap).await;
    let row = s.listed("INBOX", "alone").await;
    s.core
        .message_move(&[row.id], s.folder("Work").await)
        .await
        .unwrap();
    wait_for("moved", async || {
        s.imap.uids("INBOX").is_empty() && s.imap.uids("Work").len() == 1
    })
    .await;
    assert_eq!(s.calls(Op::Expunge), 1);
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t27_create_rename_delete_a_unicode_folder() {
    let s = start(server(FULL)).await;
    let name = "Café/日本語";
    s.core.folder_create(s.account, name).await.unwrap();
    assert!(
        s.core
            .folder_list(s.account)
            .await
            .unwrap()
            .iter()
            .any(|f| f.path == name),
        "instant"
    );
    wait_for("created", async || {
        s.imap.folder_paths().contains(&name.to_string())
    })
    .await;
    let renamed = "Café/日本語 2";
    s.core
        .folder_rename(s.account, name, renamed)
        .await
        .unwrap();
    wait_for("renamed", async || {
        let paths = s.imap.folder_paths();
        paths.contains(&renamed.to_string()) && !paths.contains(&name.to_string())
    })
    .await;
    s.core.folder_delete(s.account, renamed).await.unwrap();
    wait_for("deleted", async || {
        !s.imap.folder_paths().contains(&renamed.to_string())
    })
    .await;
    wait_for("pending 0", async || s.pending_zero_seen()).await;
    let local: Vec<String> = s
        .core
        .folder_list(s.account)
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.path)
        .collect();
    let mut server = s.imap.folder_paths();
    server.sort();
    let mut mine = local.clone();
    mine.sort();
    assert_eq!(mine, server, "the folder list matches the server");

    // Special-use folders cannot be deleted; INBOX cannot be renamed; duplicates conflict.
    assert_eq!(
        s.core
            .folder_delete(s.account, "Trash")
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        s.core
            .folder_rename(s.account, "INBOX", "X")
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        s.core
            .folder_create(s.account, "Work")
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t28_archive_then_undo_returns_the_message_with_its_flags() {
    for caps in [FULL, &["IMAP4rev1", "IDLE", "MOVE", "SPECIAL-USE"][..]] {
        let imap = server(caps);
        imap.deliver("INBOX", msg("keep me", 1).with_flags([Flag::Flagged]))
            .unwrap();
        let s = start(imap).await;
        let row = s.listed("INBOX", "keep me").await;
        assert!(row.flagged);

        // Undo after the server acted: back by COPYUID, or found by Message-ID.
        let op = s.core.message_archive(&[row.id]).await.unwrap();
        assert!(s.row("INBOX", "keep me").await.is_none(), "instant");
        wait_for("archived on the server", async || {
            s.imap.uids("INBOX").is_empty()
        })
        .await;
        s.core.message_undo(op).await.unwrap();
        wait_for("back on the server", async || {
            s.imap.uids("INBOX").len() == 1
        })
        .await;
        let uid = s.imap.uids("INBOX")[0];
        assert!(
            s.imap.flags("INBOX", uid).unwrap().contains(&Flag::Flagged),
            "flags intact"
        );
        let back = s.listed("INBOX", "keep me").await;
        assert!(back.flagged);
        assert!(s.imap.uids("Archive").is_empty(), "caps {caps:?}");
        s.core.shutdown().await;
    }

    // Undo while still queued: cancelled, nothing reaches the server.
    let imap = server(FULL);
    imap.deliver("INBOX", msg("queued", 1).with_flags([Flag::Seen]))
        .unwrap();
    let s = start(imap).await;
    let row = s.listed("INBOX", "queued").await;
    s.core.account_set_enabled(s.account, false).await.unwrap();
    let account = s.account;
    let events = Arc::clone(&s.events);
    wait_for("disabled", async || {
        events.lock().unwrap().iter().any(|e| {
            matches!(e, CoreEvent::AccountState { account_id, state: StateName::Disabled, .. } if *account_id == account)
        })
    })
    .await;
    let op = s.core.message_archive(&[row.id]).await.unwrap();
    assert!(s.row("INBOX", "queued").await.is_none());
    s.core.message_undo(op).await.unwrap();
    let back = s.row("INBOX", "queued").await.expect("visible again");
    assert!(!back.unread, "flags intact");
    s.core.account_set_enabled(s.account, true).await.unwrap();
    s.until_idle().await;
    assert_eq!(s.calls(Op::Move), 0);
    assert_eq!(s.imap.uids("INBOX").len(), 1);
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t25_gmail_labels_are_rows_and_archive_removes_only_inbox() {
    let imap = FakeServer::new(&[
        "IMAP4rev1",
        "IDLE",
        "MOVE",
        "UIDPLUS",
        "SPECIAL-USE",
        "X-GM-EXT-1",
    ]);
    imap.add_user(USER, PASS);
    imap.add_folder("[Gmail]/All Mail", Some(SpecialUse::All));
    imap.add_folder("[Gmail]/Trash", Some(SpecialUse::Trash));
    imap.add_folder("[Gmail]/Important", None);
    imap.add_folder("[Gmail]/Starred", None);
    imap.set_attributes("[Gmail]/Important", &["\\Important"])
        .unwrap();
    imap.set_attributes("[Gmail]/Starred", &["\\Flagged"])
        .unwrap();
    imap.add_folder("Work", None);
    let m = msg("two labels", 1).with_gmail_ids(77, 77);
    for folder in ["[Gmail]/All Mail", "INBOX", "Work"] {
        imap.deliver(folder, m.clone()).unwrap();
    }
    let s = start(imap).await;
    let inbox_row = s.listed("INBOX", "two labels").await;
    s.listed("Work", "two labels").await;

    // Two rows sharing one gm_msgid (the second handle reads the same database).
    let store = s.dir.store();
    let gm = |id: i64| {
        let store = store.clone();
        async move { store.get_message(id).await.unwrap() }
    };
    let work_row = s.row("Work", "two labels").await.unwrap();
    assert_ne!(inbox_row.id, work_row.id);
    let (a, b) = (gm(inbox_row.id).await, gm(work_row.id).await);
    assert_eq!(a.message_id, b.message_id);

    // Gmail's All Mail, Important and Starred start with sync off; the last two are hidden.
    let folders = s.core.folder_list(s.account).await.unwrap();
    let find = |p: &str| folders.iter().find(|f| f.path == p).unwrap().clone();
    assert!(!find("[Gmail]/All Mail").sync_enabled && !find("[Gmail]/All Mail").hidden);
    assert!(find("[Gmail]/Important").hidden && find("[Gmail]/Starred").hidden);
    assert!(!find("Work").hidden);

    // Archive moves to All Mail: only the INBOX label goes.
    let view = s.core.message_open(inbox_row.id).await.unwrap();
    assert!(view.can_archive);
    s.core.message_archive(&[inbox_row.id]).await.unwrap();
    wait_for("archived", async || s.imap.uids("INBOX").is_empty()).await;
    assert_eq!(s.imap.uids("Work").len(), 1, "the other label stays");
    assert_eq!(
        s.imap.uids("[Gmail]/All Mail").len(),
        1,
        "one message, no copy"
    );
    s.core
        .folder_refresh(s.account, s.folder("Work").await)
        .await
        .unwrap();
    wait_for("INBOX row gone", async || {
        s.row("INBOX", "two labels").await.is_none()
    })
    .await;
    let still = s
        .row("Work", "two labels")
        .await
        .expect("Work keeps its row");
    assert_eq!(still.id, work_row.id);

    // Turning sync on shows a hidden folder.
    s.core
        .folder_set_sync(s.account, "[Gmail]/Starred", true)
        .await
        .unwrap();
    let starred = s
        .core
        .folder_list(s.account)
        .await
        .unwrap()
        .into_iter()
        .find(|f| f.path == "[Gmail]/Starred")
        .unwrap();
    assert!(!starred.hidden);
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_op_is_reverted_and_reported_once() {
    let imap = server(FULL);
    imap.deliver("INBOX", msg("refused", 1)).unwrap();
    let s = start(imap).await;
    let row = s.listed("INBOX", "refused").await;
    s.imap
        .fail_next(Op::Move, ImapError::No("[CANNOT] not allowed".into()));
    let op = s
        .core
        .message_move(&[row.id], s.folder("Work").await)
        .await
        .unwrap();
    let events = Arc::clone(&s.events);
    wait_for("op-failed", async || {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, CoreEvent::OpFailed { op_id, .. } if *op_id == op))
    })
    .await;
    let failed: Vec<CoreEvent> = s
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, CoreEvent::OpFailed { .. }))
        .cloned()
        .collect();
    assert_eq!(failed.len(), 1, "reported once");
    let json = serde_json::to_value(&failed[0]).unwrap();
    assert_eq!(json["kind"], "move");
    assert_eq!(json["error"]["code"], "server_rejected");
    assert!(s.row("INBOX", "refused").await.is_some(), "the row is back");
    assert_eq!(s.imap.uids("INBOX").len(), 1);
    assert_eq!(
        s.core.message_undo(op).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_older_extends_by_90_days_and_jumps_gaps() {
    // Delivered oldest first, so UIDs grow with the date, as on a real server.
    let imap = server(FULL);
    imap.deliver("INBOX", msg("ancient", 400)).unwrap();
    imap.deliver("INBOX", msg("old", 120)).unwrap();
    imap.deliver("INBOX", msg("recent", 10)).unwrap();
    let s = start(imap).await;
    s.listed("INBOX", "recent").await;
    assert_eq!(s.rows("INBOX").await.len(), 1, "the 90-day window");
    let inbox = s.folder("INBOX").await;
    s.core.folder_extend_window(s.account, inbox).await.unwrap();
    s.listed("INBOX", "old").await;
    assert!(
        s.row("INBOX", "ancient").await.is_none(),
        "90 days at a time"
    );
    s.core.folder_extend_window(s.account, inbox).await.unwrap();
    s.listed("INBOX", "ancient").await;
    // A later sync keeps the older rows (headers are never evicted).
    s.core.folder_refresh(s.account, inbox).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.rows("INBOX").await.len(), 3);
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mark_all_read_empty_trash_and_folder_settings() {
    let imap = server(FULL);
    for n in 0..3 {
        imap.deliver("INBOX", msg(&format!("unread {n}"), 1))
            .unwrap();
        imap.deliver("Trash", msg(&format!("trash {n}"), 1))
            .unwrap();
    }
    let s = start(imap).await;
    s.listed("INBOX", "unread 2").await;
    let inbox = s.folder("INBOX").await;
    s.core.message_mark_all_read(inbox).await.unwrap();
    assert!(s.rows("INBOX").await.iter().all(|r| !r.unread), "instant");
    wait_for("seen on the server", async || {
        s.imap
            .uids("INBOX")
            .iter()
            .all(|u| s.imap.flags("INBOX", *u).unwrap().contains(&Flag::Seen))
    })
    .await;

    // Empty Trash: permanent, everything in it.
    s.listed("Trash", "trash 2").await;
    let trash = s.folder("Trash").await;
    let t = s.rows("Trash").await[0].id;
    assert!(s.core.message_open(t).await.unwrap().delete_is_permanent);
    s.core.message_delete(&[], Some(trash)).await.unwrap();
    assert!(s.rows("Trash").await.is_empty(), "instant");
    wait_for("trash emptied", async || s.imap.uids("Trash").is_empty()).await;

    // Folder settings: role override, sync, watch (at most 3).
    s.core
        .folder_set_role(s.account, "Work", Some(mail_core::views::Role::Archive))
        .await
        .unwrap();
    let folders = s.core.folder_list(s.account).await.unwrap();
    let role = |p: &str| folders.iter().find(|f| f.path == p).unwrap().role;
    assert_eq!(role("Work"), Some(mail_core::views::Role::Archive));
    assert_eq!(role("Archive"), None, "one folder per role");
    for name in ["A", "B", "C", "D"] {
        s.core.folder_create(s.account, name).await.unwrap();
    }
    for name in ["A", "B", "C"] {
        s.core
            .folder_set_watch(s.account, name, true)
            .await
            .unwrap();
    }
    assert_eq!(
        s.core
            .folder_set_watch(s.account, "D", true)
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    s.core.folder_set_sync(s.account, "D", false).await.unwrap();
    let d = s
        .core
        .folder_list(s.account)
        .await
        .unwrap()
        .into_iter()
        .find(|f| f.path == "D")
        .unwrap();
    assert!(!d.sync_enabled && !d.hidden);
    s.core.shutdown().await;
}
