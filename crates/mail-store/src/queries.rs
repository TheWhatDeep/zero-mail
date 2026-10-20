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
             body_state, list_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
             ?19, ?20, ?21, ?22, ?23, ?24)",
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
            m.list_id,
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

const FOLDER_COLUMNS: &str = "id, account_id, path, delimiter, special_use, uidvalidity, uidnext,
    highestmodseq, sync_enabled, watch_idle, unread, total, synced_at, window_start";

fn folder_row(r: &Row) -> rusqlite::Result<Folder> {
    let special_use: Option<String> = r.get(4)?;
    let special_use = match special_use {
        None => None,
        Some(text) => Some(SpecialUse::from_sql(&text).ok_or(
            rusqlite::Error::InvalidColumnType(4, text, rusqlite::types::Type::Text),
        )?),
    };
    let modseq: Option<i64> = r.get(7)?;
    Ok(Folder {
        id: r.get(0)?,
        account_id: r.get(1)?,
        path: r.get(2)?,
        delimiter: r.get(3)?,
        special_use,
        uidvalidity: r.get(5)?,
        uidnext: r.get(6)?,
        highestmodseq: modseq.and_then(|m| u64::try_from(m).ok()),
        sync_enabled: r.get(8)?,
        watch_idle: r.get(9)?,
        unread: r.get(10)?,
        total: r.get(11)?,
        synced_at: r.get(12)?,
        window_start: r.get(13)?,
    })
}

pub(crate) fn list_folders(conn: &Connection, account_id: AccountId) -> Result<Vec<Folder>> {
    let sql = format!("SELECT {FOLDER_COLUMNS} FROM folders WHERE account_id = ?1 ORDER BY path");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([account_id], folder_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn get_folder(conn: &Connection, folder_id: FolderId) -> Result<Folder> {
    let sql = format!("SELECT {FOLDER_COLUMNS} FROM folders WHERE id = ?1");
    Ok(conn.query_row(&sql, [folder_id], folder_row)?)
}

/// Makes the account's folder rows match a `LIST` result in one transaction: listed paths
/// not yet known are inserted, and known paths no longer listed are deleted with their
/// messages (section 12: a folder gone from `LIST` is purged; a rename is delete plus new).
/// Known folders keep their role and switches, which the user may have changed.
pub(crate) fn apply_folder_list(
    tx: &Transaction,
    account_id: AccountId,
    listed: &[NewFolder],
) -> Result<Vec<Folder>> {
    if listed.iter().any(|f| f.account_id != account_id) {
        return Err(StoreError::Invalid("folder of another account"));
    }
    let known: Vec<(FolderId, String)> = {
        let mut stmt = tx.prepare_cached("SELECT id, path FROM folders WHERE account_id = ?1")?;
        let rows = stmt.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    // A folder op not yet acknowledged wins over the LIST (section 6, Conflicts): a folder
    // created or renamed here stays, and one deleted or renamed away is not brought back.
    let (protected, withdrawn) = pending_folder_ops(tx, account_id)?;
    for (id, path) in &known {
        if !listed.iter().any(|f| &f.path == path) && !protected.contains(id) {
            delete_folder(tx, *id)?;
        }
    }
    for f in listed {
        if !known.iter().any(|(_, path)| *path == f.path) && !withdrawn.contains(&f.path) {
            insert_folder(tx, f)?;
        }
    }
    let sql = format!("SELECT {FOLDER_COLUMNS} FROM folders WHERE account_id = ?1 ORDER BY path");
    let mut stmt = tx.prepare_cached(&sql)?;
    let rows = stmt.query_map([account_id], folder_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Records UIDVALIDITY the first time a folder is selected, before any message is cached,
/// so a later change is always detected (invariant 1). A folder that already has one is
/// left alone.
/// Unfinished folder ops: the folders they keep (created or renamed here) and the paths
/// they withdraw (deleted, or renamed away).
fn pending_folder_ops(
    tx: &Transaction,
    account_id: AccountId,
) -> Result<(Vec<FolderId>, Vec<String>)> {
    let mut stmt = tx.prepare_cached(
        "SELECT kind, folder_id, args_json FROM pending_ops
         WHERE account_id = ?1 AND state != 'done'
           AND kind IN ('folder_create', 'folder_rename', 'folder_delete')",
    )?;
    let rows = stmt.query_map([account_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, FolderId>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut protected = Vec::new();
    let mut withdrawn = Vec::new();
    for row in rows {
        let (kind, folder_id, args) = row?;
        let args: serde_json::Value = serde_json::from_str(&args)?;
        let path = args["args"]["path"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        match kind.as_str() {
            "folder_create" => protected.push(folder_id),
            "folder_rename" => {
                protected.push(folder_id);
                withdrawn.push(path);
            }
            _ => withdrawn.push(path),
        }
    }
    Ok((protected, withdrawn))
}

pub(crate) fn set_uidvalidity(
    tx: &Transaction,
    folder_id: FolderId,
    uidvalidity: u32,
) -> Result<()> {
    let updated = tx.execute(
        "UPDATE folders SET uidvalidity = ?2 WHERE id = ?1 AND uidvalidity = 0",
        params![folder_id, uidvalidity],
    )?;
    if updated == 0 {
        // Either unknown or already set; tell the two apart.
        tx.query_row("SELECT 1 FROM folders WHERE id = ?1", [folder_id], |_| {
            Ok(())
        })?;
    }
    Ok(())
}

/// Every cached UID of a folder, ascending, including `\Deleted` rows.
pub(crate) fn cached_uids(conn: &Connection, folder_id: FolderId) -> Result<Vec<u32>> {
    let mut stmt =
        conn.prepare_cached("SELECT uid FROM messages WHERE folder_id = ?1 ORDER BY uid")?;
    let rows = stmt.query_map([folder_id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Inserts synced headers and assigns threads (section 10; `threads`): one message at a
/// time, so a batch links within itself. The input `thread_id` is ignored.
pub(crate) fn insert_synced(
    tx: &Transaction,
    folder_id: FolderId,
    messages: Vec<NewMessage>,
) -> Result<Vec<MessageId>> {
    let account_id: AccountId = tx.query_row(
        "SELECT account_id FROM folders WHERE id = ?1",
        [folder_id],
        |r| r.get(0),
    )?;
    let subject_merge = crate::threads::subject_merge_on(tx)?;
    let mut ids = Vec::with_capacity(messages.len());
    for mut m in messages {
        // Contacts come from the From, To and Cc of mail received (section 10).
        let people: Vec<(String, String)> = std::iter::once(&m.from)
            .chain(&m.to)
            .chain(&m.cc)
            .map(|a| (a.addr.clone(), a.name.clone()))
            .collect();
        crate::outbox::bump_contacts(tx, account_id, &people, 1, m.date_utc)?;
        let links = crate::threads::link_ids(&m);
        m.thread_id = crate::threads::assign(tx, account_id, &m, &links, subject_merge)?;
        let id = insert_messages(tx, folder_id, std::slice::from_ref(&m))?;
        for id in id {
            crate::threads::record(tx, account_id, id, &links)?;
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Replaces the flags of cached messages by UID. Returns how many rows changed.
pub(crate) fn apply_flags(
    tx: &Transaction,
    folder_id: FolderId,
    changes: &[(u32, Flags)],
) -> Result<usize> {
    let mut update = tx.prepare_cached(
        "UPDATE messages SET flags = ?3 WHERE folder_id = ?1 AND uid = ?2 AND flags != ?3",
    )?;
    let mut changed = 0;
    for (uid, flags) in changes {
        changed += update.execute(params![folder_id, uid, flags.0])?;
    }
    // Pending ops win until acknowledged (invariant 2): their overlays go back on in the
    // same transaction, so no reader ever sees the server's flags alone.
    crate::ops::relay(tx, folder_id)?;
    Ok(changed)
}

/// Section 6, step 7: stores the server state and recounts the folder.
pub(crate) fn finish_folder_sync(
    tx: &Transaction,
    folder_id: FolderId,
    state: &FolderSyncState,
) -> Result<()> {
    let modseq = state
        .highestmodseq
        .map(i64::try_from)
        .transpose()
        .map_err(|_| StoreError::Invalid("modseq"))?;
    let updated = tx.execute(
        "UPDATE folders SET uidvalidity = ?2, uidnext = ?3, highestmodseq = ?4, synced_at = ?5,
             total = (SELECT count(*) FROM messages WHERE folder_id = ?1 AND (flags & ?6) = 0),
             unread = (SELECT count(*) FROM messages
                       WHERE folder_id = ?1 AND (flags & ?6) = 0 AND (flags & ?7) = 0)
         WHERE id = ?1",
        params![
            folder_id,
            state.uidvalidity,
            state.uidnext,
            modseq,
            state.synced_at,
            HIDDEN,
            i64::from(Flags::SEEN),
        ],
    )?;
    match updated {
        0 => Err(StoreError::NotFound),
        _ => Ok(()),
    }
}

/// Bodyless messages among the folder's newest `limit` (section 6, step 8: the prefetch cap
/// is the newest 200 messages per folder, whether or not they already have a body).
pub(crate) fn prefetch_candidates(
    conn: &Connection,
    folder_id: FolderId,
    limit: u32,
) -> Result<Vec<PrefetchItem>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, uid FROM (
             SELECT id, uid, body_state, date_utc FROM messages
             WHERE folder_id = ?1 AND (flags & ?2) = 0
             ORDER BY date_utc DESC, id DESC LIMIT ?3)
         WHERE body_state = 'none'
         ORDER BY date_utc DESC, id DESC",
    )?;
    let rows = stmt.query_map(params![folder_id, HIDDEN, limit], |r| {
        Ok(PrefetchItem {
            message_id: r.get(0)?,
            uid: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Stores a fetched body with the preview made from it, in one transaction.
pub(crate) fn store_body(
    tx: &Transaction,
    message_id: MessageId,
    body: &Body,
    state: BodyState,
    preview: &str,
) -> Result<()> {
    put_body(tx, message_id, body, state)?;
    tx.execute(
        "UPDATE messages SET preview = ?2 WHERE id = ?1",
        params![message_id, preview],
    )?;
    Ok(())
}

pub(crate) fn get_message(conn: &Connection, id: MessageId) -> Result<MessageRecord> {
    let (record, to, cc, reply_to) = conn.query_row(
        "SELECT id, account_id, folder_id, uid, thread_id, message_id, subject, from_name,
                from_addr, to_json, cc_json, reply_to_json, date_utc, internal_utc, size, flags,
                has_attach, body_state
         FROM messages WHERE id = ?1",
        [id],
        |r| {
            let size: i64 = r.get(14)?;
            Ok((
                MessageRecord {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    folder_id: r.get(2)?,
                    uid: r.get(3)?,
                    thread_id: r.get(4)?,
                    message_id: r.get(5)?,
                    subject: r.get(6)?,
                    from: Address {
                        name: r.get(7)?,
                        addr: r.get(8)?,
                    },
                    to: Vec::new(),
                    cc: Vec::new(),
                    reply_to: Vec::new(),
                    date_utc: r.get(12)?,
                    internal_utc: r.get(13)?,
                    size: u64::try_from(size).unwrap_or(0),
                    flags: Flags(r.get(15)?),
                    has_attach: r.get(16)?,
                    body_state: enum_column(r, 17, BodyState::from_sql)?,
                },
                r.get::<_, String>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, String>(11)?,
            ))
        },
    )?;
    Ok(MessageRecord {
        to: serde_json::from_str(&to)?,
        cc: serde_json::from_str(&cc)?,
        reply_to: serde_json::from_str(&reply_to)?,
        ..record
    })
}

pub(crate) fn get_body(conn: &Connection, id: MessageId) -> Result<Option<Body>> {
    let mut stmt = conn.prepare_cached(
        "SELECT text_plain, text_html, fetched_at FROM bodies WHERE message_id = ?1",
    )?;
    let mut rows = stmt.query([id])?;
    match rows.next()? {
        Some(r) => Ok(Some(Body {
            text_plain: r.get(0)?,
            text_html: r.get(1)?,
            fetched_at: r.get(2)?,
        })),
        None => Ok(None),
    }
}

const ATTACHMENT_COLUMNS: &str =
    "id, message_id, part_id, filename, mime, size, content_id, is_inline";

fn attachment_row(r: &Row) -> rusqlite::Result<AttachmentRecord> {
    let size: i64 = r.get(5)?;
    Ok(AttachmentRecord {
        id: r.get(0)?,
        message_id: r.get(1)?,
        attachment: NewAttachment {
            part_id: r.get(2)?,
            filename: r.get(3)?,
            mime: r.get(4)?,
            size: u64::try_from(size).unwrap_or(0),
            content_id: r.get(6)?,
            is_inline: r.get(7)?,
        },
    })
}

/// A message's attachment rows, in part order.
pub(crate) fn list_attachments(conn: &Connection, id: MessageId) -> Result<Vec<AttachmentRecord>> {
    let sql =
        format!("SELECT {ATTACHMENT_COLUMNS} FROM attachments WHERE message_id = ?1 ORDER BY id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([id], attachment_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn get_attachment(conn: &Connection, id: AttachmentId) -> Result<AttachmentRecord> {
    let sql = format!("SELECT {ATTACHMENT_COLUMNS} FROM attachments WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], attachment_row)?)
}

/// Always load remote images from this sender (section 8, `remote_image_allow`).
pub(crate) fn allow_remote_images(
    tx: &Transaction,
    account_id: AccountId,
    sender: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO remote_image_allow (account_id, sender_addr) VALUES (?1, ?2)
         ON CONFLICT DO NOTHING",
        params![account_id, sender],
    )?;
    Ok(())
}

pub(crate) fn remote_images_allowed(
    conn: &Connection,
    account_id: AccountId,
    sender: &str,
) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM remote_image_allow WHERE account_id = ?1 AND sender_addr = ?2",
        params![account_id, sender],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `PRAGMA quick_check` (section 12, SPEC_GAPS G48).
pub(crate) fn quick_check(conn: &Connection) -> Result<()> {
    let result: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if result == "ok" {
        Ok(())
    } else {
        Err(StoreError::Corrupt)
    }
}

/// `PRAGMA integrity_check` (section 12).
pub(crate) fn integrity_check(conn: &Connection) -> Result<()> {
    let result: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if result == "ok" {
        Ok(())
    } else {
        Err(StoreError::Corrupt)
    }
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

/// `m.folder_id IN (...)` for ids from the store itself (integers, so formatting is safe).
fn in_list(ids: &[FolderId]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// The visible rows of the same Gmail message (one per label, section 18.1), including the
/// message itself. Empty for a message without `X-GM-MSGID`.
pub(crate) fn copies(conn: &Connection, message: MessageId) -> Result<Vec<MessageCopy>> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.id, c.folder_id, c.uid FROM messages AS m
         JOIN messages AS c ON c.account_id = m.account_id AND c.gm_msgid = m.gm_msgid
         WHERE m.id = ?1 AND m.gm_msgid IS NOT NULL AND (c.flags & ?2) = 0
         ORDER BY c.id",
    )?;
    let rows = stmt.query_map(params![message, HIDDEN], |r| {
        Ok(MessageCopy {
            id: r.get(0)?,
            folder_id: r.get(1)?,
            uid: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The `X-GM-LABELS` stored with a message; `None` when it has none (not Gmail).
pub(crate) fn gm_labels(conn: &Connection, message: MessageId) -> Result<Option<Vec<String>>> {
    let json: Option<String> = conn
        .prepare_cached("SELECT gm_labels_json FROM messages WHERE id = ?1")?
        .query_row([message], |r| r.get(0))?;
    Ok(json.map(|j| serde_json::from_str(&j)).transpose()?)
}

/// Newest first across several folders (All Inboxes, section 11), strictly after the cursor.
pub(crate) fn folders_page(
    conn: &Connection,
    folders: &[FolderId],
    cursor: Option<Cursor>,
    limit: u32,
) -> Result<Page> {
    if folders.is_empty() {
        return Ok(Page {
            rows: Vec::new(),
            next: None,
        });
    }
    let (date, id) = start(cursor);
    let sql = format!(
        concat!(
            "SELECT ",
            row_columns!(),
            " FROM messages AS m
              WHERE m.folder_id IN ({}) AND (m.flags & ?1) = 0 AND (m.date_utc, m.id) < (?2, ?3)
              ORDER BY m.date_utc DESC, m.id DESC
              LIMIT ?4"
        ),
        in_list(folders)
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![HIDDEN, date, id, fetch_limit(limit)?], list_row)?;
    page(rows, limit)
}

/// One row per thread among `folders`, newest first by the thread's latest message there
/// (section 10, rule 5). The counts and participants cover the thread's messages outside
/// `exclude` (Trash, Junk and Drafts unless one of them is being listed, rule 6), and a
/// Gmail message under two labels counts once (section 9).
pub(crate) fn threads_page(
    conn: &Connection,
    folders: &[FolderId],
    exclude: &[FolderId],
    cursor: Option<Cursor>,
    limit: u32,
) -> Result<ThreadPage> {
    if folders.is_empty() {
        return Ok(ThreadPage {
            rows: Vec::new(),
            next: None,
        });
    }
    let (date, id) = start(cursor);
    let sql = format!(
        concat!(
            "SELECT * FROM (SELECT ",
            row_columns!(),
            ", row_number() OVER (PARTITION BY m.thread_id ORDER BY m.date_utc DESC, m.id DESC) AS rn
              FROM messages AS m WHERE m.folder_id IN ({}) AND (m.flags & ?1) = 0)
              WHERE rn = 1 AND (date_utc, id) < (?2, ?3)
              ORDER BY date_utc DESC, id DESC
              LIMIT ?4"
        ),
        in_list(folders)
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![HIDDEN, date, id, fetch_limit(limit)?], list_row)?;
    let Page { rows, next } = page(rows, limit)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let summary = thread_summary(conn, row.thread_id, exclude)?;
        out.push(ThreadRow {
            row,
            messages: summary.0,
            unread: summary.1,
            participants: summary.2,
        });
    }
    Ok(ThreadPage { rows: out, next })
}

/// Message count, unread count and up to 3 participant names of a thread.
fn thread_summary(
    conn: &Connection,
    thread: ThreadId,
    exclude: &[FolderId],
) -> Result<(u32, u32, Vec<String>)> {
    let not_in = if exclude.is_empty() {
        String::new()
    } else {
        format!(" AND folder_id NOT IN ({})", in_list(exclude))
    };
    let (messages, unread): (u32, u32) = conn
        .prepare_cached(&format!(
            "SELECT count(DISTINCT coalesce(gm_msgid, -id)),
                    count(DISTINCT CASE WHEN (flags & 1) = 0 THEN coalesce(gm_msgid, -id) END)
             FROM messages WHERE thread_id = ?1 AND (flags & ?2) = 0{not_in}"
        ))?
        .query_row(params![thread, HIDDEN], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT from_name, from_addr FROM messages
         WHERE thread_id = ?1 AND (flags & ?2) = 0{not_in} ORDER BY date_utc, id"
    ))?;
    let senders = stmt.query_map(params![thread, HIDDEN], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut names: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for s in senders {
        let (name, addr) = s?;
        let key = addr.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        if names.len() < 3 {
            names.push(if name.is_empty() { addr } else { name });
        }
    }
    Ok((messages, unread, names))
}

/// A thread's messages outside `exclude`, oldest first (section 10, rule 6). A Gmail message
/// under two labels is listed once, preferring its row in `prefer` (the open folder).
pub(crate) fn thread_messages(
    conn: &Connection,
    thread: ThreadId,
    exclude: &[FolderId],
    prefer: Option<FolderId>,
) -> Result<Vec<ListRow>> {
    let not_in = if exclude.is_empty() {
        String::new()
    } else {
        format!(" AND m.folder_id NOT IN ({})", in_list(exclude))
    };
    let sql = format!(
        concat!(
            "SELECT ",
            row_columns!(),
            ", m.gm_msgid FROM messages AS m
              WHERE m.thread_id = ?1 AND (m.flags & ?2) = 0{}
              ORDER BY m.date_utc, m.id"
        ),
        not_in
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![thread, HIDDEN], |r| {
        Ok((list_row(r)?, r.get::<_, Option<i64>>(12)?))
    })?;
    let mut out: Vec<(ListRow, Option<i64>)> = Vec::new();
    for row in rows {
        let ((mut row, to_json), gm) = row?;
        row.to = serde_json::from_str(&to_json)?;
        match gm.and_then(|g| out.iter().position(|(_, o)| *o == Some(g))) {
            Some(i) => {
                if Some(row.folder_id) == prefer {
                    out[i] = (row, gm);
                }
            }
            None => out.push((row, gm)),
        }
    }
    Ok(out.into_iter().map(|(r, _)| r).collect())
}

/// Newest first among messages matching a clause the `search` module built (section 10),
/// strictly after the cursor. The clause uses `?` placeholders bound to `q.params` in order.
/// A Gmail message under two labels is listed once (section 9).
pub(crate) fn query_page(
    conn: &Connection,
    q: &SearchSql,
    cursor: Option<Cursor>,
    limit: u32,
) -> Result<Page> {
    let (date, id) = start(cursor);
    let sql = format!(
        concat!(
            "SELECT ",
            row_columns!(),
            " FROM messages AS m
              WHERE (m.flags & {hidden}) = 0 AND ({clause})
                AND (m.gm_msgid IS NULL OR m.id = (SELECT min(g.id) FROM messages AS g
                     WHERE g.account_id = m.account_id AND g.gm_msgid = m.gm_msgid
                       AND (g.flags & {hidden}) = 0 AND ({clause_g})))
                AND (m.date_utc, m.id) < (?, ?)
              ORDER BY m.date_utc DESC, m.id DESC
              LIMIT ?"
        ),
        hidden = HIDDEN,
        clause = q.clause,
        clause_g = q.clause.replace("m.", "g."),
    );
    let mut values: Vec<rusqlite::types::Value> = q.params.iter().map(SqlParam::value).collect();
    values.extend(q.params.iter().map(SqlParam::value));
    values.push(date.into());
    values.push(id.into());
    values.push(fetch_limit(limit)?.into());
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(values), list_row)?;
    page(rows, limit)
}

/// How many visible messages match a clause the `search` module built.
pub(crate) fn query_count(conn: &Connection, q: &SearchSql) -> Result<u32> {
    let sql = format!(
        "SELECT count(*) FROM messages AS m WHERE (m.flags & {HIDDEN}) = 0 AND ({})",
        q.clause
    );
    let values: Vec<rusqlite::types::Value> = q.params.iter().map(SqlParam::value).collect();
    Ok(conn.query_row(&sql, rusqlite::params_from_iter(values), |r| r.get(0))?)
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

/// Sets where a folder's sync window starts (SPEC_GAPS G75). With `only_back`, the start
/// only moves earlier (Load older); a first sync sets it outright.
pub(crate) fn set_window_start(
    tx: &Transaction,
    folder_id: FolderId,
    start: i64,
    only_back: bool,
) -> Result<()> {
    let sql = if only_back {
        "UPDATE folders SET window_start = ?2
         WHERE id = ?1 AND (window_start = 0 OR ?2 < window_start)"
    } else {
        "UPDATE folders SET window_start = ?2 WHERE id = ?1"
    };
    tx.execute(sql, params![folder_id, start])?;
    Ok(())
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
