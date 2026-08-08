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

    /// An error from the synchronous storage-neutral engine.
    #[error(transparent)]
    Engine(continuity_engine::Error),

    /// An error from the platform-neutral host operation envelope.
    #[error(transparent)]
    Host(continuity_host::Error),

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

impl From<continuity_host::Error> for Error {
    fn from(error: continuity_host::Error) -> Self {
        match error {
            continuity_host::Error::UnknownBuffer => Self::UnknownBuffer,
            continuity_host::Error::Engine(engine) => Self::from(engine),
            other => Self::Host(other),
        }
    }
}

impl From<continuity_engine::Error> for Error {
    fn from(error: continuity_engine::Error) -> Self {
        match error {
            continuity_engine::Error::UnknownBuffer => Self::UnknownBuffer,
            other => Self::Engine(other),
        }
    }
}
