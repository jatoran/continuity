//! Registry-side delivery of update-host events to live windows.
//!
//! Thread ownership: registry thread; reads `LiveState`.

use continuity_ui::WindowControl;

use crate::registry::LiveState;

/// Send `message` to every live window and wake its control poll.
pub(crate) fn fan_out(state: &LiveState, message: WindowControl) {
    for (window_id, tx) in &state.control_senders {
        if tx.send(message.clone()).is_ok() {
            if let Some(raw_window) = state.control_windows.get(window_id).copied() {
                continuity_ui::wake_window_control(raw_window);
            }
        }
    }
}

/// Post `WM_CLOSE` to every live window so each runs its normal close
/// sequence (vault autosave flush, placement save, closed-history
/// archive) before the staged installer replaces the executable.
pub(crate) fn close_all_windows(state: &LiveState) {
    for raw_window in state.control_windows.values().copied() {
        continuity_ui::request_window_close(raw_window);
    }
}
