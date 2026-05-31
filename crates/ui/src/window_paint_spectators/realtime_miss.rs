//! Current-geometry spectator miss builds.
//!
//! A spectator cache miss must not blank the pane or paint stale wrap
//! geometry while a projection-worker request is pending. This helper
//! builds the visible source region at the pane's current wrap width and
//! leaves the offscreen full fill to the worker.

use std::ops::Range;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation};
use continuity_render::FrameDisplay;

use crate::window::Window;
use crate::window_paint::VIEWPORT_OVERSCAN_ROWS;

/// Source-line range to walk for a spectator miss at current geometry.
#[must_use]
pub(crate) fn compute_spectator_viewport_source_range(
    seed_frame: Option<&FrameDisplay>,
    visible_rows: Range<u32>,
    source_line_count: usize,
) -> Range<u32> {
    if source_line_count == 0 {
        return 0..0;
    }
    let line_count = u32::try_from(source_line_count).unwrap_or(u32::MAX);
    let source_range = seed_frame
        .map(|frame| {
            frame
                .row_index()
                .source_lines_for_display_rows(visible_rows.clone())
        })
        .filter(|range| range.start < range.end)
        .map(|range| {
            let start = u32::try_from(range.start)
                .unwrap_or(u32::MAX)
                .min(line_count);
            let end = u32::try_from(range.end).unwrap_or(u32::MAX).min(line_count);
            start..end.max(start)
        })
        .unwrap_or_else(|| {
            let start = visible_rows.start.min(line_count);
            let end = visible_rows.end.min(line_count).max(start);
            start..end
        });
    if source_range.start < source_range.end {
        source_range
    } else {
        let start = source_range.start.min(line_count.saturating_sub(1));
        start..start.saturating_add(1).min(line_count)
    }
}

/// Build a partial spectator frame for the current pane geometry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_spectator_viewport_partial(
    window: &Window,
    rope: &ropey::Rope,
    revision: u64,
    decorations: Option<&Decorations>,
    caret_bytes: &[usize],
    folds: &[FoldRange],
    image_reservations: &[ImageRowReservation],
    wrap_width_dip: u32,
    fallback_char_width_dip: f32,
    visible_rows: Range<u32>,
    seed_frame: Option<&FrameDisplay>,
) -> FrameDisplay {
    let viewport_source_range =
        compute_spectator_viewport_source_range(seed_frame, visible_rows.clone(), rope.len_lines());
    window.build_frame_display_viewport_partial_with_trace(
        rope,
        revision,
        decorations,
        caret_bytes,
        folds,
        image_reservations,
        wrap_width_dip,
        fallback_char_width_dip,
        visible_rows,
        VIEWPORT_OVERSCAN_ROWS,
        viewport_source_range,
        continuity_display_map::PARTIAL_WALK_SAFETY_MARGIN,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn frame_with_counts(counts: &[u16]) -> FrameDisplay {
        let text = (0..counts.len())
            .map(|idx| format!("line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        let rope = Rope::from_str(&text);
        FrameDisplay::build(&rope, 1, None, &[0], 0, 8.0)
    }

    #[test]
    fn seed_frame_maps_visible_rows_to_source_lines() {
        let rope = Rope::from_str("aaaa bbbb cccc dddd eeee ffff\nshort\n");
        let seed = FrameDisplay::build(&rope, 1, None, &[0], 40, 8.0);
        let range = compute_spectator_viewport_source_range(Some(&seed), 1..2, rope.len_lines());

        assert_eq!(range, 0..1);
    }

    #[test]
    fn fallback_uses_display_rows_as_source_floor() {
        assert_eq!(
            compute_spectator_viewport_source_range(None, 20..40, 30),
            20..30,
        );
    }

    #[test]
    fn empty_mapped_range_keeps_one_source_line_near_viewport() {
        assert_eq!(
            compute_spectator_viewport_source_range(None, 40..40, 30),
            29..30,
        );
    }

    #[test]
    fn empty_document_stays_empty() {
        assert_eq!(
            compute_spectator_viewport_source_range(None, 0..40, 0),
            0..0,
        );
    }

    #[test]
    fn frame_count_helper_keeps_fixture_alive() {
        let frame = frame_with_counts(&[1, 1, 1]);
        assert_eq!(
            compute_spectator_viewport_source_range(Some(&frame), 0..2, 3),
            0..2,
        );
    }
}
