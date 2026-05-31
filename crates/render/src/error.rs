//! Errors for the `continuity-render` crate.

use thiserror::Error;

/// Errors that can arise during rendering.
#[derive(Debug, Error)]
pub enum Error {
    /// A wrapped Win32 error from the `win` crate.
    #[error(transparent)]
    Win(#[from] continuity_win::Error),

    /// An error from the layout layer.
    #[error(transparent)]
    Layout(#[from] continuity_layout::Error),

    /// A direct windows / D3D11 / DXGI / D2D call failed.
    #[error("graphics: {0}")]
    Graphics(#[from] windows::core::Error),
}
