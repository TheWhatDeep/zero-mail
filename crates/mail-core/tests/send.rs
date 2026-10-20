//! M7 on the fakes: T10, T11, T12, T13, T15, T16, T17 and T18, plus the Answered flag,
//! contacts and identities. T14's schedule is in mail-outbox's tests (an explicit clock).
#![cfg(test)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{config, FakeHttp, TempDir};
use mail_core::compose::{DraftContent, IdentityView, Timing};
use mail_core::views::{AddressView, ListRequest, Scope};
use mail_core::{Core, CoreConfig, CoreEvent, ErrorCode, EventSink, StateName};
use mail_imap::fake::{FakeMessage, FakeServer, Op};
use mail_imap::{Flag, SpecialUse};
use mail_secrets::{MemoryStore, SecretStore};
use mail_smtp::fake::FakeSmtp;
use mail_smtp::{SmtpError, SubmitError};

const USER: &str = "alice@test.local";
const PASS: &str = "alice-pw";

type TestCore = Core<FakeServer, FakeSmtp, FakeHttp>;

struct Setup {
    dir: TempDir,
    imap: FakeServer,
    smtp: FakeSmtp,
    secrets: Arc<dyn SecretStore>,
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

fn server(caps: &[&str]) -> FakeServer {
    let s = FakeServer::new(caps);
    s.add_user(USER, PASS);
    s.add_folder("Sent", Some(SpecialUse::Sent));
    s.add_folder("Drafts", Some(SpecialUse::Drafts));
    s
}

const FULL: &[&str] = &["IMAP4rev1", "IDLE", "MOVE", "UIDPLUS", "SPECIAL-USE"];

/// Sends at once; draft uploads only when a window closes.
fn fast() -> Timing {
    Timing {
        undo_window: Duration::ZERO,
        draft_upload: Duration::from_secs(3600),
        outbox_tick: Duration::from_millis(50),
        ..Timing::default()
    }
}

async fn start(
    dir: TempDir,
    imap: FakeServer,
    smtp: FakeSmtp,
    secrets: Arc<dyn SecretStore>,
    add: bool,
) -> Setup {
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
    core.set_timing(fast());
    let account = if add {
        core.account_add(config(USER, USER, PASS)).await.unwrap().id
    } else {
        core.account_list().await.unwrap()[0].summary.id
    };
    let s = Setup {
        dir,
        imap,
        smtp,
        secrets,
        events,
        core,
        account,
    };
    let (events, account) = (Arc::clone(&s.events), s.account);
    wait_for("idle", || {
        events.lock().unwrap().iter().any(|e| {
            matches!(e, CoreEvent::AccountState { account_id, state: StateName::Idle, .. } if *account_id == account)
        })
    })
    .await;
    s
}

async fn fresh(caps: &[&str]) -> Setup {
    let smtp = FakeSmtp::default();
    smtp.add_user(USER, PASS);
    start(
        TempDir::new("send"),
        server(caps),
        smtp,
        Arc::new(MemoryStore::default()),
        true,
    )
    .await
}

async fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn addr(name: &str, a: &str) -> AddressView {
    AddressView {
        name: name.into(),
        addr: a.into(),
    }
}

impl Setup {
    /// A new draft with these recipients, saved, and its identity.
    async fn draft(&self, to: &[&str], cc: &[&str], bcc: &[&str], subject: &str) -> (i64, i64) {
        let view = self.core.compose_new(self.account, None).await.unwrap();
        let identity = view.identities[0].id.unwrap();
        let content = DraftContent {
            to: to.iter().map(|a| addr("", a)).collect(),
            cc: cc.iter().map(|a| addr("", a)).collect(),
            bcc: bcc.iter().map(|a| addr("Hidden", a)).collect(),
            subject: subject.into(),
            html: "<p>Hello <b>there</b></p>".into(),
            text: "Hello there".into(),
            ..view.content
        };
        self.core.draft_save(view.draft_id, content).await.unwrap();
        (view.draft_id, identity)
    }

    async fn outbox_state(&self, id: i64) -> Option<String> {
        self.core
            .outbox_list(self.account)
            .await
            .unwrap()
            .into_iter()
            .find(|o| o.id == id)
            .map(|o| o.state.to_string())
    }

    async fn wait_state(&self, id: i64, state: &str) {
        let start = Instant::now();
        loop {
            if self.outbox_state(id).await.as_deref() == Some(state) {
                return;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "never {state}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t10_to_cc_and_bcc_each_once_and_no_bcc_header() {
    let s = fresh(FULL).await;
    let (draft, identity) = s
        .draft(
            &["bob@test.local"],
            &["carol@test.local"],
            &["dave@test.local"],
            "Plans",
        )
        .await;
    let id = s.core.draft_send(draft, identity, None).await.unwrap();
    s.wait_state(id, "sent").await;
    let deliveries = s.smtp.deliveries();
    assert_eq!(deliveries.len(), 1, "one session, every recipient once");
    assert_eq!(
        deliveries[0].envelope.to,
        vec!["bob@test.local", "carol@test.local", "dave@test.local"]
    );
    assert_eq!(deliveries[0].envelope.from, USER);
    let raw = String::from_utf8(deliveries[0].raw.clone()).unwrap();
    assert!(!raw.to_ascii_lowercase().contains("bcc"), "no Bcc header");
    assert!(
        !raw.contains("dave@test.local"),
        "the Bcc address is invisible"
    );
    assert!(raw.contains("To: bob@test.local") && raw.contains("Cc: carol@test.local"));
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t11_one_sent_copy_and_none_on_gmail() {
    let s = fresh(FULL).await;
    let (draft, identity) = s.draft(&["bob@test.local"], &[], &[], "Copy").await;
    let id = s.core.draft_send(draft, identity, None).await.unwrap();
    s.wait_state(id, "sent").await;
    wait_for("Sent copy", || s.imap.uids("Sent").len() == 1).await;
    let uid = s.imap.uids("Sent")[0];
    assert!(s.imap.flags("Sent", uid).unwrap().contains(&Flag::Seen));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.imap.uids("Sent").len(), 1, "exactly one");
    s.core.shutdown().await;

    let mut caps = FULL.to_vec();
    caps.push("X-GM-EXT-1");
    let s = fresh(&caps).await;
    let (draft, identity) = s.draft(&["bob@test.local"], &[], &[], "Gmail").await;
    let id = s.core.draft_send(draft, identity, None).await.unwrap();
    s.wait_state(id, "sent").await;
    let events = Arc::clone(&s.events);
    wait_for("pending 0", || {
        events.lock().unwrap().iter().rev().find_map(|e| match e {
            CoreEvent::PendingCount { total } => Some(*total),
            _ => None,
        }) == Some(0)
    })
    .await;
    assert!(s.imap.uids("Sent").is_empty(), "Gmail files its own copy");
    assert!(!s
        .imap
        .calls()
        .iter()
        .any(|c| c.op == Op::Append && c.folder.as_deref() == Some("Sent")));
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t12_undo_at_5_s_of_a_10_s_window() {
    let s = fresh(FULL).await;
    s.core.set_timing(Timing {
        undo_window: Duration::from_secs(10),
        ..fast()
    });
    let (draft, identity) = s.draft(&["bob@test.local"], &[], &[], "Undo me").await;
    let id = s.core.draft_send(draft, identity, None).await.unwrap();
    assert_eq!(s.outbox_state(id).await.as_deref(), Some("queued"));
    tokio::time::sleep(Duration::from_secs(5)).await;
    s.core.outbox_undo(id).await.unwrap();
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(s.smtp.submits(), 0, "nothing reached SMTP");
    assert!(s.outbox_state(id).await.is_none());
    let restored = s.core.draft_view(draft).await.unwrap();
    assert_eq!(restored.content.subject, "Undo me", "the draft is restored");
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t13_a_crash_after_data_is_unknown_and_never_resent() {
    let s = fresh(FULL).await;
    s.smtp.hang_after_data();
    let (draft, identity) = s.draft(&["bob@test.local"], &[], &[], "Crash").await;
    let id = s.core.draft_send(draft, identity, None).await.unwrap();
    let smtp = s.smtp.clone();
    wait_for("DATA sent", || smtp.data_started() == 1).await;
    // The app dies while the answer to DATA is outstanding.
    s.core.shutdown().await;
    let (dir, imap, smtp, secrets) = (s.dir, s.imap, s.smtp, s.secrets);
    drop(s.core);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let again = start(dir, imap, smtp, secrets, false).await;
    assert_eq!(again.outbox_state(id).await.as_deref(), Some("unknown"));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(again.smtp.submits(), 1, "nothing resends");
    assert!(again.smtp.deliveries().is_empty());
    // The user decides: Retry sends it once.
    again.core.outbox_retry(id).await.unwrap();
    again.wait_state(id, "sent").await;
    assert_eq!(again.smtp.deliveries().len(), 1);
    again.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t15_a_refused_recipient_fails_before_data() {
    let s = fresh(FULL).await;
    s.smtp.reject_recipient("nobody@test.local");
    let (draft, identity) = s
        .draft(
            &["bob@test.local", "nobody@test.local"],
            &[],
            &[],
            "Refused",
        )
        .await;
    let id = s.core.draft_send(draft, identity, None).await.unwrap();
    s.wait_state(id, "failed").await;
    assert_eq!(s.smtp.data_started(), 0, "no DATA");
    let item = s
        .core
        .outbox_list(s.account)
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.id == id)
        .unwrap();
    let error = item.error.unwrap();
    assert_eq!(error["code"], "recipient_rejected");
    assert_eq!(error["addresses"][0], "nobody@test.local");
    // A transient failure waits to retry instead (T14's first step).
    s.smtp
        .fail_next_submit(SubmitError::Transient(SmtpError::Unavailable("451".into())));
    let (draft, identity) = s.draft(&["bob@test.local"], &[], &[], "Later").await;
    let id = s.core.draft_send(draft, identity, None).await.unwrap();
    s.wait_state(id, "retry_wait").await;
    let item = s
        .core
        .outbox_list(s.account)
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.id == id)
        .unwrap();
    assert!(item.send_after_utc >= now() + 25, "about 30 s later");
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t16_over_the_size_limit_is_blocked_before_queueing() {
    let s = fresh(FULL).await;
    s.smtp.set_size_limit(Some(2_000));
    let (draft, identity) = s.draft(&["bob@test.local"], &[], &[], "Big").await;
    let file = s.dir.path().join("big.bin");
    std::fs::write(&file, vec![7u8; 10_000]).unwrap();
    let content = s.core.draft_add_attachment(draft, file).await.unwrap();
    assert_eq!(content.attachments.len(), 1);
    assert_eq!(content.attachments[0].size, 10_000);
    let err = s.core.draft_send(draft, identity, None).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::TooLarge);
    assert!(err.message.contains("2000"), "{}", err.message);
    assert!(
        s.core.outbox_list(s.account).await.unwrap().is_empty(),
        "nothing queued"
    );
    assert_eq!(s.smtp.submits(), 0);
    // Removing it makes the message small enough.
    let content = s
        .core
        .draft_remove_attachment(draft, &content.attachments[0].id)
        .await
        .unwrap();
    assert!(content.attachments.is_empty());
    s.core.draft_send(draft, identity, None).await.unwrap();
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t17_reply_all_excludes_own_addresses_and_threads_correctly() {
    let imap = server(FULL);
    let raw = format!(
        "Message-ID: <orig@x.test>\r\nReferences: <root@x.test>\r\nFrom: Bob <bob@test.local>\r\n\
         To: Me <{USER}>, Alias <alias@test.local>, Carol <carol@test.local>\r\n\
         Cc: Dave <dave@test.local>, BOB@test.local\r\nSubject: Re: Plans\r\n\r\nSee you there.\r\n"
    );
    imap.deliver("INBOX", FakeMessage::new(raw, now() - 60))
        .unwrap();
    let smtp = FakeSmtp::default();
    smtp.add_user(USER, PASS);
    let s = start(
        TempDir::new("t17"),
        imap,
        smtp,
        Arc::new(MemoryStore::default()),
        true,
    )
    .await;
    s.core
        .identity_save(IdentityView {
            id: None,
            account_id: s.account,
            name: "Alias".into(),
            address: "alias@test.local".into(),
            reply_to: None,
            is_default: false,
        })
        .await
        .unwrap();
    let inbox = s.core.folder_list(s.account).await.unwrap()[0].id;
    let message = loop {
        let page = s
            .core
            .list_page(ListRequest {
                scope: Scope::Folder { folder_id: inbox },
                cursor: None,
                limit: 10,
                group_by_thread: false,
            })
            .await
            .unwrap();
        if let Some(r) = page.rows.first() {
            break r.id;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let view = s.core.compose_reply(message, true).await.unwrap();
    let c = &view.content;
    let to: Vec<&str> = c.to.iter().map(|a| a.addr.as_str()).collect();
    let cc: Vec<&str> = c.cc.iter().map(|a| a.addr.as_str()).collect();
    assert_eq!(to, vec!["bob@test.local", "carol@test.local"]);
    assert_eq!(
        cc,
        vec!["dave@test.local"],
        "Bob only once, own addresses gone"
    );
    assert_eq!(c.subject, "Re: Plans", "prefixes never stack");
    assert_eq!(c.in_reply_to.as_deref(), Some("orig@x.test"));
    assert_eq!(c.references, vec!["root@x.test", "orig@x.test"]);
    assert_eq!(c.mode, "reply_all");
    let quote = c.quote.as_ref().unwrap();
    assert_eq!(quote.from.addr, "bob@test.local");
    // The identity the message was sent to answers.
    let alias = view
        .identities
        .iter()
        .find(|i| i.address == "alias@test.local")
        .unwrap();
    assert_ne!(
        c.identity_id, alias.id,
        "the account address comes first in To"
    );
    let reply = s.core.compose_reply(message, false).await.unwrap();
    assert_eq!(
        reply
            .content
            .to
            .iter()
            .map(|a| a.addr.as_str())
            .collect::<Vec<_>>(),
        vec!["bob@test.local"]
    );
    assert!(reply.content.cc.is_empty());

    // Sending the reply marks the original Answered.
    let id = s
        .core
        .draft_send(reply.draft_id, reply.content.identity_id.unwrap(), None)
        .await
        .unwrap();
    s.wait_state(id, "sent").await;
    let uid = s.imap.uids("INBOX")[0];
    wait_for("Answered", || {
        s.imap
            .flags("INBOX", uid)
            .unwrap()
            .contains(&Flag::Answered)
    })
    .await;
    let sent = String::from_utf8(s.smtp.deliveries()[0].raw.clone()).unwrap();
    assert!(sent.contains("In-Reply-To: <orig@x.test>"));
    assert!(sent.contains("References: <root@x.test> <orig@x.test>"));
    // Recipients are contacts now.
    let suggested = s.core.contacts_suggest("bo", s.account).await.unwrap();
    assert_eq!(suggested[0].addr, "bob@test.local");
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t18_one_draft_on_the_server_at_all_times() {
    for caps in [FULL, &["IMAP4rev1", "IDLE", "MOVE", "SPECIAL-USE"][..]] {
        let s = fresh(caps).await;
        let (draft, _) = s.draft(&["bob@test.local"], &[], &[], "Draft v1").await;
        s.core.draft_close(draft).await.unwrap();
        wait_for("first upload", || s.imap.uids("Drafts").len() == 1).await;
        let first = s.imap.uids("Drafts")[0];
        assert!(s
            .imap
            .flags("Drafts", first)
            .unwrap()
            .contains(&Flag::Draft));
        // Edit again: the new version replaces the old one.
        let mut view = s.core.draft_view(draft).await.unwrap();
        view.content.subject = "Draft v2".into();
        s.core.draft_save(draft, view.content).await.unwrap();
        s.core.draft_close(draft).await.unwrap();
        wait_for("second upload", || {
            let uids = s.imap.uids("Drafts");
            uids.len() == 1 && uids[0] != first
        })
        .await;
        assert!(
            !s.imap.uids("Drafts").contains(&first),
            "old UID removed ({caps:?})"
        );
        // Discarding removes the uploaded copy.
        s.core.draft_discard(draft).await.unwrap();
        wait_for("discarded", || s.imap.uids("Drafts").is_empty()).await;
        assert!(s.core.draft_view(draft).await.is_err());
        s.core.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sent_draft_leaves_no_copy_in_drafts_and_identities_keep_a_default() {
    let s = fresh(FULL).await;
    let (draft, identity) = s.draft(&["bob@test.local"], &[], &[], "Sent").await;
    s.core.draft_close(draft).await.unwrap();
    wait_for("uploaded", || s.imap.uids("Drafts").len() == 1).await;
    let id = s.core.draft_send(draft, identity, None).await.unwrap();
    s.wait_state(id, "sent").await;
    wait_for("draft removed", || s.imap.uids("Drafts").is_empty()).await;

    let list = s.core.identity_list(s.account).await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].is_default);
    let err = s
        .core
        .identity_delete(list[0].id.unwrap())
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "never the last one");
    let list = s
        .core
        .identity_save(IdentityView {
            id: None,
            account_id: s.account,
            name: "Work".into(),
            address: "work@test.local".into(),
            reply_to: Some("replies@test.local".into()),
            is_default: true,
        })
        .await
        .unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list.iter().filter(|i| i.is_default).count(), 1);
    assert!(
        list.iter()
            .find(|i| i.address == "work@test.local")
            .unwrap()
            .is_default
    );
    assert!(s
        .core
        .identity_save(IdentityView {
            id: None,
            account_id: s.account,
            name: "Bad".into(),
            address: "not an address".into(),
            reply_to: None,
            is_default: false,
        })
        .await
        .is_err());
    s.core.shutdown().await;
}
