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
use mail_auth::HttpGet;
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
