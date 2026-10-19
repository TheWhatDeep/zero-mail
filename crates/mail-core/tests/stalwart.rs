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
