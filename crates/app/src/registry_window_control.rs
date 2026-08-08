//! Prompt delivery of registry control messages to live window threads.

use continuity_buffer::WindowId;
use continuity_ui::WindowControl;

use crate::registry::LiveState;

/// Send one control message and wake the owning UI thread immediately.
pub(crate) fn try_send_window_control(
    state: &LiveState,
    window_id: WindowId,
    message: WindowControl,
) -> bool {
    let Some(sender) = state.control_senders.get(&window_id) else {
        return false;
    };
    if sender.send(message).is_err() {
        return false;
    }
    if let Some(raw_window) = state.control_windows.get(&window_id).copied() {
        continuity_ui::wake_window_control(raw_window);
    }
    true
}
