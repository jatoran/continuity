//! δ.1 — tab pin/unpin support.
//!
//! Toggles the `pinned` flag on the focused group's active tab, then
//! reorders the group's tab vector so pinned tabs anchor leftmost
//! (stable within both partitions). Render-side ordering is therefore
//! purely a consequence of the storage order — no separate sort pass
//! lives in `pane_chrome`.
//!
//! Thread ownership: UI thread of one window.

use crate::Window;

impl Window {
    /// Flip the active tab's pinned flag and reorder the focused
    /// group's tab list to keep pinned entries leftmost. Returns
    /// silently when no tab is focused (defensive — the keymap chord
    /// only fires under the `editor.focused` predicate but the test
    /// harness can dispatch directly).
    pub(crate) fn tab_pin_toggle_impl(&mut self) {
        let pane = self.tree.focused;
        let active_tab = match self.tree.groups.get(&pane) {
            Some(g) => g.active,
            None => return,
        };
        if let Some(tab) = self.tree.tabs.get_mut(&active_tab) {
            tab.pinned = !tab.pinned;
        }
        reorder_pinned_first(self, pane);
    }
}

/// Stable-partition the focused group's `tabs` list so pinned tabs
/// are leftmost. Within each partition (pinned and unpinned) the
/// relative order is preserved.
fn reorder_pinned_first(window: &mut Window, pane: crate::pane_tree::PaneId) {
    // Snapshot the tab id list so the classification loop borrows
    // only `tree.tabs`, leaving `tree.groups` free to assign the
    // reordered vector after.
    let tab_ids: Vec<_> = match window.tree.groups.get(&pane) {
        Some(g) => g.tabs.clone(),
        None => return,
    };
    let mut pinned: Vec<_> = Vec::with_capacity(tab_ids.len());
    let mut unpinned: Vec<_> = Vec::with_capacity(tab_ids.len());
    for tid in &tab_ids {
        let is_pinned = window.tree.tabs.get(tid).map(|t| t.pinned).unwrap_or(false);
        if is_pinned {
            pinned.push(*tid);
        } else {
            unpinned.push(*tid);
        }
    }
    if let Some(group) = window.tree.groups.get_mut(&pane) {
        let mut combined = pinned;
        combined.extend(unpinned);
        group.tabs = combined;
    }
}
