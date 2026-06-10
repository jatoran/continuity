//! Build a [`DisplayMap`] from a rope snapshot + decorations + caret + folds.
//!
//! See the crate-root docs for the invariants this builder maintains.
//!
//! ## Threading
//!
//! Building is pure CPU work: a worker thread (typically the decoration
//! worker) calls [`DisplayMapBuilder::build`] off the UI thread, then hands
//! the resulting `Arc<DisplayMap>` back via a channel. No win32 / D2D /
//! DirectWrite handles are touched in the builder.

use std::sync::Arc;

use continuity_buffer::RopeSnapshot;
use continuity_decorate::Decorations;
use ropey::Rope;

use crate::error::Error;
use crate::fold::{FoldRange, FoldSignature};
use crate::id::{SourceByte, SourceLine};
use crate::image_row_reservation_provider::ImageRowReservation;
use crate::line::DisplayLineSpec;
use crate::map::DisplayMap;
use crate::markdown_toggles::MarkdownRenderToggles;
use crate::row_index::{DisplayRowIndex, IndexStamps};
use crate::segment_cache::SegmentCache;
use crate::wrap::{WidthMeasure, WrapConfig};
use crate::wrap_cache::WrapCache;

/// Whole-document row-count walker callers that are allowed to rebuild
/// the full [`DisplayRowIndex`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WalkerCallReason {
    /// Initial or unavoidable paint cold path.
    PaintCold,
    /// Paint path rebuilding after dirty projection state.
    PaintDirty,
    /// Paint path realizing a viewport from an existing projection plan.
    ViewportRealize,
    /// Idle prewarm path.
    Prewarm,
}

impl WalkerCallReason {
    /// Stable trace spelling for `event:row_count_walker reason=...`.
    #[must_use]
    pub const fn as_trace_reason(self) -> &'static str {
        match self {
            Self::PaintCold => "paint_cold",
            Self::PaintDirty => "paint_dirty",
            Self::ViewportRealize => "viewport_realize",
            Self::Prewarm => "prewarm",
        }
    }
}

/// Builder for [`DisplayMap`] snapshots.
pub struct DisplayMapBuilder<'a> {
    snapshot: &'a RopeSnapshot,
    decorations: &'a Decorations,
    caret_bytes: &'a [SourceByte],
    folds: &'a [FoldRange],
    image_reservations: &'a [ImageRowReservation],
    /// Document-absolute `EvaluatedTable.block_range`s of tables the
    /// current selection has reached past a single cell. Tables in
    /// this list skip the pipe-hide pass so raw markdown renders for
    /// the selection to land on; the render side consults the same
    /// list to skip painting visual chrome. Empty (`&[]`) means no
    /// suppression — every table renders as cells, the default.
    suppressed_table_blocks: &'a [std::ops::Range<usize>],
    /// Per-decoration render toggles. Gates emphasis/strong styling and
    /// delimiter hiding, the `==` highlight delimiter hide, setext
    /// heading rendering, and thematic-break marker hiding inside
    /// [`segments::build_line_segments`]. Default
    /// ([`MarkdownRenderToggles::default`]) is italic-off / rest-on.
    /// Carried on the builder (rather than passed per call) so every
    /// row-count walker and spec-materialization path funnels through
    /// `build_line_segments` with the same toggle set — the soft-wrap
    /// row-count / segment-agreement invariant.
    markdown_toggles: MarkdownRenderToggles,
    wrap: WrapConfig,
    row_count_cache: Option<row_counts::RowCountCacheContext<'a>>,
    walker_reason: WalkerCallReason,
}

impl<'a> DisplayMapBuilder<'a> {
    /// Construct a builder against the given inputs.
    #[must_use]
    pub fn new(
        snapshot: &'a RopeSnapshot,
        decorations: &'a Decorations,
        caret_bytes: &'a [SourceByte],
        folds: &'a [FoldRange],
        wrap: WrapConfig,
    ) -> Self {
        Self {
            snapshot,
            decorations,
            caret_bytes,
            folds,
            image_reservations: &[],
            suppressed_table_blocks: &[],
            markdown_toggles: MarkdownRenderToggles::default(),
            wrap,
            row_count_cache: None,
            walker_reason: WalkerCallReason::ViewportRealize,
        }
    }

    /// Set the per-decoration markdown render toggles. Defaults to
    /// [`MarkdownRenderToggles::default`] (italic off, rest on) when not
    /// called. The toggle set gates emphasis/strong styling + delimiter
    /// hide, the `==` highlight delimiter hide, setext heading
    /// rendering, and thematic-break marker hide — never decoration
    /// production. Threaded into every `build_line_segments` call site
    /// (full build, viewport materialize, row-count walker, dirty /
    /// spliced rebuild) so the realized segments and the soft-wrap row
    /// counts always agree.
    #[must_use]
    pub fn with_markdown_toggles(mut self, toggles: MarkdownRenderToggles) -> Self {
        self.markdown_toggles = toggles;
        self
    }

    /// Mark the listed `EvaluatedTable.block_range`s as
    /// selection-suppressed. The hide pass returns nothing for those
    /// tables (raw pipes + alignment row + formula source render),
    /// and the render side skips painting visual chrome so the user
    /// sees what the active selection actually covers. Default
    /// (`&[]`) preserves the always-render behaviour from Phase A.
    #[must_use]
    pub fn with_suppressed_table_blocks(mut self, blocks: &'a [std::ops::Range<usize>]) -> Self {
        self.suppressed_table_blocks = blocks;
        self
    }

    /// Set the allowed whole-document walker reason for viewport row
    /// index construction.
    #[must_use]
    pub fn with_walker_reason(mut self, reason: WalkerCallReason) -> Self {
        self.walker_reason = reason;
        self
    }

    /// Attach a sorted `&[ImageRowReservation]` slice produced by
    /// [`crate::image_row_reservation_provider::compute_image_row_reservations`].
    /// Each entry tells the builder to inject
    /// `reserved_display_rows - <rows actually pushed for that source
    /// line>` phantom display rows after the source line's natural
    /// projection, so content below an expanded inline image flows
    /// beneath the bitmap rather than being overdrawn.
    ///
    /// Phantom rows are non-editable (zero source bytes, no segments)
    /// and share their `source_line` with the image's natural row, so
    /// caret navigation, selection ranges, and mouse hit-tests all
    /// resolve to the image's source line without traversing the
    /// reserved space.
    #[must_use]
    pub fn with_image_reservations(mut self, reservations: &'a [ImageRowReservation]) -> Self {
        self.image_reservations = reservations;
        self
    }

    /// Attach shared row-count walker caches. Callers pass `font_state`
    /// as raw bits to keep `display_map` independent of the `layout`
    /// crate that defines `FontStateId`.
    #[must_use]
    pub fn with_row_count_caches(
        mut self,
        font_state: u64,
        locale: &'a str,
        wrap_cache: &'a WrapCache,
        segment_cache: &'a SegmentCache,
    ) -> Self {
        self.row_count_cache = Some(row_counts::RowCountCacheContext {
            font_state,
            locale,
            wrap_cache,
            segment_cache,
        });
        self
    }

    /// Produce an immutable `Arc<DisplayMap>` snapshot that realizes
    /// every visible display row in the document.
    ///
    /// `measure` is invoked when soft-wrap is active (`wrap.enabled()`);
    /// tests typically pass [`crate::wrap::FixedCharWidth`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::CaretOutOfBounds`] if a caret byte is past the
    /// rope's length; [`Error::FoldOutOfBounds`] for an out-of-range fold;
    /// [`Error::BadMeasurement`] if the width-measure callback returns a
    /// non-finite or negative value.
    pub fn build(self, measure: &mut dyn WidthMeasure) -> Result<Arc<DisplayMap>, Error> {
        self.validate_inputs()?;
        let rope = self.snapshot.rope();
        let source_line_count = rope.len_lines() as u32;
        let mut lines: Vec<DisplayLineSpec> = Vec::with_capacity(source_line_count as usize);
        let mut row_counts: Vec<u16> = vec![0u16; source_line_count as usize];
        let mut reservation_cursor: usize = 0;

        for source_line_idx in 0..source_line_count {
            let pushed = self.materialize_source_line(
                rope,
                source_line_idx,
                &mut lines,
                &mut reservation_cursor,
                measure,
            )?;
            row_counts[source_line_idx as usize] = pushed;
        }

        let stamps = self.build_stamps();
        let row_index = Arc::new(DisplayRowIndex::from_row_counts(row_counts, stamps));

        Ok(Arc::new(DisplayMap::from_parts(
            self.decorations.revision,
            self.wrap.width_dip,
            row_index,
            lines,
        )))
    }

    /// ε.2 — produce a `DisplayMap` that realizes only the source lines
    /// whose display rows intersect `visible_rows` (expanded by
    /// `overscan` rows above and below). The whole-document
    /// [`DisplayRowIndex`] is still computed (offscreen consumers depend
    /// on it), but no `DisplayLineSpec`s are materialized outside the
    /// realized window.
    ///
    /// `visible_rows` is a half-open absolute display-row range. Pass
    /// `0..u32::MAX` (or any range covering the whole document) to fall
    /// back to a full realization — the result is equivalent to
    /// [`Self::build`] in that case but pays the cheap-walker overhead.
    ///
    /// # Errors
    ///
    /// Same as [`Self::build`].
    pub fn build_viewport(
        self,
        visible_rows: std::ops::Range<u32>,
        overscan: u32,
        measure: &mut dyn WidthMeasure,
    ) -> Result<Arc<DisplayMap>, Error> {
        self.build_viewport_with_stats(visible_rows, overscan, measure, None)
    }

    /// Variant of [`Self::build_viewport`] that captures
    /// [`WalkerStats`] for the cheap row-count walker phase. The UI
    /// thread passes `Some` when paint tracing is enabled so the
    /// `paint:row_count_walker_stats` event names which fast/slow
    /// paths each source line took.
    pub fn build_viewport_with_stats(
        self,
        visible_rows: std::ops::Range<u32>,
        overscan: u32,
        measure: &mut dyn WidthMeasure,
        stats: Option<&mut WalkerStats>,
    ) -> Result<Arc<DisplayMap>, Error> {
        self.validate_inputs()?;
        let rope = self.snapshot.rope();
        let _reason = self.walker_reason;

        // Cheap walker — fills `row_counts` without materializing specs.
        let row_counts = compute_row_counts(
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
            stats,
        )?;
        let stamps = self.build_stamps();
        let row_index = Arc::new(DisplayRowIndex::from_row_counts(row_counts, stamps));
        self.build_viewport_finish(rope, row_index, visible_rows, overscan, measure)
    }

    /// Run only the cheap row-count walker and return the
    /// `DisplayRowIndex`. Used by paint paths that want to time the
    /// walker and the spec-materialization step separately for
    /// `paint:row_count_walker` / `paint:viewport_materialize` trace
    /// attribution. The follow-up materialization should go through
    /// [`Self::build_viewport_with_row_index`] on a freshly-constructed
    /// builder; the row-index identity test (rope rev, decoration rev,
    /// wrap, fold signature, font state) is the caller's
    /// responsibility — passing back the same projection inputs that
    /// produced the index is sufficient.
    pub fn compute_row_index_with_stats(
        self,
        measure: &mut dyn WidthMeasure,
        mut stats: Option<&mut WalkerStats>,
    ) -> Result<Arc<DisplayRowIndex>, Error> {
        self.validate_inputs()?;
        let _reason = self.walker_reason;
        let row_counts = compute_row_counts(
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
            stats.as_deref_mut(),
        )?;
        let stamps = self.build_stamps();
        let t_fenwick = stats.as_ref().map(|_| std::time::Instant::now());
        let index = DisplayRowIndex::from_row_counts(row_counts, stamps);
        if let (Some(stats), Some(t0)) = (stats.as_mut(), t_fenwick) {
            let us = t0.elapsed().as_micros() as u64;
            stats.fenwick_build_us = stats.fenwick_build_us.saturating_add(us);
        }
        Ok(Arc::new(index))
    }

    /// Viewport build that **reuses** a pre-built `DisplayRowIndex`
    /// instead of walking the whole document with the cheap row-count
    /// walker. The caller is responsible for verifying that the index
    /// was built against compatible inputs (same buffer, rope revision,
    /// decoration revision, wrap width, font state, fold signature) —
    /// the stamps check in `IndexStamps` is the standard way to do
    /// that. On a 9 k-line markdown buffer the walker dominates the
    /// per-frame cost (~400 ms in
    /// `perf-snapshots/manual-lag_after-coalesce_20260518-121136.tsv`);
    /// skipping it drops the cold viewport build to spec-realization
    /// time (~5-10 ms for ~50 visible rows + overscan).
    ///
    /// # Errors
    /// Same as [`Self::build_viewport`].
    pub fn build_viewport_with_row_index(
        self,
        row_index: Arc<DisplayRowIndex>,
        visible_rows: std::ops::Range<u32>,
        overscan: u32,
        measure: &mut dyn WidthMeasure,
    ) -> Result<Arc<DisplayMap>, Error> {
        self.validate_inputs()?;
        let rope = self.snapshot.rope();
        self.build_viewport_finish(rope, row_index, visible_rows, overscan, measure)
    }

    fn build_viewport_finish(
        self,
        rope: &Rope,
        row_index: Arc<DisplayRowIndex>,
        visible_rows: std::ops::Range<u32>,
        overscan: u32,
        measure: &mut dyn WidthMeasure,
    ) -> Result<Arc<DisplayMap>, Error> {
        // Translate visible rows + overscan → source-line range.
        let total_rows = row_index.display_row_count();
        let expanded_start = visible_rows.start.saturating_sub(overscan);
        let expanded_end = visible_rows.end.saturating_add(overscan).min(total_rows);
        let source_range = row_index.source_lines_for_display_rows(expanded_start..expanded_end);

        // Realize specs only for source lines in range.
        let realized_row_start = if source_range.start < row_index.source_line_count() as usize {
            row_index
                .first_display_row_of_source_line(SourceLine::from_usize(source_range.start))
                .raw()
        } else {
            total_rows
        };
        let mut lines: Vec<DisplayLineSpec> =
            Vec::with_capacity(source_range.end.saturating_sub(source_range.start));
        let mut reservation_cursor: usize = 0;
        // Wind the reservation cursor past source lines before the
        // realized range so the per-line lookup inside
        // `materialize_source_line` stays amortised-O(1).
        while reservation_cursor < self.image_reservations.len()
            && (self.image_reservations[reservation_cursor]
                .source_line
                .raw() as usize)
                < source_range.start
        {
            reservation_cursor += 1;
        }
        // Reconcile the row index to the *materialized* row counts for
        // the realized window. `row_index` came from the cheap row-count
        // walker (`compute_row_counts`); the realized `lines` vector is
        // the ground truth. On a caret-reveal line the raw markdown wraps
        // to a different row count than the cheap walker proves, and the
        // two can disagree. Left unreconciled the frame is born with its
        // index out of sync with its specs; a later `rebuild_dirty` reuse
        // then slices the wrong spec count via
        // `DisplayMap::realized_lines_for_source`, duplicating and
        // dropping display rows (the click-to-reveal scramble, seen on
        // the read-only tutorial whose frames lean on this cold path).
        // Clone-and-patch only when a divergence is actually observed, so
        // the common no-divergence build pays nothing beyond the per-line
        // comparison; offscreen lines keep their cheap-walker counts (no
        // specs exist to reconcile against, and they are never painted).
        let mut reconciled_counts: Vec<(usize, u16)> = Vec::new();
        for source_line_idx in source_range.start..source_range.end {
            let pushed = self.materialize_source_line(
                rope,
                source_line_idx as u32,
                &mut lines,
                &mut reservation_cursor,
                measure,
            )?;
            if u32::from(pushed)
                != row_index.display_row_count_for_source(SourceLine::from_usize(source_line_idx))
            {
                reconciled_counts.push((source_line_idx, pushed));
            }
        }

        let row_index = if reconciled_counts.is_empty() {
            row_index
        } else {
            let mut patched = (*row_index).clone();
            for (source_line_idx, count) in reconciled_counts {
                patched.set_row_count(SourceLine::from_usize(source_line_idx), count);
            }
            Arc::new(patched)
        };

        Ok(Arc::new(DisplayMap::from_parts_viewport(
            self.decorations.revision,
            self.wrap.width_dip,
            row_index,
            lines,
            realized_row_start,
        )))
    }

    fn validate_inputs(&self) -> Result<(), Error> {
        let rope = self.snapshot.rope();
        let len = rope.len_bytes();
        for c in self.caret_bytes {
            if c.as_usize() > len {
                return Err(Error::CaretOutOfBounds {
                    byte: c.as_usize(),
                    len,
                });
            }
        }
        for f in self.folds {
            if f.start.as_usize() > len || f.end.as_usize() > len {
                return Err(Error::FoldOutOfBounds {
                    start: f.start.as_usize(),
                    end: f.end.as_usize(),
                    len,
                });
            }
        }
        Ok(())
    }

    fn build_stamps(&self) -> IndexStamps {
        IndexStamps {
            rope_revision: self.snapshot.revision().0,
            decoration_revision: self.decorations.revision,
            wrap_width_dip: self.wrap.width_dip,
            // Fold the markdown toggle set into the opaque font-state
            // slot so a hot-reload toggle flip drifts the index stamps
            // and forces a row-count rebuild — toggling italic widens
            // lines (markers become visible), changing soft-wrap row
            // counts, so a stale index must not be reused.
            font_state: self.markdown_toggles.hash_key(),
            fold_signature: FoldSignature::compute(self.folds),
        }
    }

    /// Build the display rows for one source line, pushing the resulting
    /// `DisplayLineSpec`s onto `lines` and stepping `reservation_cursor`
    /// past any image-reservation entry that targets this source line.
    /// Returns the number of rows pushed (0 if the source line is
    /// fully folded). Shared between [`Self::build`] (full document) and
    /// [`Self::build_viewport`] (viewport realization).
    fn materialize_source_line(
        &self,
        rope: &Rope,
        source_line_idx: u32,
        lines: &mut Vec<DisplayLineSpec>,
        reservation_cursor: &mut usize,
        measure: &mut dyn WidthMeasure,
    ) -> Result<u16, Error> {
        let (line_start, line_end) = source_line_range(rope, source_line_idx as usize);
        let line_text = read_line_text(rope, line_start, line_end);

        // Fully-folded source lines (and hidden continuity directives)
        // contribute nothing.
        if line_is_hidden(self.folds, &line_text, line_start, line_end) {
            return Ok(0);
        }
        // The synthetic trailing empty line (after a final `\n`) emits a
        // normal empty `DisplayLineSpec` so the caret has a row to paint
        // on when the user hits Enter at end-of-buffer.

        let segments = build_line_segments(
            self.decorations,
            self.caret_bytes,
            self.folds,
            self.suppressed_table_blocks,
            self.markdown_toggles,
            line_start,
            line_end,
            &line_text,
        );

        let spec = DisplayLineSpec::new(
            SourceLine(source_line_idx),
            SourceByte::from_usize(line_start),
            SourceByte::from_usize(line_end),
            false,
            segments,
            &line_text,
        );

        let before_count = lines.len();
        if self.wrap.enabled() {
            let split = soft_wrap_spec(spec, &line_text, self.wrap, measure)?;
            for s in split {
                lines.push(s);
            }
        } else {
            lines.push(spec);
        }
        let pushed = lines.len() - before_count;

        // γ — phantom-row reservation for expanded inline images.
        if pushed > 0 {
            while *reservation_cursor < self.image_reservations.len()
                && self.image_reservations[*reservation_cursor]
                    .source_line
                    .raw()
                    < source_line_idx
            {
                *reservation_cursor += 1;
            }
            if let Some(reservation) = self.image_reservations.get(*reservation_cursor) {
                if reservation.source_line.raw() == source_line_idx {
                    let target = reservation.reserved_display_rows as usize;
                    for _ in pushed..target {
                        lines.push(phantom_display_line(SourceLine(source_line_idx), line_end));
                    }
                    *reservation_cursor += 1;
                }
            }
        }

        let total = (lines.len() - before_count) as u32;
        Ok(u16::try_from(total).unwrap_or(u16::MAX))
    }
}

mod build_partial;
mod line_helpers;
pub(crate) mod progressive_walker;
mod rebuild_dirty;
mod rebuild_spliced;
mod row_counts;
mod segment_coalescing;
mod segments;
mod segments_helpers;
mod soft_wrap;
pub mod splice_row_index;
pub(crate) mod stats;
mod targeted_row_index;
use line_helpers::{line_is_hidden, phantom_display_line, read_line_text, source_line_range};
use row_counts::compute_row_counts;
use segments::build_line_segments;
use soft_wrap::soft_wrap_spec;
pub(crate) use stats::{SlowestLineRecord, WalkerStats};

#[cfg(test)]
mod tests;
