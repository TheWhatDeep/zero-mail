/// Every failure the store can report. Messages never contain message content.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// `SQLITE_FULL`: the transaction was rolled back and nothing was written (section 12).
    #[error("disk is full")]
    DiskFull,
    /// The file is not a database or failed an integrity check (section 12).
    #[error("database is corrupt")]
    Corrupt,
    /// Written by a newer build. Migrations are forward-only, so it is never downgraded.
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    TooNew { found: i64, supported: i64 },
    /// SQLite refused WAL mode (for example on some network file systems).
    #[error("database could not switch to WAL mode (got {0})")]
    NotWal(String),
    #[error("not found")]
    NotFound,
    #[error("invalid argument: {0}")]
    Invalid(&'static str),
    /// The store was dropped, or its writer thread is gone.
    #[error("store is closed")]
    Closed,
    #[error("database error: {0}")]
    Sqlite(rusqlite::Error),
    #[error("JSON column could not be read: {0}")]
    Json(#[from] serde_json::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode;
        match err.sqlite_error_code() {
            Some(ErrorCode::DiskFull) => Self::DiskFull,
            Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => Self::Corrupt,
            _ if matches!(err, rusqlite::Error::QueryReturnedNoRows) => Self::NotFound,
            _ => Self::Sqlite(err),
        }
    }
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;
