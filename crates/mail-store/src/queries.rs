//! SQL for the store API. Writers take the writer's open transaction; readers take a
//! read-only connection. Lists use keyset pagination on `(date, id)`, never an offset
//! (section 14), and every list hides `\Deleted` messages (invariant 8).

use crate::error::{Result, StoreError};
use crate::types::*;
use rusqlite::{params, Connection, Row, Transaction};

const HIDDEN: i64 = Flags::DELETED as i64;

pub(crate) fn insert_account(tx: &Transaction, a: &NewAccount) -> Result<AccountId> {
    tx.execute(
        "INSERT INTO accounts (name, email, imap_host, imap_port, imap_security, smtp_host,
             smtp_port, smtp_security, username, auth_kind, oauth_client_id, pinned_cert_sha256,
             sync_days, save_sent_copy, signature_html, signature_text, enabled, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            a.name,
            a.email,
            a.imap_host,
            a.imap_port,
            a.imap_security.as_sql(),
            a.smtp_host,
            a.smtp_port,
            a.smtp_security.as_sql(),
            a.username,
            a.auth_kind.as_sql(),
            a.oauth_client_id,
            a.pinned_cert_sha256,
            a.sync_days,
            a.save_sent_copy.as_sql(),
            a.signature_html,
            a.signature_text,
            a.enabled,
            a.created_at,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// An account and its first identity, in one transaction: every account has at least one
/// identity (section 5).
pub(crate) fn add_account(
    tx: &Transaction,
    a: &NewAccount,
    identity: &NewIdentity,
) -> Result<(AccountId, IdentityId)> {
    let account_id = insert_account(tx, a)?;
    let identity_id = insert_identity(tx, account_id, identity)?;
    Ok((account_id, identity_id))
}

pub(crate) fn insert_identity(
    tx: &Transaction,
    account_id: AccountId,
    i: &NewIdentity,
) -> Result<IdentityId> {
    tx.execute(
        "INSERT INTO identities (account_id, name, address, reply_to, is_default)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![account_id, i.name, i.address, i.reply_to, i.is_default],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Replaces every column of an account except its id.
pub(crate) fn update_account(tx: &Transaction, id: AccountId, a: &NewAccount) -> Result<()> {
    let updated = tx.execute(
        "UPDATE accounts SET name = ?2, email = ?3, imap_host = ?4, imap_port = ?5,
             imap_security = ?6, smtp_host = ?7, smtp_port = ?8, smtp_security = ?9,
             username = ?10, auth_kind = ?11, oauth_client_id = ?12, pinned_cert_sha256 = ?13,
             sync_days = ?14, save_sent_copy = ?15, signature_html = ?16, signature_text = ?17,
             enabled = ?18, created_at = ?19
         WHERE id = ?1",
        params![
            id,
            a.name,
            a.email,
            a.imap_host,
            a.imap_port,
            a.imap_security.as_sql(),
            a.smtp_host,
            a.smtp_port,
            a.smtp_security.as_sql(),
            a.username,
            a.auth_kind.as_sql(),
            a.oauth_client_id,
            a.pinned_cert_sha256,
            a.sync_days,
            a.save_sent_copy.as_sql(),
            a.signature_html,
            a.signature_text,
            a.enabled,
            a.created_at,
        ],
    )?;
    match updated {
        0 => Err(StoreError::NotFound),
        _ => Ok(()),
    }
}

pub(crate) fn set_account_enabled(tx: &Transaction, id: AccountId, enabled: bool) -> Result<()> {
    match tx.execute(
        "UPDATE accounts SET enabled = ?2 WHERE id = ?1",
        params![id, enabled],
    )? {
        0 => Err(StoreError::NotFound),
        _ => Ok(()),
    }
}

const ACCOUNT_COLUMNS: &str = "id, name, email, imap_host, imap_port, imap_security, smtp_host,
    smtp_port, smtp_security, username, auth_kind, oauth_client_id, pinned_cert_sha256,
    sync_days, save_sent_copy, signature_html, signature_text, enabled, created_at";

/// Reads an enumerated text column, rejecting values the schema's CHECK would not allow.
fn enum_column<T>(r: &Row, i: usize, parse: fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    let text: String = r.get(i)?;
    parse(&text).ok_or(rusqlite::Error::InvalidColumnType(
        i,
        text,
        rusqlite::types::Type::Text,
    ))
}

fn account_row(r: &Row) -> rusqlite::Result<Account> {
    Ok(Account {
        id: r.get(0)?,
        config: NewAccount {
            name: r.get(1)?,
            email: r.get(2)?,
            imap_host: r.get(3)?,
            imap_port: r.get(4)?,
            imap_security: enum_column(r, 5, Security::from_sql)?,
            smtp_host: r.get(6)?,
            smtp_port: r.get(7)?,
            smtp_security: enum_column(r, 8, Security::from_sql)?,
            username: r.get(9)?,
            auth_kind: enum_column(r, 10, AuthKind::from_sql)?,
            oauth_client_id: r.get(11)?,
            pinned_cert_sha256: r.get(12)?,
            sync_days: r.get(13)?,
            save_sent_copy: enum_column(r, 14, SaveSentCopy::from_sql)?,
            signature_html: r.get(15)?,
            signature_text: r.get(16)?,
            enabled: r.get(17)?,
            created_at: r.get(18)?,
        },
    })
}

pub(crate) fn get_account(conn: &Connection, id: AccountId) -> Result<Account> {
    let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], account_row)?)
}

pub(crate) fn list_accounts(conn: &Connection) -> Result<Vec<Account>> {
    let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM accounts ORDER BY id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], account_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn list_identities(conn: &Connection, account_id: AccountId) -> Result<Vec<Identity>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, account_id, name, address, reply_to, is_default FROM identities
         WHERE account_id = ?1 ORDER BY is_default DESC, id",
    )?;
    let rows = stmt.query_map([account_id], |r| {
        Ok(Identity {
            id: r.get(0)?,
            account_id: r.get(1)?,
            identity: NewIdentity {
                name: r.get(2)?,
                address: r.get(3)?,
                reply_to: r.get(4)?,
                is_default: r.get(5)?,
            },
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn get_setting(conn: &Connection, key: &str) -> Result<Option<serde_json::Value>> {
    let mut stmt = conn.prepare_cached("SELECT value_json FROM settings WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    match rows.next()? {
        Some(row) => {
            let text: String = row.get(0)?;
            Ok(Some(serde_json::from_str(&text)?))
        }
        None => Ok(None),
    }
}

pub(crate) fn set_setting(tx: &Transaction, key: &str, value: &serde_json::Value) -> Result<()> {
    tx.execute(
        "INSERT INTO settings (key, value_json) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json",
        params![key, serde_json::to_string(value)?],
    )?;
    Ok(())
}

/// Removes the account and, by cascade, every row that belongs to it.
pub(crate) fn delete_account(tx: &Transaction, account_id: AccountId) -> Result<()> {
    match tx.execute("DELETE FROM accounts WHERE id = ?1", [account_id])? {
        0 => Err(StoreError::NotFound),
        _ => Ok(()),
    }
}

pub(crate) fn insert_folder(tx: &Transaction, f: &NewFolder) -> Result<FolderId> {
    tx.execute(
        "INSERT INTO folders (account_id, path, delimiter, special_use, uidvalidity, uidnext,
             highestmodseq, sync_enabled, watch_idle, unread, total, synced_at)
         VALUES (?1, ?2, ?3, ?4, 0, 0, NULL, ?5, ?6, 0, 0, 0)",
        params![
            f.account_id,
            f.path,
            f.delimiter,
            f.special_use.map(SpecialUse::as_sql),
            f.sync_enabled,
            f.watch_idle,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Deletes the folder; messages, bodies and attachments follow by cascade (invariant 7).
pub(crate) fn delete_folder(tx: &Transaction, folder_id: FolderId) -> Result<()> {
    match tx.execute("DELETE FROM folders WHERE id = ?1", [folder_id])? {
        0 => Err(StoreError::NotFound),
        _ => Ok(()),
    }
}

/// UIDVALIDITY changed (invariant 1, section 12): drops every message, body and attachment
/// of the folder and its pending ops, stores the new UIDVALIDITY and resets the sync state
/// so the next pull is a first sync. The caller runs this inside one transaction.
pub(crate) fn purge_folder(tx: &Transaction, folder_id: FolderId, uidvalidity: u32) -> Result<()> {
    let updated = tx.execute(
        "UPDATE folders SET uidvalidity = ?2, uidnext = 0, highestmodseq = NULL,
             unread = 0, total = 0, synced_at = 0
         WHERE id = ?1",
        params![folder_id, uidvalidity],
    )?;
    if updated == 0 {
        return Err(StoreError::NotFound);
    }
    tx.execute("DELETE FROM messages WHERE folder_id = ?1", [folder_id])?;
    tx.execute("DELETE FROM pending_ops WHERE folder_id = ?1", [folder_id])?;
    Ok(())
}

pub(crate) fn insert_thread(
    tx: &Transaction,
    account_id: AccountId,
    last_date_utc: i64,
    subject_norm: &str,
) -> Result<ThreadId> {
    tx.execute(
        "INSERT INTO threads (account_id, last_date_utc, subject_norm) VALUES (?1, ?2, ?3)",
        params![account_id, last_date_utc, subject_norm],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Inserts a batch of headers into one folder. A UID already cached is an error, so a
/// resumed sync never creates a duplicate row (invariant 1).
pub(crate) fn insert_messages(
    tx: &Transaction,
    folder_id: FolderId,
    messages: &[NewMessage],
) -> Result<Vec<MessageId>> {
    let account_id: AccountId = tx.query_row(
        "SELECT account_id FROM folders WHERE id = ?1",
        [folder_id],
        |r| r.get(0),
    )?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO messages (account_id, folder_id, uid, gm_msgid, gm_thrid, message_id,
             in_reply_to, refs, gm_labels_json, thread_id, subject, from_name, from_addr, to_json,
             cc_json, reply_to_json, date_utc, internal_utc, size, flags, has_attach, preview,
             body_state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
             ?19, ?20, ?21, ?22, ?23)",
    )?;
    let mut attach = tx.prepare_cached(
        "INSERT INTO attachments (message_id, part_id, filename, mime, size, content_id, is_inline)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    let mut ids = Vec::with_capacity(messages.len());
    for m in messages {
        let labels = m
            .gm_labels
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        insert.execute(params![
            account_id,
            folder_id,
            m.uid,
            m.gm_msgid,
            m.gm_thrid,
            m.message_id,
            m.in_reply_to,
            m.refs,
            labels,
            m.thread_id,
            m.subject,
            m.from.name,
            m.from.addr,
            serde_json::to_string(&m.to)?,
            serde_json::to_string(&m.cc)?,
            serde_json::to_string(&m.reply_to)?,
            m.date_utc,
            m.internal_utc,
            i64::try_from(m.size).map_err(|_| StoreError::Invalid("size"))?,
            m.flags.0,
            m.has_attach,
            m.preview,
            m.body_state.as_sql(),
        ])?;
        let id = tx.last_insert_rowid();
        for a in &m.attachments {
            attach.execute(params![
                id,
                a.part_id,
                a.filename,
                a.mime,
                i64::try_from(a.size).map_err(|_| StoreError::Invalid("size"))?,
                a.content_id,
                a.is_inline,
            ])?;
        }
        ids.push(id);
    }
    Ok(ids)
}

/// Deletes messages by UID (expunged on the server); bodies and attachments cascade.
pub(crate) fn delete_messages(
    tx: &Transaction,
    folder_id: FolderId,
    uids: &[u32],
) -> Result<usize> {
    let mut delete = tx.prepare_cached("DELETE FROM messages WHERE folder_id = ?1 AND uid = ?2")?;
    let mut removed = 0;
    for uid in uids {
        removed += delete.execute(params![folder_id, uid])?;
    }
    Ok(removed)
}

/// Stores or replaces a message's body and its `body_state`. The FTS index follows.
pub(crate) fn put_body(
    tx: &Transaction,
    message_id: MessageId,
    body: &Body,
    state: BodyState,
) -> Result<()> {
    let updated = tx.execute(
        "UPDATE messages SET body_state = ?2 WHERE id = ?1",
        params![message_id, state.as_sql()],
    )?;
    if updated == 0 {
        return Err(StoreError::NotFound);
    }
    tx.execute(
        "INSERT INTO bodies (message_id, text_plain, text_html, fetched_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (message_id) DO UPDATE SET
             text_plain = excluded.text_plain,
             text_html = excluded.text_html,
             fetched_at = excluded.fetched_at",
        params![message_id, body.text_plain, body.text_html, body.fetched_at],
    )?;
    Ok(())
}

/// The `ListRow` columns, in `list_row` order.
macro_rules! row_columns {
    () => {
        "m.id, m.account_id, m.folder_id, m.thread_id, m.subject, m.from_name, m.from_addr,
         m.to_json, m.date_utc, m.flags, m.has_attach, m.preview"
    };
}

/// Newest first in one folder, strictly after the cursor.
pub(crate) const FOLDER_PAGE_SQL: &str = concat!(
    "SELECT ",
    row_columns!(),
    " FROM messages AS m
      WHERE m.folder_id = ?1 AND (m.flags & ?2) = 0 AND (m.date_utc, m.id) < (?3, ?4)
      ORDER BY m.date_utc DESC, m.id DESC
      LIMIT ?5"
);

/// Newest first among FTS matches, strictly after the cursor.
pub(crate) const SEARCH_PAGE_SQL: &str = concat!(
    "SELECT ",
    row_columns!(),
    " FROM message_fts JOIN messages AS m ON m.id = message_fts.rowid
      WHERE message_fts MATCH ?1 AND (m.flags & ?2) = 0 AND (m.date_utc, m.id) < (?3, ?4)
      ORDER BY m.date_utc DESC, m.id DESC
      LIMIT ?5"
);

pub(crate) fn folder_page(
    conn: &Connection,
    folder_id: FolderId,
    cursor: Option<Cursor>,
    limit: u32,
) -> Result<Page> {
    let (date, id) = start(cursor);
    let mut stmt = conn.prepare_cached(FOLDER_PAGE_SQL)?;
    let rows = stmt.query_map(
        params![folder_id, HIDDEN, date, id, fetch_limit(limit)?],
        list_row,
    )?;
    page(rows, limit)
}

/// Full-text match (FTS5 query syntax), newest first. Search scoping and the query language
/// are the `search` module's job (section 10).
pub(crate) fn search_page(
    conn: &Connection,
    fts_query: &str,
    cursor: Option<Cursor>,
    limit: u32,
) -> Result<Page> {
    let (date, id) = start(cursor);
    let mut stmt = conn.prepare_cached(SEARCH_PAGE_SQL)?;
    let rows = stmt.query_map(
        params![fts_query, HIDDEN, date, id, fetch_limit(limit)?],
        list_row,
    )?;
    page(rows, limit)
}

fn start(cursor: Option<Cursor>) -> (i64, i64) {
    cursor.map_or((i64::MAX, i64::MAX), |c| (c.date_utc, c.id))
}

/// One row more than requested, to know whether another page exists.
fn fetch_limit(limit: u32) -> Result<i64> {
    match limit {
        0 => Err(StoreError::Invalid("limit must be at least 1")),
        n => Ok(i64::from(n) + 1),
    }
}

fn page(
    rows: impl Iterator<Item = rusqlite::Result<(ListRow, String)>>,
    limit: u32,
) -> Result<Page> {
    let mut out = Vec::new();
    let mut more = false;
    for row in rows {
        if out.len() == limit as usize {
            more = true;
            break;
        }
        let (mut row, to_json) = row?;
        row.to = serde_json::from_str(&to_json)?;
        out.push(row);
    }
    let next = match (more, out.last()) {
        (true, Some(last)) => Some(Cursor {
            date_utc: last.date_utc,
            id: last.id,
        }),
        _ => None,
    };
    Ok(Page { rows: out, next })
}

fn list_row(r: &Row) -> rusqlite::Result<(ListRow, String)> {
    Ok((
        ListRow {
            id: r.get(0)?,
            account_id: r.get(1)?,
            folder_id: r.get(2)?,
            thread_id: r.get(3)?,
            subject: r.get(4)?,
            from: Address {
                name: r.get(5)?,
                addr: r.get(6)?,
            },
            to: Vec::new(),
            date_utc: r.get(8)?,
            flags: Flags(r.get(9)?),
            has_attach: r.get(10)?,
            preview: r.get(11)?,
        },
        r.get(7)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(sql: &str) -> String {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&mut conn).unwrap();
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let lines: Vec<String> = stmt
            .query_map(params!["x", HIDDEN, i64::MAX, i64::MAX, 101], |r| {
                r.get::<_, String>(3)
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        lines.join("\n")
    }

    #[test]
    fn folder_page_is_an_index_range_scan_without_a_sort() {
        let plan = plan(FOLDER_PAGE_SQL);
        assert!(
            // A seek into the index at the cursor date; equal dates are cut by id in order.
            plan.contains("SEARCH m USING INDEX messages_folder_date (folder_id=? AND date_utc<"),
            "{plan}"
        );
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }

    #[test]
    fn no_query_uses_an_offset() {
        // Section 14: keyset pagination, never OFFSET. SQL here is written in upper case.
        let needle = concat!("OFF", "SET");
        for src in [include_str!("queries.rs"), include_str!("lib.rs")] {
            let code = src.split("#[cfg(test)]").next().unwrap_or(src);
            assert!(!code.contains(needle));
        }
    }
}
