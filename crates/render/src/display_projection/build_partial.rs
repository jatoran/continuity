//! P18.5 — viewport-priority partial row-index entry point on
//! [`FrameDisplay`].
//!
//! Sibling of [`super::build`]. Kept here so [`super::build`] stays
//! under the 600-line cap and so the partial-walk path is grep-locatable
//! by responsibility rather than buried in a sea of full-walk
//! constructors.

use std::ops::Range;
use std::sync::Arc;

use continuity_buffer::{Revision, RopeSnapshot};
use continuity_decorate::Decorations;
use continuity_display_map::wrap::WidthMeasure;
use continuity_display_map::{
    DisplayMapBuilder, DisplayRowIndex, FoldRange, ImageRowReservation, MarkdownRenderToggles,
    SegmentCache, SourceByte, WalkerCallReason, WalkerStats, WrapCache, WrapConfig,
};
use continuity_text::RopeEditDelta;
use ropey::Rope;

use super::FrameDisplay;

impl FrameDisplay {
    /// P18.5 — compute a viewport-priority *partial* row index plus
    /// walker stats, with shared row-count walker caches attached. The
    /// returned `Arc<DisplayRowIndex>` carries a
    /// [`continuity_display_map::PartialRowIndexState`] (see
    /// [`continuity_display_map::DisplayRowIndex::partial_state`]) until
    /// the background fill installs the full index.
    ///
    /// `viewport_source_range` is the half-open source-line range the
    /// caller wants real row counts for; `safety_margin` pads it on each
    /// side. Lines outside the walked range hold the placeholder count
    /// [`continuity_display_map::UNWALKED_PLACEHOLDER_ROW_COUNT`].
    ///
    /// Same caches populated here are reused by the eventual full walk
    /// (cache integration contract — both passes hit the same shaping
    /// cache).
    ///
    /// Callers that own the projection plumbing also emit
    /// `event:partial_row_index_walk viewport_source_lines=N partial_us=N
    /// estimated_total_rows=N` from the returned
    /// [`continuity_display_map::DisplayRowIndex::partial_state`].
    #[allow(clippy::too_many_arguments)]
    pub fn compute_partial_row_index_for_viewport_measured_with_caches(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        markdown_toggles: MarkdownRenderToggles,
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
        font_state: u64,
        locale: &str,
        wrap_cache: &WrapCache,
        segment_cache: &SegmentCache,
        walker_reason: WalkerCallReason,
        viewport_source_range: Range<u32>,
        safety_margin: u32,
    ) -> (Arc<DisplayRowIndex>, WalkerStats) {
        let snap = RopeSnapshot::new(Arc::new(rope.clone()), Revision(revision));
        let owned_empty = Decorations::empty(revision);
        let decos = decorations.unwrap_or(&owned_empty);
        let carets: Vec<SourceByte> = caret_bytes
            .iter()
            .map(|b| SourceByte::from_usize(*b))
            .collect();
        let wrap = if wrap_width_dip > 0 {
            WrapConfig::new(wrap_width_dip)
        } else {
            WrapConfig::NONE
        };
        let mut stats = WalkerStats::default();
        let row_index = DisplayMapBuilder::new(&snap, decos, &carets, folds, wrap)
            .with_image_reservations(image_reservations)
            .with_markdown_toggles(markdown_toggles)
            .with_row_count_caches(font_state, locale, wrap_cache, segment_cache)
            .with_walker_reason(walker_reason)
            .compute_partial_row_index_for_viewport_with_stats(
                viewport_source_range,
                safety_margin,
                measure,
                Some(&mut stats),
            )
            .unwrap_or_else(|_| {
                use continuity_display_map::IndexStamps;
                Arc::new(DisplayRowIndex::from_row_counts(
                    vec![1u16; rope.len_lines()],
                    IndexStamps {
                        rope_revision: revision,
                        decoration_revision: decorations.map_or(revision, |d| d.revision),
                        wrap_width_dip,
                        font_state: 0,
                        fold_signature: 0,
                    },
                ))
            });
        (row_index, stats)
    }

    /// P18.6 — compute a viewport-priority partial row index for a
    /// large dirty rebuild, with shared row-count caches attached.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_partial_dirty_row_index_for_viewport_measured_with_caches(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        markdown_toggles: MarkdownRenderToggles,
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
        font_state: u64,
        locale: &str,
        wrap_cache: &WrapCache,
        segment_cache: &SegmentCache,
        walker_reason: WalkerCallReason,
        viewport_source_range: Range<u32>,
        safety_margin: u32,
        dirty_source_ranges: &[Range<u32>],
        prev_row_index: &DisplayRowIndex,
    ) -> (Arc<DisplayRowIndex>, WalkerStats) {
        let snap = RopeSnapshot::new(Arc::new(rope.clone()), Revision(revision));
        let owned_empty = Decorations::empty(revision);
        let decos = decorations.unwrap_or(&owned_empty);
        let carets: Vec<SourceByte> = caret_bytes
            .iter()
            .map(|b| SourceByte::from_usize(*b))
            .collect();
        let wrap = if wrap_width_dip > 0 {
            WrapConfig::new(wrap_width_dip)
        } else {
            WrapConfig::NONE
        };
        let mut stats = WalkerStats::default();
        let row_index = DisplayMapBuilder::new(&snap, decos, &carets, folds, wrap)
            .with_image_reservations(image_reservations)
            .with_markdown_toggles(markdown_toggles)
            .with_row_count_caches(font_state, locale, wrap_cache, segment_cache)
            .with_walker_reason(walker_reason)
            .compute_partial_dirty_row_index_for_viewport_with_stats(
                viewport_source_range,
                safety_margin,
                dirty_source_ranges,
                prev_row_index,
                measure,
                Some(&mut stats),
            )
            .unwrap_or_else(|_| fallback_row_index(rope, revision, decorations, wrap_width_dip));
        (row_index, stats)
    }

    /// P18.6 — compute a viewport-priority partial row index for a
    /// splice rebuild, with shared row-count caches attached.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_partial_splice_row_index_for_viewport_measured_with_caches(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        markdown_toggles: MarkdownRenderToggles,
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
        font_state: u64,
        locale: &str,
        wrap_cache: &WrapCache,
        segment_cache: &SegmentCache,
        walker_reason: WalkerCallReason,
        viewport_source_range: Range<u32>,
        safety_margin: u32,
        deltas: &[RopeEditDelta],
        prev_row_index: &DisplayRowIndex,
    ) -> (Arc<DisplayRowIndex>, WalkerStats) {
        let snap = RopeSnapshot::new(Arc::new(rope.clone()), Revision(revision));
        let owned_empty = Decorations::empty(revision);
        let decos = decorations.unwrap_or(&owned_empty);
        let carets: Vec<SourceByte> = caret_bytes
            .iter()
            .map(|b| SourceByte::from_usize(*b))
            .collect();
        let wrap = if wrap_width_dip > 0 {
            WrapConfig::new(wrap_width_dip)
        } else {
            WrapConfig::NONE
        };
        let mut stats = WalkerStats::default();
        let row_index = DisplayMapBuilder::new(&snap, decos, &carets, folds, wrap)
            .with_image_reservations(image_reservations)
            .with_markdown_toggles(markdown_toggles)
            .with_row_count_caches(font_state, locale, wrap_cache, segment_cache)
            .with_walker_reason(walker_reason)
            .compute_partial_splice_row_index_for_viewport_with_stats(
                viewport_source_range,
                safety_margin,
                deltas,
                prev_row_index,
                measure,
                Some(&mut stats),
            )
            .unwrap_or_else(|_| fallback_row_index(rope, revision, decorations, wrap_width_dip));
        (row_index, stats)
    }
}

fn fallback_row_index(
    rope: &Rope,
    revision: u64,
    decorations: Option<&Decorations>,
    wrap_width_dip: u32,
) -> Arc<DisplayRowIndex> {
    use continuity_display_map::IndexStamps;
    Arc::new(DisplayRowIndex::from_row_counts(
        vec![1u16; rope.len_lines()],
        IndexStamps {
            rope_revision: revision,
            decoration_revision: decorations.map_or(revision, |d| d.revision),
            wrap_width_dip,
            font_state: 0,
            fold_signature: 0,
        },
    ))
}
