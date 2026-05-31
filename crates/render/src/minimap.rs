//! Scaled-text minimap — pure geometry.
//!
//! A narrow column docked on the right edge of an editor pane that
//! paints the buffer at roughly 1/12 horizontal scale so the reader
//! can navigate by silhouette (VS Code / Sublime style). This module
//! owns only the math: where the strip sits, how each source line
//! maps to a minimap y, where the viewport indicator goes, and how
//! the minimap scrolls when the buffer is taller than the pane.
//!
//! The painter ([`crate::minimap_paint`]) consumes a [`MinimapLayout`]
//! and produces the actual D2D + DirectWrite calls. Keeping the math
//! here means it stays unit-testable without spinning up a swap chain.
//!
//! Thread ownership: caller (render thread of the owning window).

use crate::params::Rgba;

/// Width of the minimap column in DIPs. Matches the right margin the
/// [`crate::chrome::ContentMargins`] resolver reserves when
/// `view_options.minimap` is on, so glyphs in the editor body wrap
/// flush against the minimap's left edge.
pub const MINIMAP_WIDTH_DIP: f32 = 80.0;

/// Font size in DIPs used for the minimap's tiny [`IDWriteTextLayout`]
/// per-line glyphs. Sized so a 13-DIP body font compresses to roughly
/// one-sixth height, which is small enough that whole paragraphs read
/// as silhouettes but tall enough that headings remain distinguishable.
pub const MINIMAP_FONT_SIZE_DIP: f32 = 2.4;

/// Per-line vertical advance inside the minimap, in DIPs. Slightly
/// taller than the font size so descenders don't touch the next line.
pub const MINIMAP_LINE_HEIGHT_DIP: f32 = 2.7;

/// Inner horizontal padding (DIPs) inside the minimap strip. Keeps
/// glyphs off the strip's left/right edges so the column reads as a
/// thumbnail rather than a wall of text.
pub const MINIMAP_INNER_PADDING_DIP: f32 = 4.0;

/// Theme-derived minimap colors.
#[derive(Copy, Clone, Debug, Default)]
pub struct MinimapColors {
    /// `editor.minimap.background` — strip fill.
    pub bg: Rgba,
    /// `editor.minimap.foreground` — scaled-glyph color.
    pub fg: Rgba,
    /// `editor.minimap.viewport_indicator` — translucent box drawn over
    /// the section of the minimap currently visible in the editor body.
    pub viewport_indicator: Rgba,
}

/// Per-frame minimap layout.
///
/// All coordinates are pane-local DIPs (the renderer applies
/// `body_origin` via `SetTransform` before painting). `rect` is the
/// strip's outer box, `indicator_rect` is the visible-viewport overlay,
/// and `(scroll_y_dip, font_size_dip, line_height_dip)` plus
/// `total_lines` are everything the painter needs to place the per-
/// source-line scaled glyphs.
#[derive(Clone, Debug, PartialEq)]
pub struct MinimapLayout {
    /// Outer rect of the strip `(x, y, w, h)`.
    pub rect: (f32, f32, f32, f32),
    /// Rect of the visible-viewport indicator box `(x, y, w, h)`.
    pub indicator_rect: (f32, f32, f32, f32),
    /// Vertical advance per source line inside the minimap (DIPs).
    pub line_height_dip: f32,
    /// Font size to render scaled-down glyphs at (DIPs).
    pub font_size_dip: f32,
    /// Minimap-local y of the first visible source line. Subtracted
    /// from `line_idx * line_height_dip` to scroll the strip when the
    /// buffer is taller than the pane.
    pub scroll_y_dip: f32,
    /// Total source-line count from the rope. Empty buffers store `1`
    /// so callers never have to clamp.
    pub total_lines: u64,
    /// First source-line index whose minimap row is at or below the
    /// strip's top edge after `scroll_y_dip` is applied. Lines below
    /// this index don't need painting.
    pub first_visible_line: u64,
    /// One past the last source-line index whose minimap row is still
    /// above the strip's bottom edge. Lines at or beyond this index
    /// don't need painting.
    pub last_visible_line: u64,
}

/// Result of [`hit_test`]. Source-line index a click on the minimap
/// strip resolves to, plus the strip's center y for the clicked line
/// (the painter and the click handler share this so the indicator
/// always lines up with the clicked row).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MinimapHit {
    /// Source line the click resolved to (0-based).
    pub line: u64,
    /// Center y of that line's row in pane-local DIPs.
    pub y_center_dip: f32,
}

/// Build the layout for a pane's minimap.
///
/// - `pane_rect` — `(x, y, w, h)` of the pane body in client DIPs. The
///   strip docks at `pane_rect.x + pane_rect.w - right_inset_dip -
///   MINIMAP_WIDTH_DIP`.
/// - `scroll_y_dip` — current vertical scroll of the editor body (DIPs).
/// - `line_height_dip` — the editor's body line-height in DIPs. Used to
///   compute the visible-viewport indicator height proportional to how
///   many full body lines fit in the pane.
/// - `total_lines` — total source lines in the buffer (≥1 enforced).
/// - `right_inset_dip` — DIPs already reserved on the right edge by an
///   outer chrome consumer (currently the outline sidebar). Pass `0.0`
///   when no other sidebar is active.
#[must_use]
pub fn compute_minimap_layout(
    pane_rect: (f32, f32, f32, f32),
    scroll_y_dip: f32,
    line_height_dip: f32,
    total_lines: u64,
    right_inset_dip: f32,
) -> MinimapLayout {
    let (px, py, pw, ph) = pane_rect;
    let right_inset = right_inset_dip.max(0.0);
    let strip_w = MINIMAP_WIDTH_DIP.min((pw - right_inset).max(0.0));
    let strip_x = (px + pw - right_inset - strip_w).max(px);
    let strip_h = ph.max(0.0);

    let total = total_lines.max(1);
    let minimap_full_h = total as f32 * MINIMAP_LINE_HEIGHT_DIP;
    let body_total_h = total as f32 * line_height_dip.max(1.0);
    let body_visible_h = strip_h.max(1.0);

    // How far through the buffer is the editor scrolled? Same fraction
    // applies to the minimap so the indicator stays aligned with the
    // section of the buffer currently on screen.
    let scrollable_body = (body_total_h - body_visible_h).max(0.0);
    let progress = if scrollable_body > 0.0 {
        (scroll_y_dip / scrollable_body).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // The minimap itself only needs to scroll when the strip is too
    // short to hold every line at MINIMAP_LINE_HEIGHT_DIP. Otherwise
    // every line is painted in place at the top of the strip.
    let scrollable_minimap = (minimap_full_h - body_visible_h).max(0.0);
    let minimap_scroll_y = progress * scrollable_minimap;

    let indicator_h_uncapped =
        (body_visible_h / body_total_h.max(1.0)) * minimap_full_h.min(body_visible_h);
    let indicator_h = indicator_h_uncapped
        .min(body_visible_h)
        .max(MINIMAP_LINE_HEIGHT_DIP.min(body_visible_h));
    let indicator_top_in_strip =
        (scroll_y_dip / body_total_h.max(1.0)) * minimap_full_h - minimap_scroll_y;
    let indicator_y = py + indicator_top_in_strip.max(0.0);

    let first_visible_line = {
        let f = (minimap_scroll_y / MINIMAP_LINE_HEIGHT_DIP).floor();
        (f.max(0.0) as u64).min(total - 1)
    };
    let last_visible_line = {
        let f = ((minimap_scroll_y + body_visible_h) / MINIMAP_LINE_HEIGHT_DIP).ceil();
        (f.max(0.0) as u64).min(total)
    };

    MinimapLayout {
        rect: (strip_x, py, strip_w, strip_h),
        indicator_rect: (strip_x, indicator_y, strip_w, indicator_h),
        line_height_dip: MINIMAP_LINE_HEIGHT_DIP,
        font_size_dip: MINIMAP_FONT_SIZE_DIP,
        scroll_y_dip: minimap_scroll_y,
        total_lines: total,
        first_visible_line,
        last_visible_line,
    }
}

/// Resolve a `(x, y)` click in pane-local DIPs to the source line whose
/// minimap row contains it, or `None` when the click misses the strip.
#[must_use]
pub fn hit_test(layout: &MinimapLayout, x_dip: f32, y_dip: f32) -> Option<MinimapHit> {
    let (rx, ry, rw, rh) = layout.rect;
    if rw <= 0.0 || rh <= 0.0 {
        return None;
    }
    if x_dip < rx || x_dip > rx + rw || y_dip < ry || y_dip > ry + rh {
        return None;
    }
    let local_y = y_dip - ry + layout.scroll_y_dip;
    let line = (local_y / layout.line_height_dip).floor().max(0.0) as u64;
    let line = line.min(layout.total_lines.saturating_sub(1));
    let center = ry - layout.scroll_y_dip + (line as f32 + 0.5) * layout.line_height_dip;
    Some(MinimapHit {
        line,
        y_center_dip: center,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_short() -> MinimapLayout {
        // 3-line buffer in an 800x600 pane.
        compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 3, 0.0)
    }

    #[test]
    fn strip_docks_at_right_edge_minus_inset() {
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 50, 0.0);
        let (x, _, w, _) = l.rect;
        assert!((w - MINIMAP_WIDTH_DIP).abs() < 0.1);
        assert!((x - (800.0 - MINIMAP_WIDTH_DIP)).abs() < 0.1);

        // With an outline sidebar inset of 220 DIP, the strip is pulled
        // left by exactly that much.
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 50, 220.0);
        let (x, _, _, _) = l.rect;
        assert!((x - (800.0 - 220.0 - MINIMAP_WIDTH_DIP)).abs() < 0.1);
    }

    #[test]
    fn short_buffer_does_not_stretch_minimap_lines() {
        // The whole point of the rewrite: 3 lines should map to 3 short
        // rows at the top, not 3 quarter-height rows spread across the
        // entire pane. Each row is exactly MINIMAP_LINE_HEIGHT_DIP tall.
        let l = layout_short();
        assert_eq!(l.line_height_dip, MINIMAP_LINE_HEIGHT_DIP);
        // No minimap-scroll when the buffer's full minimap height is
        // already inside the strip.
        assert_eq!(l.scroll_y_dip, 0.0);
        assert_eq!(l.first_visible_line, 0);
        // Last visible is clamped to total_lines so the painter loop
        // never asks for a line past the rope.
        assert!(l.last_visible_line <= l.total_lines);
    }

    #[test]
    fn long_buffer_scrolls_minimap_proportionally() {
        // 1000 lines × 18-DIP body line height = 18000-DIP content, but
        // only 600-DIP pane. At MINIMAP_LINE_HEIGHT_DIP = 2.7 DIP the
        // full minimap is 2700 DIP — more than the pane, so it scrolls.
        let mid = (1000.0 * 18.0 - 600.0) * 0.5;
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), mid, 18.0, 1000, 0.0);
        // At 50% body-scroll the minimap should be ~halfway through its
        // own scrollable range, give or take rounding from the
        // indicator math.
        let scrollable_minimap = 1000.0 * MINIMAP_LINE_HEIGHT_DIP - 600.0;
        let expected = 0.5 * scrollable_minimap;
        assert!((l.scroll_y_dip - expected).abs() < 1.0);
        // And the first/last visible window is non-trivial.
        assert!(l.first_visible_line > 0);
        assert!(l.last_visible_line < 1000);
    }

    #[test]
    fn indicator_rect_tracks_visible_portion() {
        // Buffer fits entirely in pane — indicator covers everything
        // from top of strip down through the visible-content fraction.
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 10, 0.0);
        let (_, iy, _, ih) = l.indicator_rect;
        assert!(iy >= 0.0);
        assert!(ih > 0.0 && ih <= 600.0);

        // Long buffer scrolled to ~25% — indicator lives somewhere in
        // the upper half of the strip.
        let scroll = 0.25 * (1000.0 * 18.0 - 600.0);
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), scroll, 18.0, 1000, 0.0);
        let (_, iy, _, ih) = l.indicator_rect;
        assert!(iy >= 0.0);
        assert!(iy < 600.0);
        assert!(ih > 0.0);
    }

    #[test]
    fn empty_buffer_clamps_to_one_line() {
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 0, 0.0);
        assert_eq!(l.total_lines, 1);
        assert!(l.last_visible_line >= 1);
    }

    #[test]
    fn hit_test_inside_strip_returns_clicked_line() {
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 50, 0.0);
        let (rx, ry, rw, _) = l.rect;
        // Click on the 10th minimap row.
        let click_y = ry + 10.0 * MINIMAP_LINE_HEIGHT_DIP + 0.1;
        let hit = hit_test(&l, rx + rw * 0.5, click_y).expect("click lands in strip");
        assert_eq!(hit.line, 10);
    }

    #[test]
    fn hit_test_outside_strip_returns_none() {
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 50, 0.0);
        let (rx, ry, _, rh) = l.rect;
        assert!(hit_test(&l, rx - 5.0, ry + 100.0).is_none());
        assert!(hit_test(&l, rx + 4.0, ry + rh + 10.0).is_none());
    }
}
