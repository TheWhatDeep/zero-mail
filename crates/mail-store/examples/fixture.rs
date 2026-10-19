//! The perf fixture (BUILD.md section 14: the reference dataset; tests/fixtures generator).
//! Writes a data directory with one disabled account (so nothing touches the network), the
//! usual folders, and N messages in INBOX over the last 90 days, newest with bodies.
//!
//!   cargo run --release -p mail-store --example fixture -- <data dir> [count]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use mail_store::*;

const FIRST: &[&str] = &[
    "Maya", "Jonas", "Priya", "Leo", "Amara", "Kenji", "Sofia", "Omar", "Elena", "Tomás", "Ines",
    "Wei", "Noah", "Zara", "Hugo", "Freya",
];
const LAST: &[&str] = &[
    "Okafor",
    "Lindqvist",
    "Raman",
    "Moreau",
    "Haddad",
    "Sato",
    "Rossi",
    "Nakamura",
    "Novak",
    "García",
    "Berg",
    "Chen",
    "Fischer",
    "Mensah",
];
const TOPICS: &[&str] = &[
    "Quarterly budget review",
    "Re: Design handoff for the onboarding flow",
    "Invoice 2026-",
    "Lunch on Thursday?",
    "Fwd: Contract draft v3",
    "Server maintenance window",
    "Your order has shipped",
    "Re: Re: Offsite agenda",
    "Weekly metrics",
    "Notes from today's call",
];
const SNIPPETS: &[&str] = &[
    "Attached are the numbers we discussed. Let me know if anything looks off before Friday.",
    "Thanks for sending this over. I left a few comments inline, mostly small wording changes.",
    "Quick reminder that the review moved to 3 pm. Same room, same link.",
    "Could you take a look when you have a minute? No rush, sometime this week is fine.",
    "Here is the summary. The short version: we are on track, with two open questions.",
];

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: fixture <data dir> [count]"));
    let count: u32 = args.next().map_or(50_000, |c| c.parse().expect("count"));
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
                name: "Perf Fixture".into(),
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
                name: "Perf Fixture".into(),
                address: "me@fixture.test".into(),
                reply_to: None,
                is_default: true,
            },
        )
        .await
        .unwrap();
    let folder = |path: &str, role: Option<SpecialUse>| NewFolder {
        account_id: account,
        path: path.into(),
        delimiter: "/".into(),
        special_use: role,
        sync_enabled: true,
        watch_idle: false,
    };
    let folders = store
        .apply_folder_list(
            account,
            vec![
                folder("INBOX", Some(SpecialUse::Inbox)),
                folder("Drafts", Some(SpecialUse::Drafts)),
                folder("Sent", Some(SpecialUse::Sent)),
                folder("Archive", Some(SpecialUse::Archive)),
                folder("Trash", Some(SpecialUse::Trash)),
                folder("Projects", None),
                folder("Projects/Alpha", None),
            ],
        )
        .await
        .unwrap();
    let inbox = folders.iter().find(|f| f.path == "INBOX").unwrap().id;
    store.set_uidvalidity(inbox, 1).await.unwrap();

    let span = 90 * 86_400i64;
    let step = span / i64::from(count.max(1));
    let mut batch = Vec::new();
    for n in 0..count {
        let uid = count - n;
        let i = n as usize;
        let first = FIRST[i % FIRST.len()];
        let last = LAST[(i / FIRST.len()) % LAST.len()];
        let topic = TOPICS[(i * 7) % TOPICS.len()];
        let subject = if topic.ends_with('-') {
            format!("{topic}{uid:05}")
        } else {
            topic.to_string()
        };
        let mut flags = 0u8;
        if n % 3 != 0 {
            flags |= Flags::SEEN;
        }
        if n % 17 == 0 {
            flags |= Flags::FLAGGED;
        }
        let has_attach = n % 11 == 0;
        batch.push(NewMessage {
            uid,
            gm_msgid: None,
            gm_thrid: None,
            message_id: format!("fixture-{uid}@fixture.test"),
            in_reply_to: None,
            refs: None,
            gm_labels: None,
            thread_id: 0,
            subject,
            from: Address {
                name: format!("{first} {last}"),
                addr: format!(
                    "{}.{}@example.test",
                    first.to_lowercase(),
                    last.to_lowercase()
                ),
            },
            to: vec![Address {
                name: "Perf Fixture".into(),
                addr: "me@fixture.test".into(),
            }],
            cc: Vec::new(),
            reply_to: Vec::new(),
            date_utc: now - i64::from(n) * step,
            internal_utc: now - i64::from(n) * step,
            size: 4_000 + u64::from(uid % 900) * 10,
            flags: Flags(flags),
            has_attach,
            preview: SNIPPETS[i % SNIPPETS.len()].to_string(),
            body_state: BodyState::None,
            attachments: if has_attach {
                vec![NewAttachment {
                    part_id: "2".into(),
                    filename: "report.pdf".into(),
                    mime: "application/pdf".into(),
                    size: 120_000,
                    content_id: None,
                    is_inline: false,
                }]
            } else {
                Vec::new()
            },
        });
        if batch.len() == 1_000 {
            store
                .insert_synced(inbox, std::mem::take(&mut batch))
                .await
                .unwrap();
        }
    }
    if !batch.is_empty() {
        store.insert_synced(inbox, batch).await.unwrap();
    }
    for item in store.prefetch_candidates(inbox, 200).await.unwrap() {
        let text = format!(
            "Hi,\n\n{}\n\nBest,\nFixture sender {}",
            SNIPPETS[item.uid as usize % SNIPPETS.len()],
            item.uid
        );
        let preview = SNIPPETS[item.uid as usize % SNIPPETS.len()].to_string();
        store
            .store_body(
                item.message_id,
                Body {
                    text_plain: text,
                    text_html: String::new(),
                    fetched_at: now,
                },
                BodyState::Full,
                preview,
            )
            .await
            .unwrap();
    }
    store
        .finish_folder_sync(
            inbox,
            FolderSyncState {
                uidvalidity: 1,
                uidnext: count + 1,
                highestmodseq: None,
                synced_at: now,
            },
        )
        .await
        .unwrap();
    println!("fixture: {count} messages in {}", db.display());
}
