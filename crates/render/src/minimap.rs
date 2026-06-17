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

/// Result of [`hit_test`]. The click is resolved in **display-row**
/// space so it stays consistent with the editor's scroll (which lives in
/// display rows, not source lines) — under soft-wrap a source-line click
/// would land at the wrong scroll position. `target_scroll_dip` is the
/// editor scroll offset the click maps to; the click handler applies it
/// directly. `line` is the source line the click is nearest (for traces /
/// callers that still want a row hint) and `y_center_dip` is the strip y
/// the indicator should center on.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MinimapHit {
    /// Source line the click resolved to (0-based). Best-effort hint only;
    /// scrolling uses [`Self::target_scroll_dip`].
    pub line: u64,
    /// Center y of the clicked point in pane-local DIPs.
    pub y_center_dip: f32,
    /// Editor scroll offset (DIPs, display-row space) the click maps to.
    /// Proportional to the click's fraction down the strip track.
    pub target_scroll_dip: f32,
}

/// Build the layout for a pane's minimap.
///
/// §28 — the indicator + scroll geometry are driven off the editor's
/// **display-row** content height (`content_height_dip`), not the source
/// line count, so the strip stays consistent with the editor scroll under
/// soft-wrap / folds / reserved rows (the editor scroll lives in display
/// rows). The per-line glyph paint still walks source lines because the
/// minimap renders source content; the discrepancy between that and the
/// display-row scroll is absorbed by the proportional indicator + click
/// math, mirroring how the scrollbar tracks display rows.
///
/// - `pane_rect` — `(x, y, w, h)` of the pane body in client DIPs. The
///   strip docks at `pane_rect.x + pane_rect.w - right_inset_dip -
///   MINIMAP_WIDTH_DIP`.
/// - `scroll_y_dip` — current vertical scroll of the editor body (DIPs,
///   display-row space).
/// - `line_height_dip` — the editor's body line-height in DIPs.
/// - `total_lines` — total source lines in the buffer (≥1 enforced). Used
///   only for the glyph-paint window.
/// - `content_height_dip` — total editor content height in display-row
///   space (`display_row_count * line_height`). Drives the indicator and
///   click resolution. Falls back to `total_lines * line_height` when a
///   caller passes `0.0` (no projection yet).
/// - `right_inset_dip` — DIPs already reserved on the right edge by an
///   outer chrome consumer (currently the outline sidebar). Pass `0.0`
///   when no other sidebar is active.
#[must_use]
pub fn compute_minimap_layout(
    pane_rect: (f32, f32, f32, f32),
    scroll_y_dip: f32,
    line_height_dip: f32,
    total_lines: u64,
    content_height_dip: f32,
    right_inset_dip: f32,
) -> MinimapLayout {
    let (px, py, pw, ph) = pane_rect;
    let right_inset = right_inset_dip.max(0.0);
    let strip_w = MINIMAP_WIDTH_DIP.min((pw - right_inset).max(0.0));
    let strip_x = (px + pw - right_inset - strip_w).max(px);
    let strip_h = ph.max(0.0);

    let total = total_lines.max(1);
    let line_h = line_height_dip.max(1.0);
    let minimap_full_h = total as f32 * MINIMAP_LINE_HEIGHT_DIP;
    let body_visible_h = strip_h.max(1.0);
    // Display-row content height is the ground truth; fall back to the
    // source-line estimate when the caller has no projection yet.
    let content_h = if content_height_dip > 0.0 {
        content_height_dip
    } else {
        total as f32 * line_h
    }
    .max(body_visible_h);

    // Editor scroll progress in display-row space — identical to the
    // scrollbar's notion of "how far through the content am I?".
    let scroll_max = (content_h - body_visible_h).max(0.0);
    let progress = if scroll_max > 0.0 {
        (scroll_y_dip / scroll_max).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // The minimap glyph column only needs to scroll when the strip is too
    // short to hold every source line at MINIMAP_LINE_HEIGHT_DIP.
    let scrollable_minimap = (minimap_full_h - body_visible_h).max(0.0);
    let minimap_scroll_y = progress * scrollable_minimap;

    // Proportional thumb/track indicator (matches the scrollbar): the
    // indicator height is the visible fraction of the content scaled to
    // the strip, and it travels the full strip track by `progress`.
    let visible_fraction = (body_visible_h / content_h).clamp(0.0, 1.0);
    let indicator_h = (strip_h * visible_fraction)
        .max(MINIMAP_LINE_HEIGHT_DIP.min(strip_h))
        .min(strip_h);
    let travel = (strip_h - indicator_h).max(0.0);
    let indicator_y = py + progress * travel;

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

/// Resolve a `(x, y)` click in pane-local DIPs to an editor scroll target
/// in display-row space, or `None` when the click misses the strip.
///
/// §28 — the click is resolved **proportionally**: the fraction of the
/// strip track the click sits at maps to the same fraction of the
/// editor's scrollable range. This keeps clicks consistent with the
/// editor scroll (and with the indicator built above) under soft-wrap,
/// where a source-line click would jump to the wrong place.
///
/// `content_height_dip` is the editor's display-row content height and
/// `viewport_height_dip` the visible body height — the same pair the
/// editor clamps its scroll against. `target_scroll_dip` on the result is
/// already clamped to `[0, content - viewport]`.
#[must_use]
pub fn hit_test(
    layout: &MinimapLayout,
    x_dip: f32,
    y_dip: f32,
    content_height_dip: f32,
    viewport_height_dip: f32,
) -> Option<MinimapHit> {
    let (rx, ry, rw, rh) = layout.rect;
    if rw <= 0.0 || rh <= 0.0 {
        return None;
    }
    if x_dip < rx || x_dip > rx + rw || y_dip < ry || y_dip > ry + rh {
        return None;
    }
    // Click fraction down the visible strip track, centered so a click in
    // the middle of the strip lands mid-content.
    let track_fraction = ((y_dip - ry) / rh).clamp(0.0, 1.0);
    let viewport_h = viewport_height_dip.max(0.0);
    let content_h = content_height_dip.max(viewport_h.max(1.0));
    let scroll_max = (content_h - viewport_h).max(0.0);
    // Center the viewport on the clicked fraction of content.
    let target_center = track_fraction * content_h;
    let target_scroll_dip = (target_center - viewport_h * 0.5).clamp(0.0, scroll_max);

    // Best-effort source-line hint for traces (the minimap glyph column
    // is still source-indexed).
    let local_y = y_dip - ry + layout.scroll_y_dip;
    let line = (local_y / layout.line_height_dip).floor().max(0.0) as u64;
    let line = line.min(layout.total_lines.saturating_sub(1));

    Some(MinimapHit {
        line,
        y_center_dip: y_dip,
        target_scroll_dip,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_short() -> MinimapLayout {
        // 3-line buffer in an 800x600 pane; content fits the viewport.
        compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 3, 3.0 * 18.0, 0.0)
    }

    #[test]
    fn strip_docks_at_right_edge_minus_inset() {
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 50, 50.0 * 18.0, 0.0);
        let (x, _, w, _) = l.rect;
        assert!((w - MINIMAP_WIDTH_DIP).abs() < 0.1);
        assert!((x - (800.0 - MINIMAP_WIDTH_DIP)).abs() < 0.1);

        // With an outline sidebar inset of 220 DIP, the strip is pulled
        // left by exactly that much.
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 50, 50.0 * 18.0, 220.0);
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
        let content_h = 1000.0 * 18.0;
        let mid = (content_h - 600.0) * 0.5;
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), mid, 18.0, 1000, content_h, 0.0);
        // At 50% editor-scroll the minimap glyph column should be ~halfway
        // through its own scrollable range.
        let scrollable_minimap = 1000.0 * MINIMAP_LINE_HEIGHT_DIP - 600.0;
        let expected = 0.5 * scrollable_minimap;
        assert!((l.scroll_y_dip - expected).abs() < 1.0);
        // And the first/last visible window is non-trivial.
        assert!(l.first_visible_line > 0);
        assert!(l.last_visible_line < 1000);
    }

    #[test]
    fn indicator_uses_display_row_content_height() {
        // Soft-wrap case: 1000 source lines but the display projection has
        // 2000 rows (every line wraps once). Driving the indicator off the
        // display-row content height keeps it consistent with editor
        // scroll: a tall content ⇒ a short indicator.
        let source_content = 1000.0 * 18.0;
        let display_content = 2000.0 * 18.0;
        let source_driven = compute_minimap_layout(
            (0.0, 0.0, 800.0, 600.0),
            0.0,
            18.0,
            1000,
            source_content,
            0.0,
        );
        let display_driven = compute_minimap_layout(
            (0.0, 0.0, 800.0, 600.0),
            0.0,
            18.0,
            1000,
            display_content,
            0.0,
        );
        // Taller display content ⇒ a smaller visible fraction ⇒ a shorter
        // indicator than the source-line-only estimate would produce.
        assert!(display_driven.indicator_rect.3 < source_driven.indicator_rect.3);
    }

    #[test]
    fn indicator_rect_tracks_visible_portion() {
        // Buffer fits entirely in pane — indicator covers ~everything.
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 10, 10.0 * 18.0, 0.0);
        let (_, iy, _, ih) = l.indicator_rect;
        assert!(iy >= 0.0);
        assert!(ih > 0.0 && ih <= 600.0);

        // Long buffer scrolled to ~25% — indicator lives somewhere in
        // the upper half of the strip.
        let content_h = 1000.0 * 18.0;
        let scroll = 0.25 * (content_h - 600.0);
        let l =
            compute_minimap_layout((0.0, 0.0, 800.0, 600.0), scroll, 18.0, 1000, content_h, 0.0);
        let (_, iy, _, ih) = l.indicator_rect;
        assert!(iy >= 0.0);
        assert!(iy < 600.0);
        assert!(ih > 0.0);
    }

    #[test]
    fn empty_buffer_clamps_to_one_line() {
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 0, 0.0, 0.0);
        assert_eq!(l.total_lines, 1);
        assert!(l.last_visible_line >= 1);
    }

    #[test]
    fn hit_test_resolves_proportional_scroll_target() {
        let content_h = 1000.0 * 18.0;
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 1000, content_h, 0.0);
        let (rx, ry, rw, rh) = l.rect;
        // Click at the vertical midpoint of the strip ⇒ center the
        // viewport on the middle of the content.
        let hit = hit_test(&l, rx + rw * 0.5, ry + rh * 0.5, content_h, 600.0)
            .expect("click lands in strip");
        let scroll_max = content_h - 600.0;
        let expected = (content_h * 0.5 - 300.0).clamp(0.0, scroll_max);
        assert!((hit.target_scroll_dip - expected).abs() < 1.0);
    }

    #[test]
    fn hit_test_top_click_scrolls_to_origin() {
        let content_h = 1000.0 * 18.0;
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 1000, content_h, 0.0);
        let (rx, ry, rw, _) = l.rect;
        let hit = hit_test(&l, rx + rw * 0.5, ry + 0.5, content_h, 600.0).expect("in strip");
        assert_eq!(hit.target_scroll_dip, 0.0);
    }

    #[test]
    fn hit_test_outside_strip_returns_none() {
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 50, 50.0 * 18.0, 0.0);
        let (rx, ry, _, rh) = l.rect;
        assert!(hit_test(&l, rx - 5.0, ry + 100.0, 50.0 * 18.0, 600.0).is_none());
        assert!(hit_test(&l, rx + 4.0, ry + rh + 10.0, 50.0 * 18.0, 600.0).is_none());
    }
}
