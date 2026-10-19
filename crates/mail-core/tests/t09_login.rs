//! T09 at the login level (SPEC_GAPS G33): the app password is revoked on the server. The
//! account's next login fails with `auth_failed` (class NeedsAttention), exactly one login
//! is attempted with no retry, and another account on the same server still logs in and
//! lists. M3 repeats T09 with sync actors.
#![cfg(test)]

mod common;

use std::sync::Arc;

use common::{config, secrets, FakeHttp, TempDir};
use mail_core::{Accounts, ErrorCode};
use mail_imap::fake::{FakeServer, Op};
use mail_imap::{ErrorClass, ImapError};
use mail_secrets::SecretStore;
use mail_smtp::fake::FakeSmtp;

#[tokio::test]
async fn t09_revoked_password_needs_attention_with_one_attempt_and_others_keep_working() {
    let dir = TempDir::new("t09");
    let imap = FakeServer::full_featured();
    let smtp = FakeSmtp::default();
    for (user, pass) in [
        ("alice@test.local", "alice-pw"),
        ("bob@test.local", "bob-pw"),
    ] {
        imap.add_user(user, pass);
        smtp.add_user(user, pass);
    }
    let accounts = Accounts::new(
        dir.store(),
        secrets() as Arc<dyn SecretStore>,
        imap.clone(),
        smtp,
        FakeHttp::default(),
    );
    let alice = accounts
        .add(config("alice@test.local", "alice@test.local", "alice-pw"))
        .await
        .unwrap();
    let bob = accounts
        .add(config("bob@test.local", "bob@test.local", "bob-pw"))
        .await
        .unwrap();
    assert!(accounts.check_login(alice.id).await.is_ok());

    // Revoke Alice's app password on the server.
    imap.set_password("alice@test.local", "revoked");
    imap.clear_calls();

    let err = accounts.check_login(alice.id).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthFailed);
    assert_eq!(err.account_id, Some(alice.id));
    assert_eq!(ImapError::AuthFailed.class(), ErrorClass::NeedsAttention);
    let connects = imap.calls().iter().filter(|c| c.op == Op::Connect).count();
    assert_eq!(connects, 1, "exactly one login attempt, no retry");
    assert!(
        !imap.calls().iter().any(|c| c.op == Op::List),
        "nothing runs after a failed login"
    );

    // Bob is unaffected.
    assert_eq!(accounts.check_login(bob.id).await.unwrap(), 1);
}
