//! Top-level HWND close and destruction lifecycle.
//!
//! Thread ownership: the window UI thread flushes its vault exports,
//! persistence snapshot and registry entry before quitting.

use windows::Win32::Foundation::{HWND, LRESULT};
use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;

use crate::Window;

impl Window {
    /// Confirm and destroy a window after preparing any zero-tab folder state
    /// for persistence.
    pub(crate) fn handle_window_close(&mut self, hwnd: HWND) -> LRESULT {
        if !self.confirm_close_window() {
            return LRESULT(0);
        }
        self.prepare_empty_folder_workspace_for_close();
        let _ = unsafe { DestroyWindow(hwnd) };
        LRESULT(0)
    }

    /// Flush UI-owned state and unregister a destroyed HWND.
    pub(crate) fn handle_window_destroy(&mut self, hwnd: HWND) -> LRESULT {
        self.flush_due_vault_autosaves(true);
        if self.vault.is_active() {
            self.persist_vault_workspace_state();
        }
        self.prepare_empty_folder_workspace_for_close();
        self.save_window_placement_state();
        crate::window_registry::unregister(hwnd);
        crate::desktop_shell::request_message_loop_quit();
        LRESULT(0)
    }
}
