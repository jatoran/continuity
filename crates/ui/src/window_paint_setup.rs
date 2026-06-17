//! Per-frame paint-setup helpers for [`crate::Window`].
//!
//! Sibling of `window_paint.rs`. Holds the early-frame bookkeeping
//! ([`Window::prepare_paint_frame`]) that `on_paint` runs before the
//! rope/decoration/layout pipeline — draining worker results, refreshing
//! spell + time-machine state, and lazy-allocating the buffer-history
//! render buffer. Pulled out so `window_paint.rs` stays under the
//! 600-line cap.
//!
//! Thread ownership: UI thread of one window. Mutates UI-thread-owned
//! state only (`spell_state`, `tree`, `buffer_id`, decoration cache).

use windows::Win32::Foundation::HWND;

use crate::{Error, Window};

impl Window {
    /// Run the guarded frame-entry setup before snapshot selection.
    /// Returns `false` when the background paint throttle intentionally
    /// skips this frame.
    pub(crate) fn begin_paint_frame(&mut self, hwnd: HWND) -> Result<bool, Error> {
        // Keep cheap OS chrome sync before the background-paint skip so
        // throttled frames still pick up theme/title changes.
        self.sync_titlebar_theme();
        self.sync_window_title();
        if self.should_skip_background_paint() {
            return Ok(false);
        }

        // Deferred font-swap: if the worker has delivered the target
        // font_state, swap before `ensure_renderer` rebuilds text_format.
        self.try_apply_pending_font_swap(self.tree.focused);
        self.ensure_renderer(hwnd)?;
        self.ensure_projection_worker();
        self.prepare_paint_frame();
        self.drain_spectator_projection_worker_results();
        Ok(true)
    }

    /// Run the per-frame setup that must happen before snapshotting the
    /// rope: drain pending decoration results, request fresh decorations
    /// for spectator panes, ensure spell errors are fresh, refresh the
    /// time-machine preview, and lazy-allocate the buffer-history render
    /// buffer when the focused tab is a history view.
    pub(crate) fn prepare_paint_frame(&mut self) {
        // §I2: the metrics buffer overlay is now applied *after* the
        // regular paint pipeline runs (chrome + empty-rope body), not
        // as a full-window bypass — otherwise the panel paints over
        // the tab strip and status bar. The overlay step happens later
        // in `on_paint`, between `draw_buffer_no_present` and `Present`.
        {
            let _scope = crate::paint_trace::EventScope::new("drain_decoration_results");
            let _ = self.drain_decoration_results();
        }
        // Activation-grace gate: skip spectator-pane decoration
        // submission for the first ~second after the user returned
        // to the window. Submission itself only sends a request to
        // the worker pool, but the per-buffer `rope.to_string()` it
        // does for stale panes can total tens of ms on big buffers
        // — that is exactly the kind of work that contributes to
        // focus-return stall.
        if !self.in_activation_grace() {
            let _scope =
                crate::paint_trace::EventScope::new("submit_decorations_for_visible_panes");
            self.submit_decorations_for_visible_panes();
            // Block 2.1: reclaim ~35 MB/buffer of tree-sitter heap by
            // dropping retained trees for off-screen buffers. Cheap unless
            // the visible/MRU keep-set changed since the last paint.
            self.prune_offscreen_decoration_trees();
        } else {
            crate::paint_trace::log_event(
                "submit_decorations_for_visible_panes",
                "skipped=activation_grace",
            );
        }
        // Phase 16.5: refresh cached spell errors *before* taking
        // borrows for the rest of the paint frame — `ensure_spell_fresh`
        // mutates `self.spell_state`, while the rest of the function
        // holds shared borrows of view / cache / theme.
        if !self.in_activation_grace() {
            let _scope = crate::paint_trace::EventScope::new("ensure_spell_fresh");
            self.ensure_spell_fresh();
        } else {
            crate::paint_trace::log_event("ensure_spell_fresh", "skipped=activation_grace");
        }
        // Phase-I1: when the time-machine slider has a preview revision
        // pinned, refresh the cache (which materializes the historical
        // rope via persist) and substitute the preview snapshot for the
        // live editor snapshot. Read-only — never mutates buffer state
        // or persistence.
        {
            let _scope = crate::paint_trace::EventScope::new("refresh_time_machine_preview");
            self.refresh_time_machine_preview_if_needed();
        }
        // Buffer-history tab: lazy-allocate the synthetic render
        // buffer if a restored history tab is pointing at a stale or
        // nil buffer id. The rest of the paint pipeline then runs
        // normally with an empty rope behind the panel overlay.
        if self.focused_tab_is_buffer_history() {
            let now_ms = self.now_ms() as i64;
            let render_id = self.ensure_buffer_history_render_buffer(now_ms);
            let focused = self.tree.focused;
            if let Some(group) = self.tree.groups.get(&focused) {
                let active_tab_id = group.active;
                if let Some(tab) = self.tree.tabs.get_mut(&active_tab_id) {
                    if tab.kind == crate::pane_tree_kind::TabKind::BufferHistory
                        && tab.buffer_id != render_id
                    {
                        tab.buffer_id = render_id;
                    }
                }
            }
            self.buffer_id = render_id;
        }
    }
}
