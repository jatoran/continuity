//! Errors for the `continuity-core` crate.

use thiserror::Error;

/// Errors that can arise during command dispatch or core-thread operation.
#[derive(Debug, Error)]
pub enum Error {
    /// A command targeted a buffer id that no longer exists.
    #[error("unknown buffer id")]
    UnknownBuffer,

    /// An error from the buffer layer.
    #[error(transparent)]
    Buffer(#[from] continuity_buffer::Error),

    /// An error from the text-coordinate layer.
    #[error(transparent)]
    Text(#[from] continuity_text::Error),

    /// An error from the persistence layer.
    #[error(transparent)]
    Persist(#[from] continuity_persist::Error),

    /// A command was given an out-of-range or otherwise invalid argument.
    #[error("invalid argument for `{name}`: {reason}")]
    InvalidArgument {
        /// Command name that received the bad argument.
        name: &'static str,
        /// Human-readable reason.
        reason: String,
    },
}
