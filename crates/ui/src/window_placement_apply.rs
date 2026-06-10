//! Phase-14 glue between `Window` and `WindowPersistence`.
//!
//! Lives next to `window.rs` to keep that file under the 600-line cap. The
//! single-writer rule still holds: every method here runs on the window's
//! UI thread.

use continuity_buffer::BufferId;
use continuity_command::Error as CommandError; // alias: collides with crate::Error
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::pane_tree_codec;
use crate::window::{Window, STATE_SAVE_DEBOUNCE_MS, STATE_SAVE_TIMER_ID};
use crate::window_placement_persistence::{
    apply_placement, capture_placement, current_desktop_guid, try_move_to_desktop,
    WindowStateSnapshot,
};

impl Window {
    /// Replay any restored placement / desktop GUID at construction time.
    /// Called from `Window::new` once the HWND is live but before the
    /// window is shown.
    pub(crate) fn apply_initial_placement(&mut self, hwnd: HWND) {
        let Some(p) = self.persistence.as_ref() else {
            return;
        };
        let Some(initial) = p.initial.as_ref() else {
            return;
        };
        if let Some(blob) = initial.placement_blob.as_deref() {
            apply_placement(hwnd, blob);
        }
        // `window.restore_to_virtual_desktops = false` skips the
        // move-to-desktop call so the window comes up on the active
        // desktop instead of being yanked back to its persisted one.
        let restore_desktops = self
            .live_reload
            .as_ref()
            .is_none_or(|lr| lr.current_settings().window.restore_to_virtual_desktops);
        if restore_desktops {
            if let Some(guid) = initial.virtual_desktop_guid {
                // A window restored onto a *different* desktop must never
                // activate at show time — `SetForegroundWindow` on a
                // window parked elsewhere makes Windows switch the user's
                // desktop, which is exactly the launch-time yank this
                // guards against.
                let active_guid = current_desktop_guid(hwnd, self.virtual_desktop.as_ref());
                if active_guid.is_some_and(|active| active != guid) {
                    self.activate_on_show = false;
                }
                try_move_to_desktop(hwnd, self.virtual_desktop.as_ref(), guid);
            }
        }
    }

    /// Push the current state out through the persist sink. Called on
    /// `WM_DESTROY` (synchronous) and from [`Self::on_state_save_tick`]
    /// once a debounced mid-session save fires.
    pub(crate) fn save_window_placement_state(&self) {
        let Some(p) = self.persistence.as_ref() else {
            return;
        };
        // §H3 — persist `folded_lines` alongside the tree. F5 piggy-
        // backs per-buffer image expand state on the same blob via
        // the codec's state-aware variant; both new fields carry
        // `#[serde(default)]` so older readers still decode the JSON.
        let snapshot = WindowStateSnapshot {
            pane_tree_json: pane_tree_codec::encode_with_state(
                &self.tree,
                &self.view_options.pane_modes.folded_lines,
                &self.image_expand_state,
            ),
            virtual_desktop_guid: current_desktop_guid(self.hwnd, self.virtual_desktop.as_ref()),
            monitor_id: None,
            placement_blob: capture_placement(self.hwnd),
        };
        (p.save)(snapshot);
    }

    /// Arm (or re-arm) the Phase-17 debounced state-save timer.
    ///
    /// Coarse mutations — pane split/close, tab move, placement change,
    /// virtual-desktop move — call this to schedule a `save_window_placement_state`
    /// in [`STATE_SAVE_DEBOUNCE_MS`] ms. Re-arming inside the window
    /// coalesces bursts (e.g. `Ctrl+Alt+5` rebuilding a 2×2 grid emits one
    /// save, not six). `WM_DESTROY` keeps its synchronous save so session
    /// teardown never races a pending timer.
    pub(crate) fn request_state_save(&mut self) {
        if self.persistence.is_none() {
            return;
        }
        if self.hwnd.0.is_null() {
            // Headless test path: persist sink (if any) will be invoked on
            // shutdown by `save_window_placement_state` directly.
            return;
        }
        // SetTimer with an existing id resets the wait — exactly the
        // coalescing behavior we want.
        let armed = unsafe {
            SetTimer(
                Some(self.hwnd),
                STATE_SAVE_TIMER_ID,
                STATE_SAVE_DEBOUNCE_MS,
                None,
            )
        };
        self.state_save_pending = armed != 0;
    }

    /// `WM_TIMER` handler for the Phase-17 debounced state-save timer.
    /// Kills the timer (one-shot semantics) and flushes the current
    /// window state.
    pub(crate) fn on_state_save_tick(&mut self, hwnd: HWND) {
        if self.state_save_pending {
            unsafe {
                let _ = KillTimer(Some(hwnd), STATE_SAVE_TIMER_ID);
            }
            self.state_save_pending = false;
        }
        self.save_window_placement_state();
    }

    /// Implementation of `Context::tear_off_focused_tab`. Removes the
    /// focused tab from the local pane tree and returns its `BufferId`.
    ///
    /// Tearing the *only* tab of the *only* pane no longer fails — the
    /// shared `close_active_tab` path replaces it with a fresh empty
    /// buffer so the source window stays alive while the torn buffer
    /// opens in a brand new window. Refusing here would silently
    /// "swallow" a drag-out drop and confuse the user.
    pub(crate) fn tear_off_focused_tab_impl(&mut self) -> Result<BufferId, CommandError> {
        let Some(group) = self.tree.groups.get(&self.tree.focused) else {
            return Err(CommandError::UnsupportedContext("tear_off_focused_tab"));
        };
        let active_tab = group.active;
        let buffer_id = self
            .tree
            .tabs
            .get(&active_tab)
            .map(|t| t.buffer_id)
            .ok_or(CommandError::UnsupportedContext("tear_off_focused_tab"))?;
        // Reuse Phase 13's close-tab path so MRU/group cleanup happens
        // in exactly one place. When this leaves the source pane empty
        // AND it was the last pane, `close_active_tab` opens a fresh
        // empty buffer so the source window keeps a tab.
        let _ = self.close_active_tab();
        Ok(buffer_id)
    }
}
