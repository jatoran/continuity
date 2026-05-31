//! Find-bar target tracking for pane and tab focus changes.

use crate::find_bar::FindTarget;
use crate::Window;

impl Window {
    /// Recompute after focus retargeting without moving the caret.
    pub(crate) fn retarget_find_bar_to_focused_pane(&mut self) {
        if self.overlays.find_bar().is_none() {
            return;
        }
        self.refresh_find_selection_scope_for_focused_pane();
        self.recompute_find_matches_impl(false, false);
        self.view_options.search_minimap_layout = None;
        self.request_repaint();
    }

    /// Make sure match navigation never consumes stale byte ranges.
    pub(crate) fn ensure_find_matches_current_for_focused_pane(&mut self) {
        if self.find_matches_are_current_for_focused_pane() {
            return;
        }
        self.refresh_find_selection_scope_for_focused_pane();
        self.recompute_find_matches_impl(false, false);
    }

    /// Step the active find bar and jump to the resulting match.
    pub(crate) fn step_find_bar(&mut self, delta: i32) {
        self.ensure_find_matches_current_for_focused_pane();
        if let Some(fb) = self.overlays.find_bar_mut() {
            fb.step(delta);
        }
        self.jump_to_current_find_match();
    }

    /// `true` when the find bar's match set belongs to the focused target.
    pub(crate) fn find_matches_are_current_for_focused_pane(&self) -> bool {
        let Some(fb) = self.overlays.find_bar() else {
            return false;
        };
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return false;
        };
        fb.matches_target(self.current_find_target(snap.rope_snapshot().revision().0))
    }

    /// Build the current focused-pane find target for `revision`.
    pub(crate) fn current_find_target(&self, revision: u64) -> FindTarget {
        FindTarget {
            pane_id: self.tree.focused,
            buffer_id: self.buffer_id,
            revision,
        }
    }

    /// Compact label shown beside the find counter.
    pub(crate) fn current_find_target_label(&self) -> String {
        let pane_number = self
            .tree
            .root
            .leaf_ids()
            .iter()
            .position(|pane_id| *pane_id == self.tree.focused)
            .map(|idx| idx + 1)
            .unwrap_or(1);
        let title = self
            .tree
            .groups
            .get(&self.tree.focused)
            .and_then(|group| self.tree.tabs.get(&group.active))
            .map(|tab| self.tab_label(tab))
            .unwrap_or_else(|| "Untitled".to_string());
        format!("P{}: {}", pane_number, compact_find_target_title(&title))
    }

    fn refresh_find_selection_scope_for_focused_pane(&mut self) {
        let ranges = self.current_find_selection_ranges();
        let Some(fb) = self.overlays.find_bar_mut() else {
            return;
        };
        fb.selection_scope_ranges = ranges;
    }
}

fn compact_find_target_title(title: &str) -> String {
    const MAX_CHARS: usize = 32;
    let mut chars = title.chars();
    let mut compact: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        compact.push_str("...");
    }
    compact
}
