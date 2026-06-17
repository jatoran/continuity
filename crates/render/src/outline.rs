//! Phase F2 — right-docked outline sidebar.
//!
//! A narrow strip painted on the right edge of a pane that lists the
//! buffer's heading tree, with the current heading highlighted and
//! click-to-jump targets. The strip's width is configurable; when
//! hidden, the rendering layer collapses the sidebar to a thin chevron
//! that re-opens it on click (handled by the UI orchestrator — this
//! module only carries the data + layout).
//!
//! This module is pure layout + types: no D2D / DirectWrite calls. The
//! UI builds an [`OutlineData`] from the active buffer's heading list
//! (via [`continuity_decorate::headings::headings`]) and the renderer
//! paints it from there. Hit-tests use [`OutlineLayout`] returned by
//! [`compute_outline_layout`].
//!
//! Thread ownership: UI thread of the owning window (caller).

use crate::params::Rgba;

/// Default outline-sidebar width in DIPs (per spec §F2 "narrow strip").
pub const OUTLINE_DEFAULT_WIDTH_DIP: f32 = 220.0;

/// Per-row vertical height in DIPs. Taller than the body line height so
/// heading rows get breathing room and don't read as a cramped list; the
/// glyph is vertically centered within the row (see
/// [`crate::outline_paint`]).
pub const OUTLINE_ROW_HEIGHT_DIP: f32 = 26.0;

/// Horizontal inset for the row text run.
pub const OUTLINE_ROW_INDENT_DIP: f32 = 12.0;

/// Per heading level: extra indent applied so the visual tree mirrors
/// heading depth.
pub const OUTLINE_LEVEL_INDENT_DIP: f32 = 12.0;

/// One heading row in the outline sidebar. Built by the UI from
/// [`continuity_decorate::headings::HeadingEntry`] — the list is
/// ordered top-to-bottom by document line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineEntry {
    /// Heading text (`# Foo` stored as `Foo`).
    pub text: String,
    /// Heading level (1..=6). Drives per-row indent.
    pub level: u8,
    /// Source-byte offset of the heading line's start. The click
    /// handler scrolls this byte into view.
    pub target_byte: u32,
}

/// Theme-derived outline-sidebar colors.
#[derive(Copy, Clone, Debug, Default)]
pub struct OutlineColors {
    /// `editor.outline.background` — strip fill.
    pub bg: Rgba,
    /// `editor.outline.foreground` — default row text color.
    pub fg: Rgba,
    /// `editor.outline.foreground_active` — color for the row matching
    /// the current heading at the viewport top.
    pub fg_active: Rgba,
    /// `editor.outline.separator` — vertical edge separating the
    /// sidebar from the pane body.
    pub separator: Rgba,
}

/// All data the renderer needs to lay out + paint one pane's outline
/// sidebar.
#[derive(Clone, Debug)]
pub struct OutlineData<'a> {
    /// Heading rows top-to-bottom.
    pub entries: &'a [OutlineEntry],
    /// Index into `entries` of the row that contains the caret /
    /// viewport-top heading. `None` ⇒ no active row.
    pub current_index: Option<u32>,
    /// Theme colors.
    pub colors: OutlineColors,
    /// Sidebar width in DIPs.
    pub width_dip: f32,
    /// Font size in DIPs used for the row text + width estimate.
    pub font_size_dip: f32,
    /// Independent vertical scroll offset for the outline list.
    pub scroll_offset_dip: f32,
}

/// Bounds of one entry's clickable row inside the sidebar.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct OutlineRowBounds {
    /// Index into [`OutlineData::entries`].
    pub entry_index: u32,
    /// Top edge in pane DIPs.
    pub top: f32,
    /// Bottom edge in pane DIPs.
    pub bottom: f32,
}

/// Per-frame outline-sidebar layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutlineLayout {
    /// Outer rect of the sidebar `(x, y, w, h)` in client DIPs.
    pub rect: (f32, f32, f32, f32),
    /// Per-row hit-test bounds in paint order. Empty when the
    /// sidebar is collapsed or has no entries.
    pub rows: Vec<OutlineRowBounds>,
    /// Full unscrolled height of the outline rows.
    pub content_height_dip: f32,
    /// Clamped scroll offset consumed by this layout.
    pub scroll_offset_dip: f32,
}

impl OutlineLayout {
    /// Hit-test a point in pane-DIP coordinates and return the entry
    /// index whose row contains it, or `None` when the point is outside
    /// the sidebar or in a gap between rows.
    #[must_use]
    pub fn entry_at(&self, x: f32, y: f32) -> Option<u32> {
        let (rx, ry, rw, rh) = self.rect;
        if x < rx || x > rx + rw || y < ry || y > ry + rh {
            return None;
        }
        for row in &self.rows {
            if y >= row.top && y <= row.bottom {
                return Some(row.entry_index);
            }
        }
        None
    }
}

/// Build an [`OutlineLayout`] for the supplied data inside `pane_rect`
/// (`(x, y, w, h)` in client DIPs). The sidebar docks on the right edge:
/// `rect.x = pane_rect.x + pane_rect.w - data.width_dip`.
///
/// `scroll_offset_dip` shifts the rows up by that many DIPs so a long
/// outline can scroll independently of the buffer.
#[must_use]
pub fn compute_outline_layout(
    data: &OutlineData<'_>,
    pane_rect: (f32, f32, f32, f32),
    scroll_offset_dip: f32,
) -> OutlineLayout {
    let (px, py, pw, ph) = pane_rect;
    let w = data.width_dip.max(0.0).min(pw);
    let content_height_dip = data.entries.len() as f32 * OUTLINE_ROW_HEIGHT_DIP;
    let scroll_offset_dip =
        compute_outline_scroll_offset(scroll_offset_dip, content_height_dip, ph);
    if w <= 0.0 || ph <= 0.0 || data.entries.is_empty() {
        return OutlineLayout {
            rect: (px + pw - w, py, w, ph.max(0.0)),
            rows: Vec::new(),
            content_height_dip,
            scroll_offset_dip,
        };
    }
    let rect = (px + pw - w, py, w, ph);
    let mut rows = Vec::with_capacity(data.entries.len());
    let mut cursor_y = py - scroll_offset_dip;
    for (i, _entry) in data.entries.iter().enumerate() {
        let top = cursor_y;
        let bottom = top + OUTLINE_ROW_HEIGHT_DIP;
        // Only include rows that overlap the visible pane band; rows
        // entirely above or below are skipped so the hit-test list
        // stays tight.
        if bottom > py && top < py + ph {
            rows.push(OutlineRowBounds {
                entry_index: i as u32,
                top,
                bottom,
            });
        }
        cursor_y = bottom;
    }
    OutlineLayout {
        rect,
        rows,
        content_height_dip,
        scroll_offset_dip,
    }
}

/// Clamp an outline scroll offset to the current content/viewport pair.
#[must_use]
pub fn compute_outline_scroll_offset(
    scroll_offset_dip: f32,
    content_height_dip: f32,
    viewport_height_dip: f32,
) -> f32 {
    let max_scroll = (content_height_dip - viewport_height_dip.max(0.0)).max(0.0);
    scroll_offset_dip.clamp(0.0, max_scroll)
}

/// Compute the thin scrollbar thumb for an overflowing outline list.
#[must_use]
pub(crate) fn compute_outline_scroll_indicator(
    layout: &OutlineLayout,
) -> Option<(f32, f32, f32, f32)> {
    let (rx, ry, rw, rh) = layout.rect;
    if rw <= 0.0 || rh <= 0.0 || layout.content_height_dip <= rh {
        return None;
    }
    let gutter_w = 4.0_f32.min(rw);
    let thumb_h = ((rh / layout.content_height_dip) * rh).clamp(24.0_f32.min(rh), rh);
    let travel = (rh - thumb_h).max(0.0);
    let max_scroll = (layout.content_height_dip - rh).max(1.0);
    let top = ry + (layout.scroll_offset_dip / max_scroll).clamp(0.0, 1.0) * travel;
    Some((rx + rw - gutter_w, top, gutter_w, thumb_h))
}

/// Per-row horizontal indent (DIPs) for `level`. Used by the painter
/// to place text at `rect.left + indent_for_level(level)`.
#[must_use]
pub fn indent_for_level(level: u8) -> f32 {
    let base = level.saturating_sub(1) as f32;
    OUTLINE_ROW_INDENT_DIP + base * OUTLINE_LEVEL_INDENT_DIP
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str, level: u8, target: u32) -> OutlineEntry {
        OutlineEntry {
            text: text.into(),
            level,
            target_byte: target,
        }
    }

    fn data<'a>(entries: &'a [OutlineEntry]) -> OutlineData<'a> {
        OutlineData {
            entries,
            current_index: None,
            colors: OutlineColors::default(),
            width_dip: OUTLINE_DEFAULT_WIDTH_DIP,
            font_size_dip: 14.0,
            scroll_offset_dip: 0.0,
        }
    }

    #[test]
    fn empty_entries_yields_empty_rows() {
        let segs: Vec<OutlineEntry> = Vec::new();
        let layout = compute_outline_layout(&data(&segs), (0.0, 0.0, 800.0, 600.0), 0.0);
        assert!(layout.rows.is_empty());
        // Rect still anchors on the right edge.
        assert_eq!(layout.rect.0, 800.0 - OUTLINE_DEFAULT_WIDTH_DIP);
    }

    #[test]
    fn rows_anchor_on_pane_right_edge() {
        let segs = vec![entry("A", 1, 0), entry("B", 2, 16)];
        let layout = compute_outline_layout(&data(&segs), (10.0, 20.0, 500.0, 400.0), 0.0);
        let (rx, ry, rw, _rh) = layout.rect;
        assert_eq!(rx, 10.0 + 500.0 - OUTLINE_DEFAULT_WIDTH_DIP);
        assert_eq!(ry, 20.0);
        assert_eq!(rw, OUTLINE_DEFAULT_WIDTH_DIP);
        assert_eq!(layout.rows.len(), 2);
    }

    #[test]
    fn rows_stack_vertically_by_row_height() {
        let segs = vec![entry("A", 1, 0), entry("B", 1, 16), entry("C", 1, 32)];
        let layout = compute_outline_layout(&data(&segs), (0.0, 0.0, 800.0, 600.0), 0.0);
        assert_eq!(layout.rows.len(), 3);
        for w in layout.rows.windows(2) {
            assert_eq!(w[1].top, w[0].bottom);
        }
    }

    #[test]
    fn off_screen_rows_are_clipped_from_layout() {
        let segs: Vec<OutlineEntry> = (0..50)
            .map(|i| entry(&format!("row{i}"), 1, (i * 4) as u32))
            .collect();
        // Pane is 200 DIPs tall; only ~10 rows fit at 22-DIP height.
        let layout = compute_outline_layout(&data(&segs), (0.0, 0.0, 800.0, 200.0), 0.0);
        assert!(layout.rows.len() <= 12);
        assert!(!layout.rows.is_empty());
    }

    #[test]
    fn scroll_offset_shifts_visible_window() {
        let segs: Vec<OutlineEntry> = (0..20)
            .map(|i| entry(&format!("row{i}"), 1, (i * 4) as u32))
            .collect();
        let unscrolled = compute_outline_layout(&data(&segs), (0.0, 0.0, 800.0, 200.0), 0.0);
        let scrolled = compute_outline_layout(
            &data(&segs),
            (0.0, 0.0, 800.0, 200.0),
            OUTLINE_ROW_HEIGHT_DIP * 5.0,
        );
        // First visible row index advances.
        let first_unscrolled = unscrolled.rows.first().map(|r| r.entry_index);
        let first_scrolled = scrolled.rows.first().map(|r| r.entry_index);
        assert!(first_scrolled.unwrap_or(0) > first_unscrolled.unwrap_or(0));
    }

    #[test]
    fn scrolled_rows_start_at_scroll_offset_entry() {
        let segs: Vec<OutlineEntry> = (0..20)
            .map(|i| entry(&format!("row{i}"), 1, (i * 4) as u32))
            .collect();
        let layout = compute_outline_layout(
            &data(&segs),
            (0.0, 0.0, 800.0, 200.0),
            OUTLINE_ROW_HEIGHT_DIP * 5.0,
        );
        assert_eq!(layout.rows.first().map(|row| row.entry_index), Some(5));
    }

    #[test]
    fn bottom_most_painted_row_origin_stays_inside_sidebar_rect() {
        let segs: Vec<OutlineEntry> = (0..50)
            .map(|i| entry(&format!("row{i}"), 1, (i * 4) as u32))
            .collect();
        let layout = compute_outline_layout(&data(&segs), (0.0, 0.0, 800.0, 201.0), 0.0);
        let (_, top, _, height) = layout.rect;
        let bottom = top + height;
        let last = layout.rows.last().expect("visible row");
        assert!(last.top >= top);
        assert!(last.top < bottom);
    }

    #[test]
    fn scroll_indicator_appears_only_for_overflow() {
        let segs: Vec<OutlineEntry> = (0..50)
            .map(|i| entry(&format!("row{i}"), 1, (i * 4) as u32))
            .collect();
        let overflowing = compute_outline_layout(&data(&segs), (0.0, 0.0, 800.0, 200.0), 88.0);
        assert!(compute_outline_scroll_indicator(&overflowing).is_some());

        let fitting = compute_outline_layout(&data(&segs[..2]), (0.0, 0.0, 800.0, 200.0), 0.0);
        assert!(compute_outline_scroll_indicator(&fitting).is_none());
    }

    #[test]
    fn entry_at_returns_row_index_inside_strip() {
        let segs = vec![entry("A", 1, 0), entry("B", 2, 16)];
        let layout = compute_outline_layout(&data(&segs), (0.0, 0.0, 800.0, 600.0), 0.0);
        let first = layout.rows[0];
        let mid_y = (first.top + first.bottom) / 2.0;
        let mid_x = layout.rect.0 + layout.rect.2 / 2.0;
        assert_eq!(layout.entry_at(mid_x, mid_y), Some(0));
    }

    #[test]
    fn entry_at_returns_none_outside_strip() {
        let segs = vec![entry("A", 1, 0)];
        let layout = compute_outline_layout(&data(&segs), (0.0, 0.0, 800.0, 600.0), 0.0);
        // Click to the left of the sidebar.
        assert!(layout.entry_at(layout.rect.0 - 10.0, 50.0).is_none());
    }

    #[test]
    fn indent_for_level_grows_with_depth() {
        assert!(indent_for_level(1) < indent_for_level(2));
        assert!(indent_for_level(2) < indent_for_level(3));
        // Level-1 sits at the base indent.
        assert_eq!(indent_for_level(1), OUTLINE_ROW_INDENT_DIP);
    }

    #[test]
    fn zero_width_collapses_rows() {
        let segs = vec![entry("A", 1, 0)];
        let mut d = data(&segs);
        d.width_dip = 0.0;
        let layout = compute_outline_layout(&d, (0.0, 0.0, 800.0, 600.0), 0.0);
        assert!(layout.rows.is_empty());
    }
}
