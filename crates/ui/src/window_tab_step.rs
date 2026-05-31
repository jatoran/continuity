//! Positional + MRU tab-step methods on `Window`.
//!
//! Split out of `window_panes.rs` to keep that file under the 600-line
//! cap. Same `impl Window` block, same thread ownership (UI thread of
//! the owning window), same access surface — these methods just live
//! in their own topic-named module.

use crate::Window;

impl Window {
    /// Step to the next positional tab in the focused group (wraps).
    /// `delta > 0` advances, `delta < 0` retreats.
    pub(crate) fn step_tab_positional(&mut self, delta: i32) {
        let focused = self.tree.focused;
        if let Some(g) = self.tree.groups.get_mut(&focused) {
            g.step_positional(delta);
        }
        self.adopt_focused_tab();
        self.request_state_save();
    }

    /// Activate the 1-indexed positional tab in the focused group.
    pub(crate) fn activate_positional_tab(&mut self, one_indexed: usize) -> bool {
        let focused = self.tree.focused;
        let ok = if let Some(g) = self.tree.groups.get_mut(&focused) {
            g.activate_positional(one_indexed)
        } else {
            false
        };
        if ok {
            self.adopt_focused_tab();
            self.request_state_save();
        }
        ok
    }

    /// MRU step (Ctrl+Tab semantics).
    pub(crate) fn step_tab_mru(&mut self, delta: i32) {
        let focused = self.tree.focused;
        if let Some(g) = self.tree.groups.get_mut(&focused) {
            g.step_mru(delta);
        }
        self.adopt_focused_tab();
        self.request_state_save();
    }
}
