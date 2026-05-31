//! Scroll-tick placeholder strip for rows that are visible but not yet
//! realized in the painted [`crate::FrameDisplay`].
//!
//! When inertia scrolls the viewport faster than the projection worker
//! can publish, the scroll-tick paint reuses the previous frame to stay
//! non-blocking (`paint:frame_display:scroll_anim_reuse` in
//! `worker_outcome_dispatch`). The reused frame's realized row range
//! may not cover the live viewport — under fast flick scroll it almost
//! certainly does not. Without this helper the per-line paint loop
//! simply skips the unrealized rows and the body falls back to a flat
//! background fill, which reads to the user as "scroll is stuck".
//!
//! [`compute_unrealized_strips`] returns the body-relative DIP rects
//! the painter must mark as "content loading"; [`paint_scroll_placeholder_strips`]
//! draws the soft dim fill over those rects. The painter still issues
//! its normal per-row loop; the placeholder paints *before* the loop so
//! realized rows draw over it.
//!
//! Thread ownership: UI thread (D2D handles).

use std::ops::Range;

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};

/// Vertical DIP rect of one placeholder strip. `top..bottom` are in the
/// same coordinate space as the per-row paint loop (body-relative,
/// after `body_translate` has been applied — i.e. `y = row * line_height - scroll_y`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaceholderStrip {
    /// Top edge in DIPs.
    pub top_dip: f32,
    /// Bottom edge in DIPs.
    pub bottom_dip: f32,
    /// First unrealized display-row index in this strip (inclusive).
    pub first_row: u32,
    /// One past the last unrealized display-row index in this strip.
    pub end_row: u32,
}

impl PlaceholderStrip {
    /// Number of rows covered by this strip.
    #[must_use]
    pub fn row_count(&self) -> u32 {
        self.end_row.saturating_sub(self.first_row)
    }
}

/// Walk the visible row range against the realized row range and return
/// every contiguous run of *visible* rows that the frame has not
/// realized. The result is the strips that need a placeholder fill.
///
/// `visible` is `first_visible..last_visible` in absolute display-row
/// coordinates (the renderer already iterates this range). `realized`
/// is `frame.realized_row_range()` — likewise absolute. Both are
/// half-open.
///
/// Returns up to two strips: one above `realized.start` and one below
/// `realized.end`. When the visible range is fully covered the vec is
/// empty (and the painter does nothing).
#[must_use]
pub fn compute_unrealized_strips(
    realized: Range<u32>,
    visible: Range<u32>,
    line_height_dip: f32,
    scroll_y_dip: f32,
) -> Vec<PlaceholderStrip> {
    let mut strips: Vec<PlaceholderStrip> = Vec::new();
    if visible.start >= visible.end || line_height_dip <= 0.0 {
        return strips;
    }
    let push = |strips: &mut Vec<PlaceholderStrip>, first: u32, end: u32| {
        if first >= end {
            return;
        }
        let top = first as f32 * line_height_dip - scroll_y_dip;
        let bottom = end as f32 * line_height_dip - scroll_y_dip;
        strips.push(PlaceholderStrip {
            top_dip: top,
            bottom_dip: bottom,
            first_row: first,
            end_row: end,
        });
    };
    // Above the realized window.
    if visible.start < realized.start {
        let first = visible.start;
        let end = visible.end.min(realized.start);
        push(&mut strips, first, end);
    }
    // Below the realized window.
    if visible.end > realized.end {
        let first = visible.start.max(realized.end);
        let end = visible.end;
        push(&mut strips, first, end);
    }
    strips
}

/// Sum of rows across all strips. Used by the `event:scroll_path` trace
/// payload so the analyzer can show "rows_placeholder=N".
#[must_use]
pub fn placeholder_row_count(strips: &[PlaceholderStrip]) -> u32 {
    strips.iter().map(PlaceholderStrip::row_count).sum()
}

/// Paint each placeholder strip as a flat fill spanning the body width
/// in body-relative coordinates. The renderer is responsible for
/// installing the body-level transform before calling — the strip top /
/// bottom are interpreted as `y` offsets within the body's coordinate
/// space.
///
/// `body_left_dip` / `body_right_dip` are the horizontal bounds of the
/// fill (typically `0` and the editor text column width). The fill
/// stays inside the body's clip rect because the renderer's body clip
/// is already pushed at this point.
///
/// # Safety
///
/// The Direct2D context must be inside an active `BeginDraw` bracket
/// with the body transform installed.
pub unsafe fn paint_scroll_placeholder_strips(
    ctx: &ID2D1DeviceContext,
    strips: &[PlaceholderStrip],
    body_left_dip: f32,
    body_right_dip: f32,
    brush: &ID2D1SolidColorBrush,
) {
    if body_right_dip <= body_left_dip {
        return;
    }
    for strip in strips {
        if strip.bottom_dip <= strip.top_dip {
            continue;
        }
        let rect = D2D_RECT_F {
            left: body_left_dip,
            top: strip.top_dip,
            right: body_right_dip,
            bottom: strip.bottom_dip,
        };
        ctx.FillRectangle(&rect, brush);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fully_covered_visible_range_yields_no_strips() {
        let strips = compute_unrealized_strips(0..100, 10..60, 20.0, 200.0);
        assert!(strips.is_empty());
    }

    #[test]
    fn visible_below_realized_returns_bottom_strip() {
        // Cached frame realized rows 0..40, viewport at rows 50..60.
        let strips = compute_unrealized_strips(0..40, 50..60, 20.0, 1000.0);
        assert_eq!(strips.len(), 1);
        assert_eq!(strips[0].first_row, 50);
        assert_eq!(strips[0].end_row, 60);
        assert_eq!(strips[0].row_count(), 10);
        // y = row * 20 - 1000, so 50..60 → 0..200.
        assert!((strips[0].top_dip - 0.0).abs() < f32::EPSILON);
        assert!((strips[0].bottom_dip - 200.0).abs() < f32::EPSILON);
    }

    #[test]
    fn visible_above_realized_returns_top_strip() {
        let strips = compute_unrealized_strips(100..200, 80..120, 16.0, 1280.0);
        assert_eq!(strips.len(), 1);
        assert_eq!(strips[0].first_row, 80);
        assert_eq!(strips[0].end_row, 100);
    }

    #[test]
    fn visible_disjoint_from_realized_returns_one_strip_covering_visible() {
        // Visible is entirely below realized — only bottom strip fires;
        // the "above" case would underflow into the same range so the
        // function returns one strip covering visible exactly.
        let strips = compute_unrealized_strips(0..40, 100..120, 20.0, 2000.0);
        assert_eq!(strips.len(), 1);
        assert_eq!(strips[0].first_row, 100);
        assert_eq!(strips[0].end_row, 120);
    }

    #[test]
    fn visible_overlaps_both_sides_returns_two_strips() {
        // Realized 50..60, visible 40..80 — strip above (40..50) and
        // strip below (60..80).
        let strips = compute_unrealized_strips(50..60, 40..80, 20.0, 800.0);
        assert_eq!(strips.len(), 2);
        assert_eq!(strips[0].first_row, 40);
        assert_eq!(strips[0].end_row, 50);
        assert_eq!(strips[1].first_row, 60);
        assert_eq!(strips[1].end_row, 80);
        assert_eq!(placeholder_row_count(&strips), 30);
    }

    #[test]
    fn empty_visible_range_returns_no_strips() {
        let strips = compute_unrealized_strips(0..40, 10..10, 20.0, 0.0);
        assert!(strips.is_empty());
    }

    #[test]
    fn zero_line_height_returns_no_strips() {
        let strips = compute_unrealized_strips(0..40, 100..120, 0.0, 0.0);
        assert!(strips.is_empty());
    }
}
