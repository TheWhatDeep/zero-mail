//! Schema and forward-only migrations (BUILD.md section 4).
//!
//! `PRAGMA user_version` holds the number of migrations applied. Each migration runs in its
//! own transaction together with the version bump, so a crash or a failing statement leaves
//! the database at the previous version. A database newer than this build is refused, never
//! downgraded.
//!
//! Column lists are the section 4 contract. Columns the spec does not mark `NULL` are
//! `NOT NULL`; "not set yet" is `''` or `0` (`uidvalidity` and `synced_at` are 0 until the
//! folder is first selected). Times are UTC epoch seconds (invariant 6). No column holds a
//! secret (invariant 5).

use crate::error::{Result, StoreError};
use rusqlite::{Connection, TransactionBehavior};

/// Every migration, in order. Never edit a released entry; append a new one.
pub(crate) const MIGRATIONS: &[&str] = &[V1];

/// Applies every migration the database has not seen yet.
pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    migrate_with(conn, MIGRATIONS)
}

pub(crate) fn user_version(conn: &Connection) -> Result<i64> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

fn migrate_with(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    let supported =
        i64::try_from(migrations.len()).map_err(|_| StoreError::Invalid("migrations"))?;
    let found = user_version(conn)?;
    if found > supported {
        return Err(StoreError::TooNew { found, supported });
    }
    let applied = usize::try_from(found).map_err(|_| StoreError::Corrupt)?;
    for (version, sql) in (1..).zip(migrations.iter()).skip(applied) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

/// Version 1: the full section 4 data model.
const V1: &str = r#"
CREATE TABLE accounts (
    id                 INTEGER PRIMARY KEY,
    name               TEXT    NOT NULL,
    email              TEXT    NOT NULL,
    imap_host          TEXT    NOT NULL,
    imap_port          INTEGER NOT NULL CHECK (imap_port BETWEEN 1 AND 65535),
    imap_security      TEXT    NOT NULL CHECK (imap_security IN ('tls', 'starttls')),
    smtp_host          TEXT    NOT NULL,
    smtp_port          INTEGER NOT NULL CHECK (smtp_port BETWEEN 1 AND 65535),
    smtp_security      TEXT    NOT NULL CHECK (smtp_security IN ('tls', 'starttls')),
    username           TEXT    NOT NULL,
    auth_kind          TEXT    NOT NULL CHECK (auth_kind IN ('password', 'oauth_google')),
    oauth_client_id    TEXT    NOT NULL,
    pinned_cert_sha256 TEXT    CHECK (pinned_cert_sha256 IS NULL OR length(pinned_cert_sha256) = 64),
    sync_days          INTEGER NOT NULL DEFAULT 90 CHECK (sync_days > 0),
    save_sent_copy     TEXT    NOT NULL CHECK (save_sent_copy IN ('auto', 'on', 'off')),
    signature_html     TEXT    NOT NULL,
    signature_text     TEXT    NOT NULL,
    enabled            INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    created_at         INTEGER NOT NULL
) STRICT;

CREATE TABLE identities (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    address    TEXT    NOT NULL,
    reply_to   TEXT,
    is_default INTEGER NOT NULL CHECK (is_default IN (0, 1))
) STRICT;
CREATE INDEX identities_account ON identities(account_id);
CREATE UNIQUE INDEX identities_one_default ON identities(account_id) WHERE is_default = 1;

CREATE TABLE folders (
    id            INTEGER PRIMARY KEY,
    account_id    INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    path          TEXT    NOT NULL,
    delimiter     TEXT    NOT NULL,
    special_use   TEXT    CHECK (special_use IS NULL OR special_use IN
                          ('inbox', 'sent', 'drafts', 'trash', 'junk', 'archive', 'all')),
    uidvalidity   INTEGER NOT NULL CHECK (uidvalidity BETWEEN 0 AND 4294967295),
    uidnext       INTEGER NOT NULL CHECK (uidnext BETWEEN 0 AND 4294967295),
    highestmodseq INTEGER,
    sync_enabled  INTEGER NOT NULL CHECK (sync_enabled IN (0, 1)),
    watch_idle    INTEGER NOT NULL CHECK (watch_idle IN (0, 1)),
    unread        INTEGER NOT NULL,
    total         INTEGER NOT NULL,
    synced_at     INTEGER NOT NULL,
    UNIQUE (account_id, path)
) STRICT;

CREATE TABLE threads (
    id            INTEGER PRIMARY KEY,
    account_id    INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    last_date_utc INTEGER NOT NULL,
    subject_norm  TEXT    NOT NULL
) STRICT;
CREATE INDEX threads_account ON threads(account_id);

CREATE TABLE messages (
    id             INTEGER PRIMARY KEY,
    account_id     INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    folder_id      INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    uid            INTEGER NOT NULL CHECK (uid BETWEEN 1 AND 4294967295),
    gm_msgid       INTEGER,
    gm_thrid       INTEGER,
    message_id     TEXT    NOT NULL,
    in_reply_to    TEXT,
    refs           TEXT,
    gm_labels_json TEXT    CHECK (gm_labels_json IS NULL OR json_valid(gm_labels_json)),
    thread_id      INTEGER NOT NULL REFERENCES threads(id),
    subject        TEXT    NOT NULL,
    from_name      TEXT    NOT NULL,
    from_addr      TEXT    NOT NULL,
    to_json        TEXT    NOT NULL CHECK (json_valid(to_json)),
    cc_json        TEXT    NOT NULL CHECK (json_valid(cc_json)),
    reply_to_json  TEXT    NOT NULL CHECK (json_valid(reply_to_json)),
    date_utc       INTEGER NOT NULL,
    internal_utc   INTEGER NOT NULL,
    size           INTEGER NOT NULL CHECK (size >= 0),
    flags          INTEGER NOT NULL CHECK (flags BETWEEN 0 AND 63),
    has_attach     INTEGER NOT NULL CHECK (has_attach IN (0, 1)),
    preview        TEXT    NOT NULL,
    body_state     TEXT    NOT NULL CHECK (body_state IN ('none', 'text', 'full')),
    UNIQUE (folder_id, uid)
) STRICT;
-- Keyset pagination on (date, id), newest first (section 14).
CREATE INDEX messages_folder_date ON messages(folder_id, date_utc DESC, id DESC);
CREATE INDEX messages_account_msgid ON messages(account_id, message_id);
CREATE INDEX messages_thread ON messages(thread_id);

CREATE TABLE bodies (
    message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    text_plain TEXT    NOT NULL,
    text_html  TEXT    NOT NULL,
    fetched_at INTEGER NOT NULL
) STRICT;

CREATE TABLE attachments (
    id         INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    part_id    TEXT    NOT NULL,
    filename   TEXT    NOT NULL,
    mime       TEXT    NOT NULL,
    size       INTEGER NOT NULL CHECK (size >= 0),
    content_id TEXT,
    is_inline  INTEGER NOT NULL CHECK (is_inline IN (0, 1))
) STRICT;
CREATE INDEX attachments_message ON attachments(message_id);

CREATE TABLE outbox (
    id             INTEGER PRIMARY KEY,
    account_id     INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    identity_id    INTEGER NOT NULL REFERENCES identities(id),
    raw_mime       BLOB    NOT NULL,
    envelope_json  TEXT    NOT NULL CHECK (json_valid(envelope_json)),
    state          TEXT    NOT NULL CHECK (state IN
                           ('queued', 'sending', 'retry_wait', 'sent', 'failed', 'unknown')),
    send_after_utc INTEGER NOT NULL,
    attempts       INTEGER NOT NULL CHECK (attempts >= 0),
    last_error     TEXT    NOT NULL,
    created_at     INTEGER NOT NULL
) STRICT;
CREATE INDEX outbox_account ON outbox(account_id);
CREATE INDEX outbox_identity ON outbox(identity_id);

-- folder_id has no foreign key: a queued folder delete must outlive the local folder row.
-- The UIDVALIDITY purge drops the folder's ops explicitly (section 12).
CREATE TABLE pending_ops (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    folder_id  INTEGER NOT NULL,
    uids_json  TEXT    NOT NULL CHECK (json_valid(uids_json)),
    kind       TEXT    NOT NULL,
    args_json  TEXT    NOT NULL CHECK (json_valid(args_json)),
    state      TEXT    NOT NULL,
    attempts   INTEGER NOT NULL CHECK (attempts >= 0),
    last_error TEXT    NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;
CREATE INDEX pending_ops_account ON pending_ops(account_id);
CREATE INDEX pending_ops_folder ON pending_ops(folder_id);

CREATE TABLE drafts (
    id           INTEGER PRIMARY KEY,
    account_id   INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    imap_uid     INTEGER,
    content_json TEXT    NOT NULL CHECK (json_valid(content_json)),
    updated_at   INTEGER NOT NULL,
    uploaded_at  INTEGER
) STRICT;
CREATE INDEX drafts_account ON drafts(account_id);

CREATE TABLE contacts (
    account_id    INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    address       TEXT    NOT NULL COLLATE NOCASE,
    name          TEXT    NOT NULL,
    last_used_utc INTEGER NOT NULL,
    use_count     INTEGER NOT NULL CHECK (use_count >= 0),
    PRIMARY KEY (account_id, address)
) STRICT;

CREATE TABLE remote_image_allow (
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    sender_addr TEXT    NOT NULL COLLATE NOCASE,
    PRIMARY KEY (account_id, sender_addr)
) STRICT;

-- origin_folder_id has no foreign key, for the same reason as pending_ops.folder_id.
CREATE TABLE snoozes (
    message_id       INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    account_id       INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    due_utc          INTEGER NOT NULL,
    origin_folder_id INTEGER NOT NULL
) STRICT;
CREATE INDEX snoozes_account ON snoozes(account_id);
CREATE INDEX snoozes_due ON snoozes(due_utc);

CREATE TABLE muted_threads (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    thread_id  INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    PRIMARY KEY (account_id, thread_id)
) STRICT;
CREATE INDEX muted_threads_thread ON muted_threads(thread_id);

CREATE TABLE rules (
    id              INTEGER PRIMARY KEY,
    account_id      INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
    position        INTEGER NOT NULL,
    name            TEXT    NOT NULL,
    enabled         INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    conditions_json TEXT    NOT NULL CHECK (json_valid(conditions_json)),
    actions_json    TEXT    NOT NULL CHECK (json_valid(actions_json)),
    stop_processing INTEGER NOT NULL CHECK (stop_processing IN (0, 1))
) STRICT;
CREATE INDEX rules_account ON rules(account_id);

CREATE TABLE rules_applied (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    rule_id    INTEGER NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
    PRIMARY KEY (message_id, rule_id)
) STRICT;
CREATE INDEX rules_applied_rule ON rules_applied(rule_id);

CREATE TABLE templates (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    subject    TEXT    NOT NULL,
    body_html  TEXT    NOT NULL,
    body_text  TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value_json TEXT NOT NULL CHECK (json_valid(value_json))
) STRICT;

-- message_fts: FTS5 over subject, from, to/cc and body text, external content (section 10).
-- The content source spans messages and bodies, so it is a view. Address lists are JSON
-- arrays of [name, addr] pairs, so their raw text tokenizes to names and addresses only. Triggers keep the index in step with both tables, including
-- cascaded deletes: a message's entry is removed before its row, and a body trigger that
-- runs after its message is gone finds nothing to change. Writers never use INSERT OR
-- REPLACE on these tables: its implicit delete does not fire triggers.
CREATE VIEW message_fts_content AS
SELECT
    m.id AS id,
    m.subject AS subject,
    m.from_name || ' ' || m.from_addr AS sender,
    m.to_json || ' ' || m.cc_json AS recipients,
    coalesce(b.text_plain, '') AS body
FROM messages AS m
LEFT JOIN bodies AS b ON b.message_id = m.id;

CREATE VIRTUAL TABLE message_fts USING fts5(
    subject, sender, recipients, body,
    content = 'message_fts_content',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO message_fts(rowid, subject, sender, recipients, body)
    SELECT id, subject, sender, recipients, body FROM message_fts_content WHERE id = NEW.id;
END;
CREATE TRIGGER messages_fts_delete BEFORE DELETE ON messages BEGIN
    INSERT INTO message_fts(message_fts, rowid, subject, sender, recipients, body)
    SELECT 'delete', id, subject, sender, recipients, body FROM message_fts_content WHERE id = OLD.id;
END;
CREATE TRIGGER messages_fts_update_old BEFORE UPDATE OF subject, from_name, from_addr, to_json, cc_json
ON messages BEGIN
    INSERT INTO message_fts(message_fts, rowid, subject, sender, recipients, body)
    SELECT 'delete', id, subject, sender, recipients, body FROM message_fts_content WHERE id = OLD.id;
END;
CREATE TRIGGER messages_fts_update_new AFTER UPDATE OF subject, from_name, from_addr, to_json, cc_json
ON messages BEGIN
    INSERT INTO message_fts(rowid, subject, sender, recipients, body)
    SELECT id, subject, sender, recipients, body FROM message_fts_content WHERE id = NEW.id;
END;

-- No BEFORE INSERT trigger on bodies: it would also fire for an upsert that turns into an
-- update. A new bodies row means the indexed body was empty until now.
CREATE TRIGGER bodies_fts_insert AFTER INSERT ON bodies BEGIN
    INSERT INTO message_fts(message_fts, rowid, subject, sender, recipients, body)
    SELECT 'delete', id, subject, sender, recipients, '' FROM message_fts_content WHERE id = NEW.message_id;
    INSERT INTO message_fts(rowid, subject, sender, recipients, body)
    SELECT id, subject, sender, recipients, body FROM message_fts_content WHERE id = NEW.message_id;
END;
CREATE TRIGGER bodies_fts_update_old BEFORE UPDATE OF text_plain ON bodies BEGIN
    INSERT INTO message_fts(message_fts, rowid, subject, sender, recipients, body)
    SELECT 'delete', id, subject, sender, recipients, body FROM message_fts_content WHERE id = OLD.message_id;
END;
CREATE TRIGGER bodies_fts_update_new AFTER UPDATE OF text_plain ON bodies BEGIN
    INSERT INTO message_fts(rowid, subject, sender, recipients, body)
    SELECT id, subject, sender, recipients, body FROM message_fts_content WHERE id = NEW.message_id;
END;
CREATE TRIGGER bodies_fts_delete_old BEFORE DELETE ON bodies BEGIN
    INSERT INTO message_fts(message_fts, rowid, subject, sender, recipients, body)
    SELECT 'delete', id, subject, sender, recipients, body FROM message_fts_content WHERE id = OLD.message_id;
END;
CREATE TRIGGER bodies_fts_delete_new AFTER DELETE ON bodies BEGIN
    INSERT INTO message_fts(rowid, subject, sender, recipients, body)
    SELECT id, subject, sender, recipients, body FROM message_fts_content WHERE id = OLD.message_id;
END;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn version(conn: &Connection) -> i64 {
        user_version(conn).unwrap()
    }

    fn has_table(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    }

    #[test]
    fn fresh_database_reaches_latest_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), MIGRATIONS.len() as i64);
        assert!(has_table(&conn, "messages"));
        assert!(has_table(&conn, "message_fts"));
    }

    #[test]
    fn rerunning_is_a_no_op() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO settings(key, value_json) VALUES ('k', '1')",
            [],
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let kept: i64 = conn
            .query_row("SELECT count(*) FROM settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 1);
        assert_eq!(version(&conn), MIGRATIONS.len() as i64);
    }

    #[test]
    fn only_unapplied_migrations_run() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_with(&mut conn, &["CREATE TABLE a(x INTEGER)"]).unwrap();
        conn.execute("INSERT INTO a VALUES (1)", []).unwrap();
        migrate_with(
            &mut conn,
            &["CREATE TABLE a(x INTEGER)", "CREATE TABLE b(y INTEGER)"],
        )
        .unwrap();
        assert_eq!(version(&conn), 2);
        assert!(has_table(&conn, "b"));
    }

    #[test]
    fn failed_migration_rolls_back_entirely() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_with(&mut conn, &["CREATE TABLE a(x INTEGER)"]).unwrap();
        let broken = [
            "CREATE TABLE a(x INTEGER)",
            "CREATE TABLE b(y INTEGER); NOT SQL",
        ];
        assert!(migrate_with(&mut conn, &broken).is_err());
        assert_eq!(version(&conn), 1);
        assert!(!has_table(&conn, "b"), "partial DDL must roll back");
    }

    #[test]
    fn newer_database_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", MIGRATIONS.len() as i64 + 1)
            .unwrap();
        let supported = MIGRATIONS.len() as i64;
        assert!(matches!(
            migrate(&mut conn),
            Err(StoreError::TooNew { found, supported: s }) if found == supported + 1 && s == supported
        ));
        assert_eq!(version(&conn), supported + 1, "never downgraded");
    }
}
