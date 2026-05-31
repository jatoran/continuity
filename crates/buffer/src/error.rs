//! Errors for the `continuity-buffer` crate.

use thiserror::Error;

/// Errors that can arise while operating on a `Buffer`.
#[derive(Debug, Error)]
pub enum Error {
    /// An edit was applied against a revision that is no longer current.
    #[error("stale revision {applied} (current {current})")]
    StaleRevision {
        /// The revision the edit targeted.
        applied: u64,
        /// The buffer's current revision.
        current: u64,
    },

    /// Attempted to apply an edit to a read-only buffer (e.g. the
    /// tutorial tab). Surfaced to the user as a transient banner; the
    /// rope is never touched.
    #[error("buffer is read-only")]
    ReadOnly,

    /// An error originating from the `text` crate.
    #[error(transparent)]
    Text(#[from] continuity_text::Error),
}
