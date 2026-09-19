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
    /// An HTTP fetch was asked for a URL that is not absolute `https://`.
    #[error("not an https:// URL: {0}")]
    InvalidUrl(String),
    /// The server answered with a non-2xx status.
    #[error("HTTP {status} from {url}")]
    HttpStatus {
        /// Response status code.
        status: u32,
        /// Requested URL.
        url: String,
    },
    /// The response body exceeded the buffer cap.
    #[error("HTTP body exceeded {0} bytes")]
    HttpTooLarge(usize),
}

impl Error {
    /// Build a `Win32` error from an api name and a `windows::core::Error`.
    #[must_use]
    pub fn win32(api: &'static str, source: windows::core::Error) -> Self {
        Self::Win32 { api, source }
    }
}
