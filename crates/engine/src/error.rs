//! Errors returned by the synchronous editor engine.

use continuity_buffer::Revision;
use thiserror::Error;

/// Errors from storage-neutral editor operations.
#[derive(Debug, Error)]
pub enum Error {
    /// A mutation targeted a buffer that is not owned by this engine.
    #[error("unknown buffer id")]
    UnknownBuffer,

    /// A revision-checked mutation targeted stale content.
    #[error("revision mismatch: expected {expected:?}, actual {actual:?}")]
    RevisionMismatch {
        /// Revision supplied by the caller.
        expected: Revision,
        /// Current engine revision.
        actual: Revision,
    },

    /// A host-provided change-batch sequence was internally inconsistent.
    #[error("invalid change batch: {0}")]
    InvalidChangeBatch(String),

    /// An error from the buffer layer.
    #[error(transparent)]
    Buffer(#[from] continuity_buffer::Error),

    /// An error from the text-coordinate layer.
    #[error(transparent)]
    Text(#[from] continuity_text::Error),

    /// A command argument was outside its supported domain.
    #[error("invalid argument for `{name}`: {reason}")]
    InvalidArgument {
        /// Command that received the invalid argument.
        name: &'static str,
        /// Human-readable failure reason.
        reason: String,
    },
}
