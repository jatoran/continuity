//! Errors for the `continuity-search` crate.

use thiserror::Error;

/// Errors that can arise during search or index maintenance.
#[derive(Debug, Error)]
pub enum Error {
    /// A user-supplied regex failed to compile.
    #[error("invalid regex: {0}")]
    InvalidRegex(String),

    /// A SQLite operation against the FTS5 index failed.
    #[error("fts: {0}")]
    Fts(#[from] rusqlite::Error),

    /// I/O failure while searching a buffer's contents.
    #[error("search io: {0}")]
    Io(#[from] std::io::Error),

    /// An FTS row contained a non-UUID value in the buffer_id column.
    #[error("invalid buffer id in fts row: {0}")]
    InvalidId(String),
}
