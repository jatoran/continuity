//! P18.5 — viewport-priority partial row-index entry point on
//! [`super::DisplayMapBuilder`].
//!
//! Sibling of [`super::progressive_walker`]: the walker module owns the
//! `Vec<u16>`-producing function; this module wraps it on the builder
//! so callers can use the same `DisplayMapBuilder::new(...).with_*(...)`
//! chain they use for cold full walks. Kept here so
//! [`super::super::builder`] (the parent `builder.rs`) stays under the
//! 600-line cap.

use std::ops::Range;
use std::sync::Arc;

use continuity_text::RopeEditDelta;

use crate::error::Error;
use crate::row_index::{DisplayRowIndex, PartialRowIndexState};
use crate::wrap::WidthMeasure;

use super::progressive_walker::{
    compute_partial_dirty_row_counts_for_viewport_range,
    compute_partial_row_counts_for_viewport_range,
    compute_partial_splice_row_counts_for_viewport_range, PartialWalkOutcome,
};
use super::stats::WalkerStats;
use super::DisplayMapBuilder;

impl<'a> DisplayMapBuilder<'a> {
    /// P18.5 — compute a viewport-priority *partial* row index without
    /// walking the entire document.
    ///
    /// Walks only the source lines in `viewport_source_range` expanded
    /// by `safety_margin` on each side (use
    /// [`super::progressive_walker::PARTIAL_WALK_SAFETY_MARGIN`] for the
    /// default). Source lines outside the walked range get
    /// [`super::progressive_walker::UNWALKED_PLACEHOLDER_ROW_COUNT`] in
    /// `row_counts`; the
    /// resulting index carries a [`PartialRowIndexState`] whose
    /// [`scrollbar_estimate`](PartialRowIndexState::scrollbar_estimate)
    /// is the density-based total the scrollbar can show until the
    /// background fill completes.
    ///
    /// Same caches the cold walker uses are populated here, so the
    /// background fill is just the rest of the same walk — not a
    /// separate code path.
    ///
    /// # Errors
    ///
    /// Same as [`Self::compute_row_index_with_stats`]:
    /// [`Error::CaretOutOfBounds`] / [`Error::FoldOutOfBounds`] from
    /// `validate_inputs`; [`Error::BadMeasurement`] from the walker.
    pub fn compute_partial_row_index_for_viewport_with_stats(
        self,
        viewport_source_range: Range<u32>,
        safety_margin: u32,
        measure: &mut dyn WidthMeasure,
        mut stats: Option<&mut WalkerStats>,
    ) -> Result<Arc<DisplayRowIndex>, Error> {
        self.validate_inputs()?;
        let (row_counts, outcome) = compute_partial_row_counts_for_viewport_range(
            self.snapshot,
            self.decorations,
            self.caret_bytes,
            self.folds,
            self.image_reservations,
            self.suppressed_table_blocks,
            self.markdown_toggles,
            self.wrap,
            measure,
            self.row_count_cache,
            viewport_source_range,
            safety_margin,
            stats.as_deref_mut(),
        )?;
        let stamps = self.build_stamps();
        let t_fenwick = stats.as_ref().map(|_| std::time::Instant::now());
        let partial = PartialRowIndexState {
            walked_source_range: outcome.walked_source_range,
            scrollbar_estimate: outcome.estimated_total_rows,
            full_revision_target: stamps.rope_revision,
        };
        let index = DisplayRowIndex::from_partial_row_counts(row_counts, stamps, partial);
        if let (Some(stats), Some(t0)) = (stats.as_mut(), t_fenwick) {
            let us = t0.elapsed().as_micros() as u64;
            stats.fenwick_build_us = stats.fenwick_build_us.saturating_add(us);
        }
        Ok(Arc::new(index))
    }

    /// P18.6 — compute a viewport-priority partial row index for a
    /// large dirty rebuild. Dirty lines inside the walked viewport are
    /// measured; clean lines copy exact previous counts where possible.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_partial_dirty_row_index_for_viewport_with_stats(
        self,
        viewport_source_range: Range<u32>,
        safety_margin: u32,
        dirty_source_ranges: &[Range<u32>],
        prev_row_index: &DisplayRowIndex,
        measure: &mut dyn WidthMeasure,
        mut stats: Option<&mut WalkerStats>,
    ) -> Result<Arc<DisplayRowIndex>, Error> {
        self.validate_inputs()?;
        let (row_counts, outcome) = compute_partial_dirty_row_counts_for_viewport_range(
            self.snapshot,
            self.decorations,
            self.caret_bytes,
            self.folds,
            self.image_reservations,
            self.suppressed_table_blocks,
            self.markdown_toggles,
            self.wrap,
            measure,
            self.row_count_cache,
            viewport_source_range,
            safety_margin,
            dirty_source_ranges,
            prev_row_index,
            stats.as_deref_mut(),
        )?;
        Ok(self.finish_partial_row_index(row_counts, outcome, stats))
    }

    /// P18.6 — compute a viewport-priority partial row index for a
    /// splice rebuild. The previous index is mapped forward only for
    /// the walked viewport region; the background fill completes the
    /// full-document splice/cold work after paint.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_partial_splice_row_index_for_viewport_with_stats(
        self,
        viewport_source_range: Range<u32>,
        safety_margin: u32,
        deltas: &[RopeEditDelta],
        prev_row_index: &DisplayRowIndex,
        measure: &mut dyn WidthMeasure,
        mut stats: Option<&mut WalkerStats>,
    ) -> Result<Arc<DisplayRowIndex>, Error> {
        self.validate_inputs()?;
        let (row_counts, outcome) = compute_partial_splice_row_counts_for_viewport_range(
            self.snapshot,
            self.decorations,
            self.caret_bytes,
            self.folds,
            self.image_reservations,
            self.suppressed_table_blocks,
            self.markdown_toggles,
            self.wrap,
            measure,
            self.row_count_cache,
            viewport_source_range,
            safety_margin,
            deltas,
            prev_row_index,
            stats.as_deref_mut(),
        )?;
        Ok(self.finish_partial_row_index(row_counts, outcome, stats))
    }

    fn finish_partial_row_index(
        &self,
        row_counts: Vec<u16>,
        outcome: PartialWalkOutcome,
        mut stats: Option<&mut WalkerStats>,
    ) -> Arc<DisplayRowIndex> {
        let stamps = self.build_stamps();
        let t_fenwick = stats.as_ref().map(|_| std::time::Instant::now());
        let partial = PartialRowIndexState {
            walked_source_range: outcome.walked_source_range,
            scrollbar_estimate: outcome.estimated_total_rows,
            full_revision_target: stamps.rope_revision,
        };
        let index = DisplayRowIndex::from_partial_row_counts(row_counts, stamps, partial);
        if let (Some(stats), Some(t0)) = (stats.as_mut(), t_fenwick) {
            let us = t0.elapsed().as_micros() as u64;
            stats.fenwick_build_us = stats.fenwick_build_us.saturating_add(us);
        }
        Arc::new(index)
    }
}
