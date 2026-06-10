//! App-activation and focus-loss input-state handling for [`Window`].
//!
//! Extracted from `window_dispatch.rs` (which carries the raw message
//! match) so the dispatch file stays under the 600-line cap and the
//! focus semantics live in one place.
//!
//! Thread ownership: all state touched here (`is_window_focused`,
//! `chord_hud`, `pending_chord_sequence`, tab-overlay chord) is owned by
//! the window's UI thread; both entry points run inside the wndproc.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::SystemInformation::GetTickCount64;

use crate::overlays::Overlays;
use crate::Window;

impl Window {
    /// `WM_ACTIVATEAPP` — the window's process gained or lost the
    /// foreground (alt-tab, click into another app, …).
    pub(crate) fn on_activate_app(&mut self, hwnd: HWND, becoming_active: bool) {
        crate::paint_trace::log_event(
            "wm_activateapp",
            if becoming_active {
                "activate"
            } else {
                "deactivate"
            },
        );
        self.is_window_focused = becoming_active;
        if becoming_active {
            // Coming back to the foreground: force a repaint so the
            // surface refreshes immediately rather than waiting for the
            // next external invalidate. Also stamp `last_activation_tick`
            // so the activation-grace gate suppresses spell recheck / MRU
            // prewarm / spectator decoration submission for the first
            // `Window::ACTIVATION_GRACE_MS`.
            self.last_activation_tick = unsafe { GetTickCount64() };
        } else {
            self.clear_unsaved_close_arm();
            let _ = self.on_focus_lost_clear_input_state();
        }
        // Repaint in both directions: the active-pane highlight is
        // gated on window focus, so the border must restyle the moment
        // focus changes — not on the next incidental invalidate.
        self.invalidate(hwnd);
    }

    /// `WM_SETFOCUS` — this HWND gained keyboard focus (covers
    /// switches between two continuity windows, which never fire
    /// WM_ACTIVATEAPP).
    pub(crate) fn on_set_focus(&mut self, hwnd: HWND) {
        self.has_keyboard_focus = true;
        self.invalidate(hwnd);
    }

    /// `WM_KILLFOCUS` — keyboard focus moved elsewhere. Clears
    /// held-modifier UI state and drops the active-pane highlight.
    pub(crate) fn on_kill_focus(&mut self, hwnd: HWND) {
        self.has_keyboard_focus = false;
        let _ = self.on_focus_lost_clear_input_state();
        self.invalidate(hwnd);
    }

    /// Reset every held-modifier-derived input state. Called on focus
    /// loss (`WM_KILLFOCUS` and `WM_ACTIVATEAPP(false)`): the modifier
    /// key-up that would normally clear these is delivered to the newly
    /// focused window, so without this the chord HUD stays pinned "as if
    /// Alt were held" after an alt-tab, and a pending chord leader
    /// silently survives into the next focus session. Returns `true`
    /// when any visible state changed (caller should invalidate).
    pub(crate) fn on_focus_lost_clear_input_state(&mut self) -> bool {
        let mut changed = !self.pending_chord_sequence.is_empty();
        self.pending_chord_sequence.clear();
        changed |= self.chord_hud.is_visible()
            || !matches!(self.chord_hud, crate::chord_hud::HudState::Idle);
        self.chord_hud = crate::chord_hud::HudState::Idle;
        if self.view_options.pane_modes.tab_overlay_chord.is_some()
            || matches!(self.overlays, Overlays::TabSwitcher(_))
        {
            // Treat focus loss as the Ctrl release it effectively is:
            // commit the previewed tab and dismiss the switcher.
            self.on_tab_overlay_ctrl_release();
            changed = true;
        }
        changed
    }
}
