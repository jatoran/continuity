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

pub(super) fn estimate_from_frame(
    frame: &FrameDisplay,
    source_line: usize,
    total_source_lines: usize,
    continuation: u32,
    source: &'static str,
) -> Option<CaretVisibilityEstimate> {
    // A partial (viewport-priority) row index holds placeholder counts for
    // source lines outside its walked range, so `first_display_line_index_
    // for_source` under-counts the caret's absolute display row whenever a
    // wrapped line above the caret has not been measured yet. Returning a
    // confident, projection-backed estimate from that under-count makes the
    // reveal think an on-screen caret is above the viewport and scroll up —
    // then the background fill corrects the count and it snaps back. That is
    // the "viewport jumps while typing in a long file" bug. Refuse to
    // estimate from a partial index; the caller falls through to the
    // measured path (`try_resolve_caret_display_row_exact`), which patches
    // the index prefix and scrolls only when the caret is genuinely off
    // screen.
    if frame.row_index().is_partial() {
        return None;
    }
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

    use super::{approximate_caret_continuation_row, estimate_from_frame};

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
    fn partial_index_frame_refuses_to_estimate() {
        use std::sync::Arc;

        use continuity_display_map::{
            DisplayMap, DisplayRowIndex, IndexStamps, PartialRowIndexState,
        };

        // 5 source lines; only line 2 was walked at viewport-priority time.
        // Outside the walked range `row_counts` holds placeholder 1s, so the
        // prefix sum to the caret line under-counts whenever a line above it
        // actually soft-wraps. A partial index must NOT yield a confident
        // (scroll-driving) estimate — the caller falls through to the
        // measured path instead (the fix for the "viewport jumps while
        // typing in a long file" bug).
        let stamps = IndexStamps {
            rope_revision: 1,
            decoration_revision: 1,
            wrap_width_dip: 100,
            font_state: 0,
            fold_signature: 0,
        };
        let partial = PartialRowIndexState {
            walked_source_range: 2..3,
            scrollbar_estimate: 5,
            full_revision_target: 1,
        };
        let index = DisplayRowIndex::from_partial_row_counts(vec![1, 1, 1, 1, 1], stamps, partial);
        assert!(index.is_partial());
        let map = DisplayMap::from_parts_viewport(1, 100, Arc::new(index), Vec::new(), 0);
        let frame = FrameDisplay::from_display_map(Arc::new(map));

        assert!(
            estimate_from_frame(&frame, 3, 5, 0, "partial").is_none(),
            "a partial row index must not produce a confident caret-visibility estimate",
        );
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
