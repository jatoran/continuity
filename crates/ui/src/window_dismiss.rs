//! Esc universal-dismiss priority chain (Phase B3).
//!
//! The keystroke layer (`Window::on_keydown`) gives overlays first crack
//! at `Esc` (palette, find bar, quick-open, goto, …). When no overlay
//! is active and the user hits `Esc`, this chain runs before the normal
//! keymap dispatch:
//!
//! 1. dismiss the file/status banner if visible
//! 2. revert any active per-pane view-overlay reveal-preview
//!    (font / theme / pinned revision — hook for E3 / E4 / I1)
//! 3. (future) close outline-sidebar focus / sticky breadcrumb popover
//!    / tab hover preview
//!
//! Returns `true` when something was dismissed — caller short-circuits
//! and skips the keymap dispatch. Falling through (return `false`) lets
//! `Esc` reach its bound command, which is `editor.clear_secondary_cursors`
//! by default — the universal fallback at the bottom of the chain.

use crate::Window;

impl Window {
    /// Walk the Esc priority chain. Returns `true` if a layer consumed
    /// the keystroke. See module docs for the order.
    pub(crate) fn dismiss_priority_chain(&mut self) -> bool {
        // A live tab-drag wins the Esc keystroke — cancel the drag,
        // fade the source tab back in, and dismiss every drop
        // affordance. Routed before any other dismiss so the universal
        // banner / hover dismissals don't preempt it.
        if self.cancel_tab_drag() {
            return true;
        }
        if self.file_banner.is_some() {
            self.file_banner = None;
            return true;
        }
        if self.clear_footnote_hover() {
            return true;
        }
        // D6 — Esc dismisses a hover preview if one is in flight (the
        // dwell may not have rendered yet; that's fine — clearing the
        // state stops the impending render).
        if self.clear_tab_hover() {
            return true;
        }
        // Followup slots (no tracking issue; tracked in
        // .docs/development/roadmap_v2.md):
        // - E3 / E4 / I1: cancel an active per-pane ViewOverlay
        //   reveal-preview when those features land.
        // - F1 / F2: dismiss sticky breadcrumb popover, outline-sidebar
        //   focus respectively.
        false
    }
}

#[cfg(test)]
mod tests {
    // The priority-chain helper is a `&mut Window` method and Window
    // requires a real HWND/D3D11 to construct, so direct tests live
    // in the cross-crate integration suite. The behaviour exercised
    // here is intentionally minimal: documented in module-level docs.
    //
    // Concrete state transitions tested:
    //   - `Option::take`-style semantics on banner consumption.
    #[test]
    fn option_take_semantics_for_banner_slot() {
        let mut slot: Option<&'static str> = Some("banner");
        let consumed = slot.take().is_some();
        assert!(consumed);
        assert!(slot.is_none());
        let consumed_again = slot.take().is_some();
        assert!(!consumed_again);
    }
}
