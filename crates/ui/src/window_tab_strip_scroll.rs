//! Item 8 — tab-strip horizontal scroll + crowding helpers, plus the
//! empty-ribbon double-click new-tab path (item 11). Split out of
//! [`crate::window_mouse_tabs`] / [`crate::window_mouse`] to keep both
//! under the 600-line conventions cap.
//!
//! All methods mutate UI-thread-owned state on [`crate::Window`]; the
//! per-pane scroll offset lives in `Window::tab_session.scroll_by_pane`.

use continuity_render::{compute_tab_strip_metrics, TabStripMetrics, TAB_CHEVRON_WIDTH_DIP};

use crate::pane_layout::metrics;
use crate::pane_tree::{PaneId, TabId};
use crate::Window;

/// Item 18 — per-tab session view bookmark: the scroll offset and primary
/// selection captured when the user switches away from a tab, restored when
/// they switch back. SESSION-scoped; never persisted across restart. Keyed
/// by [`TabId`] so two tabs sharing one buffer keep independent state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TabViewBookmark {
    /// Vertical scroll offset in DIPs at switch-away time.
    pub(crate) scroll_y_dip: f32,
    /// Primary selection (anchor + head) at switch-away time. `None` when
    /// the outgoing buffer had no live snapshot at save time.
    pub(crate) primary_selection: Option<continuity_text::Selection>,
}

/// Items 8 + 18 — session-only tab state kept on [`Window`] as a single
/// sub-state field so the canonical `Window` struct stays under the
/// 600-line cap. UI-thread-owned; never persisted across restart.
#[derive(Clone, Debug, Default)]
pub(crate) struct TabSessionState {
    /// Item 8 — per-pane horizontal tab-strip scroll offset (DIPs from the
    /// content's left). Keyed by [`PaneId`] so every leaf (focused or not)
    /// shares one lookup the painter and hit-test both read. Populated only
    /// when a strip overflows; clamped against the live layout.
    pub(crate) scroll_by_pane: std::collections::HashMap<PaneId, f32>,
    /// Item 18 — per-tab view bookmark (scroll + primary selection) keyed
    /// by [`TabId`]. Saved when a tab is switched away from and restored
    /// when it is switched back to.
    pub(crate) view_bookmarks: std::collections::HashMap<TabId, TabViewBookmark>,
    /// Item 18 — the [`TabId`] the focused scalar mirror currently
    /// represents. Used by [`Window::adopt_focused_tab`] to detect a tab
    /// switch (even when both tabs share one buffer). `None` until the
    /// first adopt.
    pub(crate) adopted_tab: Option<TabId>,
}

impl Window {
    /// Item 18 — bookmark the outgoing (currently-adopted) tab's scroll +
    /// primary selection so a later switch back restores them. No-op when
    /// no tab has been adopted yet (program startup / first adopt).
    pub(crate) fn save_tab_view_bookmark_for_adopted(&mut self) {
        let Some(outgoing) = self.tab_session.adopted_tab else {
            return;
        };
        // Skip stale bookmarks for tabs that no longer exist (the outgoing
        // tab was just closed).
        if !self.tree.tabs.contains_key(&outgoing) {
            return;
        }
        let primary_selection = self
            .editor
            .snapshot(self.buffer_id)
            .and_then(|snap| snap.selections().first().copied());
        self.tab_session.view_bookmarks.insert(
            outgoing,
            TabViewBookmark {
                scroll_y_dip: self.view.scroll_y_dip,
                primary_selection,
            },
        );
    }

    /// Item 18 — restore `tab`'s session bookmark: set the scroll offset and
    /// the primary selection, WITHOUT re-anchoring the caret into view (so
    /// the restored scroll position is preserved exactly).
    ///
    /// When `tab` has no bookmark the incoming view defaults to the top only
    /// if the underlying buffer actually changed (`buffer_changed`) — an
    /// in-pane switch to a never-visited tab. A pane-focus switch whose
    /// scalars were just loaded from the pane's own `PerPaneState` (buffer
    /// unchanged) keeps that loaded scroll rather than snapping to top.
    pub(crate) fn restore_tab_view_bookmark(&mut self, tab: TabId, buffer_changed: bool) {
        let Some(bookmark) = self.tab_session.view_bookmarks.get(&tab).copied() else {
            if buffer_changed {
                // No saved state for a freshly-shown buffer — start at top,
                // matching a freshly-opened pane.
                self.view.scroll_y_dip = 0.0;
            }
            return;
        };
        if let Some(selection) = bookmark.primary_selection {
            let _ = self.editor.set_selections(self.buffer_id, vec![selection]);
        }
        self.view.scroll_y_dip = bookmark.scroll_y_dip.max(0.0);
    }

    /// Item 8 — compute the tab-strip layout metrics for `pane` exactly as
    /// the renderer paints it: same `(labels, strip_w, scroll_offset)`. The
    /// scroll offset is read from `Window::tab_session.scroll_by_pane` (the
    /// same value `build_pane_chrome` feeds into `PaneStripDraw`), so the
    /// hit-test and the paint stay byte-identical. Returns the metrics plus
    /// the pane's positional tab ids.
    pub(crate) fn tab_strip_metrics_for_pane(
        &self,
        pane: PaneId,
        outer_w: f32,
    ) -> Option<(TabStripMetrics, Vec<TabId>)> {
        let group = self.tree.groups.get(&pane)?;
        if group.tabs.is_empty() {
            return None;
        }
        let labels: Vec<String> = group
            .tabs
            .iter()
            .map(|tid| {
                self.tree
                    .tabs
                    .get(tid)
                    .map(|t| self.tab_label(t))
                    .unwrap_or_default()
            })
            .collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let offset = self
            .tab_session
            .scroll_by_pane
            .get(&pane)
            .copied()
            .unwrap_or(0.0);
        let strip_metrics = compute_tab_strip_metrics(&label_refs, outer_w, offset);
        Some((strip_metrics, group.tabs.clone()))
    }

    /// Item 8 — resolve a strip-local x click on an overflowing strip to a
    /// chevron, and apply the scroll. Returns `true` when the click hit a
    /// chevron (consumed). `x_in_strip` is relative to the strip's left
    /// edge. A page is one chevron-bounded viewport width.
    pub(crate) fn try_tab_strip_chevron_click(
        &mut self,
        pane: PaneId,
        strip_metrics: &TabStripMetrics,
        x_in_strip: f32,
    ) -> bool {
        if !strip_metrics.overflowing {
            return false;
        }
        let left_cell = strip_metrics.left_chevron_left();
        let right_cell = strip_metrics.right_chevron_left();
        let viewport_w = (strip_metrics.strip_w - TAB_CHEVRON_WIDTH_DIP * 2.0).max(1.0);
        let page = viewport_w * 0.75;
        let delta = if x_in_strip >= left_cell && x_in_strip < left_cell + TAB_CHEVRON_WIDTH_DIP {
            -page
        } else if x_in_strip >= right_cell && x_in_strip < right_cell + TAB_CHEVRON_WIDTH_DIP {
            page
        } else {
            return false;
        };
        self.scroll_tab_strip(pane, delta, strip_metrics);
        self.clear_unsaved_close_arm();
        true
    }

    /// Item 8 — scroll the tab strip of `pane` by `delta_dip` (positive =
    /// right), clamping against the live layout's max scroll. Stores the
    /// clamped value in `Window::tab_session.scroll_by_pane`; removes the
    /// entry when it falls to zero so a non-overflowing strip leaves no
    /// residue. Returns `true` when the stored offset actually changed.
    pub(crate) fn scroll_tab_strip(
        &mut self,
        pane: PaneId,
        delta_dip: f32,
        strip_metrics: &TabStripMetrics,
    ) -> bool {
        let current = self
            .tab_session
            .scroll_by_pane
            .get(&pane)
            .copied()
            .unwrap_or(0.0);
        let next = (current + delta_dip).clamp(0.0, strip_metrics.max_scroll_offset_dip);
        if (next - current).abs() < f32::EPSILON {
            return false;
        }
        if next <= f32::EPSILON {
            self.tab_session.scroll_by_pane.remove(&pane);
        } else {
            self.tab_session.scroll_by_pane.insert(pane, next);
        }
        self.request_repaint();
        true
    }

    /// Item 11 — `WM_LBUTTONDBLCLK` over a pane's tab strip but *not* over
    /// any tab opens a fresh empty tab in that pane. Mirrors the empty-area
    /// branch of [`Window::on_middle_button_down`]: resolve the pane under
    /// the cursor, confirm the point is inside the strip band, confirm
    /// [`Window::tab_at_position`] finds no tab, then focus the pane and
    /// dispatch `tab.new`. Returns `true` when consumed.
    pub(crate) fn try_tab_strip_empty_area_dbl(&mut self, x: i32, y: i32) -> bool {
        let xf = x as f32;
        let yf = y as f32;
        let Some((pane, outer)) = self
            .pane_outer_rects()
            .into_iter()
            .find(|(_, r)| r.contains(xf, yf))
        else {
            return false;
        };
        if yf >= outer.y + metrics::TAB_STRIP_HEIGHT_DIP {
            return false;
        }
        // Only the empty trailing/blank area (no tab under the cursor) opens
        // a new tab; a double-click on a tab keeps its existing behavior
        // (Win32 still fired WM_LBUTTONDOWN, which routed the tab click
        // through `try_tab_strip_left_down`).
        if self.tab_at_position(x, y).is_some() {
            return false;
        }
        self.switch_focus(pane);
        let _ = self.dispatch_command("tab.new", &serde_json::Value::Null);
        true
    }
}
