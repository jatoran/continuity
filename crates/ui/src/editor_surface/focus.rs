//! Keyboard and nested-input focus for one editor surface.

/// Focus state whose lifetime belongs to an editor control.
///
/// **Thread ownership:** the surface's UI thread is the sole writer. Host
/// application activation remains outside this state because a child editor
/// can gain or lose keyboard focus without activating or deactivating its host.
#[derive(Debug)]
pub(crate) struct FocusState {
    /// Whether the editor control owns Win32 keyboard focus.
    pub(crate) has_keyboard_focus: bool,
    /// Whether a text input nested inside an editor overlay owns input.
    pub(crate) overlay_input_focused: bool,
}

impl Default for FocusState {
    fn default() -> Self {
        Self {
            has_keyboard_focus: true,
            overlay_input_focused: false,
        }
    }
}
