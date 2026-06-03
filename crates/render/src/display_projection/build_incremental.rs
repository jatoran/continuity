//! Incremental [`FrameDisplay`] constructors — row-index refresh /
//! forward-splice, the row-index-reusing viewport build, and the dirty /
//! spliced viewport rebuilds. Split out of
//! [`super::build`](super::build) to keep that file under the 600-line
//! conventions cap. Every entry point threads the same
//! [`MarkdownRenderToggles`] set as the cold builders so the realized
//! segments and soft-wrap row counts stay in agreement.
//!
//! **Thread ownership**: UI thread (focused-pane inline rebuilds) and the
//! projection worker thread (`Cold` / `Dirty` / `Splice` plans).

use std::ops::Range;
use std::sync::Arc;

use continuity_buffer::{Revision, RopeSnapshot};
use continuity_decorate::Decorations;
use continuity_display_map::wrap::WidthMeasure;
use continuity_display_map::{
    DisplayMap, DisplayMapBuilder, DisplayRowIndex, FoldRange, ImageRowReservation,
    MarkdownRenderToggles, SegmentCache, SourceByte, WrapCache, WrapConfig,
};
use ropey::Rope;

use super::FrameDisplay;

impl FrameDisplay {
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
        markdown_toggles: MarkdownRenderToggles,
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
            .with_markdown_toggles(markdown_toggles)
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
        markdown_toggles: MarkdownRenderToggles,
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
            .with_markdown_toggles(markdown_toggles)
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
        markdown_toggles: MarkdownRenderToggles,
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
            .with_markdown_toggles(markdown_toggles)
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
        markdown_toggles: MarkdownRenderToggles,
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
            .with_markdown_toggles(markdown_toggles)
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
        markdown_toggles: MarkdownRenderToggles,
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
            .with_markdown_toggles(markdown_toggles)
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
