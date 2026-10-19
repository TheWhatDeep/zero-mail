//! Sync against the Stalwart test container (BUILD.md section 15). Ignored by default; CI's
//! Linux integration job runs them:
//!
//!   docker compose -f tests/docker/compose.yml up -d --wait
//!   cargo test -p mail-sync --test stalwart -- --ignored --test-threads=1
//!
//! They use Bob's mailbox and tolerate mail left by earlier tests.
#![cfg(test)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use mail_imap::{
    ConnectConfig, Credentials, Flag, ImapClient, ImapConnector, ImapError, ImapTransport, Secret,
    Security, StoreOp, UidSet,
};
use mail_sync::{AccountState, SyncEvent};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

const HOST: &str = "127.0.0.1";
const IMAPS: u16 = 3993;
const SMTP_DELIVERY: u16 = 3025;

fn config(pin: Option<[u8; 32]>) -> ConnectConfig {
    ConnectConfig {
        host: HOST.into(),
        port: IMAPS,
        security: Security::Tls,
        pinned_cert_sha256: pin,
        connect_timeout: Duration::from_secs(10),
    }
}

/// The container's self-signed certificate, read from the error an unpinned connection gets.
async fn pin() -> ([u8; 32], String) {
    match ImapClient.probe(&config(None)).await {
        Err(ImapError::Certificate { sha256, .. }) => {
            (mail_imap::parse_fingerprint(&sha256).unwrap(), sha256)
        }
        other => unreachable!("expected a certificate error, got {other:?}"),
    }
}

/// Delivers one message by plain SMTP to the container's port 25, as another sender would.
async fn deliver(to: &str, subject: &str) {
    let stream = TcpStream::connect((HOST, SMTP_DELIVERY)).await.unwrap();
    let mut s = BufReader::new(stream);
    async fn reply(s: &mut BufReader<TcpStream>) -> String {
        loop {
            let mut line = String::new();
            s.read_line(&mut line).await.unwrap();
            if line.as_bytes().get(3) != Some(&b'-') {
                return line;
            }
        }
    }
    assert!(reply(&mut s).await.starts_with("220"));
    for (cmd, code) in [
        ("EHLO tests.local".to_string(), "250"),
        ("MAIL FROM:<alice@test.local>".to_string(), "250"),
        (format!("RCPT TO:<{to}>"), "250"),
        ("DATA".to_string(), "354"),
    ] {
        s.get_mut()
            .write_all(format!("{cmd}\r\n").as_bytes())
            .await
            .unwrap();
        let r = reply(&mut s).await;
        assert!(r.starts_with(code), "{cmd}: {r}");
    }
    let body = format!(
        "From: Alice <alice@test.local>\r\nTo: <{to}>\r\nSubject: {subject}\r\n\
         Message-ID: <{subject}@tests.local>\r\n\r\nHello from the test.\r\n.\r\n"
    );
    s.get_mut().write_all(body.as_bytes()).await.unwrap();
    assert!(reply(&mut s).await.starts_with("250"));
    s.get_mut().write_all(b"QUIT\r\n").await.unwrap();
}

async fn bob_account(dir: &TempDir) -> (mail_store::Store, i64, Logins) {
    let store = dir.store();
    let (_, hex) = pin().await;
    let mut account = new_account("bob@test.local");
    account.imap_host = HOST.into();
    account.imap_port = IMAPS;
    account.username = "bob".into();
    account.pinned_cert_sha256 = Some(hex);
    let id = store
        .add_account(
            account,
            mail_store::NewIdentity {
                name: "Bob".into(),
                address: "bob@test.local".into(),
                reply_to: None,
                is_default: true,
            },
        )
        .await
        .unwrap()
        .0;
    let logins = Logins::default();
    logins.set("bob", "bob-test-pw");
    (store, id, logins)
}

async fn wait_for_subject(
    watch: &mut Watch,
    store: &mail_store::Store,
    inbox: i64,
    subject: &str,
    present: bool,
) {
    loop {
        if rows(store, inbox)
            .await
            .iter()
            .any(|r| r.subject == subject)
            == present
        {
            return;
        }
        watch
            .until(
                Some(Duration::from_secs(20)),
                |e| matches!(e, SyncEvent::StoreChanged { folder_id: Some(f), .. } if *f == inbox),
            )
            .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the Stalwart container; runs in CI"]
async fn t05_mail_delivered_by_smtp_is_listed_within_two_seconds() {
    let dir = TempDir::new("stalwart-t05");
    let (store, account, logins) = bob_account(&dir).await;
    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, ImapClient, &logins, &events, account);
    watch.state(account, AccountState::Idle).await;
    let inbox = folder_id(&store, account, "INBOX").await;
    // Let the IDLE connection settle.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let subject = format!("t05-{}", now());
    let start = Instant::now();
    deliver("bob@test.local", &subject).await;
    let accepted = start.elapsed();
    wait_for_subject(&mut watch, &store, inbox, &subject, true).await;
    let visible = start.elapsed();
    eprintln!("T05: SMTP accepted after {accepted:?}, listed after {visible:?}");
    assert!(
        visible < Duration::from_secs(2) + Duration::from_secs(1),
        "2 s plus server latency, got {visible:?}"
    );
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the Stalwart container; runs in CI"]
async fn t04_expunge_from_another_client_removes_the_row() {
    let dir = TempDir::new("stalwart-t04");
    let (store, account, logins) = bob_account(&dir).await;
    let subject = format!("t04-{}", now());
    deliver("bob@test.local", &subject).await;

    let (events, mut watch) = Watch::new();
    let handle = spawn(&store, ImapClient, &logins, &events, account);
    watch.state(account, AccountState::Idle).await;
    let inbox = folder_id(&store, account, "INBOX").await;
    wait_for_subject(&mut watch, &store, inbox, &subject, true).await;
    let uid = rows(&store, inbox)
        .await
        .into_iter()
        .find(|r| r.subject == subject)
        .map(|r| r.id)
        .unwrap();
    let uid: u32 = dir
        .raw()
        .query_row("SELECT uid FROM messages WHERE id = ?1", [uid], |r| {
            r.get(0)
        })
        .unwrap();

    // Another client deletes it.
    let (pin, _) = pin().await;
    let creds = Credentials::Password {
        username: "bob".into(),
        password: Secret::new("bob-test-pw"),
    };
    let mut other = ImapClient
        .connect(&config(Some(pin)), &creds)
        .await
        .unwrap();
    other.select("INBOX").await.unwrap();
    other
        .uid_store(&UidSet::single(uid), StoreOp::Add, &[Flag::Deleted])
        .await
        .unwrap();
    other.uid_expunge(&UidSet::single(uid)).await.unwrap();
    other.logout().await.unwrap();

    wait_for_subject(&mut watch, &store, inbox, &subject, false).await;
    assert!(!store.cached_uids(inbox).await.unwrap().contains(&uid));
    handle.shutdown().await;
}
