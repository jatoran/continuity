//! P18.5 — viewport-priority progressive row-count walker.
//!
//! Walks only a caller-supplied source-line range to produce a partial
//! row-index that paint can block on. The remaining source lines get a
//! placeholder count of 1 each; the resulting [`PartialWalkOutcome`]
//! carries an estimated total display-row count derived from the walked
//! sample's density.
//!
//! ## Why it exists
//!
//! The cheap row-count walker that backs cold paint
//! ([`super::row_counts::compute_row_counts`]) is `O(source_line_count)`.
//! On a 9 k-line markdown buffer with ~3500 complex-script lines, that
//! is ~1 s of shaping work — paid synchronously by paint on the first
//! focus into a never-walked buffer. Capacity tuning (P18.4) makes the
//! revisit cheap, but the first walk is still bounded by how many lines
//! we have to shape.
//!
//! P18.5's architectural lever is to change *what paint blocks on*.
//! The viewport only needs row counts for ~50 source lines; the rest can
//! be filled in by a background worker within ~1 s of the first paint
//! without the user feeling it. This module is the substrate.
//!
//! ## Contract
//!
//! [`compute_partial_row_counts_for_viewport_range`] returns:
//!
//! 1. A `Vec<u16>` of `total_source_lines` row counts. Inside the walked
//!    range these are the real counts produced by
//!    [`super::row_counts::row_count_for_source_line`]; outside, they
//!    are [`UNWALKED_PLACEHOLDER_ROW_COUNT`] (one row per source line, the
//!    plain-text estimate).
//! 2. A [`PartialWalkOutcome`] describing what was walked plus a
//!    density-based scrollbar estimate.
//!
//! The placeholder choice is load-bearing for the existing
//! [`crate::DisplayRowIndex`] read paths: with `1` per unwalked line,
//! `source_lines_for_display_rows` still returns sensible source-line
//! ranges for queries inside the walked viewport, and the Fenwick total
//! remains a coarse upper bound. The density-based scrollbar estimate
//! lives separately on [`crate::row_index::PartialRowIndexState`] so
//! scrollbar consumers can opt in via
//! [`crate::DisplayRowIndex::estimated_total_rows`].
//!
//! ## Thread ownership
//!
//! Same as [`super::row_counts::compute_row_counts`] — runs on the
//! worker that owns the parent [`crate::DisplayMapBuilder`].

use std::ops::Range;
use std::time::Instant;

use continuity_buffer::RopeSnapshot;
use continuity_decorate::Decorations;

use crate::error::Error;
use crate::fold::FoldRange;
use crate::id::SourceByte;
use crate::image_row_reservation_provider::ImageRowReservation;
use crate::markdown_toggles::MarkdownRenderToggles;
use crate::wrap::{WidthMeasure, WrapConfig};

use super::row_counts::{row_count_for_source_line, RowCountCacheContext};
use super::WalkerStats;

mod partial_variants;
pub(super) use partial_variants::{
    compute_partial_dirty_row_counts_for_viewport_range,
    compute_partial_splice_row_counts_for_viewport_range,
};

/// Placeholder row count assigned to source lines outside the walked
/// viewport range. One row per source line is the plain-text estimate;
/// soft-wrapped lines undercount until the background fill replaces the
/// placeholder with the real count. The density-based scrollbar
/// estimate on [`PartialWalkOutcome`] compensates for the undercount in
/// the scrollbar geometry.
pub const UNWALKED_PLACEHOLDER_ROW_COUNT: u16 = 1;

/// Default safety margin on each side of the requested viewport range.
/// Roughly one viewport's worth of overscan keeps caret motion and small
/// scrolls within the walked range without triggering a second partial
/// walk before the background fill completes.
pub const PARTIAL_WALK_SAFETY_MARGIN: u32 = 64;

/// Outcome of a viewport-priority partial walk.
///
/// Paired with the `Vec<u16>` row counts to construct a
/// [`crate::row_index::PartialRowIndexState`] on the resulting
/// [`crate::DisplayRowIndex`].
#[derive(Clone, Debug, PartialEq)]
pub struct PartialWalkOutcome {
    /// Source-line range actually walked, after safety-margin expansion
    /// and clamping to the document. Half-open.
    pub walked_source_range: Range<u32>,
    /// Sum of real display rows over the walked range. Excludes
    /// placeholder rows.
    pub walked_display_rows: u32,
    /// Density-based estimate of the document's total display-row count.
    /// Equal to the walked rows plus `unwalked_lines × avg_walked_density`,
    /// rounded. Falls back to the document's source-line count when the
    /// walked range is empty.
    pub estimated_total_rows: u32,
    /// Wall-clock microseconds the partial walk took, excluding caller
    /// overhead. Forms the `partial_us=N` field of the
    /// `event:partial_row_index_walk` trace event.
    pub partial_walk_us: u64,
}

/// Walk only the source lines in `viewport_source_range` (expanded by
/// `safety_margin` on each side) and return per-source-line row counts
/// plus a [`PartialWalkOutcome`] summary.
///
/// Behavioural contract:
///
/// * Lines inside the walked range get their real row counts from
///   [`super::row_counts::row_count_for_source_line`] — byte-identical to
///   what a cold full walker would emit at the same inputs (see
///   `progressive_partial_matches_cold_full` proptest).
/// * Lines outside the walked range hold
///   [`UNWALKED_PLACEHOLDER_ROW_COUNT`] (`1`). This keeps the resulting
///   Fenwick prefix-sum tree well-formed; the *real* total is unknown
///   until the background fill completes.
/// * The walker populates the same `SegmentCache`/`WrapCache` slots a
///   full walk would, so the eventual background fill is just the rest
///   of the same walk — not a separate code path.
///
/// # Errors
///
/// Same as [`super::row_counts::compute_row_counts`]:
/// [`Error::BadMeasurement`] on a non-finite measurer reply, plus any
/// error the per-line helper surfaces.
#[allow(clippy::too_many_arguments, clippy::needless_option_as_deref)]
pub(super) fn compute_partial_row_counts_for_viewport_range(
    snapshot: &RopeSnapshot,
    decorations: &Decorations,
    caret_bytes: &[SourceByte],
    folds: &[FoldRange],
    image_reservations: &[ImageRowReservation],
    suppressed_table_blocks: &[std::ops::Range<usize>],
    markdown_toggles: MarkdownRenderToggles,
    wrap: WrapConfig,
    measure: &mut dyn WidthMeasure,
    cache_context: Option<RowCountCacheContext<'_>>,
    viewport_source_range: Range<u32>,
    safety_margin: u32,
    mut stats: Option<&mut WalkerStats>,
) -> Result<(Vec<u16>, PartialWalkOutcome), Error> {
    let rope = snapshot.rope();
    let total_source_lines = rope.len_lines() as u32;
    let mut row_counts: Vec<u16> =
        vec![UNWALKED_PLACEHOLDER_ROW_COUNT; total_source_lines as usize];

    // Apply safety margin and clamp to document bounds.
    let walk_start = viewport_source_range
        .start
        .saturating_sub(safety_margin)
        .min(total_source_lines);
    let walk_end = viewport_source_range
        .end
        .saturating_add(safety_margin)
        .min(total_source_lines)
        .max(walk_start);

    // Wind the reservation cursor past lines preceding the walk window
    // so the per-line lookup stays amortised-O(1).
    let mut reservation_cursor: usize = 0;
    while reservation_cursor < image_reservations.len()
        && image_reservations[reservation_cursor].source_line.raw() < walk_start
    {
        reservation_cursor += 1;
    }

    let t_walk = Instant::now();
    let mut walked_display_rows: u32 = 0;
    for source_line_idx in walk_start..walk_end {
        if let Some(stats) = stats.as_deref_mut() {
            stats.lines_total = stats.lines_total.saturating_add(1);
        }
        while reservation_cursor < image_reservations.len()
            && image_reservations[reservation_cursor].source_line.raw() < source_line_idx
        {
            reservation_cursor += 1;
        }
        let count = row_count_for_source_line(
            rope,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            suppressed_table_blocks,
            markdown_toggles,
            wrap,
            measure,
            cache_context,
            source_line_idx,
            reservation_cursor,
            stats.as_deref_mut(),
        )?;
        row_counts[source_line_idx as usize] = count;
        walked_display_rows = walked_display_rows.saturating_add(u32::from(count));
        if image_reservations
            .get(reservation_cursor)
            .is_some_and(|r| r.source_line.raw() == source_line_idx)
        {
            reservation_cursor += 1;
        }
    }
    let partial_walk_us = t_walk.elapsed().as_micros() as u64;

    let walked_lines = walk_end.saturating_sub(walk_start);
    let estimated_total_rows = if walked_lines == 0 {
        // Empty walk: fall back to a one-row-per-line estimate so the
        // scrollbar still has a non-zero content height.
        total_source_lines
    } else {
        let avg = f64::from(walked_display_rows) / f64::from(walked_lines);
        let unwalked = total_source_lines.saturating_sub(walked_lines);
        let est = f64::from(walked_display_rows) + avg * f64::from(unwalked);
        let clamped = est.round().clamp(0.0, f64::from(u32::MAX));
        clamped as u32
    };

    Ok((
        row_counts,
        PartialWalkOutcome {
            walked_source_range: walk_start..walk_end,
            walked_display_rows,
            estimated_total_rows,
            partial_walk_us,
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use continuity_buffer::{Revision, RopeSnapshot};
    use continuity_decorate::Decorations;
    use proptest::prelude::*;
    use ropey::Rope;

    use crate::row_index::{DisplayRowIndex, IndexStamps};
    use crate::wrap::FixedCharWidth;
    use crate::{DisplayMapBuilder, SegmentCache, SourceLine, WalkerStats, WrapCache, WrapConfig};

    // Equivalence proptest: the partial walk over a chosen viewport
    // range produces row counts byte-identical to the cold full walk
    // *for the walked range*, and the merge of the partial counts with
    // the cold counts (outside the walked range) is byte-identical to
    // the cold full counts everywhere. This is the load-bearing claim
    // of P18.5: partial + fill = cold full walk.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn progressive_partial_matches_cold_full(
            lines in proptest::collection::vec("[αβγδεζηθ a-z]{8,48}", 1..32),
            wrap_dip in proptest::sample::select(vec![0_u32, 24, 40, 64]),
            viewport_start in 0_usize..16,
            viewport_len in 1_usize..16,
            safety_margin in 0_u32..16,
        ) {
            let text = lines.join("\n");
            let total_lines = lines.len() as u32 + 1; // ropey adds the synthetic empty line
            let snapshot = RopeSnapshot::new(Arc::new(Rope::from_str(&text)), Revision(1));
            let decorations = Decorations::empty(1);
            let mut measure = FixedCharWidth::new(8.0);
            let wrap = if wrap_dip == 0 {
                WrapConfig::NONE
            } else {
                WrapConfig::new(wrap_dip.max(1))
            };

            // Cold full walk — reference.
            let cold_index = DisplayMapBuilder::new(&snapshot, &decorations, &[], &[], wrap)
                .compute_row_index_with_stats(&mut measure, None)
                .expect("cold row index");
            let cold_counts: Vec<u16> = cold_index.row_counts().to_vec();

            // Partial walk over a viewport range.
            let viewport_start_u32 = (viewport_start as u32).min(total_lines);
            let viewport_end_u32 = viewport_start_u32
                .saturating_add(viewport_len as u32)
                .min(total_lines);
            let viewport_range = viewport_start_u32..viewport_end_u32;
            let partial_index = DisplayMapBuilder::new(&snapshot, &decorations, &[], &[], wrap)
                .compute_partial_row_index_for_viewport_with_stats(
                    viewport_range.clone(),
                    safety_margin,
                    &mut measure,
                    None,
                )
                .expect("partial row index");
            let partial_counts: Vec<u16> = partial_index.row_counts().to_vec();

            // Walked range counts must match cold exactly.
            let walked = partial_index
                .partial_state()
                .expect("partial state must be set")
                .walked_source_range
                .clone();
            for i in walked.clone() {
                let i_usize = i as usize;
                prop_assert_eq!(
                    partial_counts[i_usize],
                    cold_counts[i_usize],
                    "walked-range row count mismatch at line {}",
                    i_usize,
                );
            }

            // Merge: replace placeholders outside the walked range with
            // cold values; the result must equal cold exactly.
            let mut merged = partial_counts.clone();
            for (i, count) in cold_counts.iter().enumerate() {
                if (i as u32) < walked.start || (i as u32) >= walked.end {
                    merged[i] = *count;
                }
            }
            prop_assert_eq!(merged, cold_counts);
        }
    }

    /// Viewport-only walk on a 9 k-line buffer with a 30-row viewport
    /// touches ≤ 60 source lines + 2 × safety_margin, not 9000.
    /// (Exit criterion: first paint of a never-walked 9 k-line buffer
    /// must not pay full-document shaping cost.)
    #[test]
    fn viewport_walk_touches_only_viewport_plus_margin() {
        let lines: Vec<String> = (0..9000).map(|i| format!("line {i}")).collect();
        let text = lines.join("\n");
        let snapshot = RopeSnapshot::new(Arc::new(Rope::from_str(&text)), Revision(1));
        let decorations = Decorations::empty(1);
        let mut measure = FixedCharWidth::new(8.0);
        let wrap = WrapConfig::new(80);
        let safety_margin = 8;
        let viewport_range = 100u32..130u32;

        let mut stats = WalkerStats::default();
        let index = DisplayMapBuilder::new(&snapshot, &decorations, &[], &[], wrap)
            .compute_partial_row_index_for_viewport_with_stats(
                viewport_range.clone(),
                safety_margin,
                &mut measure,
                Some(&mut stats),
            )
            .expect("partial row index");

        // Walked lines = viewport_len + 2 × safety_margin (clamped).
        let expected_walked = (viewport_range.end - viewport_range.start) + 2 * safety_margin;
        assert_eq!(stats.lines_total, expected_walked);
        assert!(stats.lines_total < 100, "must not touch full document");

        // Index reports the partial range correctly.
        let state = index.partial_state().expect("partial state");
        assert_eq!(state.walked_source_range.start, 100 - safety_margin);
        assert_eq!(state.walked_source_range.end, 130 + safety_margin);
    }

    /// Scrollbar estimate must land within ±20 % of actual on a uniform
    /// document, and snap exact once the row index is rebuilt with all
    /// counts (background-fill completion).
    #[test]
    fn scrollbar_estimate_within_tolerance_then_snaps_exact() {
        // Build a 1000-line buffer with predictable wrapping.
        let lines: Vec<String> = (0..1000)
            .map(|_| "the quick brown fox jumps over the lazy dog".to_string())
            .collect();
        let text = lines.join("\n");
        let snapshot = RopeSnapshot::new(Arc::new(Rope::from_str(&text)), Revision(1));
        let decorations = Decorations::empty(1);
        let mut measure = FixedCharWidth::new(8.0);
        let wrap = WrapConfig::new(120);

        // Cold full walk: the truth.
        let cold_index = DisplayMapBuilder::new(&snapshot, &decorations, &[], &[], wrap)
            .compute_row_index_with_stats(&mut measure, None)
            .expect("cold row index");
        let actual_total = cold_index.display_row_count();

        // Partial walk over a 30-line viewport.
        let partial_index = DisplayMapBuilder::new(&snapshot, &decorations, &[], &[], wrap)
            .compute_partial_row_index_for_viewport_with_stats(
                500u32..530u32,
                8,
                &mut measure,
                None,
            )
            .expect("partial row index");
        let estimate = partial_index
            .partial_state()
            .expect("partial state")
            .scrollbar_estimate;

        // Within ±20 %.
        let lo = actual_total.saturating_mul(80) / 100;
        let hi = actual_total.saturating_mul(120) / 100;
        assert!(
            estimate >= lo && estimate <= hi,
            "estimate {estimate} not within 20% of actual {actual_total}",
        );

        // A fully-walked index (no partial state) reports the exact total.
        let exact_index = DisplayRowIndex::from_row_counts(
            cold_index.row_counts().to_vec(),
            IndexStamps::default(),
        );
        assert_eq!(exact_index.display_row_count(), actual_total);
        // `estimated_total_rows` on a non-partial index equals the Fenwick total.
        assert_eq!(exact_index.estimated_total_rows(), actual_total);
    }

    /// First display row of every walked source line matches between
    /// the partial index (with placeholders elsewhere) and the cold
    /// full index, so paint can realize the viewport against either.
    #[test]
    fn walked_range_first_display_rows_match_cold() {
        let text = (0..200)
            .map(|i| format!("line {i} {}", "α".repeat(20)))
            .collect::<Vec<_>>()
            .join("\n");
        let snapshot = RopeSnapshot::new(Arc::new(Rope::from_str(&text)), Revision(1));
        let decorations = Decorations::empty(1);
        let mut measure = FixedCharWidth::new(8.0);
        let wrap = WrapConfig::new(64);

        let cold = DisplayMapBuilder::new(&snapshot, &decorations, &[], &[], wrap)
            .compute_row_index_with_stats(&mut measure, None)
            .expect("cold");
        let partial = DisplayMapBuilder::new(&snapshot, &decorations, &[], &[], wrap)
            .compute_partial_row_index_for_viewport_with_stats(50u32..80u32, 4, &mut measure, None)
            .expect("partial");

        let walked = partial
            .partial_state()
            .expect("partial state")
            .walked_source_range
            .clone();
        // Inside the walked range, per-source row counts agree.
        for line in walked.clone() {
            assert_eq!(
                partial.display_row_count_for_source(SourceLine(line)),
                cold.display_row_count_for_source(SourceLine(line)),
                "row count disagreement at walked source line {line}",
            );
        }

        // The shared SegmentCache that backs the cold walker is also
        // populated by the partial walker (cache integration contract).
        let segment_cache = SegmentCache::new(256);
        let wrap_cache = WrapCache::new(256);
        let mut warm_stats = WalkerStats::default();
        let _ = DisplayMapBuilder::new(&snapshot, &decorations, &[], &[], wrap)
            .with_row_count_caches(99, "en-us", &wrap_cache, &segment_cache)
            .compute_partial_row_index_for_viewport_with_stats(
                50u32..80u32,
                4,
                &mut measure,
                Some(&mut warm_stats),
            )
            .expect("partial warm");
        let mut exact_revisit_stats = WalkerStats::default();
        let _ = DisplayMapBuilder::new(&snapshot, &decorations, &[], &[], wrap)
            .with_row_count_caches(99, "en-us", &wrap_cache, &segment_cache)
            .compute_partial_row_index_for_viewport_with_stats(
                50u32..80u32,
                4,
                &mut measure,
                Some(&mut exact_revisit_stats),
            )
            .expect("partial exact revisit");
        // Same walked range second pass should now hit exact wrap rows
        // before segment construction or measurement.
        assert!(
            exact_revisit_stats.wrap_cache_hits > 0,
            "expected exact wrap-cache hits on second partial walk; got {exact_revisit_stats:?}",
        );
        assert_eq!(exact_revisit_stats.measure_calls, 0);
        assert_eq!(exact_revisit_stats.segment_build_us, 0);
        assert_eq!(exact_revisit_stats.measure_us, 0);

        let cold_wrap_cache = WrapCache::new(256);
        let mut segment_revisit_stats = WalkerStats::default();
        let _ = DisplayMapBuilder::new(&snapshot, &decorations, &[], &[], wrap)
            .with_row_count_caches(99, "en-us", &cold_wrap_cache, &segment_cache)
            .compute_partial_row_index_for_viewport_with_stats(
                50u32..80u32,
                4,
                &mut measure,
                Some(&mut segment_revisit_stats),
            )
            .expect("partial segment revisit");
        // A cold wrap cache still exercises the segment cache populated
        // by the partial walker.
        assert!(
            segment_revisit_stats.segment_cache_hits > 0,
            "expected segment-cache hits when wrap cache is cold; got {segment_revisit_stats:?}",
        );
    }
}
