//! Caret-visibility row estimates for `Window` view commands.
//!
//! Thread ownership: pure helpers only. Callers read UI-thread projection
//! caches and pass immutable frame/rope data in.

use continuity_render::FrameDisplay;
use ropey::Rope;

#[derive(Clone, Copy)]
pub(super) struct CaretVisibilityEstimate {
    pub(super) display_row: u32,
    pub(super) total_display_rows: u32,
    pub(super) source: &'static str,
    pub(super) is_projection_backed: bool,
}

/// Estimate the caret's display row for the visibility reveal.
///
/// When `needs_scaled` is false the resolved `lookup` is an exact display
/// position (the caret's own row was realized, or the row index is fully
/// built) and is used directly. When true, the lookup under-counts the
/// soft-wrap rows above the caret (a partial row index placeholders
/// off-viewport lines at one row each, and the source-line floor ignores
/// wrap entirely), so scale the source line by the document's average
/// wrap factor (`total_display_rows / total_source_lines`). The result is
/// floored at the source-line index and at whatever the (under-counting)
/// lookup already proved, so it is never below the caret's true row.
#[must_use]
pub(super) fn reveal_caret_display_row(
    caret_source_line: u32,
    lookup: Option<u32>,
    total_source_lines: u32,
    estimated_total_display_rows: u32,
    needs_scaled: bool,
) -> f32 {
    if !needs_scaled {
        if let Some(row) = lookup {
            return row as f32;
        }
    }
    let total = total_source_lines.max(1);
    // Display rows are always >= source lines, so clamp the scale factor.
    let scale_total = estimated_total_display_rows.max(total) as f64;
    let scaled = caret_source_line as f64 * scale_total / total as f64;
    scaled
        .max(caret_source_line as f64)
        .max(lookup.unwrap_or(0) as f64) as f32
}

pub(super) fn estimate_from_frame(
    frame: &FrameDisplay,
    source_line: usize,
    total_source_lines: usize,
    continuation: u32,
    source: &'static str,
) -> Option<CaretVisibilityEstimate> {
    let frame_source_lines = frame.row_index().source_line_count() as usize;
    if source_line < frame_source_lines && frame_source_lines == total_source_lines {
        let source_line_rows = frame.display_line_count_for_source(source_line);
        if source_line_rows == 0 {
            return None;
        }
        let first_row = frame.first_display_line_index_for_source(source_line);
        let display_row = first_row.saturating_add(continuation);
        return Some(CaretVisibilityEstimate {
            display_row,
            total_display_rows: frame
                .display_line_count()
                .max(display_row.saturating_add(1)),
            source,
            is_projection_backed: true,
        });
    }

    if frame_source_lines.saturating_add(1) == total_source_lines
        && source_line == frame_source_lines
    {
        let display_row = frame.display_line_count();
        return Some(CaretVisibilityEstimate {
            display_row,
            total_display_rows: display_row.saturating_add(1),
            source,
            is_projection_backed: true,
        });
    }
    None
}

pub(super) fn approximate_caret_continuation_row(
    rope: &Rope,
    source_line: usize,
    byte_in_line: usize,
    wrap_width_dip: u32,
    char_width_dip: f32,
) -> u32 {
    if wrap_width_dip == 0 || char_width_dip <= 0.0 || source_line >= rope.len_lines() {
        return 0;
    }
    let line_start = rope.line_to_byte(source_line);
    let line_end = if source_line + 1 < rope.len_lines() {
        rope.line_to_byte(source_line + 1)
    } else {
        rope.len_bytes()
    };
    let abs_byte = line_start + byte_in_line.min(line_end.saturating_sub(line_start));
    let prefix_chars = rope
        .byte_to_char(abs_byte)
        .saturating_sub(rope.byte_to_char(line_start));
    let columns_per_row = ((wrap_width_dip as f32 / char_width_dip).floor() as usize).max(1);
    u32::try_from(prefix_chars / columns_per_row).unwrap_or(u32::MAX)
}

pub(super) fn is_source_floor_visibility_safe(
    estimate: &CaretVisibilityEstimate,
    view: &continuity_layout::ViewState,
    line_height: f32,
) -> bool {
    if estimate.is_projection_backed || view.scroll_y_dip > line_height {
        return false;
    }
    let viewport_bot = view.scroll_y_dip + view.viewport_height_dip;
    let line_top = estimate.display_row as f32 * line_height;
    let line_bottom = line_top + line_height;
    let margin = (line_height * 4.0).min(view.viewport_height_dip * 0.25);
    line_top >= 0.0 && line_bottom <= viewport_bot - margin
}

#[cfg(test)]
mod tests {
    use continuity_display_map::wrap::FixedCharWidth;
    use continuity_render::FrameDisplay;
    use ropey::Rope;

    use super::{
        approximate_caret_continuation_row, estimate_from_frame, reveal_caret_display_row,
    };

    #[test]
    fn reveal_uses_exact_lookup_when_not_scaled() {
        assert_eq!(
            reveal_caret_display_row(5000, Some(5123), 10156, 10234, false),
            5123.0
        );
    }

    #[test]
    fn reveal_scales_source_line_by_wrap_factor_when_unreliable() {
        let scaled = reveal_caret_display_row(10076, Some(10076), 10156, 10234, true);
        assert!(
            (10130.0..=10180.0).contains(&scaled),
            "scaled row {scaled} should approximate the true display row ~10153"
        );
    }

    #[test]
    fn reveal_scaling_grows_with_depth() {
        let shallow = reveal_caret_display_row(1000, Some(1000), 10156, 10234, true);
        let deep = reveal_caret_display_row(9000, Some(9000), 10156, 10234, true);
        assert!(deep - 9000.0 > shallow - 1000.0);
    }

    #[test]
    fn reveal_never_below_source_line_floor() {
        assert!(reveal_caret_display_row(5000, Some(4000), 10156, 0, true) >= 5000.0);
    }

    #[test]
    fn reveal_first_line_is_top() {
        assert_eq!(
            reveal_caret_display_row(0, Some(0), 10156, 10234, true),
            0.0
        );
    }

    #[test]
    fn approximate_continuation_counts_prior_wrap_rows() {
        let rope = Rope::from_str("abcdefghi\n");
        assert_eq!(approximate_caret_continuation_row(&rope, 0, 0, 30, 10.0), 0);
        assert_eq!(approximate_caret_continuation_row(&rope, 0, 4, 30, 10.0), 1);
        assert_eq!(approximate_caret_continuation_row(&rope, 0, 8, 30, 10.0), 2);
    }

    #[test]
    fn approximate_continuation_is_zero_without_wrap() {
        let rope = Rope::from_str("abcdefghi\n");
        assert_eq!(approximate_caret_continuation_row(&rope, 0, 8, 0, 10.0), 0);
    }

    #[test]
    fn projection_estimate_reuses_same_line_count_frame() {
        let rope = Rope::from_str("alpha\nbeta");
        let mut measure = FixedCharWidth::new(10.0);
        let frame = FrameDisplay::build_viewport_measured(
            &rope,
            1,
            None,
            &[0],
            &[],
            &[],
            0,
            &mut measure,
            0..10,
            0,
        );

        let estimate = estimate_from_frame(&frame, 1, 2, 0, "test")
            .expect("same source-line count should estimate from row index");

        assert_eq!(estimate.display_row, 1);
        assert_eq!(estimate.total_display_rows, 2);
        assert!(estimate.is_projection_backed);
    }

    #[test]
    fn projection_estimate_handles_appended_final_line() {
        let rope = Rope::from_str("alpha\nbeta");
        let mut measure = FixedCharWidth::new(10.0);
        let frame = FrameDisplay::build_viewport_measured(
            &rope,
            1,
            None,
            &[0],
            &[],
            &[],
            0,
            &mut measure,
            0..10,
            0,
        );

        let estimate = estimate_from_frame(&frame, 2, 3, 0, "test")
            .expect("one appended EOF line should estimate after old rows");

        assert_eq!(estimate.display_row, 2);
        assert_eq!(estimate.total_display_rows, 3);
        assert!(estimate.is_projection_backed);
    }
}
