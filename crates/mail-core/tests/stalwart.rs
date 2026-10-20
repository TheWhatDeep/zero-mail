//! M2 done-when against the Stalwart test container (BUILD.md section 15): add an account,
//! `LIST` works, and T09 at the login level. Every test here needs the container, so they
//! are ignored by default and run in CI's Linux integration job:
//!
//!   docker compose -f tests/docker/compose.yml up -d --wait
//!   cargo test -p mail-core --test stalwart -- --ignored --test-threads=1
//!
//! The container's certificate is self-signed and regenerated at every start, so each test
//! first reads its fingerprint from the certificate error and then pins it, the same path a
//! user takes for a self-hosted server (section 5, Certificates).
#![cfg(test)]

mod common;

use std::sync::Arc;

use common::{secrets, TempDir};
use mail_auth::{HttpFailure, HttpGet, HttpPost, HttpReply};
use mail_core::{AccountConfig, Accounts, AuthKind, Endpoint, ErrorCode, Security, ONLINE_LOOKUPS};
use mail_imap::{ConnectConfig, Credentials, ImapClient, ImapConnector, ImapTransport, Secret};
use mail_secrets::SecretStore;
use mail_smtp::SmtpClient;

const HOST: &str = "127.0.0.1";
const IMAPS: u16 = 3993;
const IMAP: u16 = 3143;
const SUBMISSIONS: u16 = 3465;
const SUBMISSION: u16 = 3587;

/// No online lookups in tests.
#[derive(Debug, Clone, Copy)]
struct NoHttp;

impl HttpGet for NoHttp {
    async fn get(&self, _url: &str) -> Option<Vec<u8>> {
        None
    }
}

impl HttpPost for NoHttp {
    async fn post_form(&self, _url: &str, _body: Vec<u8>) -> Result<HttpReply, HttpFailure> {
        Err(HttpFailure("no network in tests".into()))
    }
}

type Service = Accounts<ImapClient, SmtpClient, NoHttp>;

async fn service(dir: &TempDir) -> Service {
    let store = dir.store();
    store
        .set_setting(ONLINE_LOOKUPS, serde_json::json!(false))
        .await
        .unwrap();
    Accounts::new(
        store,
        secrets() as Arc<dyn SecretStore>,
        ImapClient,
        SmtpClient,
        NoHttp,
    )
}

fn config(user: &str, password: &str, starttls: bool) -> AccountConfig {
    let (imap_port, smtp_port, security) = if starttls {
        (IMAP, SUBMISSION, Security::Starttls)
    } else {
        (IMAPS, SUBMISSIONS, Security::Tls)
    };
    AccountConfig {
        name: user.into(),
        email: format!("{user}@test.local"),
        imap: Endpoint {
            host: HOST.into(),
            port: imap_port,
            security,
        },
        smtp: Endpoint {
            host: HOST.into(),
            port: smtp_port,
            security,
        },
        username: user.into(),
        auth_kind: AuthKind::Password,
        password: Some(mail_secrets::Secret::new(password)),
        pinned_cert_sha256: None,
        save_anyway: false,
        signature_html: None,
        signature_text: None,
        oauth_client_id: None,
        oauth_client_secret: None,
    }
}

/// Test connection without a pin: both halves report the self-signed certificate. Returns
/// its fingerprint.
async fn fingerprint(accounts: &Service) -> String {
    let report = accounts
        .test(&config("alice", "alice-test-pw", false))
        .await
        .unwrap();
    let imap = report
        .imap_error
        .expect("an unpinned self-signed cert fails");
    let smtp = report
        .smtp_error
        .expect("an unpinned self-signed cert fails");
    assert_eq!(imap.code, ErrorCode::Certificate);
    assert_eq!(smtp.code, ErrorCode::Certificate);
    assert_eq!(imap.message.len(), 64);
    assert_eq!(
        imap.message, smtp.message,
        "one certificate for IMAP and SMTP"
    );
    imap.message
}

#[tokio::test]
#[ignore = "needs the Stalwart container; runs in CI"]
async fn add_an_account_with_a_pinned_certificate_and_list() {
    let dir = TempDir::new("stalwart-add");
    let accounts = service(&dir).await;
    let pin = fingerprint(&accounts).await;

    for starttls in [false, true] {
        let mut c = config("alice", "alice-test-pw", starttls);
        c.pinned_cert_sha256 = Some(pin.clone());
        let report = accounts.test(&c).await.unwrap();
        assert!(report.passed(), "starttls={starttls}: {report:?}");
        assert!(report.folders >= 1, "LIST returns at least INBOX");
    }

    let mut c = config("alice", "alice-test-pw", false);
    c.pinned_cert_sha256 = Some(pin.clone());
    let added = accounts.add(c).await.unwrap();
    assert_eq!(added.email, "alice@test.local");
    assert_eq!(added.pinned_cert_sha256.as_deref(), Some(pin.as_str()));
    assert!(accounts.check_login(added.id).await.unwrap() >= 1);

    // A different pin is a hard failure (section 5: a changed certificate).
    let mut wrong = config("alice", "alice-test-pw", false);
    wrong.pinned_cert_sha256 = Some("00".repeat(32));
    let report = accounts.test(&wrong).await.unwrap();
    assert_eq!(report.imap_error.unwrap().code, ErrorCode::Certificate);
}

#[tokio::test]
#[ignore = "needs the Stalwart container; runs in CI"]
async fn list_reports_inbox_and_roles_through_the_real_transport() {
    let dir = TempDir::new("stalwart-list");
    let accounts = service(&dir).await;
    let pin = mail_imap::parse_fingerprint(&fingerprint(&accounts).await).unwrap();
    let config = ConnectConfig {
        host: HOST.into(),
        port: IMAPS,
        security: mail_imap::Security::Tls,
        pinned_cert_sha256: Some(pin),
        connect_timeout: mail_imap::CONNECT_TIMEOUT,
    };
    let caps = ImapClient.probe(&config).await.unwrap();
    assert!(caps.auth_plain || !caps.login_disabled);
    let creds = Credentials::Password {
        username: "bob".into(),
        password: Secret::new("bob-test-pw"),
    };
    let mut session = ImapClient.connect(&config, &creds).await.unwrap();
    let mut folders = session.list().await.unwrap();
    mail_imap::assign_roles(&mut folders);
    assert!(folders
        .iter()
        .any(|f| f.special_use == Some(mail_imap::SpecialUse::Inbox)));
    let inbox = session.select("INBOX").await.unwrap();
    assert!(inbox.uidvalidity > 0);
    session.noop().await.unwrap();
    session.logout().await.unwrap();
}

/// M5 through the real transport: a multipart message APPENDed to Carol's INBOX syncs, its
/// HTML body is served sanitized, its `cid:` image comes through `mail-cid`, and its base64
/// attachment is fetched on Save with the server's own BODYSTRUCTURE part ids (T23, T24).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the Stalwart container; runs in CI"]
async fn m5_html_cid_images_and_attachments_through_the_real_transport() {
    use mail_core::views::{BodyKind, ListRequest, Scope};
    use mail_core::{Core, CoreConfig, CoreEvent, EventSink, StateName};
    use std::sync::Mutex;
    use std::time::Duration;

    let dir = TempDir::new("stalwart-m5");
    let accounts = service(&dir).await;
    let pin_hex = fingerprint(&accounts).await;
    drop(accounts);
    let pin = mail_imap::parse_fingerprint(&pin_hex).unwrap();

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let subject = format!("m5-{nanos}");
    let raw = format!(
        "Message-ID: <{subject}@tests.local>\r\nFrom: Alice <alice@test.local>\r\nTo: carol@test.local\r\n\
         Subject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"m\"\r\n\r\n\
         --m\r\nContent-Type: multipart/related; boundary=\"r\"\r\n\r\n\
         --r\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p onclick=\"x()\">Hi</p><img src=\"cid:logo@m5\"><script>bad()</script>\r\n\
         --r\r\nContent-Type: image/png\r\nContent-ID: <logo@m5>\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgpmYWtlcG5n\r\n\
         --r--\r\n\
         --m\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"CON.pdf\"\r\n\
         Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQgdGVzdA==\r\n--m--\r\n"
    );
    let imap_config = ConnectConfig {
        host: HOST.into(),
        port: IMAPS,
        security: mail_imap::Security::Tls,
        pinned_cert_sha256: Some(pin),
        connect_timeout: mail_imap::CONNECT_TIMEOUT,
    };
    let creds = Credentials::Password {
        username: "carol".into(),
        password: Secret::new("carol-test-pw"),
    };
    let mut session = ImapClient.connect(&imap_config, &creds).await.unwrap();
    session.append("INBOX", &[], raw.as_bytes()).await.unwrap();
    session.logout().await.unwrap();

    let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
    let log = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |e| log.lock().unwrap().push(e));
    let core = Core::start(
        CoreConfig {
            data_dir: dir.path().join("core"),
            secrets: secrets() as Arc<dyn SecretStore>,
            imap: ImapClient,
            smtp: SmtpClient,
            http: NoHttp,
        },
        sink,
    )
    .await
    .unwrap();
    let mut c = config("carol", "carol-test-pw", false);
    c.pinned_cert_sha256 = Some(pin_hex);
    let account = core.account_add(c).await.unwrap().id;
    let start = std::time::Instant::now();
    let message = loop {
        assert!(start.elapsed() < Duration::from_secs(60), "never synced");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let idle = events.lock().unwrap().iter().any(|e| {
            matches!(e, CoreEvent::AccountState { state: StateName::Idle, account_id, .. } if *account_id == account)
        });
        if !idle {
            continue;
        }
        let folders = core.folder_list(account).await.unwrap();
        let page = core
            .list_page(ListRequest {
                scope: Scope::Folder {
                    folder_id: folders[0].id,
                },
                cursor: None,
                limit: 500,
                group_by_thread: false,
            })
            .await
            .unwrap();
        if let Some(row) = page.rows.iter().find(|r| r.subject == subject) {
            break row.id;
        }
    };
    let view = loop {
        let view = core.message_open(message).await.unwrap();
        if view.body_kind != BodyKind::Loading {
            break view;
        }
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "body never arrived"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(view.body_kind, BodyKind::Html);
    assert_eq!(view.attachments.len(), 1, "the inline image is not listed");
    let body = core.mail_body(message, "images=0").await.unwrap();
    let html = String::from_utf8(body.body).unwrap();
    assert!(html.contains("<p>Hi</p>") && !html.contains("bad()") && !html.contains("onclick"));
    assert_eq!(html.matches("<script").count(), 1);
    let image = core.mail_cid(&format!("/{message}/logo%40m5")).await;
    assert_eq!(image.status, 200);
    assert_eq!(image.body, b"\x89PNG\r\n\x1a\nfakepng");

    let pdf = view.attachments[0].id;
    assert_eq!(core.attachment_file_name(pdf).await.unwrap(), "_CON.pdf");
    let target = dir.path().join("saved.pdf");
    core.attachment_save(pdf, target.clone()).await.unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"%PDF-1.4 test");
    let source = dir.path().join("source.eml");
    core.message_save_source(message, source.clone())
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&std::fs::read(&source).unwrap()).contains(&subject));
    core.shutdown().await;
}

/// T10 and T11 against the real servers: Alice sends To Bob, Cc Carol and Bcc herself.
/// Each mailbox gets exactly one copy, no copy carries a Bcc header or Alice's address as a
/// recipient, and Alice's Sent folder holds exactly one copy with `\Seen`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the Stalwart container; runs in CI"]
async fn m7_to_cc_bcc_each_once_and_one_sent_copy() {
    use mail_core::compose::{DraftContent, Timing};
    use mail_core::views::AddressView;
    use mail_core::{Core, CoreConfig, CoreEvent, EventSink, StateName};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    let dir = TempDir::new("stalwart-m7");
    let accounts = service(&dir).await;
    let pin_hex = fingerprint(&accounts).await;
    drop(accounts);
    let pin = mail_imap::parse_fingerprint(&pin_hex).unwrap();
    let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
    let log = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |e| log.lock().unwrap().push(e));
    let core = Core::start(
        CoreConfig {
            data_dir: dir.path().join("core"),
            secrets: secrets() as Arc<dyn SecretStore>,
            imap: ImapClient,
            smtp: SmtpClient,
            http: NoHttp,
        },
        sink,
    )
    .await
    .unwrap();
    core.set_timing(Timing {
        undo_window: Duration::ZERO,
        draft_upload: Duration::from_secs(3600),
        outbox_tick: Duration::from_millis(200),
    });
    let mut c = config("alice", "alice-test-pw", false);
    c.pinned_cert_sha256 = Some(pin_hex);
    let account = core.account_add(c).await.unwrap().id;
    let start = Instant::now();
    let deadline = Duration::from_secs(120);
    while !events.lock().unwrap().iter().any(|e| {
        matches!(e, CoreEvent::AccountState { state: StateName::Idle, account_id, .. } if *account_id == account)
    }) {
        assert!(start.elapsed() < deadline, "never idle");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let subject = format!("m7-{nanos}");
    let view = core.compose_new(account, None).await.unwrap();
    let person = |a: &str| AddressView {
        name: String::new(),
        addr: a.into(),
    };
    let content = DraftContent {
        to: vec![person("bob@test.local")],
        cc: vec![person("carol@test.local")],
        bcc: vec![person("alice@test.local")],
        subject: subject.clone(),
        html: "<p>Hello from <b>M7</b></p>".into(),
        text: "Hello from M7".into(),
        ..view.content
    };
    core.draft_save(view.draft_id, content).await.unwrap();
    let identity = view.identities[0].id.unwrap();
    let id = core
        .draft_send(view.draft_id, identity, None)
        .await
        .unwrap();
    loop {
        let state = core
            .outbox_list(account)
            .await
            .unwrap()
            .into_iter()
            .find(|o| o.id == id)
            .map(|o| o.state);
        if state == Some("sent") {
            break;
        }
        assert_ne!(state, Some("failed"), "the send failed");
        assert!(start.elapsed() < deadline, "never sent: {state:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let imap_config = ConnectConfig {
        host: HOST.into(),
        port: IMAPS,
        security: mail_imap::Security::Tls,
        pinned_cert_sha256: Some(pin),
        connect_timeout: mail_imap::CONNECT_TIMEOUT,
    };
    for (user, folder) in [("bob", "INBOX"), ("carol", "INBOX"), ("alice", "INBOX")] {
        let creds = Credentials::Password {
            username: user.into(),
            password: Secret::new(format!("{user}-test-pw")),
        };
        let mut session = ImapClient.connect(&imap_config, &creds).await.unwrap();
        let uids = loop {
            session.select(folder).await.unwrap();
            let uids = session
                .uid_search(&mail_imap::SearchKey::Subject(subject.clone()))
                .await
                .unwrap();
            if !uids.is_empty() {
                break uids;
            }
            assert!(start.elapsed() < deadline, "{user} never got it");
            tokio::time::sleep(Duration::from_millis(300)).await;
        };
        assert_eq!(uids.len(), 1, "{user}: exactly one copy");
        let raw = session.uid_fetch_part(uids[0], "", None).await.unwrap();
        let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
        let head = text
            .split("\r\n\r\n")
            .next()
            .unwrap_or_default()
            .to_string();
        assert!(!head.contains("\nbcc:"), "{user}: no Bcc header");
        let recipients: String = head
            .lines()
            .filter(|l| l.starts_with("to:") || l.starts_with("cc:"))
            .collect();
        assert!(
            !recipients.contains("alice@test.local"),
            "{user}: Bcc address invisible"
        );
        session.logout().await.unwrap();
    }
    // T11: one Sent copy, `\Seen`, on a server without X-GM-EXT-1.
    let creds = Credentials::Password {
        username: "alice".into(),
        password: Secret::new("alice-test-pw"),
    };
    let mut session = ImapClient.connect(&imap_config, &creds).await.unwrap();
    let mut folders = session.list().await.unwrap();
    mail_imap::assign_roles(&mut folders);
    let sent = folders
        .iter()
        .find(|f| f.special_use == Some(mail_imap::SpecialUse::Sent))
        .expect("a Sent folder")
        .path
        .clone();
    let uids = loop {
        session.select(&sent).await.unwrap();
        let uids = session
            .uid_search(&mail_imap::SearchKey::Subject(subject.clone()))
            .await
            .unwrap();
        if !uids.is_empty() {
            break uids;
        }
        assert!(start.elapsed() < deadline, "no Sent copy");
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    assert_eq!(uids.len(), 1, "exactly one Sent copy");
    let flags = session
        .uid_fetch_flags(&mail_imap::UidSet::single(uids[0]), None)
        .await
        .unwrap();
    assert!(flags.updates[0].flags.contains(&mail_imap::Flag::Seen));
    session.logout().await.unwrap();
    core.shutdown().await;
}

async fn m6_server_has(probe: &mut mail_imap::ImapSession, path: String) -> bool {
    probe.list().await.unwrap().iter().any(|f| f.path == path)
}

async fn m6_in_inbox(probe: &mut mail_imap::ImapSession, subject: String) -> Vec<u32> {
    probe.select("INBOX").await.unwrap();
    probe
        .uid_search(&mail_imap::SearchKey::Subject(subject))
        .await
        .unwrap()
}

/// M6 through the real transport (T27, T28): a `Café/日本語` folder is created, renamed and
/// deleted with modified UTF-7 names, and a message moved to Trash comes back with Undo
/// (Stalwart answers COPYUID), its star intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the Stalwart container; runs in CI"]
async fn m6_unicode_folders_and_undo_through_the_real_transport() {
    use mail_core::views::{ListRequest, Scope};
    use mail_core::{Core, CoreConfig, CoreEvent, EventSink, StateName};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    let dir = TempDir::new("stalwart-m6");
    let accounts = service(&dir).await;
    let pin_hex = fingerprint(&accounts).await;
    drop(accounts);
    let pin = mail_imap::parse_fingerprint(&pin_hex).unwrap();
    let imap_config = ConnectConfig {
        host: HOST.into(),
        port: IMAPS,
        security: mail_imap::Security::Tls,
        pinned_cert_sha256: Some(pin),
        connect_timeout: mail_imap::CONNECT_TIMEOUT,
    };
    let creds = Credentials::Password {
        username: "alice".into(),
        password: Secret::new("alice-test-pw"),
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let subject = format!("m6-{nanos}");
    let mut probe = ImapClient.connect(&imap_config, &creds).await.unwrap();
    let raw = format!(
        "Message-ID: <{subject}@tests.local>\r\nFrom: Bob <bob@test.local>\r\nTo: alice@test.local\r\n\
         Subject: {subject}\r\n\r\nUndo me.\r\n"
    );
    probe
        .append("INBOX", &[mail_imap::Flag::Flagged], raw.as_bytes())
        .await
        .unwrap();

    let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
    let log = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |e| log.lock().unwrap().push(e));
    let core = Core::start(
        CoreConfig {
            data_dir: dir.path().join("core"),
            secrets: secrets() as Arc<dyn SecretStore>,
            imap: ImapClient,
            smtp: SmtpClient,
            http: NoHttp,
        },
        sink,
    )
    .await
    .unwrap();
    let mut c = config("alice", "alice-test-pw", false);
    c.pinned_cert_sha256 = Some(pin_hex);
    let account = core.account_add(c).await.unwrap().id;
    let start = Instant::now();
    let deadline = Duration::from_secs(90);
    while !events.lock().unwrap().iter().any(|e| {
        matches!(e, CoreEvent::AccountState { state: StateName::Idle, account_id, .. } if *account_id == account)
    }) {
        assert!(start.elapsed() < deadline, "never idle");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // T27: the folder round-trips through the server's LIST.
    let name = format!("Café/日本語 {nanos}");
    let renamed = format!("Café/日本語 {nanos} 2");
    core.folder_create(account, &name).await.unwrap();
    while !m6_server_has(&mut probe, name.clone()).await {
        assert!(start.elapsed() < deadline, "never created");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    core.folder_rename(account, &name, &renamed).await.unwrap();
    while !m6_server_has(&mut probe, renamed.clone()).await {
        assert!(start.elapsed() < deadline, "never renamed");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(!m6_server_has(&mut probe, name.clone()).await);
    core.folder_delete(account, &renamed).await.unwrap();
    while m6_server_has(&mut probe, renamed.clone()).await {
        assert!(start.elapsed() < deadline, "never deleted");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // T28 on a real server: Delete moves to Trash; Undo brings it back with its star.
    let folders = core.folder_list(account).await.unwrap();
    let inbox = folders
        .iter()
        .find(|f| f.path.eq_ignore_ascii_case("INBOX"))
        .unwrap()
        .id;
    let message = loop {
        let page = core
            .list_page(ListRequest {
                scope: Scope::Folder { folder_id: inbox },
                cursor: None,
                limit: 500,
                group_by_thread: false,
            })
            .await
            .unwrap();
        if let Some(row) = page.rows.iter().find(|r| r.subject == subject) {
            assert!(row.flagged);
            break row.id;
        }
        assert!(start.elapsed() < deadline, "never listed");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let op = core.message_delete(&[message], None).await.unwrap();
    while !m6_in_inbox(&mut probe, subject.clone()).await.is_empty() {
        assert!(start.elapsed() < deadline, "never moved");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    core.message_undo(op).await.unwrap();
    let uid = loop {
        if let Some(uid) = m6_in_inbox(&mut probe, subject.clone()).await.first() {
            break *uid;
        }
        assert!(start.elapsed() < deadline, "never came back");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let flags = probe
        .uid_fetch_flags(&mail_imap::UidSet::single(uid), None)
        .await
        .unwrap();
    assert!(
        flags.updates[0].flags.contains(&mail_imap::Flag::Flagged),
        "star intact"
    );
    core.shutdown().await;
    probe.logout().await.unwrap();
}

#[tokio::test]
#[ignore = "needs the Stalwart container; runs in CI"]
async fn t09_wrong_password_needs_attention_and_others_keep_working() {
    let dir = TempDir::new("stalwart-t09");
    let accounts = service(&dir).await;
    let pin = fingerprint(&accounts).await;

    let mut alice = config("alice", "alice-test-pw", false);
    alice.pinned_cert_sha256 = Some(pin.clone());
    let alice = accounts.add(alice).await.unwrap();

    // Carol's saved password no longer matches the server: her app password was revoked.
    let mut carol = config("carol", "revoked-pw", false);
    carol.pinned_cert_sha256 = Some(pin.clone());
    let report = accounts.test(&carol).await.unwrap();
    assert_eq!(report.imap_error.unwrap().code, ErrorCode::AuthFailed);
    assert_eq!(report.smtp_error.unwrap().code, ErrorCode::AuthFailed);
    carol.save_anyway = true;
    let carol = accounts.add(carol).await.unwrap();

    let err = accounts.check_login(carol.id).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthFailed);
    assert_eq!(err.account_id, Some(carol.id));
    assert!(accounts.check_login(alice.id).await.unwrap() >= 1);
}
