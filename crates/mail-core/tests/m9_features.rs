//! M9 on the fakes: labels (T36), snooze (T37), send later (T38), mute (T39), rules (T40)
//! and templates (T41).
#![cfg(test)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{config, FakeHttp, TempDir};
use mail_core::views::{ListRequest, RowView, Scope};
use mail_core::{Core, CoreConfig, CoreEvent, ErrorCode, EventSink, StateName};
use mail_imap::fake::{FakeMessage, FakeServer, Op};
use mail_imap::SpecialUse;
use mail_secrets::{MemoryStore, SecretStore};
use mail_smtp::fake::FakeSmtp;

const USER: &str = "alice@test.local";
const PASS: &str = "alice-pw";
const DAY: i64 = 86_400;

type TestCore = Core<FakeServer, FakeSmtp, FakeHttp>;

struct Setup {
    dir: TempDir,
    imap: FakeServer,
    smtp: FakeSmtp,
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

fn msg(id: &str, subject: &str, extra: &str) -> FakeMessage {
    FakeMessage::mime(
        format!(
            "Message-ID: <{id}>\r\nFrom: Bob <bob@test.local>\r\nTo: {USER}\r\nSubject: {subject}\r\n\
             {extra}MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBody of {subject}\r\n"
        ),
        now() - 60,
    )
}

const FULL: &[&str] = &["IMAP4rev1", "IDLE", "MOVE", "UIDPLUS", "SPECIAL-USE"];

fn gmail_server() -> FakeServer {
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
    imap
}

async fn start_in(
    dir: TempDir,
    imap: FakeServer,
    secrets: Arc<dyn SecretStore>,
    add: bool,
) -> Setup {
    let smtp = FakeSmtp::default();
    smtp.add_user(USER, PASS);
    let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
    let log = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |e| log.lock().unwrap().push(e));
    let core = Core::start(
        CoreConfig {
            data_dir: dir.path(),
            secrets: Arc::clone(&secrets),
            imap: imap.clone(),
            smtp: smtp.clone(),
            http: FakeHttp::default(),
        },
        sink,
    )
    .await
    .unwrap();
    let account = if add {
        core.account_add(config(USER, USER, PASS)).await.unwrap().id
    } else {
        core.account_list().await.unwrap()[0].summary.id
    };
    let s = Setup {
        dir,
        imap,
        smtp,
        events,
        core,
        account,
    };
    s.until_idle().await;
    s
}

async fn start(imap: FakeServer) -> Setup {
    start_in(
        TempDir::new("m9f"),
        imap,
        Arc::new(MemoryStore::default()),
        true,
    )
    .await
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

    async fn listed(&self, path: &str, subject: &str) -> RowView {
        let mut found = None;
        wait_for(&format!("{subject} in {path}"), async || {
            found = self.row(path, subject).await;
            found.is_some()
        })
        .await;
        found.unwrap()
    }

    fn calls(&self, op: Op) -> Vec<mail_imap::fake::Call> {
        self.imap
            .calls()
            .into_iter()
            .filter(|c| c.op == op)
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t36_gmail_add_then_remove_a_label() {
    let imap = gmail_server();
    imap.add_folder("Work", None);
    let m = msg("label@t", "Labelled", "").with_gmail_ids(501, 501);
    for f in ["[Gmail]/All Mail", "INBOX"] {
        imap.deliver(f, m.clone()).unwrap();
    }
    let s = start(imap).await;
    let row = s.listed("INBOX", "Labelled").await;
    let work = s.folder("Work").await;
    let view = s.core.message_open(row.id).await.unwrap();
    let names = |v: &mail_core::views::MessageView| -> Vec<String> {
        v.labels
            .as_ref()
            .unwrap()
            .iter()
            .map(|l| l.name.clone())
            .collect()
    };
    assert_eq!(names(&view), vec!["Inbox"]);

    // Add: UID COPY from INBOX to Work; the row in INBOX stays.
    s.imap.clear_calls();
    s.core.message_add_label(&[row.id], work).await.unwrap();
    s.listed("Work", "Labelled").await;
    let copies = s.calls(Op::Copy);
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].arg, "Work");
    assert_eq!(s.imap.uids("INBOX").len(), 1);
    assert_eq!(
        names(&s.core.message_open(row.id).await.unwrap()),
        vec!["Inbox", "Work"]
    );
    let store = s.dir.store();
    let work_uid = s.imap.uids("Work")[0];
    assert_eq!(
        store.copies(row.id).await.unwrap().len(),
        2,
        "two rows, one X-GM-MSGID"
    );

    // Remove: \Deleted and UID EXPUNGE of that UID only, in Work.
    s.imap.clear_calls();
    s.core.message_remove_label(&[row.id], work).await.unwrap();
    wait_for("label gone", async || s.imap.uids("Work").is_empty()).await;
    let expunges = s.calls(Op::UidExpunge);
    assert_eq!(expunges.len(), 1);
    assert_eq!(expunges[0].folder.as_deref(), Some("Work"));
    assert_eq!(expunges[0].uids, vec![work_uid], "that UID only");
    assert!(s.calls(Op::Expunge).is_empty(), "never a bare EXPUNGE");
    assert_eq!(s.imap.uids("INBOX").len(), 1, "INBOX untouched");
    wait_for("row gone", async || {
        s.row("Work", "Labelled").await.is_none()
    })
    .await;
    assert_eq!(
        names(&s.core.message_open(row.id).await.unwrap()),
        vec!["Inbox"]
    );

    // The last label cannot be removed (it would delete the message).
    let inbox = s.folder("INBOX").await;
    let only = s.core.message_remove_label(&[row.id], inbox).await;
    assert_eq!(only.unwrap_err().code, ErrorCode::InvalidInput);
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t36_label_undo_and_no_labels_outside_gmail() {
    let imap = gmail_server();
    imap.add_folder("Work", None);
    let m = msg("undo@t", "Undo label", "").with_gmail_ids(601, 601);
    for f in ["[Gmail]/All Mail", "INBOX"] {
        imap.deliver(f, m.clone()).unwrap();
    }
    let s = start(imap).await;
    let row = s.listed("INBOX", "Undo label").await;
    let work = s.folder("Work").await;
    let op = s.core.message_add_label(&[row.id], work).await.unwrap();
    wait_for("copied", async || s.imap.uids("Work").len() == 1).await;
    s.core.message_undo(op).await.unwrap();
    wait_for("undone", async || s.imap.uids("Work").is_empty()).await;
    assert_eq!(s.imap.uids("INBOX").len(), 1);
    s.core.shutdown().await;

    // A server without X-GM-EXT-1: no labels in the view, and the commands refuse.
    let plain = FakeServer::new(FULL);
    plain.add_user(USER, PASS);
    plain.add_folder("Work", None);
    plain.deliver("INBOX", msg("p@t", "Plain", "")).unwrap();
    let s = start(plain).await;
    let row = s.listed("INBOX", "Plain").await;
    assert_eq!(s.core.message_open(row.id).await.unwrap().labels, None);
    let work = s.folder("Work").await;
    assert_eq!(
        s.core
            .message_add_label(&[row.id], work)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
    let _ = DAY;
    s.core.shutdown().await;
}

fn fast() -> mail_core::compose::Timing {
    mail_core::compose::Timing {
        undo_window: Duration::ZERO,
        draft_upload: Duration::from_secs(3600),
        outbox_tick: Duration::from_millis(50),
        late_after: Duration::from_secs(1),
        snooze_tick: Duration::from_millis(200),
    }
}

fn snooze_due(s: &Setup) -> usize {
    s.events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, CoreEvent::SnoozeDue { .. }))
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t37_snooze_survives_a_restart_and_comes_back_unread() {
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    imap.deliver(
        "INBOX",
        msg("snz@t", "Later please", "").with_flags([mail_imap::Flag::Seen]),
    )
    .unwrap();
    let dir = TempDir::new("snooze");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let s = start_in(dir, imap.clone(), Arc::clone(&secrets), true).await;
    s.core.set_timing(fast());
    let row = s.listed("INBOX", "Later please").await;
    assert!(!row.unread);
    let due = now() + 3;
    s.core.message_snooze(&[row.id], due).await.unwrap();
    // The Snoozed folder is created on first use and the message moves there.
    wait_for("moved", async || s.imap.uids("Snoozed").len() == 1).await;
    assert!(s.imap.uids("INBOX").is_empty());
    s.listed("Snoozed", "Later please").await;
    // Killed before it fires.
    s.core.shutdown().await;
    let dir = s.dir;
    drop(s.core);
    while now() <= due {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let restarted = Instant::now();
    let s = start_in(dir, imap, secrets, false).await;
    s.core.set_timing(fast());
    wait_for("back in INBOX", async || s.imap.uids("INBOX").len() == 1).await;
    assert!(
        restarted.elapsed() < Duration::from_secs(10),
        "within 10 s of the start"
    );
    let uid = s.imap.uids("INBOX")[0];
    let flags = s.imap.flags("INBOX", uid).unwrap();
    assert!(!flags.contains(&mail_imap::Flag::Seen), "unread: {flags:?}");
    assert!(s.imap.uids("Snoozed").is_empty());
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(snooze_due(&s), 1, "one notification");
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn snooze_undo_and_a_reply_ends_it_early() {
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    imap.deliver("INBOX", msg("root@t", "Thread", "")).unwrap();
    let s = start(imap).await;
    s.core.set_timing(fast());
    let row = s.listed("INBOX", "Thread").await;
    // Undo: the message stays (or returns) and is no longer snoozed.
    let op = s
        .core
        .message_snooze(&[row.id], now() + 3600)
        .await
        .unwrap();
    s.core.message_undo(op).await.unwrap();
    wait_for("still in INBOX", async || {
        s.row("INBOX", "Thread").await.is_some()
    })
    .await;
    wait_for("settled", async || s.imap.uids("INBOX").len() == 1).await;
    // Snooze again, then a reply arrives: the snooze ends early (the setting's default).
    let row = s.listed("INBOX", "Thread").await;
    s.core
        .message_snooze(&[row.id], now() + 3600)
        .await
        .unwrap();
    wait_for("snoozed", async || s.imap.uids("Snoozed").len() == 1).await;
    s.imap
        .deliver(
            "INBOX",
            msg(
                "reply@t",
                "Re: Thread",
                "In-Reply-To: <root@t>\r\nReferences: <root@t>\r\n",
            ),
        )
        .unwrap();
    wait_for("woken by the reply", async || {
        s.imap.uids("INBOX").len() == 2 && s.imap.uids("Snoozed").is_empty()
    })
    .await;
    assert_eq!(snooze_due(&s), 0, "ended early, not due");
    s.core.shutdown().await;
}

async fn schedule(core: &Arc<TestCore>, account: i64, subject: &str, at: i64) -> i64 {
    let view = core.compose_new(account, None).await.unwrap();
    let identity = view.identities[0].id.unwrap();
    let content = mail_core::compose::DraftContent {
        to: vec![mail_core::views::AddressView {
            name: String::new(),
            addr: "bob@test.local".into(),
        }],
        subject: subject.into(),
        html: "<p>Hi</p>".into(),
        text: "Hi".into(),
        ..view.content
    };
    core.draft_save(view.draft_id, content).await.unwrap();
    core.draft_send(view.draft_id, identity, Some(at))
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t38_send_later_cancel_fire_and_late_after_restart() {
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    imap.add_folder("Sent", Some(SpecialUse::Sent));
    let dir = TempDir::new("later");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let s = start_in(dir, imap.clone(), Arc::clone(&secrets), true).await;
    s.core.set_timing(fast());
    let t = now();
    let fired = schedule(&s.core, s.account, "Fires", t + 2).await;
    let cancelled = schedule(&s.core, s.account, "Cancelled", t + 2).await;
    let third = schedule(&s.core, s.account, "After restart", t + 4).await;
    let list = s.core.outbox_list(s.account).await.unwrap();
    assert!(list.iter().all(|o| o.state == "queued"));
    // Cancel is Undo back to the draft (G77).
    s.core.outbox_undo(cancelled).await.unwrap();
    wait_for("fired", async || s.smtp.deliveries().len() == 1).await;
    s.core.shutdown().await;
    let dir = s.dir;
    let first_smtp = s.smtp.clone();
    drop(s.core);
    // Closed until well past the third one's time.
    while now() <= t + 6 {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let s2 = start_in(dir, imap, secrets, false).await;
    s2.core.set_timing(fast());
    // A fresh fake SMTP in this start: the deliveries are counted across both.
    wait_for("third sent", async || s2.smtp.deliveries().len() == 1).await;
    let delivered: Vec<String> = first_smtp
        .deliveries()
        .iter()
        .chain(s2.smtp.deliveries().iter())
        .map(|d| String::from_utf8_lossy(&d.raw).to_string())
        .collect();
    assert_eq!(delivered.len(), 2);
    assert!(delivered[0].contains("Subject: Fires"));
    assert!(delivered[1].contains("Subject: After restart"));
    assert!(delivered.iter().all(|d| !d.contains("Cancelled")));
    let list = s2.core.outbox_list(s2.account).await.unwrap();
    let find = |id| list.iter().find(|o| o.id == id).unwrap();
    assert_eq!((find(fired).state, find(fired).late), ("sent", false));
    assert_eq!(
        (find(third).state, find(third).late),
        ("sent", true),
        "Sent late"
    );
    assert!(list.iter().all(|o| o.id != cancelled));
    s2.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t38_send_now_on_a_scheduled_item() {
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    let s = start(imap).await;
    s.core.set_timing(fast());
    let id = schedule(&s.core, s.account, "Tomorrow", now() + 86_400).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(s.smtp.deliveries().is_empty());
    s.core.outbox_retry(id).await.unwrap();
    wait_for("sent now", async || s.smtp.deliveries().len() == 1).await;
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t39_mute_archives_replies_and_sets_muted_where_allowed() {
    for keywords in [true, false] {
        let imap = FakeServer::new(FULL);
        imap.add_user(USER, PASS);
        imap.add_folder("Archive", Some(SpecialUse::Archive));
        imap.set_new_keywords_allowed(keywords);
        imap.deliver("INBOX", msg("m-root@t", "Noisy", "")).unwrap();
        let s = start(imap).await;
        let row = s.listed("INBOX", "Noisy").await;
        s.core.message_mute(row.thread_id).await.unwrap();
        let muted = || {
            s.imap
                .flags("INBOX", 1)
                .unwrap()
                .contains(&mail_imap::Flag::Keyword("$Muted".into()))
        };
        // $Muted where PERMANENTFLAGS include \*, nowhere else.
        if keywords {
            wait_for("$Muted", async || muted()).await;
        } else {
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert!(!muted());
        }
        assert_eq!(s.imap.uids("INBOX").len(), 1, "what is there stays");
        s.imap
            .deliver(
                "INBOX",
                msg(
                    "m-reply@t",
                    "Re: Noisy",
                    "In-Reply-To: <m-root@t>\r\nReferences: <m-root@t>\r\n",
                ),
            )
            .unwrap();
        wait_for("reply archived", async || s.imap.uids("Archive").len() == 1).await;
        assert_eq!(s.imap.uids("INBOX").len(), 1);
        s.core.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t39_a_muted_keyword_from_another_install_mutes_here() {
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    imap.add_folder("Archive", Some(SpecialUse::Archive));
    let s = start(imap).await;
    s.imap
        .deliver(
            "INBOX",
            msg("o-root@t", "Muted elsewhere", "")
                .with_flags([mail_imap::Flag::Keyword("$Muted".into())]),
        )
        .unwrap();
    // It arrives in a thread muted elsewhere, so it is archived like any new message there.
    wait_for("archived on arrival", async || {
        s.imap.uids("Archive").len() == 1
    })
    .await;
    s.imap
        .deliver(
            "INBOX",
            msg(
                "o-reply@t",
                "Re: Muted elsewhere",
                "References: <o-root@t>\r\n",
            ),
        )
        .unwrap();
    wait_for("reply archived", async || s.imap.uids("Archive").len() == 2).await;
    assert!(s.imap.uids("INBOX").is_empty());
    s.core.shutdown().await;
}

fn rule(
    name: &str,
    field: mail_core::features::Field,
    value: &str,
    actions: Vec<mail_core::features::Action>,
    stop: bool,
) -> mail_core::features::RuleView {
    use mail_core::features::{Condition, Conditions, Match, RuleView};
    RuleView {
        id: 0,
        account_id: None,
        name: name.into(),
        enabled: false,
        conditions: Conditions {
            match_: Match::All,
            items: vec![Condition {
                field,
                value: value.into(),
            }],
        },
        actions,
        stop_processing: stop,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t40_three_rules_stop_processing_1000_messages_and_a_move_back() {
    use mail_core::features::{Action, Field};
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    imap.add_folder("Deals", None);
    imap.add_folder("Other", None);
    let s = start(imap).await;
    s.core
        .rules_save(rule(
            "Star bulk",
            Field::From,
            "bulk@",
            vec![Action::Star],
            false,
        ))
        .await
        .unwrap();
    let deals = Action::Move {
        folder: "Deals".into(),
    };
    s.core
        .rules_save(rule("Deals", Field::Subject, "deal", vec![deals], true))
        .await
        .unwrap();
    let other = Action::Move {
        folder: "Other".into(),
    };
    let rules = s
        .core
        .rules_save(rule(
            "Everything bulk",
            Field::From,
            "bulk@",
            vec![other],
            false,
        ))
        .await
        .unwrap();
    assert_eq!(
        rules.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        vec!["Star bulk", "Deals", "Everything bulk"]
    );
    // 1,000 matches arrive while the rules are off; then they are turned on and run.
    for i in 0..1000 {
        s.imap
            .deliver(
                "INBOX",
                FakeMessage::mime(
                    format!(
                        "Message-ID: <deal{i}@t>\r\nFrom: Shop <bulk@shop.test>\r\nTo: {USER}\r\n\
                         Subject: Deal {i}\r\n\r\nBuy\r\n"
                    ),
                    now() - 60,
                ),
            )
            .unwrap();
    }
    wait_for("1000 cached", async || {
        s.core
            .folder_list(s.account)
            .await
            .unwrap()
            .iter()
            .any(|f| f.path == "INBOX" && f.total == 1000)
    })
    .await;
    for mut r in rules {
        r.enabled = true;
        s.core.rules_save(r).await.unwrap();
    }
    let started = Instant::now();
    s.core.rules_run_now().await.unwrap();
    let per_message = started.elapsed() / 1000;
    println!("T40: {per_message:?} per message");
    assert!(
        per_message < Duration::from_millis(1),
        "{per_message:?} per message"
    );

    wait_for("filed", async || s.imap.uids("Deals").len() == 1000).await;
    assert!(s.imap.uids("Other").is_empty(), "the third rule never ran");
    let uid = s.imap.uids("Deals")[0];
    assert!(s
        .imap
        .flags("Deals", uid)
        .unwrap()
        .contains(&mail_imap::Flag::Flagged));
    // Running again changes nothing: once per message.
    s.core.rules_run_now().await.unwrap();

    // The user moves one back to INBOX: it is not filed again.
    let row = s.listed("Deals", "Deal 7").await;
    let inbox = s.folder("INBOX").await;
    s.core.message_move(&[row.id], inbox).await.unwrap();
    s.listed("INBOX", "Deal 7").await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(s.imap.uids("INBOX").len(), 1, "not re-filed");
    assert!(s.imap.uids("Other").is_empty());

    // Block sender: a preset rule; unblock deletes it.
    let blocked = s
        .core
        .rules_save(mail_core::features::block_rule("spam@x.test"))
        .await
        .unwrap();
    let block = blocked
        .iter()
        .find(|r| r.name == "Block spam@x.test")
        .unwrap();
    assert_eq!(block.actions, vec![Action::Delete]);
    assert!(block.stop_processing);
    let left = s.core.rules_delete(block.id).await.unwrap();
    assert_eq!(left.len(), 3);
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rules_run_on_new_mail_and_wait_for_bodies() {
    use mail_core::features::{Action, Condition, Conditions, Field, Match, RuleView};
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    imap.add_folder("Receipts", None);
    let s = start(imap).await;
    s.core
        .rules_save(RuleView {
            id: 0,
            account_id: Some(s.account),
            name: "Receipts".into(),
            enabled: true,
            conditions: Conditions {
                match_: Match::All,
                items: vec![
                    Condition {
                        field: Field::ListId,
                        value: "orders.shop".into(),
                    },
                    Condition {
                        field: Field::Body,
                        value: "receipt".into(),
                    },
                ],
            },
            actions: vec![Action::Move {
                folder: "Receipts".into(),
            }],
            stop_processing: true,
        })
        .await
        .unwrap();
    // On the list, no "receipt" in the body: stays.
    s.imap
        .deliver(
            "INBOX",
            msg(
                "r1@t",
                "Order 1",
                "List-Id: Shop orders <orders.shop.test>\r\n",
            ),
        )
        .unwrap();
    // Not on the list: stays.
    s.imap.deliver("INBOX", msg("r2@t", "Order 2", "")).unwrap();
    // On the list with "receipt" in the body: filed once its body is prefetched.
    s.imap
        .deliver(
            "INBOX",
            FakeMessage::mime(
                format!(
                    "Message-ID: <r3@t>\r\nFrom: Shop <shop@x.test>\r\nTo: {USER}\r\n\
                     Subject: Order 3\r\nList-Id: <orders.shop.test>\r\n\r\nYour receipt is attached\r\n"
                ),
                now() - 60,
            ),
        )
        .unwrap();
    wait_for("filed after its body", async || {
        s.imap.uids("Receipts").len() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(s.imap.uids("Receipts").len(), 1);
    assert_eq!(s.imap.uids("INBOX").len(), 2);
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t41_templates_keep_an_existing_subject() {
    use mail_core::features::TemplateView;
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    let s = start(imap).await;
    let list = s
        .core
        .templates_save(TemplateView {
            id: 0,
            name: "Thanks".into(),
            subject: "Thank you".into(),
            body_html: "<p>Thanks for <b>everything</b><script>x()</script></p>".into(),
            body_text: "Thanks for everything".into(),
        })
        .await
        .unwrap();
    let t = &list[0];
    assert!(!t.body_html.contains("script"), "sanitized");
    // Insert into a draft that has a subject: the subject stays; the body comes back for
    // the composer to put at the cursor.
    let with_subject = s.core.compose_new(s.account, None).await.unwrap();
    let content = mail_core::compose::DraftContent {
        subject: "Existing".into(),
        ..with_subject.content.clone()
    };
    s.core
        .draft_save(with_subject.draft_id, content)
        .await
        .unwrap();
    let applied = s
        .core
        .draft_apply_template(with_subject.draft_id, t.id)
        .await
        .unwrap();
    assert!(applied.body_html.contains("<b>everything</b>"));
    let store = s.dir.store();
    let saved = store.get_draft(with_subject.draft_id).await.unwrap();
    assert_eq!(saved.content["subject"], "Existing");
    // An empty subject is filled.
    let empty = s.core.compose_new(s.account, None).await.unwrap();
    s.core
        .draft_apply_template(empty.draft_id, t.id)
        .await
        .unwrap();
    let saved = store.get_draft(empty.draft_id).await.unwrap();
    assert_eq!(saved.content["subject"], "Thank you");
    assert!(s.core.templates_delete(t.id).await.unwrap().is_empty());
    s.core.shutdown().await;
}
