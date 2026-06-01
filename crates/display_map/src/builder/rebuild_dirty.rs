//! ε.3 — `DisplayMapBuilder::rebuild_dirty` implementation.
//!
//! Given a previous `DisplayMap`, a dirty source-line range, and the
//! new inputs (rope, decorations, caret, folds, image reservations,
//! soft-wrap), rebuild a viewport-realized `DisplayMap` that:
//!
//! - **Reuses** `prev`'s `DisplayLineSpec`s for clean source lines
//!   whose display rows fell inside both prev's and the new realized
//!   window.
//! - **Materializes** fresh specs for dirty source lines and any
//!   source line that newly intersects the realized window after the
//!   edit.
//! - **Updates** the row count for each dirty source line in the
//!   index and stamps it with the new rope / decoration revisions.
//!
//! For a typical one-character edit on a 6 k-line buffer with a 40-
//! row viewport, this rebuilds one source line's specs and clones
//! the other ~80 directly — sub-millisecond keystroke-to-paint cost.
//!
//! The function is the cheap counterpart to
//! [`super::DisplayMapBuilder::build_viewport`]. Callers must pass a
//! `dirty` source-line range that genuinely covers every line whose
//! row count or spec content changed (typically computed by
//! [`crate::DisplayRowIndex::dirty_after_rope_edits`]); the
//! `RowDirty::FullRebuild` sentinel forces the caller back to
//! `build_viewport` because line-count changes can't be patched in
//! place.

use std::sync::Arc;

use crate::error::Error;
use crate::id::SourceLine;
use crate::line::DisplayLineSpec;
use crate::map::DisplayMap;
use crate::row_index::DisplayRowIndex;
use crate::wrap::WidthMeasure;

use super::row_counts::row_count_for_source_line;
use super::DisplayMapBuilder;

impl<'a> DisplayMapBuilder<'a> {
    /// Produce a viewport-realized `DisplayMap` that reuses `prev`'s
    /// realized specs for clean source lines and materializes fresh
    /// specs only for the dirty source lines.
    ///
    /// Pre-conditions:
    /// - `prev`'s row index must have the same source-line count as
    ///   `rope.len_lines()` (caller pre-checked the
    ///   [`crate::RowDirty`] result was not `FullRebuild`).
    /// - `dirty` is a **sorted, deduplicated** slice of source-line
    ///   indices. Out-of-range entries are ignored.
    ///
    /// # Errors
    ///
    /// Same as [`Self::build_viewport`].
    pub fn rebuild_dirty(
        self,
        prev: &DisplayMap,
        dirty: &[u32],
        visible_rows: std::ops::Range<u32>,
        overscan: u32,
        measure: &mut dyn WidthMeasure,
    ) -> Result<Arc<DisplayMap>, Error> {
        debug_assert!(
            dirty.windows(2).all(|w| w[0] < w[1]),
            "DisplayMapBuilder::rebuild_dirty requires sorted, deduplicated dirty source lines",
        );
        self.validate_inputs()?;
        let rope = self.snapshot.rope();
        let source_line_count = rope.len_lines() as u32;

        // The dirty-rebuild contract requires line counts to match.
        if prev.row_index().source_line_count() != source_line_count {
            return self.build_viewport(visible_rows, overscan, measure);
        }

        let mut index: DisplayRowIndex = prev.row_index().clone();

        // 1. Refresh row counts for each dirty source line via the
        //    single-line cheap walker. Out-of-range entries are
        //    skipped; the underlying index would panic on `set` for
        //    those, so the filter is both safety and correctness.
        for &source_line in dirty {
            if source_line >= source_line_count {
                continue;
            }
            let cursor = image_reservation_cursor_for(self.image_reservations, source_line);
            let count = row_count_for_source_line(
                rope,
                self.decorations,
                self.caret_bytes,
                self.folds,
                self.image_reservations,
                self.suppressed_table_blocks,
                self.wrap,
                measure,
                self.row_count_cache,
                source_line,
                cursor,
                None,
            )?;
            index.set_row_count(SourceLine(source_line), count);
        }
        index.set_stamps(self.build_stamps());

        // 2. New realized source-line range from the updated index.
        let total_rows = index.display_row_count();
        let expanded_start = visible_rows.start.saturating_sub(overscan);
        let expanded_end = visible_rows.end.saturating_add(overscan).min(total_rows);
        let new_source_range = index.source_lines_for_display_rows(expanded_start..expanded_end);
        let realized_row_start = if new_source_range.start < index.source_line_count() as usize {
            index
                .first_display_row_of_source_line(SourceLine::from_usize(new_source_range.start))
                .raw()
        } else {
            total_rows
        };

        // 3. Build the new realized vector. For each source line in
        //    range: reuse prev's specs when the line is clean and was
        //    realized in prev; otherwise materialize fresh through
        //    the shared per-line path so the output matches what
        //    `build_viewport` would emit.
        let mut lines: Vec<DisplayLineSpec> =
            Vec::with_capacity(new_source_range.end.saturating_sub(new_source_range.start));
        let mut reservation_cursor: usize = 0;
        while reservation_cursor < self.image_reservations.len()
            && (self.image_reservations[reservation_cursor]
                .source_line
                .raw() as usize)
                < new_source_range.start
        {
            reservation_cursor += 1;
        }
        for source_line_idx in new_source_range.start..new_source_range.end {
            let is_dirty = dirty.binary_search(&(source_line_idx as u32)).is_ok();
            let reuse = if is_dirty {
                None
            } else {
                prev.realized_lines_for_source(SourceLine::from_usize(source_line_idx))
            };
            if let Some(prev_specs) = reuse {
                // **Correctness fix.** Within-line dirty edits leave
                // clean source lines' *content* unchanged but shift
                // every later source line's *absolute byte position*
                // by the net delta of the edit. Reused specs carry
                // pre-edit `source_byte_start` / `display_to_source`
                // values; painting with those addresses caret hits,
                // selection ranges, and fold lookups to the wrong
                // rope offsets. We derive the per-line delta from the
                // post-edit rope (`line_to_byte`) — same for every
                // wrap-continuation spec sharing the line — and apply
                // it in O(span_bytes) per spec. A `delta` of `0`
                // (clean line before the edit, or no shift) hits the
                // fast `extend_from_slice` path unchanged.
                let target_start = rope.line_to_byte(source_line_idx) as i64;
                let cached_start = prev_specs.first().map(|s| s.source_byte_start.raw() as i64);
                let delta = cached_start.map_or(0, |c| target_start - c);
                if delta == 0 {
                    lines.extend_from_slice(prev_specs);
                } else {
                    for spec in prev_specs {
                        let mut shifted = spec.clone();
                        shifted.shift_source_bytes(delta);
                        lines.push(shifted);
                    }
                }
                // Step the reservation cursor without re-applying —
                // prev's specs already include the phantom rows.
                if self
                    .image_reservations
                    .get(reservation_cursor)
                    .is_some_and(|r| r.source_line.raw() as usize == source_line_idx)
                {
                    reservation_cursor += 1;
                }
            } else {
                let pushed = self.materialize_source_line(
                    rope,
                    source_line_idx as u32,
                    &mut lines,
                    &mut reservation_cursor,
                    measure,
                )?;
                // Reconcile the row index to the *materialized* row count.
                // Phase 1 set each dirty line's count from the cheap walker
                // (`row_count_for_source_line`), but the realized `lines`
                // vector is the ground truth. On a caret-reveal line the
                // raw markdown source wraps to a different row count than
                // the rendered form, and the cheap walker can disagree with
                // what `materialize_source_line` actually pushes. Left
                // unreconciled, the index would be out of sync with the
                // specs; the *next* rebuild reuses this frame and slices the
                // wrong spec count via `DisplayMap::realized_lines_for_source`,
                // duplicating and dropping display rows (the click-to-reveal
                // scramble). Keeping the index authoritative-from-specs here
                // — exactly what `DisplayMapBuilder::build` does — guarantees
                // the rebuilt frame is internally consistent and stops the
                // desync from compounding across clicks. (Set
                // unconditionally rather than asserting agreement: the
                // divergence is real on revealed lines, so a hard assert
                // would panic debug builds on exactly the buffers we are
                // fixing.)
                index.set_row_count(SourceLine::from_usize(source_line_idx), pushed);
            }
        }

        Ok(Arc::new(DisplayMap::from_parts_viewport(
            self.decorations.revision,
            self.wrap.width_dip,
            Arc::new(index),
            lines,
            realized_row_start,
        )))
    }
}

/// Find the index into `image_reservations` of the entry targeting
/// `source_line`, or the slot just past it when no entry exists. Used
/// by the single-line cheap walker so the reservation lookup runs in
/// O(log n) over a small sorted slice.
fn image_reservation_cursor_for(
    reservations: &[crate::image_row_reservation_provider::ImageRowReservation],
    source_line: u32,
) -> usize {
    match reservations.binary_search_by(|r| r.source_line.raw().cmp(&source_line)) {
        Ok(idx) => idx,
        Err(idx) => idx,
    }
}
