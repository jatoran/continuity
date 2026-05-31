//! Errors for the `continuity-win` crate.

use thiserror::Error;

/// Errors that can arise from Win32 / COM calls.
#[derive(Debug, Error)]
pub enum Error {
    /// A Win32 / COM API returned a failure.
    #[error("win32 call `{api}` failed: {source}")]
    Win32 {
        /// Name of the Win32 function that failed.
        api: &'static str,
        /// Underlying `windows::core::Error`.
        #[source]
        source: windows::core::Error,
    },
}

impl Error {
    /// Build a `Win32` error from an api name and a `windows::core::Error`.
    #[must_use]
    pub fn win32(api: &'static str, source: windows::core::Error) -> Self {
        Self::Win32 { api, source }
    }
}
