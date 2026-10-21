//! M10 on the fakes: notifications (T05 notification part), unsubscribe (T43), the image
//! preview route and print checks (T44, Core part), settings, the image allowlist and
//! Clear body cache.
#![cfg(test)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{config, FakeHttp, TempDir};
use mail_core::features::{Action, Condition, Conditions, Field, Match, RuleView};
use mail_core::unsubscribe::Method;
use mail_core::views::{ListRequest, RowView, Scope};
use mail_core::{Core, CoreConfig, CoreEvent, ErrorCode, EventSink, StateName};
use mail_imap::fake::{FakeMessage, FakeServer};
use mail_imap::SpecialUse;
use mail_secrets::MemoryStore;
use mail_smtp::fake::FakeSmtp;
use serde_json::json;

const USER: &str = "alice@test.local";
const PASS: &str = "alice-pw";
const FULL: &[&str] = &["IMAP4rev1", "IDLE", "MOVE", "UIDPLUS", "SPECIAL-USE"];

type TestCore = Core<FakeServer, FakeSmtp, FakeHttp>;

struct Setup {
    _dir: TempDir,
    imap: FakeServer,
    http: FakeHttp,
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

fn msg_at(id: &str, subject: &str, extra: &str, internal: i64) -> FakeMessage {
    FakeMessage::mime(
        format!(
            "Message-ID: <{id}>\r\nFrom: Bob <bob@test.local>\r\nTo: {USER}\r\nSubject: {subject}\r\n\
             {extra}MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBody of {subject}\r\n"
        ),
        internal,
    )
}

fn msg(id: &str, subject: &str, extra: &str) -> FakeMessage {
    msg_at(id, subject, extra, now() - 60)
}

fn server() -> FakeServer {
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    imap.add_folder("Archive", Some(SpecialUse::Archive));
    imap
}

async fn start(tag: &str, imap: FakeServer) -> Setup {
    let smtp = FakeSmtp::default();
    smtp.add_user(USER, PASS);
    let http = FakeHttp::default();
    let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
    let log = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |e| log.lock().unwrap().push(e));
    let dir = TempDir::new(tag);
    let core = Core::start(
        CoreConfig {
            data_dir: dir.path(),
            secrets: Arc::new(MemoryStore::default()),
            imap: imap.clone(),
            smtp,
            http: http.clone(),
        },
        sink,
    )
    .await
    .unwrap();
    let account = core.account_add(config(USER, USER, PASS)).await.unwrap().id;
    let s = Setup {
        _dir: dir,
        imap,
        http,
        events,
        core,
        account,
    };
    let (events, account) = (Arc::clone(&s.events), s.account);
    wait_for("idle", async || {
        events.lock().unwrap().iter().any(|e| {
            matches!(e, CoreEvent::AccountState { account_id, state: StateName::Idle, .. } if *account_id == account)
        })
    })
    .await;
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

/// (message_id, sender, subject, preview, count) of every `notify` so far.
type Fired = (i64, String, String, Option<String>, u32);

impl Setup {
    async fn rows(&self, path: &str) -> Vec<RowView> {
        let folder_id = self
            .core
            .folder_list(self.account)
            .await
            .unwrap()
            .iter()
            .find(|f| f.path == path)
            .unwrap()
            .id;
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

    async fn listed(&self, path: &str, subject: &str) -> RowView {
        let mut found = None;
        wait_for(&format!("{subject} in {path}"), async || {
            found = self
                .rows(path)
                .await
                .into_iter()
                .find(|r| r.subject == subject);
            found.is_some()
        })
        .await;
        found.unwrap()
    }

    fn notified(&self) -> Vec<Fired> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                CoreEvent::Notify {
                    message_id,
                    sender,
                    subject,
                    preview,
                    count,
                    ..
                } => Some((
                    *message_id,
                    sender.clone(),
                    subject.clone(),
                    preview.clone(),
                    *count,
                )),
                _ => None,
            })
            .collect()
    }

    /// Delivers, waits for the row, then for anything rules and notifications still do.
    async fn arrive(&self, path: &str, m: FakeMessage, subject: &str) -> RowView {
        self.imap.deliver(path, m).unwrap();
        let row = self.listed(path, subject).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        row
    }
}

fn quiet_rule(subject: &str) -> RuleView {
    RuleView {
        id: 0,
        account_id: None,
        name: "Quiet".into(),
        enabled: true,
        conditions: Conditions {
            match_: Match::All,
            items: vec![Condition {
                field: Field::Subject,
                value: subject.into(),
            }],
        },
        actions: vec![Action::NeverNotify],
        stop_processing: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t05_one_new_message_fires_exactly_one_notification() {
    let imap = server();
    imap.deliver("INBOX", msg("first@t", "Already here", ""))
        .unwrap();
    let s = start("t05n", imap).await;
    s.listed("INBOX", "Already here").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(s.notified().is_empty(), "never during a first sync");

    let row = s.arrive("INBOX", msg("new@t", "Hello", ""), "Hello").await;
    let fired = s.notified();
    assert_eq!(
        fired,
        vec![(row.id, "Bob".into(), "Hello".into(), None, 1)],
        "exactly one, sender and subject, no preview by default"
    );
    // The account name comes along for the coalesced text.
    assert!(s.events.lock().unwrap().iter().any(|e| matches!(
        e,
        CoreEvent::Notify { account_name, .. } if account_name == "Test User"
    )));

    // Preview on: the list preview comes along.
    s.core
        .settings_set("notify.preview", json!(true))
        .await
        .unwrap();
    let row = s.arrive("INBOX", msg("p@t", "Peek", ""), "Peek").await;
    let last = s.notified().pop().unwrap();
    assert_eq!(last.0, row.id);
    assert_eq!(last.3.as_deref(), Some("Body of Peek"));
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_does_not_notify() {
    let imap = server();
    imap.deliver("INBOX", msg("root@t", "Noisy", "")).unwrap();
    let s = start("t05q", imap).await;
    let root = s.listed("INBOX", "Noisy").await;

    // Received over 30 minutes ago.
    s.arrive("INBOX", msg_at("old@t", "Old", "", now() - 3600), "Old")
        .await;
    // A rule with Never notify.
    s.core.rules_save(quiet_rule("hush")).await.unwrap();
    s.arrive("INBOX", msg("q@t", "hush now", ""), "hush now")
        .await;
    // Already read.
    let mut seen = msg("seen@t", "Seen", "");
    seen.flags.push(mail_imap::Flag::Seen);
    s.arrive("INBOX", seen, "Seen").await;
    // A muted thread (T39: no notification).
    s.core.message_mute(root.thread_id).await.unwrap();
    s.imap
        .deliver(
            "INBOX",
            msg("re@t", "Re: Noisy", "In-Reply-To: <root@t>\r\n"),
        )
        .unwrap();
    s.listed("Archive", "Re: Noisy").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Notifications off for the account.
    s.core
        .settings_set(&format!("notify.accounts.{}", s.account), json!(false))
        .await
        .unwrap();
    s.arrive("INBOX", msg("off@t", "Off", ""), "Off").await;
    assert!(s.notified().is_empty(), "{:?}", s.notified());

    // Back on: notifies again.
    s.core
        .settings_set(&format!("notify.accounts.{}", s.account), json!(true))
        .await
        .unwrap();
    let row = s.arrive("INBOX", msg("on@t", "On", ""), "On").await;
    assert_eq!(s.notified().len(), 1);
    assert_eq!(s.notified()[0].0, row.id);
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_coalesces() {
    let s = start("t05c", server()).await;
    for i in 0..6 {
        s.imap
            .deliver("INBOX", msg(&format!("b{i}@t"), &format!("Burst {i}"), ""))
            .unwrap();
    }
    s.listed("INBOX", "Burst 5").await;
    // Everything is accounted for within one window plus the flush.
    wait_for("all six counted", async || {
        s.notified().iter().map(|n| n.4).sum::<u32>() == 6
    })
    .await;
    let fired = s.notified();
    let singles = fired.iter().filter(|n| n.4 == 1).count();
    assert!(singles <= 3, "at most 3 singles: {fired:?}");
    assert!(
        fired.iter().any(|n| n.4 > 1),
        "the rest as N new messages: {fired:?}"
    );
    assert!(fired.len() <= 4, "{fired:?}");
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t43_each_unsubscribe_takes_its_path() {
    let imap = server();
    imap.deliver(
        "INBOX",
        msg(
            "oc@t",
            "One click",
            "List-Unsubscribe: <mailto:u@list.test>,\r\n <https://list.test/u?id=1>\r\n\
             List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n",
        ),
    )
    .unwrap();
    imap.deliver(
        "INBOX",
        msg(
            "mt@t",
            "Mail me",
            "List-Unsubscribe: <mailto:leave@list.test?subject=Remove%20me>, <https://list.test/web>\r\n",
        ),
    )
    .unwrap();
    imap.deliver(
        "INBOX",
        msg(
            "web@t",
            "Web",
            "List-Unsubscribe: <https://list.test/web?id=3>\r\n",
        ),
    )
    .unwrap();
    imap.deliver("INBOX", msg("none@t", "Plain", "")).unwrap();
    let s = start("t43", imap).await;
    let one = s.listed("INBOX", "One click").await;
    let mail = s.listed("INBOX", "Mail me").await;
    let web = s.listed("INBOX", "Web").await;
    let plain = s.listed("INBOX", "Plain").await;

    // The reader knows the method; opening fires nothing.
    let method = async |id| s.core.message_open(id).await.unwrap().unsubscribe;
    assert_eq!(method(one.id).await, Some(Method::OneClick));
    assert_eq!(method(mail.id).await, Some(Method::Mailto));
    assert_eq!(method(web.id).await, Some(Method::Https));
    assert_eq!(method(plain.id).await, None);
    assert!(s.http.posts().is_empty(), "nothing fires without a click");

    // One-click: one POST of exactly the RFC 8058 body.
    s.http.token_reply(200, "");
    let out = s.core.message_unsubscribe(one.id).await.unwrap();
    assert_eq!(out.method, Method::OneClick);
    assert_eq!(
        s.http.posts(),
        vec![(
            "https://list.test/u?id=1".to_string(),
            "List-Unsubscribe=One-Click".to_string()
        )]
    );
    // A refusal is reported, not hidden.
    s.http.token_reply(500, "");
    let err = s.core.message_unsubscribe(one.id).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ServerRejected);

    // mailto: a prefilled draft, nothing sent.
    let out = s.core.message_unsubscribe(mail.id).await.unwrap();
    assert_eq!(out.method, Method::Mailto);
    let draft = out.draft.unwrap();
    assert_eq!(draft.content.to[0].addr, "leave@list.test");
    assert_eq!(draft.content.subject, "Remove me");
    assert!(out.url.is_none());

    // https: the URL for the system browser; no request from Core.
    let posts = s.http.posts().len();
    let out = s.core.message_unsubscribe(web.id).await.unwrap();
    assert_eq!(out.method, Method::Https);
    assert_eq!(out.url.as_deref(), Some("https://list.test/web?id=3"));
    assert_eq!(s.http.posts().len(), posts);
    assert!(s.http.asked.lock().unwrap().is_empty());

    let err = s.core.message_unsubscribe(plain.id).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
    s.core.shutdown().await;
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t44_image_preview_route_and_print_checks() {
    let jpeg = b"\xff\xd8\xff\xe0fakejpeg";
    let body = format!(
        "--b\r\nContent-Type: text/plain\r\n\r\nSee attached\r\n\
         --b\r\nContent-Type: image/jpeg; name=photo.jpg\r\nContent-Disposition: attachment; filename=photo.jpg\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{}\r\n\
         --b\r\nContent-Type: application/pdf; name=doc.pdf\r\nContent-Disposition: attachment; filename=doc.pdf\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{}\r\n--b--\r\n",
        base64(jpeg),
        base64(b"%PDF-1.4 fake")
    );
    let imap = server();
    imap.deliver(
        "INBOX",
        FakeMessage::mime(
            format!(
                "Message-ID: <att@t>\r\nFrom: Bob <bob@test.local>\r\nTo: {USER}\r\nSubject: Files\r\n\
                 MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n{body}"
            ),
            now() - 60,
        ),
    )
    .unwrap();
    imap.deliver("INBOX", msg("other@t", "Other", "")).unwrap();
    let s = start("t44", imap).await;
    let row = s.listed("INBOX", "Files").await;
    let other = s.listed("INBOX", "Other").await;
    let view = s.core.message_open(row.id).await.unwrap();
    let id_of = |mime: &str| view.attachments.iter().find(|a| a.mime == mime).unwrap().id;
    let (img, pdf) = (id_of("image/jpeg"), id_of("application/pdf"));

    let image = s
        .core
        .mail_cid(&format!("/{}/attachment/{img}", row.id))
        .await;
    assert_eq!(
        (image.status, image.content_type.as_str()),
        (200, "image/jpeg")
    );
    assert_eq!(image.body, jpeg);
    assert_eq!(image.csp, "default-src 'none'");
    // Not an image, or not that message's attachment: nothing.
    for path in [
        format!("/{}/attachment/{pdf}", row.id),
        format!("/{}/attachment/{img}", other.id),
    ] {
        assert_eq!(s.core.mail_cid(&path).await.status, 404, "{path}");
    }
    let bad = format!("/{}/attachment/x", row.id);
    assert_eq!(s.core.mail_cid(&bad).await.status, 400);

    // Print checks its target; the thread form takes the thread id.
    s.core.message_print(Some(row.id), None).await.unwrap();
    s.core
        .message_print(None, Some(row.thread_id))
        .await
        .unwrap();
    assert_eq!(
        s.core.message_print(None, None).await.unwrap_err().code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        s.core
            .message_print(Some(999_999), None)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_allowlist_and_clear_body_cache() {
    let imap = server();
    imap.deliver("INBOX", msg("a@t", "Cached", "")).unwrap();
    let s = start("m10s", imap).await;
    let row = s.listed("INBOX", "Cached").await;

    // Settings: defaults, validation, and the stored value.
    assert_eq!(
        s.core.settings_get("ui.reading_pane").await.unwrap(),
        json!("right")
    );
    assert_eq!(
        s.core.settings_get("app.keep_in_tray").await.unwrap(),
        json!(true)
    );
    for (key, value) in [
        ("ui.reading_pane", json!("left")),
        ("compose.undo_seconds", json!(31)),
        ("no.such.key", json!(true)),
    ] {
        assert_eq!(
            s.core.settings_set(key, value).await.unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }
    s.core
        .settings_set("ui.reading_pane", json!("bottom"))
        .await
        .unwrap();
    assert_eq!(
        s.core.settings_get("ui.reading_pane").await.unwrap(),
        json!("bottom")
    );

    // The allowlist: listed, then removed.
    s.core.images_allow_sender(row.id).await.unwrap();
    assert_eq!(
        s.core.images_list_allowed(s.account).await.unwrap(),
        vec!["bob@test.local".to_string()]
    );
    s.core
        .images_disallow_sender(s.account, "bob@test.local")
        .await
        .unwrap();
    assert!(s
        .core
        .images_list_allowed(s.account)
        .await
        .unwrap()
        .is_empty());

    // Clear body cache: the body goes, the header stays, and opening fetches it again.
    wait_for("body cached", async || {
        let v = s.core.message_open(row.id).await.unwrap();
        v.body_kind != mail_core::views::BodyKind::Loading
    })
    .await;
    s.core.storage_clear_body_cache().await.unwrap();
    assert_eq!(s.rows("INBOX").await.len(), 1, "headers stay");
    let fetches = s.imap.calls().len();
    let v = s.core.message_open(row.id).await.unwrap();
    assert_eq!(v.body_kind, mail_core::views::BodyKind::Loading);
    wait_for("body fetched again", async || {
        s.imap.calls().len() > fetches
            && s.core.message_open(row.id).await.unwrap().body_kind
                != mail_core::views::BodyKind::Loading
    })
    .await;
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unread_count_for_title_tray_and_badge() {
    let imap = server();
    imap.deliver("INBOX", msg("u1@t", "One", "")).unwrap();
    imap.deliver("INBOX", msg("u2@t", "Two", "")).unwrap();
    let mut read = msg("u3@t", "Three", "");
    read.flags.push(mail_imap::Flag::Seen);
    imap.deliver("INBOX", read).unwrap();
    imap.deliver("Archive", msg("u4@t", "Elsewhere", ""))
        .unwrap();
    let s = start("m10u", imap).await;
    let one = s.listed("INBOX", "One").await;
    s.listed("INBOX", "Three").await;

    let shown: Arc<Mutex<Vec<u32>>> = Arc::default();
    let log = Arc::clone(&shown);
    s.core.watch_unread(move |n| log.lock().unwrap().push(n));
    // Only INBOX counts.
    wait_for("2 unread", async || {
        shown.lock().unwrap().last() == Some(&2)
    })
    .await;
    assert_eq!(s.core.unread_total().await.unwrap(), 2);
    // A change follows at once, and an unchanged count is not shown again.
    s.core
        .message_set_flags(
            &[one.id],
            mail_core::views::FlagChange {
                seen: Some(true),
                flagged: None,
            },
        )
        .await
        .unwrap();
    wait_for("1 unread", async || {
        shown.lock().unwrap().last() == Some(&1)
    })
    .await;
    // A disabled account does not count.
    s.core.account_set_enabled(s.account, false).await.unwrap();
    wait_for("0 unread", async || {
        shown.lock().unwrap().last() == Some(&0)
    })
    .await;
    let seen = shown.lock().unwrap().clone();
    assert!(seen.windows(2).all(|w| w[0] != w[1]), "{seen:?}");
    s.core.shutdown().await;
}
