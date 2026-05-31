//! ε.3F — `DisplayMapBuilder::rebuild_spliced` implementation.
//!
//! Sibling of [`super::rebuild_dirty`]. Where `rebuild_dirty` reuses
//! the existing row-index shape and refreshes a per-source-line dirty
//! set, `rebuild_spliced` first *splices* the row-index for a local
//! line-count edit (single-`\n` Enter, multi-line paste of `N`
//! newlines, single-`\n` delete, multi-line delete that collapses
//! `N + 1` source lines into 1), then realizes the spliced viewport.
//! The implementation is generic over both `splice.removed` and
//! `splice.inserted` — the multi-line-paste and multi-line-delete
//! classifier extensions that landed 2026-05-17 did not require
//! any change to this file.
//!
//! Reused clean specs are looked up by their *pre-splice* source-line
//! index (computed from the new index by subtracting the splice's
//! `line_delta`) and rebased through
//! [`crate::DisplayLineSpec::shift_source_bytes`] just like
//! `rebuild_dirty` does. Lines inside the spliced region
//! (`splice.at..splice.at + splice.inserted`) are always
//! materialized fresh — they have no clean predecessor in `prev`.
//!
//! The splice detection lives at
//! [`crate::DisplayRowIndex::dirty_after_rope_edits`]; callers must
//! match on [`crate::RowDirty::Splice`] and route here.

use std::sync::Arc;

use crate::error::Error;
use crate::id::SourceLine;
use crate::line::DisplayLineSpec;
use crate::map::DisplayMap;
use crate::row_index::splice::RowSplice;
use crate::row_index::DisplayRowIndex;
use crate::wrap::WidthMeasure;

use super::row_counts::row_count_for_source_line;
use super::DisplayMapBuilder;

impl<'a> DisplayMapBuilder<'a> {
    /// Apply `splice` to `prev`'s row index, recompute row counts for
    /// the spliced / dirty source lines, then realize the viewport.
    ///
    /// Reused clean specs (every source line outside the spliced
    /// region that intersects the new viewport) are mapped from
    /// post-splice index back to their pre-splice slot in `prev` and
    /// rebased through `DisplayLineSpec::shift_source_bytes` for the
    /// byte shift caused by the inserted / deleted newline.
    ///
    /// Pre-conditions:
    /// - `prev`'s row index has `splice.at + splice.removed` valid
    ///   source-line slots.
    /// - The new rope satisfies
    ///   `rope.len_lines() == prev.source_line_count() + splice.line_delta()`.
    ///
    /// # Errors
    ///
    /// Same as [`Self::build_viewport`].
    pub fn rebuild_spliced(
        self,
        prev: &DisplayMap,
        splice: &RowSplice,
        visible_rows: std::ops::Range<u32>,
        overscan: u32,
        measure: &mut dyn WidthMeasure,
    ) -> Result<Arc<DisplayMap>, Error> {
        debug_assert!(
            splice.dirty.windows(2).all(|w| w[0] < w[1]),
            "RowSplice.dirty must be sorted and deduplicated",
        );
        self.validate_inputs()?;
        let rope = self.snapshot.rope();
        let post_line_count = rope.len_lines() as u32;
        let expected_line_count =
            (prev.row_index().source_line_count() as i64 + splice.line_delta()) as u32;

        // If the splice contract doesn't line up with the rope, fall
        // through to a safe full viewport build.
        if expected_line_count != post_line_count {
            return self.build_viewport(visible_rows, overscan, measure);
        }

        // 1. Splice the row-count slots in place (placeholder 0;
        //    recomputed below). Then recompute row counts for every
        //    dirty source line. Then advance stamps.
        let mut index: DisplayRowIndex = prev.row_index().clone();
        index.splice_rows(splice, 0);
        for &source_line in &splice.dirty {
            if source_line >= post_line_count {
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

        // 2. Compute the new realized source-line range.
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

        // 3. Build the new realized vector. For each NEW source line
        //    in range:
        //    - If it falls inside the spliced region (no clean prev
        //      counterpart exists), materialize fresh.
        //    - Otherwise compute the pre-splice source-line index by
        //      undoing the line delta, look up prev's realized specs,
        //      and shift the bytes.
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
        let splice_at = splice.at as usize;
        let splice_end_new = splice_at + splice.inserted as usize;
        let line_delta = splice.line_delta();
        for source_line_idx in new_source_range.start..new_source_range.end {
            let inside_splice = source_line_idx >= splice_at && source_line_idx < splice_end_new;
            let prev_specs = if inside_splice {
                None
            } else {
                let old_idx = if source_line_idx < splice_at {
                    source_line_idx as i64
                } else {
                    source_line_idx as i64 - line_delta
                };
                if old_idx < 0 || (old_idx as u32) >= prev.row_index().source_line_count() {
                    None
                } else {
                    prev.realized_lines_for_source(SourceLine::from_usize(old_idx as usize))
                }
            };
            if let Some(prev_specs) = prev_specs {
                let target_start = rope.line_to_byte(source_line_idx) as i64;
                let cached_start = prev_specs.first().map(|s| s.source_byte_start.raw() as i64);
                let delta = cached_start.map_or(0, |c| target_start - c);
                if delta == 0 {
                    lines.extend_from_slice(prev_specs);
                } else {
                    for spec in prev_specs {
                        let mut shifted = spec.clone();
                        shifted.shift_source_bytes(delta);
                        // The reused spec belongs to a different
                        // source-line index in the new map than it
                        // did in `prev`; update the field so
                        // downstream consumers indexing by
                        // `spec.source_line` see the post-splice
                        // address.
                        shifted.source_line = SourceLine::from_usize(source_line_idx);
                        lines.push(shifted);
                    }
                }
                if self
                    .image_reservations
                    .get(reservation_cursor)
                    .is_some_and(|r| r.source_line.raw() as usize == source_line_idx)
                {
                    reservation_cursor += 1;
                }
            } else {
                self.materialize_source_line(
                    rope,
                    source_line_idx as u32,
                    &mut lines,
                    &mut reservation_cursor,
                    measure,
                )?;
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

fn image_reservation_cursor_for(
    reservations: &[crate::image_row_reservation_provider::ImageRowReservation],
    source_line: u32,
) -> usize {
    match reservations.binary_search_by(|r| r.source_line.raw().cmp(&source_line)) {
        Ok(idx) => idx,
        Err(idx) => idx,
    }
}
