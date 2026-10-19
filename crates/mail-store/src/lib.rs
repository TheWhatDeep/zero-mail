#![forbid(unsafe_code)]
//! `store` (BUILD.md section 3): schema, migrations, queries and FTS. Calls no other module.
//!
//! SQLite in WAL mode with foreign keys on (section 4). One writer thread owns the only
//! read-write connection and runs each write as one transaction, in arrival order; a pool of
//! read-only connections serves queries concurrently (section 3, process model). Callers
//! never see SQL or a connection.

mod error;
mod queries;
mod schema;
mod types;

pub use error::{Result, StoreError};
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
        schema::migrate(&mut writer)?;

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
