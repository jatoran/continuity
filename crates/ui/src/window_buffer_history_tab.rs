//! `Window` wire-up for the buffer-history visualization tab.
//!
//! Three flows live here:
//!
//!  * [`Window::show_buffer_history_tab_impl`] — open-or-focus the
//!    single buffer-history tab in this window. The tab is mounted as
//!    a [`TabKind::BufferHistory`] tab (no underlying buffer) and its
//!    [`BufferHistoryTab`] state is stored on the window keyed by
//!    [`TabId`].
//!  * [`Window::refresh_buffer_history_tab`] — re-query the persist
//!    layer for the current filter and push the result into the
//!    `BufferHistoryTab` state. Called on open, on filter cycle, and
//!    after a buffer is created or trashed.
//!  * [`Window::confirm_buffer_history_tab_selection`] — adopt the
//!    currently-highlighted lane's buffer as a new tab in the focused
//!    pane (mirrors the previous-buffer-browser overlay's commit
//!    path).
//!
//! Thread ownership: UI thread of the owning [`Window`]. Persist
//! queries block on a reply channel; they fire infrequently (open /
//! filter cycle) so the block is acceptable.

use continuity_persist::{BufferHistoryLane, BufferListFilter};
use continuity_render::{
    paint_buffer_history_panel_no_present, BufferHistoryPanelColors, BufferHistoryPanelDraw,
    BufferHistoryPanelRect, BufferHistoryRowDraw,
};

use crate::buffer_history_tab::BufferHistoryTab;
use crate::pane_layout::{compute_leaf_rects, metrics};
use crate::pane_tree::{Tab, TabId};
use crate::pane_tree_kind::TabKind;
use crate::previous_buffer_browser::{compose_subtitle, humanize_age};
use crate::Window;

// The `ViewContext::show_buffer_history_tab` impl lives next to the
// other view-context overrides in `window_view_context.rs`; it is a
// thin wrapper around [`Window::show_buffer_history_tab_impl`].

// The `cycle_*`, `confirm_*`, `recover_*`, and `focused_*_mut`
// helpers below are the input-routing surface: called from the
// pane-body keystroke / mouse dispatch once the history-tab mouse
// + keymap chord wiring lands (S3 follow-on). They are shipped now
// so the integration test in
// `crates/ui/tests/buffer_history_tab_integration.rs` exercises the
// persist↔state↔commit flow end-to-end without a Win32 surface.
#[allow(dead_code)]
impl Window {
    /// Open (or focus) the buffer-history tab in this window.
    ///
    /// At most one buffer-history tab exists per window. First call:
    /// allocate the [`Tab`] with `kind = TabKind::BufferHistory`,
    /// insert it into the focused pane, create the matching
    /// [`BufferHistoryTab`] state, and query persist for the initial
    /// lane data. Subsequent calls: focus the existing tab and
    /// refresh its data so the chart reflects activity since the last
    /// view.
    pub fn show_buffer_history_tab_impl(&mut self) -> Result<(), crate::Error> {
        let now_ms = self.now_ms() as i64;
        let render_buffer_id = self.ensure_buffer_history_render_buffer(now_ms);
        let existing = self
            .tree
            .tabs
            .iter()
            .find(|(_, tab)| tab.is_history())
            .map(|(id, _)| *id);
        let tab_id = match existing {
            Some(id) => {
                // Keep the tab pointed at the live synthetic buffer
                // (it may have been allocated since the tab was
                // restored from persist with a stale id).
                if let Some(tab) = self.tree.tabs.get_mut(&id) {
                    tab.buffer_id = render_buffer_id;
                }
                if !self.focus_existing_tab_for_tab_id(id) {
                    self.adopt_history_tab(id, now_ms);
                }
                id
            }
            None => {
                let id = self.tree.fresh_unused_tab_id();
                let mut tab = Tab::with_id(id, render_buffer_id, now_ms as u64);
                tab.kind = TabKind::BufferHistory;
                tab.label_override = Some("Buffer history".to_string());
                self.tree.tabs.insert(id, tab);
                if let Some(group) = self.tree.groups.get_mut(&self.tree.focused) {
                    group.push_tab(id, true);
                }
                self.buffer_history_tabs
                    .insert(id, BufferHistoryTab::new(now_ms));
                id
            }
        };
        // Always pull fresh data when opening so the chart reflects
        // any buffers created since the user last looked.
        self.refresh_buffer_history_tab(tab_id);
        self.request_repaint();
        Ok(())
    }

    /// Re-query persist for the supplied buffer-history tab's current
    /// filter and push the result into its state. No-op when no
    /// persist client is configured (headless tests) or when `tab_id`
    /// is not actually a buffer-history tab.
    pub(crate) fn refresh_buffer_history_tab(&mut self, tab_id: TabId) {
        let Some(state) = self.buffer_history_tabs.get(&tab_id) else {
            return;
        };
        let filter = state.filter;
        let lanes = self.query_buffer_history_lanes(filter);
        let now_ms = self.now_ms() as i64;
        if let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) {
            state.set_lanes(lanes, now_ms);
        }
    }

    /// Cycle the filter discriminant of `tab_id`'s state and re-query
    /// persist. No-op when `tab_id` is not a buffer-history tab.
    pub(crate) fn cycle_buffer_history_tab_filter(&mut self, tab_id: TabId) {
        let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) else {
            return;
        };
        let next = state.cycle_filter();
        let lanes = self.query_buffer_history_lanes(next);
        let now_ms = self.now_ms() as i64;
        if let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) {
            state.set_lanes(lanes, now_ms);
        }
        self.request_repaint();
    }

    /// Commit path: adopt the selected lane's buffer as a new tab in
    /// the focused pane. Recovers the buffer from persist when it is
    /// not already in editor state. Returns `true` when a buffer was
    /// adopted, `false` when no lane is selected or no persist client
    /// is configured.
    pub(crate) fn confirm_buffer_history_tab_selection(&mut self, tab_id: TabId) -> bool {
        let Some(state) = self.buffer_history_tabs.get(&tab_id) else {
            return false;
        };
        let Some(buffer_id) = state.selected_buffer() else {
            return false;
        };
        if self.editor.snapshot(buffer_id).is_none() {
            self.recover_and_adopt_buffer_for_history(buffer_id);
        }
        self.adopt_buffer_as_new_tab(buffer_id);
        self.request_repaint();
        true
    }

    /// Helper: focus an existing tab by [`TabId`] (parallel to the
    /// buffer-keyed [`Window::focus_existing_tab_for`] used by the
    /// tutorial path). Returns `true` if the tab was found and
    /// focused.
    fn focus_existing_tab_for_tab_id(&mut self, target: TabId) -> bool {
        if !self.tree.tabs.contains_key(&target) {
            return false;
        }
        // Walk every leaf; activate the group that owns the tab.
        let leaves: Vec<_> = self.tree.root.leaf_ids();
        for pane in leaves {
            if let Some(group) = self.tree.groups.get_mut(&pane) {
                if group.tabs.contains(&target) {
                    group.activate(target);
                    self.tree.focus(pane);
                    return true;
                }
            }
        }
        false
    }

    /// Helper for the "tab was closed, then reopened" path: ensure
    /// the tab is in the focused group and active. Mirrors the
    /// tutorial flow's `adopt_buffer_as_new_tab` shape but for a
    /// history tab (no buffer adoption needed).
    fn adopt_history_tab(&mut self, tab_id: TabId, now_ms: i64) {
        if let Some(group) = self.tree.groups.get_mut(&self.tree.focused) {
            if !group.tabs.contains(&tab_id) {
                group.push_tab(tab_id, true);
            } else {
                group.activate(tab_id);
            }
        }
        self.buffer_history_tabs
            .entry(tab_id)
            .or_insert_with(|| BufferHistoryTab::new(now_ms));
    }

    /// Ensure a synthetic empty buffer exists to back the regular
    /// paint pipeline behind the history-panel overlay. Returns the
    /// cached id when already allocated; otherwise adopts a fresh
    /// `Buffer::synthetic_read_only("")`, caches its id on
    /// `view_options`, and returns the new id. Mirrors
    /// `show_tutorial_buffer_impl`'s synthetic-buffer pattern.
    pub(crate) fn ensure_buffer_history_render_buffer(
        &mut self,
        now_ms: i64,
    ) -> continuity_buffer::BufferId {
        if let Some(id) = self.view_options.buffer_history_render_buffer_id {
            if self.editor.snapshot(id).is_some() {
                return id;
            }
        }
        let buffer = continuity_buffer::Buffer::synthetic_read_only("");
        let id = self.editor.adopt_buffer(buffer, 1, now_ms);
        self.view_options.buffer_history_render_buffer_id = Some(id);
        id
    }

    /// Bridge to the persist client. Returns an empty `Vec` when no
    /// persist client is wired (headless tests) or the query errors.
    fn query_buffer_history_lanes(&self, filter: BufferListFilter) -> Vec<BufferHistoryLane> {
        let Some(client) = self.persist_client.as_ref() else {
            return Vec::new();
        };
        client
            .list_buffer_history_timeline(filter)
            .unwrap_or_default()
    }

    /// Mirror of [`Window::recover_and_adopt_buffer`] (private in
    /// `window_previous_buffer_browser.rs`) for the buffer-history
    /// commit path. Pulled in by name so a future refactor that
    /// unifies the two recovery sites doesn't have to deal with two
    /// near-identical bodies.
    fn recover_and_adopt_buffer_for_history(&self, buffer_id: continuity_buffer::BufferId) {
        let Some(client) = self.persist_client.as_ref() else {
            return;
        };
        let Ok(Some(recovered)) = continuity_persist::recover_buffer(client, buffer_id) else {
            return;
        };
        let now = self.now_ms() as i64;
        let _ = self
            .editor
            .adopt_buffer(recovered.buffer, recovered.next_seq, now);
    }

    /// Borrow the buffer-history state for the currently-focused tab,
    /// if it is a [`TabKind::BufferHistory`] tab.
    #[must_use]
    pub(crate) fn focused_buffer_history_state(&self) -> Option<&BufferHistoryTab> {
        let tab = self.tree.active_tab()?;
        if tab.kind != TabKind::BufferHistory {
            return None;
        }
        self.buffer_history_tabs.get(&tab.id)
    }

    /// Mutable variant of [`Self::focused_buffer_history_state`].
    #[must_use]
    pub(crate) fn focused_buffer_history_state_mut(&mut self) -> Option<&mut BufferHistoryTab> {
        let tab_id = {
            let tab = self.tree.active_tab()?;
            if tab.kind != TabKind::BufferHistory {
                return None;
            }
            tab.id
        };
        self.buffer_history_tabs.get_mut(&tab_id)
    }

    /// `true` when the focused tab in the focused pane is a
    /// [`TabKind::BufferHistory`]. The paint pipeline checks this to
    /// short-circuit the rope/decoration path.
    #[must_use]
    pub(crate) fn focused_tab_is_buffer_history(&self) -> bool {
        self.tree
            .active_tab()
            .map(|t| t.kind == TabKind::BufferHistory)
            .unwrap_or(false)
    }

    /// Paint every visible active history tab into its pane body.
    /// The caller presents after all custom surfaces have painted.
    pub(crate) fn paint_visible_buffer_history_overlays_no_present(
        &self,
    ) -> Result<(), crate::Error> {
        let Some(renderer) = self.renderer.as_ref() else {
            return Ok(());
        };
        let Some(text_format) = self.text_format.as_ref() else {
            return Ok(());
        };
        let colors = self.buffer_history_panel_colors();
        for target in self.visible_buffer_history_panels() {
            let draw = self.build_buffer_history_panel_draw_for_tab(target.tab_id, target.rect);
            paint_buffer_history_panel_no_present(renderer, &draw, colors, text_format)
                .map_err(crate::Error::Render)?;
        }
        Ok(())
    }

    /// Build renderer payload for the currently focused history tab.
    pub(crate) fn build_buffer_history_panel_draw(
        &self,
        rect: BufferHistoryPanelRect,
    ) -> BufferHistoryPanelDraw {
        let tab_id = self.tree.active_tab().map(|tab| tab.id);
        self.build_buffer_history_panel_draw_for_optional_tab(tab_id, rect)
    }

    /// Build renderer payload for a specific history tab id.
    pub(crate) fn build_buffer_history_panel_draw_for_tab(
        &self,
        tab_id: TabId,
        rect: BufferHistoryPanelRect,
    ) -> BufferHistoryPanelDraw {
        self.build_buffer_history_panel_draw_for_optional_tab(Some(tab_id), rect)
    }

    fn build_buffer_history_panel_draw_for_optional_tab(
        &self,
        tab_id: Option<TabId>,
        rect: BufferHistoryPanelRect,
    ) -> BufferHistoryPanelDraw {
        let now_ms = self.now_ms() as i64;
        let state = tab_id.and_then(|id| self.buffer_history_tabs.get(&id));
        let (rows, state) = match state {
            Some(s) => (build_panel_rows(&s.lanes, now_ms), Some(s)),
            None => (Vec::new(), None),
        };
        let (
            viewport_start_ms,
            viewport_end_ms,
            selected_lane,
            hovered_lane,
            filter_label,
            scroll_lane_offset,
        ) = match state {
            Some(s) => (
                s.viewport_start_ms,
                s.viewport_end_ms,
                s.selected_lane,
                s.hovered_lane,
                match s.filter {
                    BufferListFilter::ActiveOnly => "active buffers".to_string(),
                    BufferListFilter::All => "all buffers".to_string(),
                    BufferListFilter::TrashedOnly => "trash".to_string(),
                },
                s.scroll_lane_offset,
            ),
            None => (
                now_ms.saturating_sub(30 * 24 * 60 * 60 * 1_000),
                now_ms,
                None,
                None,
                "active buffers".to_string(),
                0,
            ),
        };
        BufferHistoryPanelDraw {
            rect,
            rows,
            viewport_start_ms,
            viewport_end_ms,
            now_ms,
            filter_label,
            selected_lane,
            hovered_lane,
            scroll_lane_offset,
        }
    }

    /// Body rect of the focused pane, projected into the renderer's
    /// `BufferHistoryPanelRect` shape so mouse / wheel hit-tests can
    /// match the paint coordinate system exactly. The history-panel
    /// overlay paints into this rect (tab strip / status bar
    /// excluded).
    pub(crate) fn focused_buffer_history_panel_rect(&self) -> BufferHistoryPanelRect {
        let r = self.focused_body_rect();
        BufferHistoryPanelRect {
            x: r.x,
            y: r.y,
            w: r.w.max(1.0),
            h: r.h.max(1.0),
        }
    }

    /// True when any visible pane's active tab is a buffer-history tab.
    #[must_use]
    pub(crate) fn has_visible_buffer_history_panes(&self) -> bool {
        !self.visible_buffer_history_panels().is_empty()
    }

    fn visible_buffer_history_panels(&self) -> Vec<BufferHistoryPanelTarget> {
        let leaves = compute_leaf_rects(&self.tree, self.pane_root_rect());
        let mut out = Vec::new();
        for (pane_id, rect) in leaves {
            let Some(group) = self.tree.groups.get(&pane_id) else {
                continue;
            };
            let Some(tab) = self.tree.tabs.get(&group.active) else {
                continue;
            };
            if tab.kind != TabKind::BufferHistory {
                continue;
            }
            let body_y = rect.y + metrics::TAB_STRIP_HEIGHT_DIP;
            out.push(BufferHistoryPanelTarget {
                tab_id: tab.id,
                rect: BufferHistoryPanelRect {
                    x: rect.x,
                    y: body_y,
                    w: rect.w.max(1.0),
                    h: (rect.h - metrics::TAB_STRIP_HEIGHT_DIP).max(1.0),
                },
            });
        }
        out
    }

    fn buffer_history_panel_colors(&self) -> BufferHistoryPanelColors {
        let editor = self.active_theme.editor_colors();
        let theme = &self.active_theme.current;
        let to_rgba = crate::window_theme::rgba_from_color;
        let mut muted = editor.fg;
        muted.a *= 0.62;
        let mut hover = to_rgba(theme.panel_active_tab_background());
        hover.a = (hover.a * 0.35).max(0.08);
        BufferHistoryPanelColors {
            background: editor.bg,
            foreground: editor.fg,
            muted_foreground: muted,
            snapshot_dot: editor.fg,
            selected_outline: to_rgba(theme.pane_border_active()),
            hovered_background: hover,
            rule: to_rgba(theme.pane_border()),
            preview_divider: to_rgba(theme.pane_border_active()),
        }
    }
}

#[derive(Copy, Clone)]
struct BufferHistoryPanelTarget {
    tab_id: TabId,
    rect: BufferHistoryPanelRect,
}

/// Translate persisted [`BufferHistoryLane`]s into renderer-side
/// [`BufferHistoryRowDraw`]s with humanized subtitles.
fn build_panel_rows(lanes: &[BufferHistoryLane], now_ms: i64) -> Vec<BufferHistoryRowDraw> {
    lanes
        .iter()
        .map(|lane| {
            let raw_title = lane
                .record
                .title
                .clone()
                .unwrap_or_else(|| "Untitled".to_string());
            let title = clip_with_ellipsis(&raw_title, HISTORY_ROW_TITLE_MAX_CHARS);
            let subtitle = compose_history_row_subtitle(lane, now_ms);
            BufferHistoryRowDraw {
                title,
                subtitle,
                snapshot_times_ms: lane.snapshot_times_ms.clone(),
                is_trashed: lane.record.is_trashed,
                preview: lane.preview.clone(),
            }
        })
        .collect()
}

fn compose_history_row_subtitle(lane: &BufferHistoryLane, now_ms: i64) -> String {
    let age = humanize_age(now_ms, lane.record.last_touched_ms);
    let base = compose_subtitle(&age, lane.record.edit_count, lane.record.is_trashed);
    format!(
        "{base} · {} · {}",
        format_metadata_count(lane.line_count, "line", "lines"),
        format_metadata_count(lane.char_count, "char", "chars")
    )
}

fn format_metadata_count(count: usize, singular: &str, plural: &str) -> String {
    let label = if count == 1 { singular } else { plural };
    format!("{count} {label}")
}

const HISTORY_ROW_TITLE_MAX_CHARS: usize = 32;

fn clip_with_ellipsis(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    // The interesting behavior (open / refresh / commit) requires a
    // real `Window`, which can't be constructed in this `#[cfg(test)]`
    // module without spinning up a Win32 surface. Coverage lives in
    // `crates/ui/tests/buffer_history_tab_integration.rs`, which
    // installs a real persist handle and exercises the persist↔state
    // handoff against a headless window stub.
    use super::*;
    use continuity_buffer::BufferId;
    use continuity_persist::{BufferListFilter, BufferRecord};

    #[test]
    fn filter_cycle_visits_three_discriminants() {
        let mut state = BufferHistoryTab::new(0);
        assert_eq!(state.cycle_filter(), BufferListFilter::All);
        assert_eq!(state.cycle_filter(), BufferListFilter::TrashedOnly);
        assert_eq!(state.cycle_filter(), BufferListFilter::ActiveOnly);
    }

    #[test]
    fn history_row_titles_are_compact() {
        let title = clip_with_ellipsis(&"a".repeat(100), HISTORY_ROW_TITLE_MAX_CHARS);
        assert_eq!(title.chars().count(), HISTORY_ROW_TITLE_MAX_CHARS);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn history_row_subtitles_include_line_and_char_counts() {
        let lane = BufferHistoryLane {
            record: BufferRecord {
                id: BufferId::new(),
                title: Some("note".into()),
                created_at_ms: 0,
                last_touched_ms: 1_000,
                edit_count: 1,
                is_trashed: false,
            },
            snapshot_times_ms: Vec::new(),
            line_count: 1,
            char_count: 5,
            preview: None,
        };
        let rows = build_panel_rows(&[lane], 1_000);
        assert_eq!(rows[0].subtitle, "just now · 1 edit · 1 line · 5 chars");
    }
}
