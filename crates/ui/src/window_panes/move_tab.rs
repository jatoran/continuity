//! Moving a tab between panes of one window (tab drag onto another pane's
//! body or strip).
//!
//! Split out of `window_panes.rs` to keep that file under the 600-line cap.
//!
//! Thread ownership: UI thread of one window; mutates `Window::tree` and,
//! through `switch_focus`, the focused-pane mirror.

use crate::window::Window;
use crate::Error;

impl Window {
    /// Move `tab` from `source_pane` into `target_pane` (inside the same
    /// window). The tab keeps its `BufferId` and identity. The target
    /// pane becomes focused and activates the moved tab. If the source
    /// pane has no remaining tabs it collapses out of the tree (matching
    /// the close-the-active-tab behaviour). No-op when source == target
    /// or when either pane is unknown.
    pub(crate) fn move_tab_between_panes(
        &mut self,
        tab: crate::pane_tree::TabId,
        source_pane: crate::pane_tree::PaneId,
        target_pane: crate::pane_tree::PaneId,
    ) -> Result<(), Error> {
        if source_pane == target_pane {
            return Ok(());
        }
        // Snapshot the source group's existence + remove the moving tab.
        let source_exists = self.tree.groups.contains_key(&source_pane);
        if !source_exists || !self.tree.groups.contains_key(&target_pane) {
            return Ok(());
        }
        // Bookmark the dragged tab while the focused mirror still describes
        // it, so it lands in the destination pane at its own scroll and
        // selection instead of the top of the buffer.
        if self.tree.focused == source_pane && self.tab_session.adopted_tab == Some(tab) {
            self.save_tab_view_bookmark_for_adopted();
        }
        let removed_from_source = if let Some(g) = self.tree.groups.get_mut(&source_pane) {
            g.remove_tab(tab).is_some() || g.tabs.contains(&tab)
        } else {
            false
        };
        let _ = removed_from_source;
        // If the tab was never in source, bail.
        if !self.tree.tabs.contains_key(&tab) {
            return Ok(());
        }
        // Append into target group; make it active + MRU front.
        let now = self.now_ms();
        if let Some(g) = self.tree.groups.get_mut(&target_pane) {
            if !g.tabs.contains(&tab) {
                g.tabs.push(tab);
            }
            g.mru.retain(|t| *t != tab);
            g.mru.insert(0, tab);
            g.active = tab;
            let _ = now;
        }
        // If source pane is now empty, collapse it (mirror close_active_tab).
        let source_empty = self
            .tree
            .groups
            .get(&source_pane)
            .is_some_and(|g| g.tabs.is_empty());
        if source_empty {
            // Use the same pane-collapse path as close_focused_pane, but
            // for an arbitrary pane id. Briefly route through focus to
            // reuse the existing close path.
            let prev_focused = self.tree.focused;
            self.tree.focused = source_pane;
            let _ = self.close_focused_pane();
            // close_focused_pane re-focuses to a sibling; if the user's
            // drop target is still alive we honor it next.
            self.tree.focused = prev_focused;
        }
        // Switch focus to the destination pane (the user expects to be
        // editing the dropped tab now).
        if self.tree.groups.contains_key(&target_pane) {
            self.switch_focus(target_pane);
            self.adopt_focused_tab();
        }
        self.request_state_save();
        Ok(())
    }
}
