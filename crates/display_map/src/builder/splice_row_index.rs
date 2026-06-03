//! Cross-revision splice of a [`DisplayRowIndex`].
//!
//! Sibling of [`super::targeted_row_index`]. Where the targeted helper
//! refreshes a small set of source-line row counts on an index whose
//! source-line *shape* matches the live rope,
//! [`DisplayMapBuilder::splice_row_index_forward`] carries an older-
//! revision index forward across a chain of `RopeEditDelta`s: classify
//! via [`crate::DisplayRowIndex::dirty_after_rope_edits`], structurally
//! splice the row-count vector where the line count drifted, then
//! recompute row counts only for the dirty source lines.
//!
//! When the deltas classify as [`crate::RowDirty::FullRebuild`] or the
//! previous index's source-line shape doesn't line up with the post-
//! edit rope (recovery, external reload, multi-revision drift the
//! bounded delta history can't bridge) the helper returns `None` so the
//! caller falls back to the cold row-count walker. Splice is a fast
//! path, not a contract.

use std::sync::Arc;

use continuity_text::RopeEditDelta;

use crate::error::Error;
use crate::id::SourceLine;
use crate::row_index::dirty::RowDirty;
use crate::row_index::DisplayRowIndex;
use crate::wrap::WidthMeasure;

use super::row_counts::row_count_for_source_line;
use super::DisplayMapBuilder;

/// Per-attempt splice diagnostics. Populated by
/// [`DisplayMapBuilder::splice_row_index_forward`] for paint-time
/// `event:row_index_splice` trace emission.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct RowIndexSpliceStats {
    /// Number of post-edit source lines whose row counts were
    /// recomputed (the `dirty` length for the `Lines` path, the
    /// `splice.dirty.len()` for the `Splice` path).
    pub dirty_lines: u32,
    /// Net byte shift contributed by the delta chain (sum of each
    /// delta's `inserted_bytes - removed_bytes`).
    pub shift_bytes: i64,
    /// `true` when the row-index slot vector was structurally spliced
    /// (line-count change); `false` for in-shape per-line refresh.
    pub used_row_splice: bool,
}

impl<'a> DisplayMapBuilder<'a> {
    /// Splice `previous` forward by `deltas` into a row index keyed at
    /// the live builder inputs (rope revision, decoration revision,
    /// wrap width, font state, fold signature).
    ///
    /// Returns `Ok(Some(...))` when the splice succeeded — the row
    /// counts are byte-identical to what a from-scratch cold walk
    /// would produce under the live inputs.
    ///
    /// Returns `Ok(None)` when the classifier returned
    /// [`crate::RowDirty::FullRebuild`] or the previous index's source-
    /// line shape does not line up with the post-edit rope. Callers
    /// must fall through to a cold walker run in that case.
    ///
    /// # Errors
    ///
    /// Same validation and measurement errors as
    /// [`Self::build_viewport`].
    pub fn splice_row_index_forward(
        self,
        previous: &DisplayRowIndex,
        deltas: &[RopeEditDelta],
        measure: &mut dyn WidthMeasure,
    ) -> Result<Option<(Arc<DisplayRowIndex>, RowIndexSpliceStats)>, Error> {
        self.validate_inputs()?;
        // P18.5 — splicing a partial index forward would treat
        // placeholder slots outside the walked range as real row counts.
        // Bail out so the caller falls through to a cold walker run
        // (which on partial → full transition is the substrate fix the
        // background fill is already racing toward).
        if previous.is_partial() {
            return Ok(None);
        }
        let rope = self.snapshot.rope();
        let new_source_line_count = rope.len_lines() as u32;
        let shift_bytes: i64 = deltas.iter().map(|d| d.shift() as i64).sum();

        let dirty = previous.dirty_after_rope_edits(deltas, rope);
        let mut stats = RowIndexSpliceStats {
            shift_bytes,
            ..RowIndexSpliceStats::default()
        };

        let index = match dirty {
            RowDirty::Lines(lines) => {
                if previous.source_line_count() != new_source_line_count {
                    return Ok(None);
                }
                let mut index = previous.clone();
                for &source_line in &lines {
                    if source_line >= new_source_line_count {
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
                        self.markdown_toggles,
                        self.wrap,
                        measure,
                        self.row_count_cache,
                        source_line,
                        cursor,
                        None,
                    )?;
                    index.set_row_count(SourceLine(source_line), count);
                }
                stats.dirty_lines = lines.len() as u32;
                index
            }
            RowDirty::Splice(splice) => {
                let expected_line_count =
                    (previous.source_line_count() as i64 + splice.line_delta()) as u32;
                if expected_line_count != new_source_line_count {
                    return Ok(None);
                }
                let mut index = previous.clone();
                index.splice_rows(&splice, 0);
                for &source_line in &splice.dirty {
                    if source_line >= new_source_line_count {
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
                        self.markdown_toggles,
                        self.wrap,
                        measure,
                        self.row_count_cache,
                        source_line,
                        cursor,
                        None,
                    )?;
                    index.set_row_count(SourceLine(source_line), count);
                }
                stats.dirty_lines = splice.dirty.len() as u32;
                stats.used_row_splice = true;
                index
            }
            RowDirty::FullRebuild => return Ok(None),
        };

        let mut index = index;
        index.set_stamps(self.build_stamps());
        Ok(Some((Arc::new(index), stats)))
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use continuity_buffer::{Revision, RopeSnapshot};
    use continuity_decorate::Decorations;
    use continuity_text::RopeEditDelta;
    use proptest::prelude::*;
    use ropey::Rope;

    use crate::wrap::FixedCharWidth;
    use crate::{DisplayMapBuilder, WrapConfig};

    fn cold_row_counts(text: &str, revision: u64, wrap_width_dip: u32) -> Vec<u16> {
        let snap = RopeSnapshot::new(Arc::new(Rope::from_str(text)), Revision(revision));
        let decos = Decorations::empty(revision);
        let mut measure = FixedCharWidth::new(8.0);
        let wrap = if wrap_width_dip == 0 {
            WrapConfig::NONE
        } else {
            WrapConfig::new(wrap_width_dip)
        };
        let index = DisplayMapBuilder::new(&snap, &decos, &[], &[], wrap)
            .compute_row_index_with_stats(&mut measure, None)
            .expect("cold row index");
        index.row_counts().to_vec()
    }

    fn splice_row_counts(
        prev_text: &str,
        prev_revision: u64,
        new_text: &str,
        new_revision: u64,
        deltas: &[RopeEditDelta],
        wrap_width_dip: u32,
    ) -> Option<Vec<u16>> {
        let prev_counts = cold_row_counts(prev_text, prev_revision, wrap_width_dip);
        let prev_snap =
            RopeSnapshot::new(Arc::new(Rope::from_str(prev_text)), Revision(prev_revision));
        let prev_decos = Decorations::empty(prev_revision);
        let wrap = if wrap_width_dip == 0 {
            WrapConfig::NONE
        } else {
            WrapConfig::new(wrap_width_dip)
        };
        let prev_index = DisplayMapBuilder::new(&prev_snap, &prev_decos, &[], &[], wrap)
            .compute_row_index_with_stats(&mut FixedCharWidth::new(8.0), None)
            .expect("prev row index");
        assert_eq!(prev_index.row_counts(), prev_counts.as_slice());

        let new_snap =
            RopeSnapshot::new(Arc::new(Rope::from_str(new_text)), Revision(new_revision));
        let new_decos = Decorations::empty(new_revision);
        let mut measure = FixedCharWidth::new(8.0);
        DisplayMapBuilder::new(&new_snap, &new_decos, &[], &[], wrap)
            .splice_row_index_forward(&prev_index, deltas, &mut measure)
            .expect("splice ok")
            .map(|(arc, _)| arc.row_counts().to_vec())
    }

    #[test]
    fn splice_within_line_insert_matches_cold_walk() {
        let prev = "alpha\nbeta\ngamma";
        let new = "alXpha\nbeta\ngamma";
        let deltas = [RopeEditDelta::insert(2, 1)];
        let spliced = splice_row_counts(prev, 1, new, 2, &deltas, 0).expect("Lines path");
        let cold = cold_row_counts(new, 2, 0);
        assert_eq!(spliced, cold);
    }

    #[test]
    fn splice_single_newline_insert_matches_cold_walk() {
        let prev = "alpha\nbeta\ngamma";
        let new = "al\npha\nbeta\ngamma";
        let deltas = [RopeEditDelta::insert(2, 1)];
        let spliced = splice_row_counts(prev, 1, new, 2, &deltas, 0).expect("Splice path");
        let cold = cold_row_counts(new, 2, 0);
        assert_eq!(spliced, cold);
    }

    #[test]
    fn splice_multiline_paste_matches_cold_walk() {
        let prev = "alpha\nbeta\ngamma";
        let new = "alPP\nQQ\nRRpha\nbeta\ngamma";
        let deltas = [RopeEditDelta::insert(2, 8)];
        let spliced = splice_row_counts(prev, 1, new, 2, &deltas, 0).expect("multi-line splice");
        let cold = cold_row_counts(new, 2, 0);
        assert_eq!(spliced, cold);
    }

    #[test]
    fn splice_single_newline_delete_matches_cold_walk() {
        let prev = "alpha\nbeta\ngamma";
        let new = "alphabeta\ngamma";
        let deltas = [RopeEditDelta::delete(5, 1)];
        let spliced = splice_row_counts(prev, 1, new, 2, &deltas, 0).expect("Splice delete");
        let cold = cold_row_counts(new, 2, 0);
        assert_eq!(spliced, cold);
    }

    #[test]
    fn splice_multiline_delete_matches_cold_walk() {
        let prev = "alpha\nbeta\ngamma\ndelta";
        let new = "alphalta";
        let deltas = [RopeEditDelta::delete(5, 15)];
        let spliced = splice_row_counts(prev, 1, new, 2, &deltas, 0).expect("multi-line delete");
        let cold = cold_row_counts(new, 2, 0);
        assert_eq!(spliced, cold);
    }

    #[test]
    fn splice_with_soft_wrap_matches_cold_walk() {
        let prev = "one two three four\nshort";
        let new = "one twoX three four\nshort";
        let deltas = [RopeEditDelta::insert(7, 1)];
        let spliced = splice_row_counts(prev, 1, new, 2, &deltas, 64).expect("wrap-enabled splice");
        let cold = cold_row_counts(new, 2, 64);
        assert_eq!(spliced, cold);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn splice_within_line_insert_proptest_matches_cold_walk(
            lines in proptest::collection::vec("[a-zA-Z ]{1,40}", 1..32usize),
            target_line in 0usize..32,
            insert in "[a-zA-Z]{1,4}",
        ) {
            let prev = lines.join("\n");
            let line_idx = target_line % lines.len();
            let line_start: usize = prev
                .split('\n')
                .take(line_idx)
                .map(|l| l.len() + 1)
                .sum();
            let line_end = line_start + lines[line_idx].len();
            let insert_at = line_start + (lines[line_idx].len() / 2).min(line_end - line_start);
            let mut new = String::with_capacity(prev.len() + insert.len());
            new.push_str(&prev[..insert_at]);
            new.push_str(&insert);
            new.push_str(&prev[insert_at..]);
            let deltas = [RopeEditDelta::insert(insert_at, insert.len())];
            let spliced = splice_row_counts(&prev, 1, &new, 2, &deltas, 0)
                .expect("within-line splice should succeed");
            let cold = cold_row_counts(&new, 2, 0);
            prop_assert_eq!(spliced, cold);
        }

        #[test]
        fn splice_newline_insert_proptest_matches_cold_walk(
            lines in proptest::collection::vec("[a-z]{1,30}", 1..24usize),
            target_line in 0usize..24,
            wrap_width in proptest::sample::select(vec![0u32, 64, 128]),
        ) {
            let prev = lines.join("\n");
            let line_idx = target_line % lines.len();
            let line_start: usize = prev
                .split('\n')
                .take(line_idx)
                .map(|l| l.len() + 1)
                .sum();
            let split_at = line_start + lines[line_idx].len() / 2;
            let mut new = String::with_capacity(prev.len() + 1);
            new.push_str(&prev[..split_at]);
            new.push('\n');
            new.push_str(&prev[split_at..]);
            let deltas = [RopeEditDelta::insert(split_at, 1)];
            let spliced = splice_row_counts(&prev, 1, &new, 2, &deltas, wrap_width)
                .expect("single-newline splice should succeed");
            let cold = cold_row_counts(&new, 2, wrap_width);
            prop_assert_eq!(spliced, cold);
        }
    }
}
