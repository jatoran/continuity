//! Errors for the `continuity-ui` crate.

use thiserror::Error;

/// Errors that can arise from window / pane / tab management.
#[derive(Debug, Error)]
pub enum Error {
    /// A wrapped Win32 / COM error from the `win` crate.
    #[error(transparent)]
    Win(#[from] continuity_win::Error),

    /// An error from the render layer.
    #[error(transparent)]
    Render(#[from] continuity_render::Error),

    /// An error from the layout layer.
    #[error(transparent)]
    Layout(#[from] continuity_layout::Error),

    /// An error from the input layer.
    #[error(transparent)]
    Input(#[from] continuity_input::Error),

    /// An error from command dispatch.
    #[error(transparent)]
    Command(#[from] continuity_command::Error),

    /// An error from keymap loading.
    #[error(transparent)]
    Keymap(#[from] continuity_keymap::Error),

    /// An error from the editor core.
    #[error(transparent)]
    Core(#[from] continuity_core::Error),

    /// An error from theme loading or required-key validation.
    #[error(transparent)]
    Theme(#[from] continuity_theme::Error),

    /// A direct windows API call failed.
    #[error("win32: {0}")]
    Win32(#[from] windows::core::Error),
}
