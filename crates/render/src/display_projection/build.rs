//! Constructors for [`FrameDisplay`] — full-document builds, viewport
//! builds, and dirty / spliced incremental rebuilds. Each entry point
//! materializes an `Arc<DisplayMap>` via [`DisplayMapBuilder`] and wraps
//! it in a fresh [`FrameDisplay`].

use std::ops::Range;
use std::sync::Arc;

use continuity_buffer::{Revision, RopeSnapshot};
use continuity_decorate::Decorations;
use continuity_display_map::wrap::{FixedCharWidth, WidthMeasure};
use continuity_display_map::{
    DisplayMap, DisplayMapBuilder, DisplayRowIndex, FoldRange, ImageRowReservation, SegmentCache,
    SourceByte, WalkerCallReason, WalkerStats, WrapCache, WrapConfig,
};
use ropey::Rope;

use super::FrameDisplay;

impl FrameDisplay {
    /// Build a projection from the live rope + decorations + caret state.
    ///
    /// `wrap_width_dip == 0` disables soft wrap (the layout uses one
    /// `DisplayLineSpec` per source line). When `> 0` the builder splits
    /// long source lines at word boundaries using `char_width_dip` as
    /// the fixed-width fallback. UI paint paths should prefer
    /// [`Self::build_with_options_measured`] so proportional prose fonts
    /// wrap against DirectWrite's painted glyph advances.
    #[must_use]
    pub fn build(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        wrap_width_dip: u32,
        char_width_dip: f32,
    ) -> Self {
        Self::build_with_folds(
            rope,
            revision,
            decorations,
            caret_bytes,
            &[],
            wrap_width_dip,
            char_width_dip,
        )
    }

    /// §H3 — like [`Self::build`] but accepts user-toggled fold ranges.
    /// Each [`FoldRange`] is a half-open source-byte span whose bytes the
    /// builder erases to `DisplaySegment::Hidden` and whose fully-folded
    /// source lines produce zero display lines.
    ///
    /// Empty `folds` is equivalent to [`Self::build`].
    #[must_use]
    pub fn build_with_folds(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        wrap_width_dip: u32,
        char_width_dip: f32,
    ) -> Self {
        Self::build_with_options(
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            &[],
            wrap_width_dip,
            char_width_dip,
        )
    }

    /// γ — like [`Self::build_with_folds`] but additionally injects
    /// phantom display rows for every entry in `image_reservations`,
    /// so content below an expanded inline image flows beneath the
    /// bitmap instead of being overdrawn. Empty `image_reservations`
    /// is equivalent to [`Self::build_with_folds`].
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn build_with_options(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        char_width_dip: f32,
    ) -> Self {
        let mut measure = FixedCharWidth::new(char_width_dip.max(1.0));
        Self::build_with_options_measured(
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            wrap_width_dip,
            &mut measure,
        )
    }

    /// Build a projection using the caller's width measurer.
    ///
    /// Production UI paint paths pass a DirectWrite-backed measurer so
    /// soft-wrap uses the same glyph advances the renderer will paint.
    /// Tests and non-UI callers can keep using [`Self::build_with_options`]
    /// for the fixed-width fallback.
    #[allow(clippy::too_many_arguments)]
    pub fn build_with_options_measured(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
    ) -> Self {
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
        let map = DisplayMapBuilder::new(&snap, decos, &carets, folds, wrap)
            .with_image_reservations(image_reservations)
            .build(measure)
            .unwrap_or_else(|_| {
                Arc::new(DisplayMap::new(
                    revision,
                    rope.len_lines() as u32,
                    0,
                    vec![],
                ))
            });
        Self { map }
    }

    /// ε.2 — build a projection whose `DisplayLineSpec` vector covers
    /// only the source lines that contribute to `visible_rows`
    /// (expanded by `overscan` rows above and below). The whole-
    /// document [`continuity_display_map::DisplayRowIndex`] is still
    /// computed so offscreen queries (scrollbar content height, EOF
    /// probing, source↔display lookups for unrealized rows) return
    /// correct answers.
    ///
    /// `visible_rows` is a half-open absolute display-row range. Pass
    /// `0..u32::MAX` to fall back to a full realization.
    ///
    /// Production paint paths thread this through
    /// `crates/ui/src/window_paint.rs` once the scroll position +
    /// viewport height have been resolved. Non-paint consumers should
    /// prefer a compatible cached row index and only use whole-index
    /// viewport builds when they are allowed to pay the walker cost.
    #[allow(clippy::too_many_arguments)]
    pub fn build_viewport_measured(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
        visible_rows: Range<u32>,
        overscan: u32,
    ) -> Self {
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
        let map = DisplayMapBuilder::new(&snap, decos, &carets, folds, wrap)
            .with_image_reservations(image_reservations)
            .build_viewport(visible_rows, overscan, measure)
            .unwrap_or_else(|_| {
                Arc::new(DisplayMap::new(
                    revision,
                    rope.len_lines() as u32,
                    0,
                    vec![],
                ))
            });
        Self { map }
    }

    /// Like [`Self::build_viewport_measured`], but with shared row-count
    /// walker caches attached to the whole-document index walk.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn build_viewport_measured_with_caches(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        suppressed_table_blocks: &[Range<usize>],
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
        visible_rows: Range<u32>,
        overscan: u32,
        font_state: u64,
        locale: &str,
        wrap_cache: &WrapCache,
        segment_cache: &SegmentCache,
    ) -> Self {
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
        let map = DisplayMapBuilder::new(&snap, decos, &carets, folds, wrap)
            .with_image_reservations(image_reservations)
            .with_suppressed_table_blocks(suppressed_table_blocks)
            .with_row_count_caches(font_state, locale, wrap_cache, segment_cache)
            .build_viewport(visible_rows, overscan, measure)
            .unwrap_or_else(|_| {
                Arc::new(DisplayMap::new(
                    revision,
                    rope.len_lines() as u32,
                    0,
                    vec![],
                ))
            });
        Self { map }
    }

    /// Compute the whole-document `DisplayRowIndex` *without*
    /// materializing any `DisplayLineSpec`s. Returned alongside a
    /// [`WalkerStats`] accumulator so the paint trace can attribute
    /// the cost to the right walker sub-step (upper-bound fast path,
    /// segment-sum fast path, grapheme slow path). The follow-up
    /// materialization should go through
    /// [`Self::build_viewport_with_row_index_measured`] passing the
    /// `Arc<DisplayRowIndex>` returned here. Splitting the two phases
    /// is the standard way for paint to time `row_count_walker` and
    /// `viewport_materialize` separately in the cold-build trace.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_row_index_measured(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
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
            .compute_row_index_with_stats(measure, Some(&mut stats))
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

    /// Like [`Self::compute_row_index_measured`], but with shared
    /// row-count walker caches attached.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_row_index_measured_with_caches(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
        font_state: u64,
        locale: &str,
        wrap_cache: &WrapCache,
        segment_cache: &SegmentCache,
        walker_reason: WalkerCallReason,
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
            .with_row_count_caches(font_state, locale, wrap_cache, segment_cache)
            .with_walker_reason(walker_reason)
            .compute_row_index_with_stats(measure, Some(&mut stats))
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

    /// Refresh selected source-line row counts on top of a previous
    /// whole-document row index. This is the input-path counterpart to
    /// [`Self::compute_row_index_measured_with_caches`]: it uses the
    /// same per-line row-count machinery and shared caches, but never
    /// walks the full document.
    #[allow(clippy::too_many_arguments)]
    pub fn refresh_row_index_source_lines_measured_with_caches(
        previous: &DisplayRowIndex,
        source_lines: &[u32],
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
        font_state: u64,
        locale: &str,
        wrap_cache: &WrapCache,
        segment_cache: &SegmentCache,
    ) -> Option<Arc<DisplayRowIndex>> {
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
        DisplayMapBuilder::new(&snap, decos, &carets, folds, wrap)
            .with_image_reservations(image_reservations)
            .with_row_count_caches(font_state, locale, wrap_cache, segment_cache)
            .refresh_row_index_source_lines(previous, source_lines, measure)
            .ok()
            .flatten()
    }

    /// Splice `previous` forward by `deltas` into a row index keyed at
    /// the live builder inputs. Wraps
    /// [`DisplayMapBuilder::splice_row_index_forward`] with the same
    /// rope / revision / decoration / caret / fold / image-reservation
    /// / wrap inputs the rest of [`FrameDisplay`] takes. Returns
    /// `None` when the underlying classifier required a full rebuild
    /// (broken delta chain, ambiguous nesting, or splice contract
    /// violation) — callers fall back to a cold walker run.
    #[allow(clippy::too_many_arguments)]
    pub fn splice_row_index_forward_measured_with_caches(
        previous: &DisplayRowIndex,
        deltas: &[continuity_text::RopeEditDelta],
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
        font_state: u64,
        locale: &str,
        wrap_cache: &WrapCache,
        segment_cache: &SegmentCache,
    ) -> Option<(
        Arc<DisplayRowIndex>,
        continuity_display_map::RowIndexSpliceStats,
    )> {
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
        DisplayMapBuilder::new(&snap, decos, &carets, folds, wrap)
            .with_image_reservations(image_reservations)
            .with_row_count_caches(font_state, locale, wrap_cache, segment_cache)
            .splice_row_index_forward(previous, deltas, measure)
            .ok()
            .flatten()
    }

    /// Viewport build that reuses a caller-supplied
    /// `Arc<DisplayRowIndex>`. The caller has already verified that
    /// the index was built against the same buffer, rope revision,
    /// decoration revision, wrap width, font state, and fold
    /// signature; passing it here skips the cheap row-count walker
    /// (which is O(source_line_count) and dominates the per-frame
    /// cost on large markdown buffers).
    #[allow(clippy::too_many_arguments)]
    pub fn build_viewport_with_row_index_measured(
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
        visible_rows: Range<u32>,
        overscan: u32,
        row_index: std::sync::Arc<continuity_display_map::DisplayRowIndex>,
    ) -> Self {
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
        let map = DisplayMapBuilder::new(&snap, decos, &carets, folds, wrap)
            .with_image_reservations(image_reservations)
            .build_viewport_with_row_index(row_index, visible_rows, overscan, measure)
            .unwrap_or_else(|_| {
                Arc::new(DisplayMap::new(
                    revision,
                    rope.len_lines() as u32,
                    0,
                    vec![],
                ))
            });
        Self { map }
    }

    /// ε.3 — rebuild against a previous projection, reusing
    /// `DisplayLineSpec`s for clean source lines and only
    /// materializing the dirty range. Returns a full from-scratch
    /// viewport build when the input row index disagrees with the
    /// new rope's line count (forwarded by the underlying
    /// `DisplayMapBuilder::rebuild_dirty`).
    ///
    /// `dirty` is a half-open source-line range, typically obtained
    /// from `prev.row_index().dirty_after_rope_edits(deltas,
    /// rope_after)`.
    #[allow(clippy::too_many_arguments)]
    pub fn rebuild_dirty_measured(
        prev: &Self,
        dirty: &[u32],
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        suppressed_table_blocks: &[Range<usize>],
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
        visible_rows: Range<u32>,
        overscan: u32,
    ) -> Self {
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
        let map = DisplayMapBuilder::new(&snap, decos, &carets, folds, wrap)
            .with_image_reservations(image_reservations)
            .with_suppressed_table_blocks(suppressed_table_blocks)
            .rebuild_dirty(prev.map(), dirty, visible_rows, overscan, measure)
            .unwrap_or_else(|_| {
                Arc::new(DisplayMap::new(
                    revision,
                    rope.len_lines() as u32,
                    0,
                    vec![],
                ))
            });
        Self { map }
    }

    /// ε.3F — splice-rebuild a viewport-realized projection from
    /// `prev` after a single newline insert / delete reshapes the
    /// document's source-line count by `splice.line_delta()`. The
    /// caller has already classified the edit via
    /// [`continuity_display_map::DisplayRowIndex::dirty_after_rope_edits`]
    /// returning [`continuity_display_map::RowDirty::Splice`]; this
    /// method routes through the spliced builder path.
    #[allow(clippy::too_many_arguments)]
    pub fn rebuild_spliced_measured(
        prev: &Self,
        splice: &continuity_display_map::RowSplice,
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        suppressed_table_blocks: &[Range<usize>],
        wrap_width_dip: u32,
        measure: &mut dyn WidthMeasure,
        visible_rows: Range<u32>,
        overscan: u32,
    ) -> Self {
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
        let map = DisplayMapBuilder::new(&snap, decos, &carets, folds, wrap)
            .with_image_reservations(image_reservations)
            .with_suppressed_table_blocks(suppressed_table_blocks)
            .rebuild_spliced(prev.map(), splice, visible_rows, overscan, measure)
            .unwrap_or_else(|_| {
                Arc::new(DisplayMap::new(
                    revision,
                    rope.len_lines() as u32,
                    0,
                    vec![],
                ))
            });
        Self { map }
    }
}
