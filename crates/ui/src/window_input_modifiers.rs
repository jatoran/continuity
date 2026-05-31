//! Win32 keyboard-state helpers: live modifier snapshot and a single-
//! key down-state query. Used by the command-dispatch path in
//! `window_commanding.rs` to build a [`continuity_input::Modifiers`]
//! payload from `WM_KEYDOWN`-time `GetKeyState` reads.
//!
//! Split out of `window_commanding.rs` to keep that file under the
//! 600-line cap.

use continuity_input::Modifiers;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

/// Snapshot of the four modifier keys at the moment of the call.
/// Uses `GetKeyState` (synchronous, message-queue order) so the result
/// reflects the modifier state the user perceived for the in-flight
/// keystroke.
pub(crate) fn active_modifiers() -> Modifiers {
    Modifiers {
        ctrl: is_key_down(VK_CONTROL.0),
        alt: is_key_down(VK_MENU.0),
        shift: is_key_down(VK_SHIFT.0),
        meta: is_key_down(VK_LWIN.0) || is_key_down(VK_RWIN.0),
    }
}

/// `true` when the virtual-key `vk`'s low bit is set in `GetKeyState`.
pub(crate) fn is_key_down(vk: u16) -> bool {
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}
