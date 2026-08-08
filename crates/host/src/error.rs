//! Host-contract errors.

use continuity_buffer::Revision;
use thiserror::Error;

/// Errors returned before or during a host dispatch.
#[derive(Debug, Error)]
pub enum Error {
    /// The runtime was called from a thread other than its creator.
    #[error("host runtime called from the wrong thread")]
    WrongThread,
    /// The runtime was already torn down.
    #[error("host runtime has been closed")]
    Closed,
    /// A revision-checked request targeted stale content.
    #[error("revision mismatch: expected {expected:?}, actual {actual:?}")]
    RevisionMismatch {
        /// Revision supplied by the host.
        expected: Revision,
        /// Current engine revision.
        actual: Revision,
    },
    /// A request targeted a buffer not owned by the engine.
    #[error("unknown buffer id")]
    UnknownBuffer,
    /// A UTF-8 or UTF-16 coordinate landed outside the string or inside a
    /// scalar value.
    #[error("invalid text boundary: {0}")]
    InvalidTextBoundary(usize),
    /// The storage-neutral engine rejected the operation.
    #[error(transparent)]
    Engine(#[from] continuity_engine::Error),
}
