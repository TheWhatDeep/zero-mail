//! The T19 fixture (SPEC_GAPS G56): a data directory with one disabled account (so nothing
//! touches the network) and one INBOX message per case of the XSS corpus, each with the
//! case as its cached HTML body. tests/render/xss-harness.mjs opens every one in the real
//! webview and counts outbound requests.
//!
//!   cargo run -p mail-store --example xss_fixture -- <data dir> tests/fixtures/xss/corpus.html
//!       [beacon base, for example http://127.0.0.1:48619]
//!
//! With a beacon base, every `http://beacon.invalid` in the corpus points there instead, so
//! the CI harness's local server sees anything that leaves the webview (SPEC_GAPS G68).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use mail_store::*;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: xss_fixture <data dir> <corpus>"));
    let mut corpus = std::fs::read_to_string(args.next().expect("corpus path")).unwrap();
    if let Some(beacon) = args.next() {
        corpus = corpus.replace("http://beacon.invalid", beacon.trim_end_matches('/'));
    }
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("mail.db");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
    }
    let store = Store::open(&db).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let (account, _) = store
        .add_account(
            NewAccount {
                name: "XSS Fixture".into(),
                email: "me@fixture.test".into(),
                imap_host: "imap.fixture.test".into(),
                imap_port: 993,
                imap_security: Security::Tls,
                smtp_host: "smtp.fixture.test".into(),
                smtp_port: 465,
                smtp_security: Security::Tls,
                username: "me@fixture.test".into(),
                auth_kind: AuthKind::Password,
                oauth_client_id: String::new(),
                pinned_cert_sha256: None,
                sync_days: 90,
                save_sent_copy: SaveSentCopy::Auto,
                signature_html: String::new(),
                signature_text: String::new(),
                enabled: false,
                created_at: now,
            },
            NewIdentity {
                name: "XSS Fixture".into(),
                address: "me@fixture.test".into(),
                reply_to: None,
                is_default: true,
            },
        )
        .await
        .unwrap();
    let folders = store
        .apply_folder_list(
            account,
            vec![NewFolder {
                account_id: account,
                path: "INBOX".into(),
                delimiter: "/".into(),
                special_use: Some(SpecialUse::Inbox),
                sync_enabled: true,
                watch_idle: false,
            }],
        )
        .await
        .unwrap();
    let inbox = folders[0].id;
    store.set_uidvalidity(inbox, 1).await.unwrap();

    let cases: Vec<(String, String)> = corpus
        .split("<!-- case: ")
        .skip(1)
        .map(|chunk| {
            let (name, html) = chunk.split_once(" -->").expect("case header");
            (name.trim().to_string(), html.trim().to_string())
        })
        .collect();
    let rows: Vec<NewMessage> = cases
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let uid = u32::try_from(i + 1).unwrap();
            NewMessage {
                uid,
                gm_msgid: None,
                gm_thrid: None,
                message_id: format!("xss-{uid}@fixture.test"),
                in_reply_to: None,
                refs: None,
                gm_labels: None,
                list_id: None,
                thread_id: 0,
                subject: format!("XSS: {name}"),
                from: Address {
                    name: "Attacker".into(),
                    addr: "attacker@beacon.invalid".into(),
                },
                to: vec![Address {
                    name: "XSS Fixture".into(),
                    addr: "me@fixture.test".into(),
                }],
                cc: Vec::new(),
                reply_to: Vec::new(),
                date_utc: now - i64::from(uid) * 60,
                internal_utc: now - i64::from(uid) * 60,
                size: 1_000,
                flags: Flags(Flags::SEEN),
                has_attach: false,
                preview: String::new(),
                body_state: BodyState::None,
                attachments: Vec::new(),
            }
        })
        .collect();
    let ids = store.insert_synced(inbox, rows).await.unwrap();
    for (id, (_, html)) in ids.iter().zip(&cases) {
        store
            .store_body(
                *id,
                Body {
                    text_plain: String::new(),
                    text_html: html.clone(),
                    fetched_at: now,
                },
                BodyState::Full,
                String::new(),
            )
            .await
            .unwrap();
    }
    store
        .finish_folder_sync(
            inbox,
            FolderSyncState {
                uidvalidity: 1,
                uidnext: u32::try_from(cases.len() + 1).unwrap(),
                highestmodseq: None,
                synced_at: now,
            },
        )
        .await
        .unwrap();
    println!("xss fixture: {} cases in {}", cases.len(), db.display());
}
