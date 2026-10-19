//! Account CRUD queries (M2): accounts with their first identity, updates, enable switch and
//! settings.
#![cfg(test)]

mod common;

use common::TempDir;
use mail_store::*;

fn identity(address: &str) -> NewIdentity {
    NewIdentity {
        name: "Me".into(),
        address: address.into(),
        reply_to: None,
        is_default: true,
    }
}

#[tokio::test]
async fn add_account_writes_the_account_and_its_default_identity_together() {
    let dir = TempDir::new("accounts-add");
    let store = Store::open(&dir.db()).unwrap();
    let (id, identity_id) = store
        .add_account(
            common::account("me@example.test"),
            identity("me@example.test"),
        )
        .await
        .unwrap();
    let account = store.get_account(id).await.unwrap();
    assert_eq!(account.config, common::account("me@example.test"));
    let identities = store.list_identities(id).await.unwrap();
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0].id, identity_id);
    assert!(identities[0].identity.is_default);

    // A rejected account (the pin fails its CHECK) leaves no identity behind.
    let mut bad = common::account("bad@example.test");
    bad.pinned_cert_sha256 = Some("short".into());
    assert!(store
        .add_account(bad, identity("bad@example.test"))
        .await
        .is_err());
    assert_eq!(store.list_accounts().await.unwrap().len(), 1);
    let conn = common::raw(&dir.db());
    assert_eq!(common::count(&conn, "SELECT count(*) FROM identities"), 1);
}

#[tokio::test]
async fn update_enable_and_list() {
    let dir = TempDir::new("accounts-update");
    let store = Store::open(&dir.db()).unwrap();
    let (a, _) = store
        .add_account(
            common::account("a@example.test"),
            identity("a@example.test"),
        )
        .await
        .unwrap();
    let (b, _) = store
        .add_account(
            common::account("b@example.test"),
            identity("b@example.test"),
        )
        .await
        .unwrap();

    let mut changed = common::account("a@example.test");
    changed.imap_host = "imap.example.test".into();
    changed.imap_security = Security::StartTls;
    changed.imap_port = 143;
    changed.pinned_cert_sha256 = Some("ab".repeat(32));
    changed.save_sent_copy = SaveSentCopy::Off;
    store.update_account(a, changed.clone()).await.unwrap();
    store.set_account_enabled(b, false).await.unwrap();

    let all = store.list_accounts().await.unwrap();
    assert_eq!(all.iter().map(|x| x.id).collect::<Vec<_>>(), vec![a, b]);
    assert_eq!(all[0].config, changed);
    assert!(!all[1].config.enabled);

    assert!(matches!(
        store.update_account(999, changed).await,
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.set_account_enabled(999, true).await,
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.get_account(999).await,
        Err(StoreError::NotFound)
    ));

    store.delete_account(a).await.unwrap();
    assert!(store.list_identities(a).await.unwrap().is_empty());
}

#[tokio::test]
async fn settings_round_trip_as_json() {
    let dir = TempDir::new("accounts-settings");
    let store = Store::open(&dir.db()).unwrap();
    assert_eq!(
        store.get_setting("privacy.online_lookups").await.unwrap(),
        None
    );
    store
        .set_setting("privacy.online_lookups", serde_json::json!(false))
        .await
        .unwrap();
    store
        .set_setting("privacy.online_lookups", serde_json::json!(true))
        .await
        .unwrap();
    assert_eq!(
        store.get_setting("privacy.online_lookups").await.unwrap(),
        Some(serde_json::json!(true))
    );
}
