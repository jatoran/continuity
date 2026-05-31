//! Errors for the `continuity-persist` crate.

use thiserror::Error;

/// Errors that can arise while persisting or recovering buffer state.
#[derive(Debug, Error)]
pub enum Error {
    /// A SQLite operation failed.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Compression / decompression of a snapshot blob failed, or a decoded
    /// blob was not valid UTF-8.
    #[error("zstd: {0}")]
    Compression(#[from] std::io::Error),

    /// A persisted snapshot or edit row's checksum did not match the rope it
    /// was expected to describe.
    #[error("checksum mismatch at revision {revision}")]
    ChecksumMismatch {
        /// The revision at which replay halted.
        revision: u64,
    },

    /// An [`EditRow`](crate::EditRow) could not be decoded into an
    /// [`EditOp`](continuity_text::EditOp).
    #[error("decode: {0}")]
    Decode(String),

    /// The persistence thread received a request after it had shut down (or
    /// the response channel was dropped before a reply arrived).
    #[error("persist thread is not available")]
    ThreadGone,

    /// The expected environment variable for the database directory
    /// (`%APPDATA%` or `%LOCALAPPDATA%`) was not set.
    #[error("missing environment variable: {0}")]
    MissingEnv(&'static str),

    /// A caller passed an out-of-range / unrecognized argument to a
    /// [`Store`](crate::Store) method (e.g. an unknown `synchronous`
    /// PRAGMA value).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}
