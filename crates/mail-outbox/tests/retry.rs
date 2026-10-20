//! T14 with an explicit clock: SMTP answers 451, then recovers. `retry_wait` follows the
//! section 7 schedule, nothing is sent early, and exactly one delivery happens.
#![cfg(test)]

use std::time::Duration;

use mail_outbox::{send_one, RETRY_SECS};
use mail_smtp::fake::FakeSmtp;
use mail_smtp::{SmtpConfig, SmtpError, SmtpLogin, SubmitError};
use mail_store::*;
use serde_json::json;

fn config() -> SmtpConfig {
    SmtpConfig {
        host: "smtp.test".into(),
        port: 465,
        security: mail_smtp::Security::Tls,
        pinned_cert_sha256: None,
        connect_timeout: Duration::from_secs(5),
    }
}

fn login() -> Result<SmtpLogin, SubmitError> {
    Ok(SmtpLogin::Password {
        username: "me".into(),
        password: "pw".into(),
    })
}

#[tokio::test]
async fn t14_retry_wait_follows_the_schedule_and_delivers_once() {
    let dir = std::env::temp_dir().join(format!("zm-outbox-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("t14.db");
    let _ = std::fs::remove_file(&db);
    let store = Store::open(&db).unwrap();
    let (account, identity) = store
        .add_account(
            NewAccount {
                name: "Me".into(),
                email: "me@x.test".into(),
                imap_host: "imap.x.test".into(),
                imap_port: 993,
                imap_security: Security::Tls,
                smtp_host: "smtp.x.test".into(),
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
            },
            NewIdentity {
                name: "Me".into(),
                address: "me@x.test".into(),
                reply_to: None,
                is_default: true,
            },
        )
        .await
        .unwrap();
    let id = store
        .insert_outbox(NewOutbox {
            account_id: account,
            identity_id: identity,
            raw_mime: b"Subject: t14\r\n\r\nhi\r\n".to_vec(),
            envelope: json!({ "envelope": { "from": "me@x.test", "to": ["bob@x.test"] } }),
            send_after_utc: 1000,
            created_at: 990,
        })
        .await
        .unwrap();
    let smtp = FakeSmtp::default();
    smtp.add_user("me", "pw");
    smtp.fail_next_submit(SubmitError::Transient(SmtpError::Unavailable("451".into())));

    // Not due yet.
    assert!(send_one(&store, &smtp, &config(), login(), id, 999)
        .await
        .unwrap()
        .is_none());
    // First attempt: 451.
    let t = send_one(&store, &smtp, &config(), login(), id, 1000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, OutboxState::RetryWait);
    assert_eq!(t.item.send_after_utc, 1000 + RETRY_SECS[0]);
    assert_eq!(t.item.attempts, 1);
    // Nothing before the 30 s are up.
    assert!(send_one(&store, &smtp, &config(), login(), id, 1029)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.due_outbox(1029).await.unwrap(),
        Vec::<OutboxId>::new()
    );
    // The server recovered.
    let t = send_one(&store, &smtp, &config(), login(), id, 1030)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, OutboxState::Sent);
    assert_eq!(smtp.deliveries().len(), 1, "exactly one delivery");
    assert_eq!(smtp.submits(), 2);
    // A sent item is never sent again.
    assert!(send_one(&store, &smtp, &config(), login(), id, 99_999)
        .await
        .unwrap()
        .is_none());
    assert_eq!(smtp.deliveries().len(), 1);
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}
