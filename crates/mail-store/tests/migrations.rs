//! M1 "migration test": a fresh database migrates to the section 4 schema, reopening is a
//! no-op that keeps data, and a database from a newer build is refused.
#![cfg(test)]

mod common;

use common::*;
use mail_store::*;

/// Section 4 column lists, the contract.
const SPEC: &[(&str, &[&str])] = &[
    (
        "accounts",
        &[
            "id",
            "name",
            "email",
            "imap_host",
            "imap_port",
            "imap_security",
            "smtp_host",
            "smtp_port",
            "smtp_security",
            "username",
            "auth_kind",
            "oauth_client_id",
            "pinned_cert_sha256",
            "sync_days",
            "save_sent_copy",
            "signature_html",
            "signature_text",
            "enabled",
            "created_at",
        ],
    ),
    (
        "identities",
        &[
            "id",
            "account_id",
            "name",
            "address",
            "reply_to",
            "is_default",
        ],
    ),
    (
        "folders",
        &[
            "id",
            "account_id",
            "path",
            "delimiter",
            "special_use",
            "uidvalidity",
            "uidnext",
            "highestmodseq",
            "sync_enabled",
            "watch_idle",
            "unread",
            "total",
            "synced_at",
        ],
    ),
    (
        "messages",
        &[
            "id",
            "account_id",
            "folder_id",
            "uid",
            "gm_msgid",
            "gm_thrid",
            "message_id",
            "in_reply_to",
            "refs",
            "gm_labels_json",
            "thread_id",
            "subject",
            "from_name",
            "from_addr",
            "to_json",
            "cc_json",
            "reply_to_json",
            "date_utc",
            "internal_utc",
            "size",
            "flags",
            "has_attach",
            "preview",
            "body_state",
        ],
    ),
    (
        "bodies",
        &["message_id", "text_plain", "text_html", "fetched_at"],
    ),
    (
        "attachments",
        &[
            "id",
            "message_id",
            "part_id",
            "filename",
            "mime",
            "size",
            "content_id",
            "is_inline",
        ],
    ),
    (
        "threads",
        &["id", "account_id", "last_date_utc", "subject_norm"],
    ),
    (
        "outbox",
        &[
            "id",
            "account_id",
            "identity_id",
            "raw_mime",
            "envelope_json",
            "state",
            "send_after_utc",
            "attempts",
            "last_error",
            "created_at",
        ],
    ),
    (
        "pending_ops",
        &[
            "id",
            "account_id",
            "folder_id",
            "uids_json",
            "kind",
            "args_json",
            "state",
            "attempts",
            "last_error",
            "created_at",
        ],
    ),
    (
        "drafts",
        &[
            "id",
            "account_id",
            "imap_uid",
            "content_json",
            "updated_at",
            "uploaded_at",
        ],
    ),
    (
        "contacts",
        &[
            "account_id",
            "address",
            "name",
            "last_used_utc",
            "use_count",
        ],
    ),
    ("remote_image_allow", &["account_id", "sender_addr"]),
    (
        "snoozes",
        &["message_id", "account_id", "due_utc", "origin_folder_id"],
    ),
    ("muted_threads", &["account_id", "thread_id"]),
    (
        "rules",
        &[
            "id",
            "account_id",
            "position",
            "name",
            "enabled",
            "conditions_json",
            "actions_json",
            "stop_processing",
        ],
    ),
    ("rules_applied", &["message_id", "rule_id"]),
    (
        "templates",
        &[
            "id",
            "name",
            "subject",
            "body_html",
            "body_text",
            "updated_at",
        ],
    ),
    ("settings", &["key", "value_json"]),
];

/// Columns the spec marks `NULL`; every other column is `NOT NULL`.
const NULLABLE: &[(&str, &str)] = &[
    ("accounts", "pinned_cert_sha256"),
    ("identities", "reply_to"),
    ("folders", "special_use"),
    ("folders", "highestmodseq"),
    ("messages", "gm_msgid"),
    ("messages", "gm_thrid"),
    ("messages", "in_reply_to"),
    ("messages", "refs"),
    ("messages", "gm_labels_json"),
    ("attachments", "content_id"),
    ("drafts", "imap_uid"),
    ("drafts", "uploaded_at"),
    ("rules", "account_id"),
];

struct Column {
    name: String,
    kind: String,
    not_null: bool,
    pk: bool,
}

fn columns(conn: &rusqlite::Connection, table: &str) -> Vec<Column> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT name, type, \"notnull\", pk FROM pragma_table_info('{table}')"
        ))
        .unwrap();
    stmt.query_map([], |r| {
        Ok(Column {
            name: r.get(0)?,
            kind: r.get(1)?,
            not_null: r.get(2)?,
            pk: r.get::<_, i64>(3)? > 0,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn user_version(conn: &rusqlite::Connection) -> i64 {
    conn.pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap()
}

#[tokio::test]
async fn fresh_database_matches_the_section_4_schema() {
    let dir = TempDir::new("schema");
    drop(Store::open(&dir.db()).unwrap());
    let conn = raw(&dir.db());

    assert!(user_version(&conn) >= 1);
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");

    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'message_fts%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut expected: Vec<&str> = SPEC.iter().map(|(t, _)| *t).collect();
    expected.sort_unstable();
    assert_eq!(tables, expected, "exactly the section 4 tables");

    for (table, spec_columns) in SPEC {
        let actual = columns(&conn, table);
        let names: Vec<&str> = actual.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(&names, spec_columns, "{table} columns");
        for c in &actual {
            let nullable = NULLABLE.contains(&(table, c.name.as_str()));
            assert_eq!(
                c.not_null || c.pk,
                !nullable,
                "{table}.{} nullability",
                c.name
            );
            // Invariant 6: every time is UTC epoch seconds.
            if c.name.ends_with("_utc") || c.name.ends_with("_at") {
                assert_eq!(c.kind, "INTEGER", "{table}.{} is a time", c.name);
            }
        }
    }

    // Section 10: FTS5, external content, unicode61 with diacritics removed.
    let fts: String = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name = 'message_fts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(fts.contains("USING fts5("));
    assert!(fts.contains("content = 'message_fts_content'"));
    assert!(fts.contains("tokenize = 'unicode61 remove_diacritics 2'"));
}

#[tokio::test]
async fn no_column_can_hold_a_secret() {
    // Invariant 5: accounts reference keychain entries by id; nothing secret is stored.
    let dir = TempDir::new("secrets");
    drop(Store::open(&dir.db()).unwrap());
    let conn = raw(&dir.db());
    for (table, _) in SPEC {
        for c in columns(&conn, table) {
            for word in ["password", "secret", "token", "credential"] {
                assert!(
                    !c.name.contains(word),
                    "{table}.{} looks like a secret",
                    c.name
                );
            }
        }
    }
}

#[tokio::test]
async fn reopening_is_a_no_op_and_keeps_data() {
    let dir = TempDir::new("reopen");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, thread) = seeded(&store).await;
    store
        .insert_messages(inbox, vec![message(1, 100, thread)])
        .await
        .unwrap();
    drop(store);
    let version = user_version(&raw(&dir.db()));

    let store = Store::open(&dir.db()).unwrap();
    let page = store.folder_page(inbox, None, 10).await.unwrap();
    assert_eq!(page.rows.len(), 1);
    drop(store);
    assert_eq!(user_version(&raw(&dir.db())), version);
}

#[tokio::test]
async fn database_from_a_newer_build_is_refused() {
    let dir = TempDir::new("newer");
    drop(Store::open(&dir.db()).unwrap());
    let version = user_version(&raw(&dir.db()));
    raw(&dir.db())
        .pragma_update(None, "user_version", version + 1)
        .unwrap();

    assert!(matches!(
        Store::open(&dir.db()),
        Err(StoreError::TooNew { found, supported }) if found == version + 1 && supported == version
    ));
    assert_eq!(
        user_version(&raw(&dir.db())),
        version + 1,
        "never downgraded"
    );
}

#[tokio::test]
async fn foreign_keys_are_enforced() {
    let dir = TempDir::new("fk");
    let store = Store::open(&dir.db()).unwrap();
    assert!(store.insert_thread(42, 0, String::new()).await.is_err());
    assert!(store.insert_folder(folder(42, "INBOX")).await.is_err());
}

#[tokio::test]
async fn a_non_database_file_reports_corrupt() {
    let dir = TempDir::new("corrupt");
    std::fs::write(dir.db(), vec![0x5a; 8192]).unwrap();
    assert!(matches!(Store::open(&dir.db()), Err(StoreError::Corrupt)));
}
