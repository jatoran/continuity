//! Errors for the `continuity-input` crate.

use thiserror::Error;

/// Errors that can arise from input handling.
#[derive(Debug, Error)]
pub enum Error {
    /// A user-facing key-chord string failed to parse.
    #[error("invalid key chord: {0}")]
    InvalidChord(String),

    /// A Win32 input call failed.
    #[error(transparent)]
    Win(#[from] continuity_win::Error),
}
