//! §H6 — Ctrl+Tab hold-to-overlay chord state machine.
//!
//! Live wiring between the keymap-bound `tab.next` / `tab.prev`
//! commands and the [`crate::tab_switcher::TabSwitcher`] palette-mode
//! overlay:
//!
//! * Tap (release Ctrl within 600 ms): the positional swap fires
//!   immediately and the timer is cancelled — no overlay, no flicker.
//! * Hold (Ctrl held past 600 ms): the timer fires, the overlay opens
//!   anchored on the tab the fast-path already swapped to, and
//!   subsequent `Ctrl+Tab` taps step the overlay cursor (preview-only,
//!   no MRU mutation). Releasing Ctrl commits the highlighted tab and
//!   dismisses the overlay; pressing Esc reverts and dismisses.
//!
//! Thread ownership: UI thread of the owning [`crate::Window`]. Arming
//! / cancelling the `WM_TIMER` is single-writer per the timer
//! conventions in [`crate::window_timers`].

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::overlays::Overlays;
use crate::window::Window;
use crate::window_input_modifiers::{active_modifiers, is_key_down};
use crate::window_pane_modes::TabOverlayChord;
use crate::window_timers::{TAB_OVERLAY_HOLD_TIMER_ID, TAB_OVERLAY_HOLD_TIMER_MS};

impl Window {
    /// §H6 — handle `tab.next` (or `tab.prev` when `delta` is negative).
    /// Three branches:
    ///   1. Tab switcher overlay is already open → step its cursor and
    ///      preview the new selection (no MRU mutation).
    ///   2. The hold-timer is already pending (within the 600 ms window)
    ///      → continue stepping positional swaps; the timer keeps
    ///      ticking against its original arm time so a sustained hold
    ///      still trips the overlay.
    ///   3. First press → do the fast positional swap, then arm the
    ///      600 ms timer so a sustained hold opens the overlay.
    pub(crate) fn tab_chord_step(&mut self, delta: i32) {
        if matches!(self.overlays, Overlays::TabSwitcher(_)) {
            self.tab_switcher_step_via_chord(delta);
            self.request_repaint();
            return;
        }
        self.step_tab_positional(delta);
        self.arm_tab_overlay_hold_timer(delta);
        self.request_repaint();
    }

    /// §H6 — step the visible tab-switcher cursor by `delta` and
    /// preview the newly highlighted tab. Reachable from both the
    /// chord-keymap path (`Ctrl+Tab` → `tab.next` while the overlay
    /// is visible) and the overlay-routing path (`Tab` / `Shift+Tab`
    /// captured directly when the overlay has focus).
    pub(crate) fn tab_switcher_step_via_chord(&mut self, delta: i32) {
        let next = if let Some(ts) = self.overlays.tab_switcher_mut() {
            ts.step(delta);
            ts.selected_row().map(|r| r.tab_id)
        } else {
            None
        };
        if let Some(tab) = next {
            self.preview_tab_via_switcher(tab);
        }
    }

    /// §H6 — arm the hold timer on the **initial** Ctrl+Tab press
    /// only. A `Some` chord state means the timer is already armed
    /// from an earlier press inside the same Ctrl-hold session:
    /// `SetTimer` with the same id resets the wait, and Tab's native
    /// auto-repeat (~30 Hz on Windows) would re-arm the 600 ms
    /// countdown faster than it could ever fire — which was the
    /// original H6 regression: holding Ctrl+Tab cycled tabs without
    /// ever surfacing the overlay. Re-arming on the first press only
    /// anchors the timer to when Ctrl actually started being held,
    /// so a sustained hold trips the overlay at the genuine 600 ms
    /// mark regardless of how many Tab repeats fire in the meantime.
    /// `Ctrl` release (`on_tab_overlay_ctrl_release`) is the only
    /// path that clears the chord state and cancels the timer.
    fn arm_tab_overlay_hold_timer(&mut self, initial_delta: i32) {
        let already_armed = self.view_options.pane_modes.tab_overlay_chord.is_some();
        // Always update the direction — a Ctrl+Shift+Tab repeat after
        // a Ctrl+Tab press should reverse the eventual overlay open
        // delta — but only schedule a fresh `SetTimer` when the
        // pending state was empty (initial press).
        self.view_options.pane_modes.tab_overlay_chord = Some(TabOverlayChord { initial_delta });
        if already_armed {
            return;
        }
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            let _ = SetTimer(
                Some(self.hwnd),
                TAB_OVERLAY_HOLD_TIMER_ID,
                TAB_OVERLAY_HOLD_TIMER_MS,
                None,
            );
        }
    }

    /// §H6 — `WM_TIMER` callback for `TAB_OVERLAY_HOLD_TIMER_ID`. Kills
    /// the timer (one-shot semantics) and opens the overlay if a chord
    /// is still pending. A no-op if the chord was cleared (early
    /// release / focus loss) between scheduling and firing.
    pub(crate) fn on_tab_overlay_hold_tick(&mut self, hwnd: HWND) {
        unsafe {
            let _ = KillTimer(Some(hwnd), TAB_OVERLAY_HOLD_TIMER_ID);
        }
        if self.view_options.pane_modes.tab_overlay_chord.is_none() {
            return;
        }
        let _ = self.show_tab_overlay_impl();
    }

    /// §H6 — Ctrl release dispatcher. Three cases mirror the arm
    /// path: overlay visible (commit + dismiss), timer pending (fast
    /// swap path, just cancel the timer), neither (no-op).
    pub(crate) fn on_tab_overlay_ctrl_release(&mut self) {
        // Always kill the pending timer — Ctrl is no longer held.
        if !self.hwnd.0.is_null() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TAB_OVERLAY_HOLD_TIMER_ID);
            }
        }
        self.view_options.pane_modes.tab_overlay_chord = None;
        if matches!(self.overlays, Overlays::TabSwitcher(_)) {
            self.commit_tab_switcher_on_chord_release();
        }
    }

    /// §H6 — `WM_KEYUP` dispatch entry. Fires on any Ctrl key-up;
    /// only acts when no Ctrl key is still held (both physical Ctrls
    /// must be released) and either a chord is pending or the overlay
    /// is visible. Returns `true` when state changed and the window
    /// should be invalidated.
    pub(crate) fn on_keyup(&mut self, vk: u16) -> bool {
        let hud_changed = if is_modifier_key(vk) {
            self.on_chord_hud_modifier_edge(active_modifiers())
        } else {
            false
        };
        let is_ctrl_key = vk == VK_CONTROL.0 || vk == VK_LCONTROL.0 || vk == VK_RCONTROL.0;
        if !is_ctrl_key {
            return hud_changed;
        }
        // `WM_KEYUP` arrives after the OS clears the key's pressed
        // bit, so a direct GetKeyState query reflects the post-up
        // state. Both physical Ctrls must be released for the chord
        // commit to fire.
        if is_key_down(VK_LCONTROL.0) || is_key_down(VK_RCONTROL.0) {
            return false;
        }
        let had_chord = self.view_options.pane_modes.tab_overlay_chord.is_some()
            || matches!(self.overlays, Overlays::TabSwitcher(_));
        if !had_chord {
            return hud_changed;
        }
        self.on_tab_overlay_ctrl_release();
        true
    }

    /// §H6 — chord-release commit. Promotes the highlighted tab into
    /// MRU and dismisses the overlay.
    fn commit_tab_switcher_on_chord_release(&mut self) {
        let tab = self
            .overlays
            .tab_switcher()
            .and_then(|t| t.selected_row().map(|r| r.tab_id));
        let Some(tab) = tab else {
            self.overlays.dismiss();
            self.request_repaint();
            return;
        };
        let focused = self.tree.focused;
        if let Some(group) = self.tree.groups.get_mut(&focused) {
            group.activate(tab);
        }
        self.adopt_focused_tab();
        self.overlays.dismiss();
        self.request_repaint();
    }
}

fn is_modifier_key(vk: u16) -> bool {
    matches!(
        vk,
        v if v == VK_CONTROL.0
            || v == VK_LCONTROL.0
            || v == VK_RCONTROL.0
            || v == VK_MENU.0
            || v == VK_LMENU.0
            || v == VK_RMENU.0
            || v == VK_SHIFT.0
            || v == VK_LSHIFT.0
            || v == VK_RSHIFT.0
            || v == VK_LWIN.0
            || v == VK_RWIN.0
    )
}
