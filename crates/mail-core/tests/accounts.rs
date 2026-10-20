//! Account CRUD and resolution on the fake transport (M2).
#![cfg(test)]

mod common;

use std::sync::Arc;

use common::{config, secrets, FakeHttp, TempDir};
use mail_core::{Accounts, AuthKind, ConfigSource, ErrorCode, ONLINE_LOOKUPS};
use mail_imap::fake::{FakeServer, Op};
use mail_imap::ImapError;
use mail_secrets::{MemoryStore, Secret, SecretStore};
use mail_smtp::fake::FakeSmtp;
use mail_smtp::SmtpError;

const USER: &str = "alice@test.local";
const PASS: &str = "alice-pw";

struct Setup {
    _dir: TempDir,
    imap: FakeServer,
    smtp: FakeSmtp,
    http: FakeHttp,
    secrets: Arc<MemoryStore>,
    accounts: Accounts<FakeServer, FakeSmtp, FakeHttp>,
}

fn setup(caps: &[&str]) -> Setup {
    let dir = TempDir::new("accounts");
    let imap = FakeServer::new(caps);
    imap.add_user(USER, PASS);
    imap.add_folder("Sent", None);
    let smtp = FakeSmtp::default();
    smtp.add_user(USER, PASS);
    let http = FakeHttp::default();
    let secrets = secrets();
    let accounts = Accounts::new(
        dir.store(),
        secrets.clone() as Arc<dyn SecretStore>,
        imap.clone(),
        smtp.clone(),
        http.clone(),
    );
    Setup {
        _dir: dir,
        imap,
        smtp,
        http,
        secrets,
        accounts,
    }
}

fn full() -> Setup {
    setup(&["IMAP4rev1", "AUTH=PLAIN", "SPECIAL-USE", "IDLE"])
}

#[tokio::test]
async fn add_saves_the_account_identity_and_password_and_lists_it() {
    let s = full();
    let report = s.accounts.test(&config(USER, USER, PASS)).await.unwrap();
    assert!(report.passed(), "{report:?}");
    assert_eq!(report.folders, 2, "LIST works");

    let summary = s.accounts.add(config(USER, USER, PASS)).await.unwrap();
    assert_eq!(summary.email, USER);
    assert!(summary.enabled);
    assert_eq!(summary.sync_days, 90);
    assert_eq!(s.accounts.list().await.unwrap(), vec![summary.clone()]);
    let key = format!("account:{}:password", summary.id);
    assert_eq!(s.secrets.keys(), vec![key.clone()]);
    assert_eq!(s.secrets.get(&key).unwrap(), Some(Secret::new(PASS)));

    // Nothing the UI receives carries the password.
    let json = serde_json::to_string(&summary).unwrap();
    assert!(!json.contains(PASS));
    assert!(!format!("{:?}", config(USER, USER, PASS)).contains(PASS));

    assert_eq!(s.accounts.check_login(summary.id).await.unwrap(), 2);
}

#[tokio::test]
async fn a_failing_half_blocks_saving_unless_save_anyway() {
    let s = full();
    let err = s
        .accounts
        .add(config(USER, USER, "wrong"))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthFailed);
    assert!(s.accounts.list().await.unwrap().is_empty());
    assert!(s.secrets.keys().is_empty());

    s.smtp.fail_next(SmtpError::Network("refused".into()));
    let err = s.accounts.add(config(USER, USER, PASS)).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Network, "the SMTP half counts");
    assert!(s.accounts.list().await.unwrap().is_empty());

    s.smtp.fail_next(SmtpError::Network("refused".into()));
    let report = s.accounts.test(&config(USER, USER, PASS)).await.unwrap();
    assert!(report.imap_error.is_none());
    assert_eq!(report.smtp_error.unwrap().code, ErrorCode::Network);

    let mut anyway = config(USER, USER, "wrong");
    anyway.save_anyway = true;
    let calls_before = s.imap.calls().len();
    let attempts_before = s.smtp.attempts();
    let saved = s.accounts.add(anyway).await.unwrap();
    assert_eq!(
        s.imap.calls().len(),
        calls_before,
        "Save anyway skips the test"
    );
    assert_eq!(s.smtp.attempts(), attempts_before);
    assert_eq!(s.accounts.list().await.unwrap().len(), 1);
    assert_eq!(
        s.accounts.check_login(saved.id).await.unwrap_err().code,
        ErrorCode::AuthFailed
    );
}

#[tokio::test]
async fn update_replaces_settings_and_password() {
    let s = full();
    let added = s.accounts.add(config(USER, USER, PASS)).await.unwrap();

    // A new password on the server: update with it.
    s.imap.set_password(USER, "new-pw");
    s.smtp.add_user(USER, "new-pw");
    let mut changed = config(USER, USER, "new-pw");
    changed.name = "Alice".into();
    changed.imap.port = 1993;
    let updated = s.accounts.update(added.id, changed).await.unwrap();
    assert_eq!(updated.name, "Alice");
    assert_eq!(updated.imap.port, 1993);
    let key = format!("account:{}:password", added.id);
    assert_eq!(s.secrets.get(&key).unwrap(), Some(Secret::new("new-pw")));

    // No password: the saved one is tested and kept.
    let mut keep = config(USER, USER, "ignored");
    keep.password = None;
    keep.smtp.port = 587;
    let updated = s.accounts.update(added.id, keep).await.unwrap();
    assert_eq!(updated.smtp.port, 587);
    assert_eq!(s.secrets.get(&key).unwrap(), Some(Secret::new("new-pw")));

    let err = s
        .accounts
        .update(999, config(USER, USER, PASS))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn remove_deletes_rows_and_keychain_entries_only_locally() {
    let s = full();
    let a = s.accounts.add(config(USER, USER, PASS)).await.unwrap();
    s.imap.clear_calls();
    s.accounts.set_enabled(a.id, false).await.unwrap();
    assert!(!s.accounts.list().await.unwrap()[0].enabled);
    s.accounts.remove(a.id).await.unwrap();
    assert!(s.accounts.list().await.unwrap().is_empty());
    assert!(s.secrets.keys().is_empty());
    assert!(
        s.imap.calls().is_empty(),
        "nothing on the server is touched"
    );
    assert_eq!(
        s.accounts.remove(a.id).await.unwrap_err().code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn invalid_input_is_rejected_before_any_connection() {
    let s = full();
    let cases = [
        {
            let mut c = config("not-an-address", USER, PASS);
            c.name.clear();
            c
        },
        {
            let mut c = config(USER, USER, PASS);
            c.imap.host = " ".into();
            c
        },
        {
            let mut c = config(USER, USER, PASS);
            c.smtp.port = 0;
            c
        },
        config(USER, "", PASS),
        {
            let mut c = config(USER, USER, PASS);
            c.pinned_cert_sha256 = Some("abc".into());
            c
        },
        {
            let mut c = config(USER, USER, PASS);
            c.password = None;
            c
        },
    ];
    for c in cases {
        assert_eq!(
            s.accounts.add(c).await.unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }
    // Google sign-in needs the client ID and its secret (SPEC_GAPS G70).
    let mut oauth = config(USER, USER, PASS);
    oauth.auth_kind = AuthKind::OauthGoogle;
    oauth.oauth_client_secret = Some(Secret::new("cs"));
    assert_eq!(
        s.accounts.add(oauth.clone()).await.unwrap_err().code,
        ErrorCode::InvalidInput
    );
    oauth.oauth_client_id = Some("cid".into());
    oauth.oauth_client_secret = Some(Secret::new("  "));
    assert_eq!(
        s.accounts.add(oauth).await.unwrap_err().code,
        ErrorCode::InvalidInput
    );
    assert!(s.imap.calls().is_empty());
}

#[tokio::test]
async fn pins_are_normalized() {
    let s = full();
    let mut c = config(USER, USER, PASS);
    c.pinned_cert_sha256 = Some(format!(" {} ", "AB".repeat(32)));
    let a = s.accounts.add(c).await.unwrap();
    assert_eq!(a.pinned_cert_sha256, Some("ab".repeat(32)));
}

#[tokio::test]
async fn resolve_offers_methods_from_the_host_and_capabilities() {
    let s = setup(&["IMAP4rev1", "AUTH=PLAIN", "AUTH=XOAUTH2"]);
    let found = s.accounts.resolve("me@gmail.com").await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].source, ConfigSource::BuiltIn);
    assert_eq!(found[0].imap.host, "imap.gmail.com");
    assert_eq!(
        found[0].auth_methods,
        vec![AuthKind::OauthGoogle, AuthKind::Password]
    );
    assert!(found[0].probe_error.is_none());
    assert!(s.http.asked.lock().unwrap().is_empty());

    // Without AUTH=XOAUTH2 only the password is offered.
    let s = setup(&["IMAP4rev1", "AUTH=PLAIN"]);
    let found = s.accounts.resolve("me@gmail.com").await.unwrap();
    assert_eq!(found[0].auth_methods, vec![AuthKind::Password]);

    // LOGINDISABLED and no AUTH=PLAIN: nothing usable.
    let s = setup(&["IMAP4rev1", "LOGINDISABLED"]);
    let found = s.accounts.resolve("me@fastmail.com").await.unwrap();
    assert!(found[0].auth_methods.is_empty());
}

#[tokio::test]
async fn resolve_reports_a_certificate_to_pin_and_honors_the_offline_setting() {
    let s = full();
    s.imap.fail_next(
        Op::Probe,
        ImapError::Certificate {
            reason: "UnknownIssuer".into(),
            sha256: "cd".repeat(32),
        },
    );
    let found = s.accounts.resolve("me@icloud.com").await.unwrap();
    let probe = found[0].probe_error.clone().unwrap();
    assert_eq!(probe.code, ErrorCode::Certificate);
    assert_eq!(probe.message, "cd".repeat(32));
    assert_eq!(found[0].auth_methods, vec![AuthKind::Password]);

    let found = s.accounts.resolve("me@example.org").await.unwrap();
    assert!(found.is_empty(), "nothing found: manual form");
    assert_eq!(s.http.asked.lock().unwrap().len(), 3);

    let store_setting = TempDir::new("offline");
    let store = store_setting.store();
    store
        .set_setting(ONLINE_LOOKUPS, serde_json::json!(false))
        .await
        .unwrap();
    let http = FakeHttp::default();
    let offline = Accounts::new(
        store,
        secrets() as Arc<dyn SecretStore>,
        s.imap.clone(),
        s.smtp.clone(),
        http.clone(),
    );
    assert!(offline.resolve("me@example.org").await.unwrap().is_empty());
    assert!(
        http.asked.lock().unwrap().is_empty(),
        "online lookups off: no request"
    );
}
