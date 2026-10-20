#![forbid(unsafe_code)]
//! `store` (BUILD.md section 3): schema, migrations, queries and FTS. Calls no other module.
//!
//! SQLite in WAL mode with foreign keys on (section 4). One writer thread owns the only
//! read-write connection and runs each write as one transaction, in arrival order; a pool of
//! read-only connections serves queries concurrently (section 3, process model). Callers
//! never see SQL or a connection.

mod error;
mod ops;
mod outbox;
mod queries;
mod schema;
mod threads;
mod types;

pub use error::{Result, StoreError};
pub use threads::{subject_norm, SUBJECT_MERGE_SETTING};
pub use types::*;

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};

/// Read-only connections in the pool.
pub const READERS: usize = 4;
/// Writes waiting for the writer thread before callers wait to enqueue.
const WRITE_QUEUE: usize = 64;
/// How long a connection waits on a lock held by another connection.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// Handle to the database. Cheap to clone; the writer thread stops and every connection
/// closes when the last clone is dropped.
#[derive(Debug, Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    writer: Option<mpsc::Sender<Job>>,
    writer_thread: Option<JoinHandle<()>>,
    readers: Mutex<Vec<Connection>>,
    permits: Arc<Semaphore>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Closing the channel ends the writer loop once queued writes have committed.
        self.writer.take();
        if let Some(thread) = self.writer_thread.take() {
            let _ = thread.join();
        }
    }
}

impl Store {
    /// Opens or creates the database at `path`, switches it to WAL and applies pending
    /// migrations. Blocking; call it at startup or from a blocking task.
    pub fn open(path: &Path) -> Result<Self> {
        let mut writer = Connection::open(path)?;
        restrict_permissions(path)?;
        let mode: String = writer.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::NotWal(mode));
        }
        // FULL: a committed outbox or pending-op row survives power loss (principle 2).
        writer.pragma_update(None, "synchronous", "FULL")?;
        writer.pragma_update(None, "foreign_keys", true)?;
        writer.busy_timeout(BUSY_TIMEOUT)?;
        let before = schema::migrate(&mut writer)?;
        if before < schema::THREADING_VERSION {
            let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
            threads::rethread_all(&tx)?;
            tx.commit()?;
        }

        let mut readers = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            let reader = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_URI,
            )?;
            reader.busy_timeout(BUSY_TIMEOUT)?;
            reader.pragma_update(None, "query_only", true)?;
            readers.push(reader);
        }

        let (tx, mut rx) = mpsc::channel::<Job>(WRITE_QUEUE);
        let writer_thread = std::thread::Builder::new()
            .name("store-writer".into())
            .spawn(move || {
                while let Some(job) = rx.blocking_recv() {
                    job(&mut writer);
                }
            })?;

        Ok(Self {
            inner: Arc::new(Inner {
                writer: Some(tx),
                writer_thread: Some(writer_thread),
                readers: Mutex::new(readers),
                permits: Arc::new(Semaphore::new(READERS)),
            }),
        })
    }

    /// Runs `f` in one transaction on the writer thread. It commits if `f` returns `Ok`,
    /// and rolls back otherwise (including on `SQLITE_FULL`, section 12).
    async fn write<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Transaction) -> Result<R> + Send + 'static,
    {
        let (done, result) = oneshot::channel();
        let job: Job = Box::new(move |conn| {
            let run = || {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let value = f(&tx)?;
                tx.commit()?;
                Ok(value)
            };
            let _ = done.send(run());
        });
        let sender = self.inner.writer.as_ref().ok_or(StoreError::Closed)?;
        sender.send(job).await.map_err(|_| StoreError::Closed)?;
        result.await.map_err(|_| StoreError::Closed)?
    }

    /// Runs `f` on a pooled read-only connection. If the caller stops waiting, the query
    /// still finishes and the connection returns to the pool.
    async fn read<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> Result<R> + Send + 'static,
    {
        let permit = Arc::clone(&self.inner.permits)
            .acquire_owned()
            .await
            .map_err(|_| StoreError::Closed)?;
        let conn = lock(&self.inner.readers).pop().ok_or(StoreError::Closed)?;
        let lease = Lease {
            conn: Some(conn),
            inner: Arc::clone(&self.inner),
            _permit: permit,
        };
        tokio::task::spawn_blocking(move || match &lease.conn {
            Some(conn) => f(conn),
            None => Err(StoreError::Closed),
        })
        .await
        .map_err(|_| StoreError::Closed)?
    }

    pub async fn insert_account(&self, account: NewAccount) -> Result<AccountId> {
        self.write(move |tx| queries::insert_account(tx, &account))
            .await
    }

    /// Inserts an account with its first identity in one transaction (section 5).
    pub async fn add_account(
        &self,
        account: NewAccount,
        identity: NewIdentity,
    ) -> Result<(AccountId, IdentityId)> {
        self.write(move |tx| queries::add_account(tx, &account, &identity))
            .await
    }

    pub async fn get_account(&self, account_id: AccountId) -> Result<Account> {
        self.read(move |conn| queries::get_account(conn, account_id))
            .await
    }

    /// Every account, oldest first.
    pub async fn list_accounts(&self) -> Result<Vec<Account>> {
        self.read(queries::list_accounts).await
    }

    /// Replaces an account's columns (`account.update`).
    pub async fn update_account(&self, account_id: AccountId, account: NewAccount) -> Result<()> {
        self.write(move |tx| queries::update_account(tx, account_id, &account))
            .await
    }

    pub async fn set_account_enabled(&self, account_id: AccountId, enabled: bool) -> Result<()> {
        self.write(move |tx| queries::set_account_enabled(tx, account_id, enabled))
            .await
    }

    /// An account's identities, the default first.
    pub async fn list_identities(&self, account_id: AccountId) -> Result<Vec<Identity>> {
        self.read(move |conn| queries::list_identities(conn, account_id))
            .await
    }

    /// A setting's JSON value, or `None` when it was never set.
    pub async fn get_setting(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let key = key.to_string();
        self.read(move |conn| queries::get_setting(conn, &key))
            .await
    }

    pub async fn set_setting(&self, key: &str, value: serde_json::Value) -> Result<()> {
        let key = key.to_string();
        self.write(move |tx| queries::set_setting(tx, &key, &value))
            .await
    }

    /// Removes an account and every row that belongs to it.
    pub async fn delete_account(&self, account_id: AccountId) -> Result<()> {
        self.write(move |tx| queries::delete_account(tx, account_id))
            .await
    }

    pub async fn insert_folder(&self, folder: NewFolder) -> Result<FolderId> {
        self.write(move |tx| queries::insert_folder(tx, &folder))
            .await
    }

    /// Deletes a folder with its messages, bodies and attachments (invariant 7).
    pub async fn delete_folder(&self, folder_id: FolderId) -> Result<()> {
        self.write(move |tx| queries::delete_folder(tx, folder_id))
            .await
    }

    /// UIDVALIDITY changed: purges the folder's messages, bodies, attachments and pending
    /// ops and stores the new value, in one transaction (invariant 1, section 12).
    pub async fn purge_folder(&self, folder_id: FolderId, uidvalidity: u32) -> Result<()> {
        self.write(move |tx| queries::purge_folder(tx, folder_id, uidvalidity))
            .await
    }

    pub async fn insert_thread(
        &self,
        account_id: AccountId,
        last_date_utc: i64,
        subject_norm: String,
    ) -> Result<ThreadId> {
        self.write(move |tx| queries::insert_thread(tx, account_id, last_date_utc, &subject_norm))
            .await
    }

    /// Inserts a batch of headers into a folder as one transaction (section 6, step 4).
    pub async fn insert_messages(
        &self,
        folder_id: FolderId,
        messages: Vec<NewMessage>,
    ) -> Result<Vec<MessageId>> {
        self.write(move |tx| queries::insert_messages(tx, folder_id, &messages))
            .await
    }

    /// Deletes messages by UID; their bodies and attachments go with them.
    pub async fn delete_messages(&self, folder_id: FolderId, uids: Vec<u32>) -> Result<usize> {
        self.write(move |tx| queries::delete_messages(tx, folder_id, &uids))
            .await
    }

    pub async fn put_body(
        &self,
        message_id: MessageId,
        body: Body,
        state: BodyState,
    ) -> Result<()> {
        self.write(move |tx| queries::put_body(tx, message_id, &body, state))
            .await
    }

    /// An account's folders, by path.
    pub async fn list_folders(&self, account_id: AccountId) -> Result<Vec<Folder>> {
        self.read(move |conn| queries::list_folders(conn, account_id))
            .await
    }

    pub async fn get_folder(&self, folder_id: FolderId) -> Result<Folder> {
        self.read(move |conn| queries::get_folder(conn, folder_id))
            .await
    }

    /// Makes the folder rows match a `LIST` result: new paths are added, vanished ones are
    /// purged (section 12). Returns the account's folders.
    pub async fn apply_folder_list(
        &self,
        account_id: AccountId,
        listed: Vec<NewFolder>,
    ) -> Result<Vec<Folder>> {
        self.write(move |tx| queries::apply_folder_list(tx, account_id, &listed))
            .await
    }

    /// Records a folder's first UIDVALIDITY; a folder that has one is left alone.
    pub async fn set_uidvalidity(&self, folder_id: FolderId, uidvalidity: u32) -> Result<()> {
        self.write(move |tx| queries::set_uidvalidity(tx, folder_id, uidvalidity))
            .await
    }

    /// Every cached UID of a folder, ascending.
    pub async fn cached_uids(&self, folder_id: FolderId) -> Result<Vec<u32>> {
        self.read(move |conn| queries::cached_uids(conn, folder_id))
            .await
    }

    /// Inserts a batch of synced headers in one transaction, assigning threads (SPEC_GAPS
    /// G36). A UID already cached fails the whole batch.
    pub async fn insert_synced(
        &self,
        folder_id: FolderId,
        messages: Vec<NewMessage>,
    ) -> Result<Vec<MessageId>> {
        self.write(move |tx| queries::insert_synced(tx, folder_id, messages))
            .await
    }

    /// Replaces flags by UID with the server's, then lays the folder's pending ops over
    /// them in the same transaction (invariant 2); returns how many rows changed.
    pub async fn apply_flags(
        &self,
        folder_id: FolderId,
        changes: Vec<(u32, Flags)>,
    ) -> Result<usize> {
        self.write(move |tx| queries::apply_flags(tx, folder_id, &changes))
            .await
    }

    /// Records the end of a folder sync and recounts the folder (section 6, step 7).
    pub async fn finish_folder_sync(
        &self,
        folder_id: FolderId,
        state: FolderSyncState,
    ) -> Result<()> {
        self.write(move |tx| queries::finish_folder_sync(tx, folder_id, &state))
            .await
    }

    /// Bodyless messages among the folder's newest `limit`.
    pub async fn prefetch_candidates(
        &self,
        folder_id: FolderId,
        limit: u32,
    ) -> Result<Vec<PrefetchItem>> {
        self.read(move |conn| queries::prefetch_candidates(conn, folder_id, limit))
            .await
    }

    /// Stores a body, its `body_state` and the preview made from it.
    pub async fn store_body(
        &self,
        message_id: MessageId,
        body: Body,
        state: BodyState,
        preview: String,
    ) -> Result<()> {
        self.write(move |tx| queries::store_body(tx, message_id, &body, state, &preview))
            .await
    }

    /// `PRAGMA integrity_check`; `Corrupt` when it fails. It runs on the writer: on a
    /// pooled reader, FTS5's part of the check can report corruption while the writer is
    /// changing the index, though searches on readers stay correct
    /// (tests/fts_concurrency.rs).
    pub async fn integrity_check(&self) -> Result<()> {
        self.write(|tx| queries::integrity_check(tx)).await
    }

    /// One message's header row.
    pub async fn get_message(&self, message_id: MessageId) -> Result<MessageRecord> {
        self.read(move |conn| queries::get_message(conn, message_id))
            .await
    }

    /// A message's References, space-separated bare IDs (replies, section 7).
    pub async fn message_refs(&self, message_id: MessageId) -> Result<Option<String>> {
        self.read(move |conn| {
            Ok(conn.query_row(
                "SELECT refs FROM messages WHERE id = ?1",
                [message_id],
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// A message's attachment metadata (section 8: listed without downloading).
    pub async fn list_attachments(&self, message_id: MessageId) -> Result<Vec<AttachmentRecord>> {
        self.read(move |conn| queries::list_attachments(conn, message_id))
            .await
    }

    /// One attachment row.
    pub async fn get_attachment(&self, attachment_id: AttachmentId) -> Result<AttachmentRecord> {
        self.read(move |conn| queries::get_attachment(conn, attachment_id))
            .await
    }

    /// Records Always load from this sender (section 8). Addresses compare
    /// case-insensitively.
    pub async fn allow_remote_images(&self, account_id: AccountId, sender: &str) -> Result<()> {
        let sender = sender.to_string();
        self.write(move |tx| queries::allow_remote_images(tx, account_id, &sender))
            .await
    }

    pub async fn remote_images_allowed(&self, account_id: AccountId, sender: &str) -> Result<bool> {
        let sender = sender.to_string();
        self.read(move |conn| queries::remote_images_allowed(conn, account_id, &sender))
            .await
    }

    /// A message's cached body, if it has one.
    pub async fn get_body(&self, message_id: MessageId) -> Result<Option<Body>> {
        self.read(move |conn| queries::get_body(conn, message_id))
            .await
    }

    /// `PRAGMA quick_check` on the writer; `Corrupt` when it fails (SPEC_GAPS G48).
    pub async fn quick_check(&self) -> Result<()> {
        self.write(|tx| queries::quick_check(tx)).await
    }

    /// Queues ops and lays their overlays in one transaction (section 9: every action
    /// applies locally at once and is queued durably). Returns their ids; the first is the
    /// Undo group.
    pub async fn queue_ops(&self, ops: Vec<NewOp>, now: i64) -> Result<Vec<OpId>> {
        self.write(move |tx| ops::queue_ops(tx, &ops, now)).await
    }

    /// A folder change and its op in one transaction (create, rename, delete).
    pub async fn queue_folder_op(
        &self,
        change: FolderChange,
        op: NewOp,
        now: i64,
    ) -> Result<(FolderId, OpId)> {
        self.write(move |tx| ops::queue_folder_op(tx, &change, &op, now))
            .await
    }

    pub async fn get_op(&self, op_id: OpId) -> Result<PendingOp> {
        self.read(move |conn| ops::get_op(conn, op_id)).await
    }

    /// An account's queued and running ops, oldest first.
    pub async fn account_ops(&self, account_id: AccountId) -> Result<Vec<PendingOp>> {
        self.read(move |conn| ops::account_ops(conn, account_id))
            .await
    }

    /// Every op of one Undo group, in any state.
    pub async fn group_ops(&self, group: OpId) -> Result<Vec<PendingOp>> {
        self.read(move |conn| ops::group_ops(conn, group)).await
    }

    /// Lays a folder's unfinished ops over its rows again after a server pull.
    pub async fn relay_ops(&self, folder_id: FolderId) -> Result<()> {
        self.write(move |tx| ops::relay(tx, folder_id)).await
    }

    /// Cancels a queued op and lifts its overlay; `false` when it is already running or done.
    pub async fn cancel_op(&self, op_id: OpId) -> Result<bool> {
        self.write(move |tx| ops::cancel_op(tx, op_id)).await
    }

    /// Drops a permanently failed op and lifts its overlay (section 9: reverted).
    pub async fn fail_op(&self, op_id: OpId) -> Result<PendingOp> {
        self.write(move |tx| ops::fail_op(tx, op_id)).await
    }

    /// Moves an op to `state`, recording the server's result or an error.
    pub async fn set_op_state(
        &self,
        op_id: OpId,
        state: OpState,
        result: Option<serde_json::Value>,
        error: Option<String>,
    ) -> Result<()> {
        self.write(move |tx| ops::set_op_state(tx, op_id, state, result.as_ref(), error.as_deref()))
            .await
    }

    pub async fn delete_op(&self, op_id: OpId) -> Result<()> {
        self.write(move |tx| ops::delete_op(tx, op_id)).await
    }

    /// Unfinished ops over every account (`pending-count`).
    pub async fn pending_count(&self) -> Result<u32> {
        self.read(ops::pending_count).await
    }

    /// Drops finished ops created before `before` (the Undo window has passed).
    pub async fn purge_done_ops(&self, before: i64) -> Result<usize> {
        self.write(move |tx| ops::purge_done(tx, before)).await
    }

    /// Ops left running by a crash are queued again.
    pub async fn requeue_running_ops(&self, account_id: AccountId) -> Result<()> {
        self.write(move |tx| ops::requeue_running(tx, account_id))
            .await
    }

    pub async fn find_folder(&self, account_id: AccountId, path: &str) -> Result<FolderId> {
        let path = path.to_string();
        self.read(move |conn| ops::find_folder(conn, account_id, &path))
            .await
    }

    pub async fn set_folder_role(
        &self,
        folder_id: FolderId,
        role: Option<SpecialUse>,
    ) -> Result<()> {
        self.write(move |tx| ops::set_role(tx, folder_id, role))
            .await
    }

    pub async fn set_folder_sync(&self, folder_id: FolderId, on: bool) -> Result<()> {
        self.write(move |tx| ops::set_sync(tx, folder_id, on)).await
    }

    pub async fn set_folder_watch(&self, folder_id: FolderId, on: bool) -> Result<()> {
        self.write(move |tx| ops::set_watch(tx, folder_id, on))
            .await
    }

    /// Renames a folder and its subtree locally.
    pub async fn rename_folder(&self, folder_id: FolderId, to: &str) -> Result<()> {
        let to = to.to_string();
        self.write(move |tx| ops::rename_folder(tx, folder_id, &to))
            .await
    }

    pub async fn unread_uids(&self, folder_id: FolderId) -> Result<Vec<u32>> {
        self.read(move |conn| ops::unread_uids(conn, folder_id))
            .await
    }

    pub async fn visible_uids(&self, folder_id: FolderId) -> Result<Vec<u32>> {
        self.read(move |conn| ops::visible_uids(conn, folder_id))
            .await
    }

    /// Sets a folder's sync window start; `only_back` lets Load older move it earlier only
    /// (SPEC_GAPS G75).
    pub async fn set_window_start(
        &self,
        folder_id: FolderId,
        start: i64,
        only_back: bool,
    ) -> Result<()> {
        self.write(move |tx| queries::set_window_start(tx, folder_id, start, only_back))
            .await
    }

    pub async fn oldest_internal(&self, folder_id: FolderId) -> Result<Option<i64>> {
        self.read(move |conn| ops::oldest_internal(conn, folder_id))
            .await
    }

    pub async fn uids_by_message_id(
        &self,
        folder_id: FolderId,
        ids: Vec<String>,
    ) -> Result<Vec<u32>> {
        self.read(move |conn| ops::uids_by_message_id(conn, folder_id, &ids))
            .await
    }

    pub async fn insert_outbox(&self, item: NewOutbox) -> Result<OutboxId> {
        self.write(move |tx| outbox::insert_outbox(tx, &item)).await
    }

    pub async fn get_outbox(&self, id: OutboxId) -> Result<OutboxItem> {
        self.read(move |conn| outbox::get_outbox(conn, id)).await
    }

    pub async fn outbox_raw(&self, id: OutboxId) -> Result<Vec<u8>> {
        self.read(move |conn| outbox::outbox_raw(conn, id)).await
    }

    /// An account's outbox, newest first.
    pub async fn list_outbox(&self, account_id: AccountId) -> Result<Vec<OutboxItem>> {
        self.read(move |conn| outbox::list_outbox(conn, account_id))
            .await
    }

    /// Items due at `now`.
    pub async fn due_outbox(&self, now: i64) -> Result<Vec<OutboxId>> {
        self.read(move |conn| outbox::due_outbox(conn, now)).await
    }

    /// When the next waiting item becomes due.
    pub async fn next_outbox_due(&self) -> Result<Option<i64>> {
        self.read(outbox::next_due).await
    }

    /// Claims a due item for sending: `sending` is durable before any network call.
    pub async fn claim_outbox(&self, id: OutboxId, now: i64) -> Result<bool> {
        self.write(move |tx| outbox::claim_outbox(tx, id, now))
            .await
    }

    pub async fn set_outbox(
        &self,
        id: OutboxId,
        state: OutboxState,
        send_after: Option<i64>,
        attempt: bool,
        error: Option<String>,
    ) -> Result<()> {
        self.write(move |tx| {
            outbox::set_outbox(tx, id, state, send_after, attempt, error.as_deref())
        })
        .await
    }

    /// Retry a failed or `unknown` item now; `false` in any other state.
    pub async fn retry_outbox(&self, id: OutboxId, now: i64) -> Result<bool> {
        self.write(move |tx| outbox::retry_outbox(tx, id, now))
            .await
    }

    /// Undo send: removes a still-queued item; `false` once it is being sent.
    pub async fn undo_outbox(&self, id: OutboxId) -> Result<bool> {
        self.write(move |tx| outbox::undo_outbox(tx, id)).await
    }

    pub async fn delete_outbox(&self, id: OutboxId) -> Result<()> {
        self.write(move |tx| outbox::delete_outbox(tx, id)).await
    }

    pub async fn clear_outbox_raw(&self, id: OutboxId) -> Result<()> {
        self.write(move |tx| outbox::clear_outbox_raw(tx, id)).await
    }

    /// At start: items a crash left `sending` become `unknown` (section 7).
    pub async fn sending_to_unknown(&self) -> Result<Vec<OutboxId>> {
        self.write(outbox::sending_to_unknown).await
    }

    pub async fn outbox_pending(&self) -> Result<u32> {
        self.read(outbox::outbox_pending).await
    }

    pub async fn insert_draft(
        &self,
        account_id: AccountId,
        content: serde_json::Value,
        now: i64,
    ) -> Result<DraftId> {
        self.write(move |tx| outbox::insert_draft(tx, account_id, &content, now))
            .await
    }

    pub async fn get_draft(&self, id: DraftId) -> Result<Draft> {
        self.read(move |conn| outbox::get_draft(conn, id)).await
    }

    pub async fn update_draft(
        &self,
        id: DraftId,
        content: serde_json::Value,
        now: i64,
    ) -> Result<()> {
        self.write(move |tx| outbox::update_draft(tx, id, &content, now))
            .await
    }

    pub async fn set_draft_uploaded(&self, id: DraftId, uid: Option<u32>, now: i64) -> Result<()> {
        self.write(move |tx| outbox::set_draft_uploaded(tx, id, uid, now))
            .await
    }

    pub async fn delete_draft(&self, id: DraftId) -> Result<()> {
        self.write(move |tx| outbox::delete_draft(tx, id)).await
    }

    pub async fn get_identity(&self, id: IdentityId) -> Result<Identity> {
        self.read(move |conn| outbox::get_identity(conn, id)).await
    }

    /// Adds (`id` None) or edits an identity; one default per account (SPEC_GAPS G64).
    pub async fn save_identity(
        &self,
        account_id: AccountId,
        id: Option<IdentityId>,
        identity: NewIdentity,
    ) -> Result<IdentityId> {
        self.write(move |tx| outbox::save_identity(tx, account_id, id, &identity))
            .await
    }

    /// Deletes an identity; never the last one.
    pub async fn delete_identity(&self, id: IdentityId) -> Result<()> {
        self.write(move |tx| outbox::delete_identity(tx, id)).await
    }

    /// Counts `weight` uses of each (address, name) at `at` (section 10).
    pub async fn bump_contacts(
        &self,
        account_id: AccountId,
        people: Vec<(String, String)>,
        weight: u32,
        at: i64,
    ) -> Result<()> {
        self.write(move |tx| outbox::bump_contacts(tx, account_id, &people, weight, at))
            .await
    }

    /// Autocomplete: up to `limit` contacts starting with `prefix`.
    pub async fn suggest_contacts(
        &self,
        account_id: AccountId,
        prefix: &str,
        limit: u32,
    ) -> Result<Vec<Contact>> {
        let prefix = prefix.to_string();
        self.read(move |conn| outbox::suggest_contacts(conn, account_id, &prefix, limit))
            .await
    }

    /// One page of a folder, newest first by `(date, id)`, after `cursor`.
    pub async fn folder_page(
        &self,
        folder_id: FolderId,
        cursor: Option<Cursor>,
        limit: u32,
    ) -> Result<Page> {
        self.read(move |conn| queries::folder_page(conn, folder_id, cursor, limit))
            .await
    }

    /// One page across several folders (All Inboxes), newest first, after `cursor`.
    pub async fn folders_page(
        &self,
        folders: Vec<FolderId>,
        cursor: Option<Cursor>,
        limit: u32,
    ) -> Result<Page> {
        self.read(move |conn| queries::folders_page(conn, &folders, cursor, limit))
            .await
    }

    /// One page of threads among `folders` (section 10, rule 5). Counts leave out
    /// `exclude`.
    pub async fn threads_page(
        &self,
        folders: Vec<FolderId>,
        exclude: Vec<FolderId>,
        cursor: Option<Cursor>,
        limit: u32,
    ) -> Result<ThreadPage> {
        self.read(move |conn| queries::threads_page(conn, &folders, &exclude, cursor, limit))
            .await
    }

    /// A thread's messages outside `exclude`, oldest first, each Gmail message once
    /// (section 10, rule 6).
    pub async fn thread_messages(
        &self,
        thread: ThreadId,
        exclude: Vec<FolderId>,
        prefer: Option<FolderId>,
    ) -> Result<Vec<ListRow>> {
        self.read(move |conn| queries::thread_messages(conn, thread, &exclude, prefer))
            .await
    }

    /// One page of a search (section 10), newest first, after `cursor`.
    pub async fn query_page(
        &self,
        query: SearchSql,
        cursor: Option<Cursor>,
        limit: u32,
    ) -> Result<Page> {
        self.read(move |conn| queries::query_page(conn, &query, cursor, limit))
            .await
    }

    /// How many messages a search clause matches.
    pub async fn query_count(&self, query: SearchSql) -> Result<u32> {
        self.read(move |conn| queries::query_count(conn, &query))
            .await
    }

    /// One page of full-text matches (FTS5 query syntax), newest first, after `cursor`.
    pub async fn search_page(
        &self,
        fts_query: String,
        cursor: Option<Cursor>,
        limit: u32,
    ) -> Result<Page> {
        self.read(move |conn| queries::search_page(conn, &fts_query, cursor, limit))
            .await
    }
}

/// A pooled reader on loan. Dropping it, even after a panic, returns the connection before
/// the permit, so the pool never hands out a missing connection.
struct Lease {
    conn: Option<Connection>,
    inner: Arc<Inner>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            lock(&self.inner.readers).push(conn);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The database lives with user-only permissions (section 13, item 9). SQLite creates the
/// `-wal` and `-shm` files with the same mode as the database.
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
