//! Errors for the `continuity-text` crate.

use thiserror::Error;

/// Errors that can arise while operating on text primitives.
#[derive(Debug, Error)]
pub enum Error {
    /// An offset or position fell outside the bounds of the text.
    #[error("offset {0} out of bounds")]
    OutOfBounds(usize),
}
