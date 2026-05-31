//! Phase 13 — pane / tab manipulation methods on `Window`.
//!
//! These methods are the bridge between `command::panes` / `command::tabs`
//! handlers (which call into `ViewContext`) and the pane data model in
//! `crate::pane_tree` + `crate::pane_layout`.
//!
//! Thread ownership: UI thread of one window. The `Window`'s `tree` and
//! `panes` map are mutated only here.

use continuity_buffer::BufferId;

use crate::pane_layout::{
    close_pane, compute_leaf_rects, focus_geometric, metrics, pane_at_point, split_focused,
    FocusDirection, Rect,
};
use crate::pane_tree::{resolve_label, PaneId, SplitAxis, Tab, TabId};
use crate::window::Window;
use crate::Error;

impl Window {
    /// Outer rect of the entire pane tree.
    ///
    /// This is the window client area minus any global chrome that lives
    /// outside the pane tree — currently just the bottom status bar when
    /// `view_options.show_status_bar` is true. Reserving the strip here
    /// keeps the focused pane's body from painting underneath the status
    /// bar (Phase A §A5: status-bar paint path no longer overlays the
    /// bottom-most editor line).
    pub(crate) fn pane_root_rect(&self) -> Rect {
        let w = self.client_width_dip().max(1.0);
        let h = self.client_height_dip().max(1.0);
        let file_tree_width = self.file_tree.visible_width_dip().min((w - 1.0).max(0.0));
        let status_strip = if self.view_options.show_status_bar {
            continuity_render::STATUS_BAR_HEIGHT_DIP.min(h)
        } else {
            0.0
        };
        Rect::new(
            file_tree_width,
            0.0,
            (w - file_tree_width).max(1.0),
            (h - status_strip).max(1.0),
        )
    }

    /// Compute every leaf pane's outer rect (including its tab strip).
    pub(crate) fn pane_outer_rects(&self) -> Vec<(PaneId, Rect)> {
        compute_leaf_rects(&self.tree, self.pane_root_rect())
    }

    /// Body rect of a leaf pane = outer minus its tab strip.
    pub(crate) fn pane_body_rect(&self, pane: PaneId) -> Option<Rect> {
        for (id, outer) in self.pane_outer_rects() {
            if id == pane {
                let strip = metrics::TAB_STRIP_HEIGHT_DIP.min(outer.h);
                return Some(Rect::new(
                    outer.x,
                    outer.y + strip,
                    outer.w,
                    (outer.h - strip).max(1.0),
                ));
            }
        }
        None
    }

    /// Body rect of the currently focused pane.
    pub(crate) fn focused_body_rect(&self) -> Rect {
        self.pane_body_rect(self.tree.focused).unwrap_or_else(|| {
            let r = self.pane_root_rect();
            Rect::new(
                r.x,
                r.y + metrics::TAB_STRIP_HEIGHT_DIP,
                r.w,
                (r.h - metrics::TAB_STRIP_HEIGHT_DIP).max(1.0),
            )
        })
    }

    /// Refresh the focused pane's `view.viewport_*` from its current body
    /// rect. Called after WM_SIZE and after focus / pane-tree mutations.
    ///
    /// δ.3 — wrapped in [`Self::with_caret_line_anchored`] so every
    /// caller that funnels through here (WM_SIZE, pane resize/split,
    /// sidebar toggle, search-minimap appearance, distraction-free
    /// chrome changes) automatically preserves the caret line's screen
    /// y across the geometry change.
    pub(crate) fn refresh_focused_viewport(&mut self) {
        let r = self.focused_body_rect();
        let new_geometry = (r.w, r.h, crate::window::LINE_HEIGHT_DIP);
        let current_geometry = (
            self.view.viewport_width_dip,
            self.view.viewport_height_dip,
            crate::window::LINE_HEIGHT_DIP,
        );
        if new_geometry == current_geometry {
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event("refresh_focused_viewport", "source=unanchored_noop");
            }
            self.refresh_focused_viewport_unanchored();
            return;
        }
        // View was just reset (`viewport_*_dip == 0`): the caller either
        // adopted a new buffer, split into a fresh pane, switched into a
        // pane that has never been focused, or reopened a closed tab —
        // every reset path leaves `scroll_y_dip = 0`. With no prior
        // screen y to preserve, the anchor's restore phase would only
        // pay for nothing: on a 9 k-line markdown buffer at a new wrap
        // width it cold-walks the row-count index via DirectWrite for
        // ~450 ms (`perf-snapshots/manual-lag_after-coalesce_20260518-164726.tsv`
        // captured this as a 208 ms `click_on_left_button_down` when a
        // click into a pane after `apply_layout_shortcut` cleared the
        // saved `panes` map). Skip the anchor in that case and let the
        // first paint compute the real display-row index. Callers that
        // want the caret approximately visible (such as
        // `apply_layout_shortcut`) seed `view.scroll_y_dip` before
        // calling this helper using a source-line approximation.
        if self.view.viewport_width_dip == 0.0 || self.view.viewport_height_dip == 0.0 {
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event(
                    "refresh_focused_viewport",
                    "source=unanchored_view_reset",
                );
            }
            self.refresh_focused_viewport_unanchored();
            return;
        }
        self.with_caret_line_anchored(|w| {
            w.view.viewport_width_dip = r.w;
            w.view.viewport_height_dip = r.h;
        });
    }

    /// Cheap viewport scalar refresh — same body-rect math as
    /// [`Self::refresh_focused_viewport`] without the caret anchor.
    /// Used during a live Win32 sizing loop (WM_ENTERSIZEMOVE →
    /// WM_EXITSIZEMOVE) where running the anchor's display-projection
    /// build on every WM_SIZE is too expensive. A single anchor is
    /// captured before the sizing loop and restored once at the end
    /// against the final projection.
    pub(crate) fn refresh_focused_viewport_unanchored(&mut self) {
        let r = self.focused_body_rect();
        self.view.viewport_width_dip = r.w;
        self.view.viewport_height_dip = r.h;
    }

    /// Save the focused pane's mirrored scalars into `panes[old]`, then
    /// load `panes[new]` into the scalars. The tree's `focused` field is
    /// updated as well.
    pub(crate) fn switch_focus(&mut self, new_pane: PaneId) {
        if !self.tree.groups.contains_key(&new_pane) {
            return;
        }
        let old = self.tree.focused;
        if old == new_pane {
            return;
        }
        let from_buffer = self.buffer_id;
        let from_pane = old;
        let to_buffer = self
            .tree
            .groups
            .get(&new_pane)
            .and_then(|g| self.tree.tabs.get(&g.active).map(|t| t.buffer_id))
            .unwrap_or(BufferId::nil());
        crate::focus_change_trace::emit_buffer_focus_change(
            self,
            from_buffer,
            to_buffer,
            from_pane,
            new_pane,
        );
        self.cancel_scroll_inertia();
        self.clear_unsaved_close_arm();
        // Tree-integrity repair runs only after the early-return guards
        // so callers like `close_focused_pane` can deliberately rewind
        // `tree.focused` to a now-removed pane to drive a transition
        // through this function. A repair before the `old == new_pane`
        // check would rewrite `tree.focused` to a surviving leaf and
        // short-circuit the transition, leaving `self.buffer_id`
        // pointing at the closed pane's buffer.
        crate::window_buffer_tab_repair::repair_pane_tree_structure(&mut self.tree, &self.editor);
        if !self.tree.groups.contains_key(&new_pane) {
            return;
        }
        // Save current scalars.
        self.save_current_right_edge_chrome_state();
        let saved = self.capture_focused_pane_state();
        self.panes.insert(old, saved);
        self.tree.focused = new_pane;

        // Load new scalars.
        if let Some(state) = self.panes.remove(&new_pane) {
            self.apply_pane_state(state);
        } else {
            // Pane has no saved state yet — derive from its active tab.
            let group = &self.tree.groups[&new_pane];
            let buffer_id = self.tree.tabs[&group.active].buffer_id;
            self.apply_new_pane_state(buffer_id);
        }
        self.adopt_focused_tab();
        self.refresh_focused_viewport();
        self.refresh_language();
        self.maybe_submit_decoration();
        // Kick off the projection worker for the new focused buffer's
        // geometry now, before `WM_PAINT` arrives — the worker takes
        // ~450 ms on a 9 k-line buffer cold-walk and would otherwise
        // miss the next paint entirely (the trace surfaced as
        // `frame_display:cold_build reason=not_ready` after a focus
        // switch in
        // `perf-snapshots/manual-lag_after-coalesce_20260518-174445.tsv`).
        // Enumerate every live pane so the just-defocused pane (now a
        // spectator with no spectator-geometry cache entry) prewarms
        // alongside the new focused pane — without this, the first
        // post-focus paint of a large former-focused buffer cold-walks
        // its row index inline
        // (`perf-snapshots/trace_20260522-112026.report.md` captured
        // a 2.95 s spectator paint at t=9411 after a focus switch onto
        // a small buffer left a 19 k-line `development_log.md` in a
        // spectator pane). Fire-and-forget; safe to call before the
        // renderer is up (helper short-circuits).
        self.try_dispatch_focus_change_projection_worker_for_live_panes("switch_focus");
        self.request_state_save();
    }

    /// Mirror the focused pane's scalar state back into the tree (so
    /// `tree.tabs[active].buffer_id` matches `self.buffer_id`). Call this
    /// after any external buffer swap on the focused tab (Phase-8 quick-
    /// open / find-in-all).
    pub(crate) fn sync_focused_tab_buffer(&mut self) {
        let focused = self.tree.focused;
        if let Some(group) = self.tree.groups.get(&focused) {
            let active_tab = group.active;
            if let Some(tab) = self.tree.tabs.get_mut(&active_tab) {
                tab.buffer_id = self.buffer_id;
            }
        }
    }

    /// First non-empty trimmed line of the buffer for label resolution.
    fn first_line_for(&self, buffer_id: BufferId) -> Option<String> {
        let snap = self.editor.snapshot(buffer_id)?;
        let rope = snap.rope_snapshot().rope();
        for i in 0..rope.len_lines().min(8) {
            let line = rope.line(i).to_string();
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        None
    }

    /// Resolve a tab's display label via the standard precedence.
    pub(crate) fn tab_label(&self, tab: &Tab) -> String {
        let first = self.first_line_for(tab.buffer_id);
        resolve_label(tab, first.as_deref())
    }

    /// Open a fresh empty buffer in the focused pane as a new tab.
    pub(crate) fn open_new_tab(&mut self) -> TabId {
        self.cancel_scroll_inertia();
        self.save_current_right_edge_chrome_state();
        let buffer_id = self.editor.open_buffer(String::new());
        let now = self.now_ms();
        let tid = self.tree.open_tab_in_focused(buffer_id, now);
        self.apply_new_pane_state(buffer_id);
        self.refresh_focused_viewport();
        self.refresh_language();
        self.maybe_submit_decoration();
        // P0.8.2 — focus moves to a fresh empty buffer; prewarm the
        // worker so the first paint isn't a cold viewport realize.
        let _ = self.try_dispatch_projection_worker_early("open_new_tab", "focus_change");
        self.retarget_find_bar_to_focused_pane();
        self.request_state_save();
        tid
    }

    /// Drain every pending cross-window tab adoption targeted at `hwnd`
    /// and open each as a fresh tab. Returns `true` if at least one
    /// adoption was processed (the caller should repaint).
    pub(crate) fn drain_cross_window_adoptions(
        &mut self,
        hwnd: windows::Win32::Foundation::HWND,
    ) -> bool {
        let pending = crate::window_registry::drain_adoptions_for(hwnd);
        if pending.is_empty() {
            return false;
        }
        for adoption in pending {
            self.adopt_buffer_as_new_tab(adoption.buffer_id);
        }
        true
    }

    /// Phase 17.6: open an existing buffer as a fresh tab in the focused
    /// pane. Used by the cross-window tab-drop path: the source window
    /// queues an adoption for this buffer, posts `WM_USER + 1` to this
    /// window, and the target's wndproc drains the queue and lands here.
    pub(crate) fn adopt_buffer_as_new_tab(&mut self, buffer_id: BufferId) {
        self.cancel_scroll_inertia();
        self.save_current_right_edge_chrome_state();
        let now = self.now_ms();
        let _ = self.tree.open_tab_in_focused(buffer_id, now);
        self.apply_new_pane_state(buffer_id);
        if let Some(file) = self.editor.snapshot(buffer_id).and_then(|snap| snap.file) {
            self.mark_tab_file_associated(buffer_id, &file);
        }
        self.refresh_focused_viewport();
        self.refresh_language();
        self.maybe_submit_decoration();
        let _ =
            self.try_dispatch_projection_worker_early("adopt_buffer_as_new_tab", "focus_change");
        self.retarget_find_bar_to_focused_pane();
        self.request_state_save();
    }

    /// Split the focused pane on `axis`, creating a new empty buffer in
    /// the new sibling pane and moving focus there.
    pub(crate) fn split(&mut self, axis: SplitAxis) -> PaneId {
        self.cancel_scroll_inertia();
        // Save the currently focused pane's scalars before focus moves.
        let old_focused = self.tree.focused;
        let saved = self.capture_focused_pane_state();
        self.panes.insert(old_focused, saved);

        // Allocate a new buffer + tab for the new pane.
        let buffer_id = self.editor.open_buffer(String::new());
        let now = self.now_ms();
        let tab_id = self.tree.insert_fresh_buffer_tab(buffer_id, now);
        let new_pane = split_focused(&mut self.tree, axis, tab_id, true);

        // Initialize scalars for the new pane.
        self.apply_new_pane_state(buffer_id);
        self.refresh_focused_viewport();
        self.refresh_language();
        self.maybe_submit_decoration();
        // P0.8.2 — split changes wrap_width for every pane involved
        // and the focused buffer is fresh. Prewarm the focused
        // buffer's worker target for the new geometry; spectator-pane
        // prewarm is intentionally deferred (see task report).
        let _ = self.try_dispatch_projection_worker_early("split", "layout_change");
        self.retarget_find_bar_to_focused_pane();
        self.request_state_save();
        new_pane
    }

    /// Close the focused pane (tabs go to recently-closed; the pane
    /// collapses out of the tree). No-op if only one pane remains.
    pub(crate) fn close_focused_pane(&mut self) -> Result<(), Error> {
        let focused = self.tree.focused;
        let now = self.now_ms();
        // Resolve labels up-front since `close_pane` removes the tabs.
        let labels: Vec<(TabId, String)> = self.tree.groups[&focused]
            .tabs
            .iter()
            .filter_map(|tid| self.tree.tabs.get(tid).map(|t| (*tid, self.tab_label(t))))
            .collect();
        let label_lookup = |t: &crate::pane_tree::Tab| -> String {
            labels
                .iter()
                .find(|(id, _)| *id == t.id)
                .map(|(_, s)| s.clone())
                .unwrap_or_else(|| "Untitled".to_string())
        };
        let next = match close_pane(&mut self.tree, focused, now, &label_lookup) {
            Some(p) => p,
            None => return Ok(()),
        };
        // `close_pane` picks the next leaf and writes it into
        // `tree.focused`. Rewind the scalar mirror long enough for
        // `switch_focus` to load that pane's saved state instead of
        // treating the next leaf as already focused.
        self.tree.focused = focused;
        self.switch_focus(next);
        self.panes.remove(&focused);
        self.request_state_save();
        Ok(())
    }

    /// Focus the leaf in `direction` from the current focused pane.
    pub(crate) fn focus_direction(&mut self, direction: FocusDirection) {
        let from = self.tree.focused;
        if let Some(next) = focus_geometric(&self.tree, self.pane_root_rect(), from, direction) {
            self.switch_focus(next);
        }
    }

    /// Sync the scalar state with the focused group's currently active tab.
    pub(crate) fn adopt_focused_tab(&mut self) {
        let focused = self.tree.focused;
        let active = match self.tree.groups.get(&focused) {
            Some(g) => g.active,
            None => return,
        };
        let repaired = crate::window_buffer_tab_repair::repair_buffer_tab(
            &mut self.tree,
            &self.editor,
            active,
        );
        let buffer_id = match self.tree.tabs.get(&active) {
            Some(t) => t.buffer_id,
            None => return,
        };
        if buffer_id != self.buffer_id {
            self.cancel_scroll_inertia();
            self.save_current_right_edge_chrome_state();
            // Reset per-tab scalars; preserve view (per spec §6 tabs share
            // the pane's scroll state? No — view is per-pane and we keep it).
            self.buffer_id = buffer_id;
            self.language = Self::default_language();
            self.language_revision = None;
            self.last_submitted_decoration_revision = None;
            self.apply_right_edge_chrome_for_current_view();
            self.clear_right_edge_layout_caches();
            self.refresh_language();
            self.maybe_submit_decoration();
            // P0.8.2 — in-pane tab switch swaps the focused buffer
            // without crossing `switch_focus`. Prewarm the worker for
            // the incoming buffer so its first paint isn't a cold
            // viewport realize. Gated on `buffer_id != self.buffer_id`
            // because most `adopt_focused_tab` callers run after a
            // sibling code path already set `self.buffer_id` and an
            // additional dispatch would just duplicate work.
            let _ = self.try_dispatch_projection_worker_early("adopt_focused_tab", "focus_change");
        }
        self.retarget_find_bar_to_focused_pane();
        if repaired {
            self.request_state_save();
        }
    }

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

    /// Resolve the pane under the given client-coords (x, y), if any.
    /// Used by the file-drop path in `window_file.rs` to route a
    /// `WM_DROPFILES` to the pane the cursor is over.
    pub(crate) fn pane_at(&self, x: f32, y: f32) -> Option<PaneId> {
        pane_at_point(&self.tree, self.pane_root_rect(), x, y).map(|(p, _)| p)
    }

    /// Convenience used by Phase 13 ViewContext handlers to request a
    /// repaint after a tree mutation.
    pub(crate) fn request_repaint(&self) {
        self.invalidate_with_reason(self.hwnd, "invalidate_rect");
    }
}

mod close_reopen;
