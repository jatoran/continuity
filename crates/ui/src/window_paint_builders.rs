//! Per-frame `DrawParams` builders extracted from
//! [`crate::window_paint`] so the parent file stays under the
//! conventions cap.
//!
//! Each function takes a `&Window` and returns a slice / struct the
//! `on_paint` orchestrator hands to [`continuity_render::DrawParams`].
//! No internal mutation — the builders are read-only views over the
//! window's pane tree, editor snapshots, spell state, and decorations.
//!
//! Thread ownership: UI thread of one window.

use continuity_core::EditorSnapshot;
use continuity_decorate::HeadingEntry;
use continuity_layout::ViewState;
use continuity_render::{PaneChromeDraw, PaneStripDraw, PanelColors, SpellSquiggleSpan, TabLabel};

use crate::pane_layout::{compute_leaf_rects, metrics};
use crate::window::Window;

/// Build the per-frame [`PaneChromeDraw`] from the window's pane tree.
/// Each leaf gets a strip across the top of its outer rect; the focused
/// leaf is flagged so the chrome paints the active border.
pub(crate) fn build_pane_chrome(window: &Window) -> Option<PaneChromeDraw> {
    let leaves = compute_leaf_rects(&window.tree, window.pane_root_rect());
    if leaves.is_empty() {
        return None;
    }
    let mut panes: Vec<PaneStripDraw> = Vec::with_capacity(leaves.len());
    for (pane_id, rect) in &leaves {
        let group = match window.tree.groups.get(pane_id) {
            Some(g) => g,
            None => continue,
        };
        let mut labels: Vec<TabLabel> = Vec::with_capacity(group.tabs.len());
        let mut active_index = 0;
        for (i, tab_id) in group.tabs.iter().enumerate() {
            if let Some(tab) = window.tree.tabs.get(tab_id) {
                let is_active = *tab_id == group.active;
                // Hover mode follows the UI-thread tab-hover slot, so
                // only the tab under the cursor gets the close glyph.
                let show_close = crate::tab_hover::is_tab_close_visible(
                    window.view_options.tab_close_button,
                    window.mouse_state.tab_hover,
                    *pane_id,
                    *tab_id,
                );
                labels.push(TabLabel {
                    text: window.tab_label(tab),
                    dirty: is_tab_dirty(window, tab),
                    show_close,
                });
                if is_active {
                    active_index = i;
                }
            }
        }
        panes.push(PaneStripDraw {
            outer: (rect.x, rect.y, rect.w, rect.h),
            // The active highlight requires the window itself to hold
            // focus: a background continuity window must not advertise
            // an "active" pane while the user works in another app
            // (`is_window_focused`, WM_ACTIVATEAPP) or in another
            // continuity window (`has_keyboard_focus`, WM_SETFOCUS /
            // WM_KILLFOCUS).
            focused: *pane_id == window.tree.focused
                && window.is_window_focused
                && window.has_keyboard_focus,
            tabs: labels,
            active_index,
            focus_motion: None,
            active_tab_motion: None,
            previous_active_tab_index: None,
            // Item 8 — horizontal tab-strip scroll offset for this pane.
            // The renderer clamps it against the live layout; we feed the
            // raw stored value so the same `(labels, strip_w, offset)`
            // reaches both paint and hit-test.
            tab_scroll_offset_dip: window
                .tab_session
                .scroll_by_pane
                .get(pane_id)
                .copied()
                .unwrap_or(0.0),
        });
    }
    let panel_colors = panel_colors_from_theme(window);
    Some(PaneChromeDraw {
        panes,
        colors: panel_colors,
        strip_height: metrics::TAB_STRIP_HEIGHT_DIP,
        tab_drag: crate::window_tab_drag_overlay::build_tab_drag_overlay(window),
    })
}

fn panel_colors_from_theme(window: &Window) -> PanelColors {
    let t = &window.active_theme.current;
    PanelColors {
        bg: rgba_from_theme(t.panel_background()),
        fg: rgba_from_theme(t.panel_foreground()),
        active_tab_bg: rgba_from_theme(t.panel_active_tab_background()),
        active_tab_fg: rgba_from_theme(t.panel_active_tab_foreground()),
        inactive_tab_bg: rgba_from_theme(t.panel_inactive_tab_background()),
        inactive_tab_fg: rgba_from_theme(t.panel_inactive_tab_foreground()),
        pane_border: rgba_from_theme(t.pane_border()),
        pane_border_active: rgba_from_theme(t.pane_border_active()),
    }
}

fn rgba_from_theme(c: continuity_theme::Color) -> continuity_render::Rgba {
    crate::window_theme::rgba_from_color(c)
}

/// Compute whether the tab's marker dot should show.
///
/// Ephemeral (non-file) buffers persist automatically and are never
/// "dirty" in the editor's sense. File-associated buffers are dirty
/// when the rope's FNV-1a hash diverges from the last-saved content hash
/// recorded in [`continuity_buffer::FileAssociation`]. Computing the
/// hash is O(bytes); the result is cached on
/// [`Window::tab_dirty_cache`] keyed by `(BufferId, rope_revision)`
/// so a large (~6000-line) buffer pays the walk only when the
/// revision moves, not once per tab per paint frame.
pub(crate) fn is_tab_dirty(window: &Window, tab: &crate::pane_tree::Tab) -> bool {
    tab.file_associated && is_buffer_dirty_against_file(window, tab.buffer_id)
}

/// Whether `buffer_id` carries unexported edits relative to its file
/// association — i.e. the in-memory rope content hash differs from
/// `FileAssociation.content_hash` (the decoded text at last open / save /
/// reload). Returns `false` for buffers with no association, no live
/// snapshot, or an untouched revision-0 import.
///
/// This is the canonical "clean vs. dirty against disk" decision used by
/// both the dirty-tab gutter dot ([`is_tab_dirty`]) and external-change
/// reconciliation ([`crate::window_file_reconcile`]).
pub(crate) fn is_buffer_dirty_against_file(
    window: &Window,
    buffer_id: continuity_buffer::BufferId,
) -> bool {
    let Some(snap) = window.editor.snapshot(buffer_id) else {
        return false;
    };
    let Some(file) = snap.file.as_ref() else {
        return false;
    };
    let revision = snap.rope_snapshot().revision().get();
    if revision == 0 {
        return false;
    }
    let mut cache = window.tab_dirty_cache.borrow_mut();
    let entry = cache.entry(buffer_id).or_insert((u64::MAX, 0));
    if entry.0 != revision {
        entry.0 = revision;
        entry.1 = continuity_persist::fnv1a_64_chunks(
            snap.rope_snapshot().rope().chunks().map(str::as_bytes),
        );
    }
    entry.1 != file.content_hash
}

/// One pre-collected non-focused pane body: the data that
/// [`continuity_render::PaneBodyDraw`] will borrow from. Held in a `Vec`
/// whose lifetime covers the construction of the `pane_bodies` slice
/// passed into [`continuity_render::DrawParams`].
pub(crate) struct NonFocusedPaneRender {
    /// Owning pane in the window's tree. Keys the per-pane spectator
    /// `FrameDisplay` cache (see [`crate::window_spectator_cache`]).
    pub pane_id: crate::pane_tree::PaneId,
    /// Document id (`BufferId.as_uuid().as_u128()`) — keys the layout cache.
    pub document: u128,
    /// Strongly-typed buffer id — used by builders that take a `BufferId`
    /// (e.g. the inline-image placement builder). Kept alongside
    /// `document` so call sites don't have to round-trip through Uuid.
    pub buffer_id: continuity_buffer::BufferId,
    /// Body rect in client DIPs `(x, y, w, h)` — pane outer rect with
    /// the tab strip already subtracted.
    pub rect: (f32, f32, f32, f32),
    /// Snapshot of the pane's active buffer. Owns the rope arc + the
    /// `Vec<Selection>` borrowed by the `PaneBodyDraw`.
    pub snapshot: EditorSnapshot,
    /// Per-pane view (scroll, zoom, soft wrap, viewport dims).
    pub view: ViewState,
    /// Buffer-local minimap visibility for this pane.
    pub minimap: bool,
    /// Buffer-local outline sidebar visibility for this pane.
    pub show_outline_sidebar: bool,
}

/// Collect a [`NonFocusedPaneRender`] for every leaf in the pane tree
/// other than the focused one. Skips panes whose active tab's buffer
/// has no live snapshot (e.g. just-adopted buffers mid-frame).
pub(crate) fn collect_non_focused_panes(window: &Window) -> Vec<NonFocusedPaneRender> {
    let leaves = compute_leaf_rects(&window.tree, window.pane_root_rect());
    let strip = metrics::TAB_STRIP_HEIGHT_DIP;
    let mut out: Vec<NonFocusedPaneRender> = Vec::with_capacity(leaves.len().saturating_sub(1));
    for (pane_id, rect) in leaves {
        if pane_id == window.tree.focused {
            continue;
        }
        let group = match window.tree.groups.get(&pane_id) {
            Some(g) => g,
            None => continue,
        };
        let active_tab = match window.tree.tabs.get(&group.active) {
            Some(t) => t,
            None => continue,
        };
        let buffer_id = active_tab.buffer_id;
        let snapshot = match window.editor.snapshot(buffer_id) {
            Some(s) => s,
            None => continue,
        };
        let view = window
            .panes
            .get(&pane_id)
            .map(|s| s.view.clone())
            .unwrap_or_default();
        let right_edge_chrome = window.right_edge_chrome_state_for_buffer(buffer_id);
        let body_y = rect.y + strip;
        let body_h = (rect.h - strip).max(0.0);
        out.push(NonFocusedPaneRender {
            pane_id,
            document: buffer_id.as_uuid().as_u128(),
            buffer_id,
            rect: (rect.x, body_y, rect.w, body_h),
            snapshot,
            view,
            minimap: right_edge_chrome.minimap,
            show_outline_sidebar: right_edge_chrome.outline,
        });
    }
    out
}

/// Map every cached [`crate::window_spell::SpellSpan`] (absolute byte
/// ranges) onto the per-line `(line, byte_in_line)` form the renderer
/// expects. Returns an empty `Vec` when spell-check is off so
/// `params.spell_spans = &[]` produces zero squiggle paint.
///
/// Multi-line spans are clipped at the line break that contains
/// `range.start`; in practice spell errors are always single words and
/// therefore always single-line. Out-of-range bytes are skipped rather
/// than panicking — spell errors lag the rope by at most one frame.
pub(crate) fn build_spell_squiggle_spans(
    window: &Window,
    rope: &ropey::Rope,
) -> Vec<SpellSquiggleSpan> {
    if !window.spell().enabled() {
        return Vec::new();
    }
    let errors = window.spell().errors();
    if errors.is_empty() {
        return Vec::new();
    }
    let total_bytes = rope.len_bytes();
    let mut out: Vec<SpellSquiggleSpan> = Vec::with_capacity(errors.len());
    for span in errors {
        let start = span.range.start.min(total_bytes);
        let end = span.range.end.min(total_bytes);
        if end <= start {
            continue;
        }
        let line = rope.byte_to_line(start);
        let line_start_byte = rope.line_to_byte(line);
        let line_end_byte = if line + 1 < rope.len_lines() {
            rope.line_to_byte(line + 1)
        } else {
            rope.len_bytes()
        };
        let clipped_end = end.min(line_end_byte);
        if clipped_end <= start {
            continue;
        }
        out.push(SpellSquiggleSpan {
            line: line as u32,
            byte_in_line_start: (start - line_start_byte) as u32,
            byte_in_line_end: (clipped_end - line_start_byte) as u32,
        });
    }
    out
}

/// Phase F2 — pick the outline row matching the primary caret's
/// enclosing heading (`sections::heading_index_at`). Returns `None`
/// when the caret sits before the first heading or no headings exist.
pub(crate) fn compute_outline_current_index(
    headings: &[HeadingEntry],
    rope: &ropey::Rope,
    selections: &[continuity_text::Selection],
) -> Option<u32> {
    if headings.is_empty() {
        return None;
    }
    let sel = selections.first()?;
    let line = sel.head.line as usize;
    let line_start = if line < rope.len_lines() {
        rope.line_to_byte(line)
    } else {
        rope.len_bytes()
    };
    let caret_byte = line_start + sel.head.byte_in_line as usize;
    continuity_decorate::heading_index_at(headings, caret_byte)
        .map(|i| u32::try_from(i).unwrap_or(u32::MAX))
}
