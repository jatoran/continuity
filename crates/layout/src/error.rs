//! Errors for the `continuity-layout` crate.

use thiserror::Error;

/// Errors that can arise during text layout.
#[derive(Debug, Error)]
pub enum Error {
    /// A wrapped Win32 / COM error from the `win` crate.
    #[error(transparent)]
    Win(#[from] continuity_win::Error),

    /// A DirectWrite call failed.
    #[error("directwrite: {0}")]
    DirectWrite(#[from] windows::core::Error),
}
