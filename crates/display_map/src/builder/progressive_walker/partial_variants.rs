//! P18.6 dirty/splice variants for the viewport-priority walker.

use std::ops::Range;
use std::time::Instant;

use continuity_buffer::RopeSnapshot;
use continuity_decorate::Decorations;
use continuity_text::RopeEditDelta;

use crate::error::Error;
use crate::fold::FoldRange;
use crate::id::{SourceByte, SourceLine};
use crate::image_row_reservation_provider::ImageRowReservation;
use crate::row_index::dirty::RowDirty;
use crate::row_index::DisplayRowIndex;
use crate::wrap::{WidthMeasure, WrapConfig};

use super::super::row_counts::{row_count_for_source_line, RowCountCacheContext};
use super::super::WalkerStats;
use super::{PartialWalkOutcome, UNWALKED_PLACEHOLDER_ROW_COUNT};

/// Dirty rebuild variant of the viewport-priority row-count walker.
///
/// The returned row-count vector is exact inside
/// `viewport_source_range +/- safety_margin`: dirty lines are measured
/// against the live rope, while clean lines copy exact counts from
/// `prev_row_index` when possible. Lines outside the walked range keep
/// placeholders until the background fill completes.
#[allow(clippy::too_many_arguments, clippy::needless_option_as_deref)]
pub(in crate::builder) fn compute_partial_dirty_row_counts_for_viewport_range(
    snapshot: &RopeSnapshot,
    decorations: &Decorations,
    caret_bytes: &[SourceByte],
    folds: &[FoldRange],
    image_reservations: &[ImageRowReservation],
    suppressed_table_blocks: &[std::ops::Range<usize>],
    wrap: WrapConfig,
    measure: &mut dyn WidthMeasure,
    cache_context: Option<RowCountCacheContext<'_>>,
    viewport_source_range: Range<u32>,
    safety_margin: u32,
    dirty_source_ranges: &[Range<u32>],
    prev_row_index: &DisplayRowIndex,
    mut stats: Option<&mut WalkerStats>,
) -> Result<(Vec<u16>, PartialWalkOutcome), Error> {
    let rope = snapshot.rope();
    let total_source_lines = rope.len_lines() as u32;
    let mut row_counts: Vec<u16> =
        vec![UNWALKED_PLACEHOLDER_ROW_COUNT; total_source_lines as usize];
    let walk_range = expanded_walk_range(total_source_lines, viewport_source_range, safety_margin);
    let mut reservation_cursor = image_reservation_cursor_for(image_reservations, walk_range.start);
    let t_walk = Instant::now();
    let mut walked_display_rows = 0u32;

    for source_line_idx in walk_range.clone() {
        advance_reservation_cursor(image_reservations, &mut reservation_cursor, source_line_idx);
        let copied = if is_line_in_ranges(source_line_idx, dirty_source_ranges) {
            None
        } else {
            copy_previous_row_count(prev_row_index, source_line_idx)
        };
        let count = match copied {
            Some(count) => count,
            None => measure_source_line(
                rope,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                suppressed_table_blocks,
                wrap,
                measure,
                cache_context,
                source_line_idx,
                reservation_cursor,
                &mut stats,
            )?,
        };
        row_counts[source_line_idx as usize] = count;
        walked_display_rows = walked_display_rows.saturating_add(u32::from(count));
        consume_reservation_if_present(
            image_reservations,
            &mut reservation_cursor,
            source_line_idx,
        );
    }

    Ok((
        row_counts,
        outcome_for_walk(
            total_source_lines,
            walk_range,
            walked_display_rows,
            t_walk.elapsed().as_micros() as u64,
        ),
    ))
}

/// Splice rebuild variant of the viewport-priority row-count walker.
///
/// The previous index is mapped forward only for the walked viewport
/// range. Clean lines copy exact previous counts when the source-line
/// mapping is sound; dirty, inserted, or placeholder-backed lines are
/// measured from the live rope.
#[allow(clippy::too_many_arguments, clippy::needless_option_as_deref)]
pub(in crate::builder) fn compute_partial_splice_row_counts_for_viewport_range(
    snapshot: &RopeSnapshot,
    decorations: &Decorations,
    caret_bytes: &[SourceByte],
    folds: &[FoldRange],
    image_reservations: &[ImageRowReservation],
    suppressed_table_blocks: &[std::ops::Range<usize>],
    wrap: WrapConfig,
    measure: &mut dyn WidthMeasure,
    cache_context: Option<RowCountCacheContext<'_>>,
    viewport_source_range: Range<u32>,
    safety_margin: u32,
    deltas: &[RopeEditDelta],
    prev_row_index: &DisplayRowIndex,
    mut stats: Option<&mut WalkerStats>,
) -> Result<(Vec<u16>, PartialWalkOutcome), Error> {
    let rope = snapshot.rope();
    let total_source_lines = rope.len_lines() as u32;
    let dirty = prev_row_index.dirty_after_rope_edits(deltas, rope);
    let mut row_counts: Vec<u16> =
        vec![UNWALKED_PLACEHOLDER_ROW_COUNT; total_source_lines as usize];
    let walk_range = expanded_walk_range(total_source_lines, viewport_source_range, safety_margin);
    let mut reservation_cursor = image_reservation_cursor_for(image_reservations, walk_range.start);
    let t_walk = Instant::now();
    let mut walked_display_rows = 0u32;

    for source_line_idx in walk_range.clone() {
        advance_reservation_cursor(image_reservations, &mut reservation_cursor, source_line_idx);
        let copied = previous_line_for_clean_splice(source_line_idx, &dirty)
            .and_then(|previous_line| copy_previous_row_count(prev_row_index, previous_line));
        let count = match copied {
            Some(count) => count,
            None => measure_source_line(
                rope,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                suppressed_table_blocks,
                wrap,
                measure,
                cache_context,
                source_line_idx,
                reservation_cursor,
                &mut stats,
            )?,
        };
        row_counts[source_line_idx as usize] = count;
        walked_display_rows = walked_display_rows.saturating_add(u32::from(count));
        consume_reservation_if_present(
            image_reservations,
            &mut reservation_cursor,
            source_line_idx,
        );
    }

    Ok((
        row_counts,
        outcome_for_walk(
            total_source_lines,
            walk_range,
            walked_display_rows,
            t_walk.elapsed().as_micros() as u64,
        ),
    ))
}

fn expanded_walk_range(
    total_source_lines: u32,
    viewport_source_range: Range<u32>,
    safety_margin: u32,
) -> Range<u32> {
    let walk_start = viewport_source_range
        .start
        .saturating_sub(safety_margin)
        .min(total_source_lines);
    let walk_end = viewport_source_range
        .end
        .saturating_add(safety_margin)
        .min(total_source_lines)
        .max(walk_start);
    walk_start..walk_end
}

fn outcome_for_walk(
    total_source_lines: u32,
    walked_source_range: Range<u32>,
    walked_display_rows: u32,
    partial_walk_us: u64,
) -> PartialWalkOutcome {
    let walked_lines = walked_source_range
        .end
        .saturating_sub(walked_source_range.start);
    let estimated_total_rows = if walked_lines == 0 {
        total_source_lines
    } else {
        let avg = f64::from(walked_display_rows) / f64::from(walked_lines);
        let unwalked = total_source_lines.saturating_sub(walked_lines);
        let est = f64::from(walked_display_rows) + avg * f64::from(unwalked);
        est.round().clamp(0.0, f64::from(u32::MAX)) as u32
    };
    PartialWalkOutcome {
        walked_source_range,
        walked_display_rows,
        estimated_total_rows,
        partial_walk_us,
    }
}

fn is_line_in_ranges(source_line: u32, ranges: &[Range<u32>]) -> bool {
    ranges
        .iter()
        .any(|range| range.start <= source_line && source_line < range.end)
}

fn previous_line_for_clean_splice(source_line: u32, dirty: &RowDirty) -> Option<u32> {
    match dirty {
        RowDirty::Lines(lines) => {
            if lines.binary_search(&source_line).is_ok() {
                None
            } else {
                Some(source_line)
            }
        }
        RowDirty::Splice(splice) => {
            if splice.dirty.binary_search(&source_line).is_ok() {
                return None;
            }
            if source_line < splice.at {
                Some(source_line)
            } else {
                let inserted_end = splice.at.saturating_add(splice.inserted);
                if source_line >= inserted_end {
                    Some(
                        source_line
                            .saturating_sub(splice.inserted)
                            .saturating_add(splice.removed),
                    )
                } else {
                    None
                }
            }
        }
        RowDirty::FullRebuild => None,
    }
}

fn copy_previous_row_count(prev_row_index: &DisplayRowIndex, source_line: u32) -> Option<u16> {
    if source_line >= prev_row_index.source_line_count() {
        return None;
    }
    if let Some(partial) = prev_row_index.partial_state() {
        if !partial.walked_source_range.contains(&source_line) {
            return None;
        }
    }
    Some(
        prev_row_index
            .display_row_count_for_source(SourceLine(source_line))
            .min(u32::from(u16::MAX)) as u16,
    )
}

#[allow(clippy::too_many_arguments, clippy::needless_option_as_deref)]
fn measure_source_line(
    rope: &ropey::Rope,
    decorations: &Decorations,
    caret_bytes: &[SourceByte],
    folds: &[FoldRange],
    image_reservations: &[ImageRowReservation],
    suppressed_table_blocks: &[std::ops::Range<usize>],
    wrap: WrapConfig,
    measure: &mut dyn WidthMeasure,
    cache_context: Option<RowCountCacheContext<'_>>,
    source_line_idx: u32,
    reservation_cursor: usize,
    stats: &mut Option<&mut WalkerStats>,
) -> Result<u16, Error> {
    if let Some(stats) = stats.as_deref_mut() {
        stats.lines_total = stats.lines_total.saturating_add(1);
    }
    row_count_for_source_line(
        rope,
        decorations,
        caret_bytes,
        folds,
        image_reservations,
        suppressed_table_blocks,
        wrap,
        measure,
        cache_context,
        source_line_idx,
        reservation_cursor,
        stats.as_deref_mut(),
    )
}

fn image_reservation_cursor_for(
    image_reservations: &[ImageRowReservation],
    source_line: u32,
) -> usize {
    while_cursor_for(image_reservations, source_line, 0)
}

fn advance_reservation_cursor(
    image_reservations: &[ImageRowReservation],
    reservation_cursor: &mut usize,
    source_line: u32,
) {
    *reservation_cursor = while_cursor_for(image_reservations, source_line, *reservation_cursor);
}

fn while_cursor_for(
    image_reservations: &[ImageRowReservation],
    source_line: u32,
    mut reservation_cursor: usize,
) -> usize {
    while reservation_cursor < image_reservations.len()
        && image_reservations[reservation_cursor].source_line.raw() < source_line
    {
        reservation_cursor += 1;
    }
    reservation_cursor
}

fn consume_reservation_if_present(
    image_reservations: &[ImageRowReservation],
    reservation_cursor: &mut usize,
    source_line: u32,
) {
    if image_reservations
        .get(*reservation_cursor)
        .is_some_and(|reservation| reservation.source_line.raw() == source_line)
    {
        *reservation_cursor += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use continuity_buffer::Revision;
    use proptest::prelude::*;
    use ropey::Rope;

    use crate::wrap::FixedCharWidth;
    use crate::{DisplayMapBuilder, WalkerStats};

    use super::*;

    fn snapshot(text: &str, revision: u64) -> RopeSnapshot {
        RopeSnapshot::new(Arc::new(Rope::from_str(text)), Revision(revision))
    }

    fn wrap(width: u32) -> WrapConfig {
        if width == 0 {
            WrapConfig::NONE
        } else {
            WrapConfig::new(width)
        }
    }

    fn cold_row_counts(text: &str, revision: u64, wrap_width_dip: u32) -> Vec<u16> {
        let snap = snapshot(text, revision);
        let decorations = Decorations::empty(revision);
        let mut measure = FixedCharWidth::new(8.0);
        DisplayMapBuilder::new(&snap, &decorations, &[], &[], wrap(wrap_width_dip))
            .compute_row_index_with_stats(&mut measure, None)
            .expect("cold row index")
            .row_counts()
            .to_vec()
    }

    fn merge_with_cold_outside_walked(
        mut partial: Vec<u16>,
        cold: &[u16],
        walked: Range<u32>,
    ) -> Vec<u16> {
        for (idx, count) in cold.iter().enumerate() {
            if (idx as u32) < walked.start || (idx as u32) >= walked.end {
                partial[idx] = *count;
            }
        }
        partial
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn partial_dirty_plus_fill_matches_cold_rebuild(
            lines in proptest::collection::vec("[a-z αβγ]{4,40}", 1..32usize),
            viewport_start in 0usize..16,
            viewport_len in 1usize..16,
            dirty_start in 0usize..16,
            dirty_len in 1usize..16,
            wrap_width in proptest::sample::select(vec![0u32, 48, 96]),
        ) {
            let text = lines.join("\n");
            let snap = snapshot(&text, 2);
            let decorations = Decorations::empty(2);
            let prev_counts = cold_row_counts(&text, 1, wrap_width);
            let prev_index = crate::DisplayRowIndex::from_row_counts(
                prev_counts,
                crate::IndexStamps::default(),
            );
            let cold = cold_row_counts(&text, 2, wrap_width);
            let total_lines = cold.len() as u32;
            let viewport_start = (viewport_start as u32).min(total_lines);
            let viewport = viewport_start..viewport_start.saturating_add(viewport_len as u32).min(total_lines);
            let dirty_start = (dirty_start as u32).min(total_lines);
            let dirty = dirty_start..dirty_start.saturating_add(dirty_len as u32).min(total_lines);
            let mut measure = FixedCharWidth::new(8.0);
            let (partial, outcome) = compute_partial_dirty_row_counts_for_viewport_range(
                &snap,
                &decorations,
                &[],
                &[],
                &[],
                &[],
                wrap(wrap_width),
                &mut measure,
                None,
                viewport,
                4,
                &[dirty],
                &prev_index,
                None,
            ).expect("partial dirty");

            for line in outcome.walked_source_range.clone() {
                prop_assert_eq!(partial[line as usize], cold[line as usize]);
            }
            prop_assert_eq!(
                merge_with_cold_outside_walked(partial, &cold, outcome.walked_source_range),
                cold,
            );
        }

        #[test]
        fn partial_splice_plus_fill_matches_cold_rebuild(
            lines in proptest::collection::vec("[a-z]{1,30}", 1..24usize),
            target_line in 0usize..24,
            wrap_width in proptest::sample::select(vec![0u32, 64, 128]),
        ) {
            let prev = lines.join("\n");
            let line_idx = target_line % lines.len();
            let line_start: usize = prev
                .split('\n')
                .take(line_idx)
                .map(|line| line.len() + 1)
                .sum();
            let split_at = line_start + lines[line_idx].len() / 2;
            let mut next = String::with_capacity(prev.len() + 1);
            next.push_str(&prev[..split_at]);
            next.push('\n');
            next.push_str(&prev[split_at..]);
            let deltas = [RopeEditDelta::insert(split_at, 1)];
            let snap = snapshot(&next, 2);
            let decorations = Decorations::empty(2);
            let prev_counts = cold_row_counts(&prev, 1, wrap_width);
            let prev_index = crate::DisplayRowIndex::from_row_counts(
                prev_counts,
                crate::IndexStamps::default(),
            );
            let cold = cold_row_counts(&next, 2, wrap_width);
            let mut measure = FixedCharWidth::new(8.0);
            let (partial, outcome) = compute_partial_splice_row_counts_for_viewport_range(
                &snap,
                &decorations,
                &[],
                &[],
                &[],
                &[],
                wrap(wrap_width),
                &mut measure,
                None,
                0..(cold.len() as u32).min(8),
                4,
                &deltas,
                &prev_index,
                None,
            ).expect("partial splice");

            for line in outcome.walked_source_range.clone() {
                prop_assert_eq!(partial[line as usize], cold[line as usize]);
            }
            prop_assert_eq!(
                merge_with_cold_outside_walked(partial, &cold, outcome.walked_source_range),
                cold,
            );
        }
    }

    #[test]
    fn partial_dirty_large_range_walks_viewport_not_half_buffer() {
        let text = (0..9000)
            .map(|idx| format!("line {idx} {}", "x".repeat(12)))
            .collect::<Vec<_>>()
            .join("\n");
        let snap = snapshot(&text, 2);
        let decorations = Decorations::empty(2);
        let prev_counts = cold_row_counts(&text, 1, 80);
        let prev_index =
            crate::DisplayRowIndex::from_row_counts(prev_counts, crate::IndexStamps::default());
        let mut measure = FixedCharWidth::new(8.0);
        let mut stats = WalkerStats::default();

        let (_, outcome) = compute_partial_dirty_row_counts_for_viewport_range(
            &snap,
            &decorations,
            &[],
            &[],
            &[],
            &[],
            WrapConfig::new(80),
            &mut measure,
            None,
            200..230,
            8,
            #[allow(clippy::single_range_in_vec_init)]
            &[0..4500],
            &prev_index,
            Some(&mut stats),
        )
        .expect("partial dirty");

        assert_eq!(outcome.walked_source_range, 192..238);
        assert_eq!(stats.lines_total, 46);
        assert!(stats.lines_total < 100, "must not walk half the buffer");
    }
}
