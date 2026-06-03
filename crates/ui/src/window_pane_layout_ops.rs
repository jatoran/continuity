//! Layout-changing operations on the pane tree.
//!
//! Methods here mutate `Window::tree`'s shape (layout shortcut reshuffle,
//! pane resize, maximize toggle) and reset or refresh the focused pane's
//! mirrored scalar state to match. Sibling-split off `window_panes.rs`
//! to keep that file under the conventions cap; the per-window
//! geometric primitives (`pane_root_rect`, `focused_body_rect`,
//! `refresh_focused_viewport*`) and the focus / tab lifecycle methods
//! stay in `window_panes.rs`.
//!
//! Thread ownership: UI thread of one window. Each method assumes
//! `Window::tree`, `Window::panes`, and the focused-pane scalar
//! mirror are sole-owned by the calling thread (matching
//! `window_panes.rs`).

use std::collections::HashSet;

use crate::pane_layout::resize_focused;
use crate::pane_shortcuts::{apply_layout_with_tabs, LayoutShortcut};
use crate::pane_tree::{SplitAxis, TabId};
use crate::window::Window;

impl Window {
    /// Apply a layout shortcut, reflowing tabs round-robin per spec §6.
    pub(crate) fn apply_layout_shortcut(&mut self, shortcut: LayoutShortcut) {
        crate::window_buffer_tab_repair::repair_pane_tree_structure(&mut self.tree, &self.editor);
        crate::window_buffer_tab_repair::repair_missing_buffer_tabs(&mut self.tree, &self.editor);
        self.adopt_focused_tab();
        let old_body_rect = self.focused_body_rect();
        let line_height = self.effective_line_height();
        let old_geometry = (old_body_rect.w, old_body_rect.h, line_height);
        let right_edge_chrome = self.current_right_edge_chrome_state();
        self.save_current_right_edge_chrome_state();
        // Save current focused state into the panes map first so it is not
        // lost when the layout rebuild allocates fresh group ids.
        let old = self.tree.focused;
        let saved = self.capture_focused_pane_state();
        self.panes.insert(old, saved);

        // Top up with fresh empty buffers so every new leaf receives at
        // least one tab. Existing unedited empty placeholders are not
        // carried forward; they were created only to fill split leaves.
        let target = shortcut.leaf_count();
        let mut layout_tabs = collect_layout_tabs_without_disposable_placeholders(self);
        if layout_tabs.len() < target {
            let now = self.now_ms();
            for _ in 0..(target - layout_tabs.len()) {
                let buffer_id = self.editor.open_buffer(String::new());
                let tab_id = self.tree.insert_fresh_buffer_tab(buffer_id, now);
                layout_tabs.push(tab_id);
            }
        }

        apply_layout_with_tabs(&mut self.tree, shortcut, layout_tabs);

        // The new focused pane's active tab determines the active buffer.
        let group = &self.tree.groups[&self.tree.focused];
        let active_tab = group.active;
        let buffer_id = self.tree.tabs[&active_tab].buffer_id;

        // After layout reshuffle, prior `panes` keys are stale. Clear and
        // rebuild scalars fresh for the new focused pane.
        self.panes.clear();
        self.buffer_id = buffer_id;
        self.apply_right_edge_chrome_state(right_edge_chrome);
        self.remember_current_right_edge_chrome_state();
        let new_body_rect = self.focused_body_rect();
        let mut view = continuity_layout::ViewState::new();
        let geometry_unchanged = (new_body_rect.w, new_body_rect.h, line_height) == old_geometry;
        if geometry_unchanged {
            view.viewport_width_dip = new_body_rect.w;
            view.viewport_height_dip = new_body_rect.h;
            view.line_height_dip = line_height;
            view.overscroll_bottom_dip = crate::window_font_picker::compute_overscroll_bottom_dip(
                self.view_options.scroll_past_end,
                new_body_rect.h,
                line_height,
            );
        }
        // Approximate scroll using the caret's *source* line. This lets
        // the caret land near the top of the new pane without paying
        // [`Window::with_caret_line_anchored`], whose restore phase walks
        // the row-count index at the NEW wrap width. On a 9 k-line
        // markdown buffer that walker measures every source line via
        // DirectWrite (~54 µs/line, ~450 ms total — see
        // `perf-snapshots/manual-lag_after-coalesce_20260518-154032.tsv`,
        // events `caret_anchor_frame_source source=viewport_build
        // stale_cache=wrap_width_dip` at t≈5443/5447 followed by
        // `WM_SYSKEYDOWN dur=458365`). The anchor's "preserve the
        // pre-reflow screen y" semantic does not apply here anyway:
        // `view` was just reset to `ViewState::new()` (scroll_y_dip = 0),
        // so there is no prior screen y to preserve. Source-line
        // approximation is exact for unwrapped buffers; with wrap on it
        // is off by the wrap-row multiplier, and the first paint after
        // refines it via the real display-row index.
        if !geometry_unchanged {
            if let Some(snap) = self.editor.snapshot(buffer_id) {
                if let Some(sel) = snap.selections().first() {
                    let caret_top = sel.head.line as f32 * line_height;
                    // Land the caret roughly a third of the way down the
                    // new pane so a few rows of preceding context stay
                    // visible.
                    view.scroll_y_dip = (caret_top - new_body_rect.h * 0.3).max(0.0);
                }
            }
        }
        self.view = view;
        self.language = Self::default_language();
        self.language_revision = None;
        self.last_submitted_decoration_revision = None;
        // Skip [`Window::refresh_focused_viewport`] — its anchored
        // variant is the cost source above, and the unanchored variant
        // is the correct semantic here. `view.scroll_y_dip` was set
        // just above and `view.viewport_*_dip` will be populated by
        // [`Window::refresh_focused_viewport_unanchored`].
        self.refresh_focused_viewport_unanchored();
        self.refresh_language();
        self.maybe_submit_decoration();
        // Kick off the projection worker for the new geometry. The
        // worker takes ~450 ms on a 9 k-line buffer at a never-seen
        // wrap width; submitting here gives it a head start so the
        // next paint can pick up the result (or use a cold-deferred
        // stub if not yet ready) instead of cold-walking inline.
        self.try_dispatch_layout_projection_worker_for_live_panes("apply_layout_shortcut");
        self.retarget_find_bar_to_focused_pane();
        self.request_state_save();
    }

    /// Toggle maximize-within-window for the focused pane.
    pub(crate) fn toggle_maximize_focused_pane(&mut self) {
        let focused = self.tree.focused;
        if self.tree.maximized == Some(focused) {
            self.tree.maximized = None;
        } else {
            self.tree.maximized = Some(focused);
        }
        self.refresh_focused_viewport();
        // P0.8.2 — wrap_width changes when the focused pane goes
        // maximized or restores; prewarm the worker so the next
        // paint at the new geometry can pick up a stamp-matched
        // result instead of inline-cold-walking the row index.
        self.try_dispatch_layout_projection_worker_for_live_panes("toggle_maximize");
        self.request_state_save();
    }

    /// Resize the focused pane along `axis` by `delta` DIPs (positive grows).
    pub(crate) fn resize_focused_pane(&mut self, axis: SplitAxis, delta_dip: f32) {
        let root = self.pane_root_rect();
        let dim = match axis {
            SplitAxis::Horizontal => root.w,
            SplitAxis::Vertical => root.h,
        };
        resize_focused(&mut self.tree, axis, delta_dip, dim);
        self.refresh_focused_viewport();
        // P0.8.2 — keyboard-driven pane resize commits a wrap_width
        // change; prewarm the worker for the new geometry.
        self.try_dispatch_layout_projection_worker_for_live_panes("resize_focused_pane");
        self.request_state_save();
    }
}

fn collect_layout_tabs_without_disposable_placeholders(window: &Window) -> Vec<TabId> {
    let Some(focused_active) = window
        .tree
        .groups
        .get(&window.tree.focused)
        .map(|group| group.active)
    else {
        return Vec::new();
    };

    let mut ordered = Vec::new();
    ordered.push(focused_active);
    for pane in window.tree.root.leaf_ids() {
        let Some(group) = window.tree.groups.get(&pane) else {
            continue;
        };
        for tab in &group.tabs {
            if *tab != focused_active {
                ordered.push(*tab);
            }
        }
    }

    let mut seen = HashSet::new();
    let mut retained = Vec::new();
    for tab_id in ordered {
        if !seen.insert(tab_id) {
            continue;
        }
        if !is_disposable_layout_placeholder_tab(window, tab_id) {
            retained.push(tab_id);
        }
    }

    if retained.is_empty() && window.tree.tabs.contains_key(&focused_active) {
        retained.push(focused_active);
    }
    retained
}

fn is_disposable_layout_placeholder_tab(window: &Window, tab_id: TabId) -> bool {
    let Some(tab) = window.tree.tabs.get(&tab_id) else {
        return false;
    };
    if !tab.is_buffer() || tab.pinned || tab.label_override.is_some() {
        return false;
    }
    let Some(snapshot) = window.editor.snapshot(tab.buffer_id) else {
        return false;
    };
    if snapshot.file.is_some() {
        return false;
    }
    let rope = snapshot.rope_snapshot();
    rope.revision() == continuity_buffer::Revision::INITIAL && rope.rope().len_bytes() == 0
}
