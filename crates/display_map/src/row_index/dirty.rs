//! Dirty-line classifier — given a chain of rope edit deltas and the
//! post-edit rope, decide which source lines need their row counts
//! recomputed and whether a structural splice is required.
//!
//! Returns [`RowDirty::Lines`] when every delta sits inside an existing
//! source line (no line-count drift); [`RowDirty::Splice`] when the
//! drift can be absorbed by a single contiguous splice (the
//! single-delta fast paths below, or the multi-delta bracket path in
//! [`super::bracket`]); [`RowDirty::FullRebuild`] when neither path can
//! safely handle the chain.

use continuity_text::RopeEditDelta;
use ropey::Rope;

use super::bracket::bracket_splice;
use super::splice::RowSplice;
use super::DisplayRowIndex;

/// Result of [`DisplayRowIndex::dirty_after_rope_edits`]. ε.3's
/// in-place rebuild path takes the dirty source-line list, recomputes
/// row counts for those lines, and reuses the rest of the prefix tree
/// untouched. `FullRebuild` signals the document's line count changed
/// (newline added or removed) — the index cannot be patched in place
/// and the caller falls back to a from-scratch
/// [`crate::DisplayMapBuilder::build_viewport`].
///
/// `Lines` is a sorted, deduplicated `Vec<u32>` of source-line
/// indices. The list shape (vs a contiguous `Range`) lets ε.3D union
/// rope-derived dirty lines with decoration-diff-derived dirty lines
/// without losing precision on discontiguous edits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RowDirty {
    /// Sorted, deduplicated source-line indices to re-realize. Empty
    /// means no edits affected any source line.
    Lines(Vec<u32>),
    /// ε.3F — line-count edit the index can splice in place. Carries
    /// the splice location and the dirty source-line indices (in
    /// post-edit coordinates) the caller must recompute row counts
    /// for after applying the splice.
    Splice(RowSplice),
    /// Document line count changed in a way the splice path cannot
    /// safely handle (multi-line paste, multi-line delete, multi-delta
    /// chain). Caller falls back to a from-scratch
    /// [`crate::DisplayMapBuilder::build_viewport`].
    FullRebuild,
}

impl DisplayRowIndex {
    /// ε.3 — dirty range computation. Walks the supplied rope edit
    /// deltas and returns which source lines have to be re-evaluated
    /// for their row counts (and re-realized as `DisplayLineSpec`s if
    /// they fall inside the viewport).
    ///
    /// Returns [`RowDirty::Lines(range)`] when every delta sits inside
    /// an existing source line — the caller can update row counts for
    /// those lines in place and reuse the index unchanged for every
    /// other line.
    ///
    /// Returns [`RowDirty::Splice`] (ε.3F) when one of these shapes
    /// holds:
    /// - A single insert delta whose inserted text contains `N ≥ 1`
    ///   newlines — splits the source line containing the insertion
    ///   point into `N + 1` post-edit lines (`removed = 1`,
    ///   `inserted = N + 1`, `dirty = at..=at + N`). Covers both
    ///   single-`\n` Enter (`N = 1`) and multi-line paste (`N ≥ 2`).
    /// - A single delete delta with `removed_bytes ≥ 1`,
    ///   `inserted_bytes == 0`, and line count drift `-N` (`N ≥ 1`)
    ///   — collapses `N + 1` pre-edit source lines into 1 post-edit
    ///   line (`removed = N + 1`, `inserted = 1`, `dirty = [at]`).
    ///   Covers single-`\n` Enter-deletion (`N = 1`),
    ///   backspace-over-multi-line-selection, delete-to-end-of-paragraph,
    ///   and multi-line cut.
    ///
    /// Carries enough information for [`Self::splice_rows`] to update
    /// the index in place and for the caller to rebuild only the
    /// dirty / new source lines.
    ///
    /// Returns [`RowDirty::FullRebuild`] for line-count changes the
    /// splice path can't safely handle (nested/overlapping deltas the
    /// bracket cannot resolve, splices that would extend past the
    /// pre-edit row count, or post-edit byte ranges that don't map
    /// back to a contiguous pre-edit source-line range). The caller
    /// falls back to [`crate::DisplayMapBuilder::build_viewport`].
    ///
    /// Multi-delta chains where the line count drifts (rapid Enter
    /// bursts, paste-then-Enter, multi-character typing that crosses
    /// a newline boundary) used to route to `FullRebuild` even when
    /// each delta would individually be splice-able. ε.3F+
    /// (2026-05-17) replaces that bail with a bracket splice — the
    /// classifier computes the post-edit byte range that bounds every
    /// delta in the chain, maps it to a contiguous post-edit
    /// source-line range, and emits one [`RowSplice`] that absorbs
    /// the whole region. Lines outside the bracket keep their
    /// reused-from-prev specs and row counts; lines inside are
    /// materialised fresh.
    ///
    /// An empty `deltas` slice returns `RowDirty::Lines(Vec::new())`.
    #[must_use]
    pub fn dirty_after_rope_edits(&self, deltas: &[RopeEditDelta], rope_after: &Rope) -> RowDirty {
        if deltas.is_empty() {
            return RowDirty::Lines(Vec::new());
        }
        let old_lines = self.source_line_count();
        let new_lines = rope_after.len_lines() as u32;
        if new_lines != old_lines {
            // ε.3F splice fast path. Exactly one delta with a
            // recognised shape:
            //
            //  * **Insert with N ≥ 1 newlines.** The pre-edit source
            //    line containing the insertion point gets split
            //    into N + 1 post-edit lines. `removed = 1`,
            //    `inserted = N + 1`, `dirty = at..=at+N`. Covers
            //    the single-newline Enter case (N = 1, inserted = 2)
            //    and the multi-line paste case (N ≥ 2) — both
            //    consume the same `rebuild_spliced` codepath in
            //    `crates/display_map/src/builder/rebuild_spliced.rs`,
            //    which is already generic over `splice.inserted`.
            //  * **Single-newline delete.** Two adjacent pre-edit
            //    lines merge into one. `removed = 2`, `inserted = 1`.
            //
            // Anything else (multi-line delete, replace that shifts
            // line count, multi-delta chains, mixed insert+delete)
            // routes through FullRebuild for now. Multi-line delete
            // is the obvious next splice slice.
            if deltas.len() == 1 {
                let delta = deltas[0];
                let line_diff = new_lines as i64 - old_lines as i64;
                if line_diff > 0 && delta.removed_bytes == 0 && delta.inserted_bytes >= 1 {
                    // Insert-only: count newlines in the inserted
                    // byte range via post-edit `byte_to_line`. With
                    // `removed_bytes == 0` the line-count drift
                    // must come entirely from `\n` characters in
                    // the inserted slice, so the byte-to-line
                    // delta equals the newline count. The
                    // `line_diff == newlines_in_paste` check below
                    // is belt-and-suspenders: if they disagree, a
                    // hidden assumption is broken and FullRebuild
                    // is the safe answer.
                    let len = rope_after.len_bytes();
                    let at_byte = delta.at.min(len);
                    let end_byte = (delta.at + delta.inserted_bytes).min(len);
                    if at_byte <= end_byte {
                        let start_line = rope_after.byte_to_line(at_byte) as i64;
                        let end_line = rope_after.byte_to_line(end_byte) as i64;
                        let newlines_in_paste = end_line - start_line;
                        if newlines_in_paste == line_diff
                            && newlines_in_paste >= 1
                            && (start_line as u32) < old_lines
                        {
                            let at = start_line as u32;
                            let inserted = (newlines_in_paste as u32) + 1;
                            let dirty: Vec<u32> = (at..at + inserted).collect();
                            return RowDirty::Splice(RowSplice {
                                at,
                                removed: 1,
                                inserted,
                                dirty,
                            });
                        }
                    }
                } else if line_diff < 0 && delta.removed_bytes >= 1 && delta.inserted_bytes == 0 {
                    // Delete-only that collapses N + 1 pre-edit
                    // source lines into 1 post-edit line, where
                    // `N = -line_diff` is the number of `\n` bytes
                    // in the removed range. `merged_line` is the
                    // post-edit source-line index that holds the
                    // result: in post coordinates, byte `delta.at`
                    // is the first byte past the deletion (whatever
                    // used to follow the deletion is now merged
                    // onto the prior content).
                    //
                    // N = 1 is the single-`\n` Enter-deletion case
                    // (`removed = 2, inserted = 1`); N ≥ 2 covers
                    // backspace-over-multi-line-selection,
                    // delete-to-end-of-paragraph, multi-line cut.
                    let n = (-line_diff) as u32;
                    let merged_line =
                        rope_after.byte_to_line(delta.at.min(rope_after.len_bytes())) as u32;
                    // Belt-and-suspenders: the splice contract
                    // requires `merged_line + (N + 1) ≤ old_lines`.
                    // Algebra makes this always true (proof: at ≤
                    // new_lines - 1, old_lines = new_lines + N), but
                    // verify rather than reason in case `byte_to_line`
                    // returns something unexpected at EOF / on the
                    // synthetic trailing empty line. Falling through
                    // to FullRebuild on mismatch is the safe answer.
                    if (merged_line as u64) + (n as u64) < old_lines as u64 {
                        return RowDirty::Splice(RowSplice {
                            at: merged_line,
                            removed: n + 1,
                            inserted: 1,
                            dirty: vec![merged_line],
                        });
                    }
                }
            }
            // ε.3F+ (2026-05-17): multi-delta bracket splice. The
            // single-delta fast paths above produce a *minimal*
            // splice (dirty list is exactly the affected lines); the
            // bracket path widens the splice to a single contiguous
            // post-edit source-line range that covers every delta in
            // the chain. That's strictly more dirty lines than the
            // minimum, but still bounded by the actual edited
            // region — orders of magnitude smaller than the
            // whole-document cold rebuild the pre-ε.3F+ classifier
            // forced for `deltas.len() ≥ 2`.
            if let Some(splice) = bracket_splice(deltas, rope_after, old_lines, new_lines) {
                return RowDirty::Splice(splice);
            }
            return RowDirty::FullRebuild;
        }
        // Canonicalize delta order: descending by `at` so that walking
        // forward maps each delta's `at` into post-all-deltas
        // coordinates correctly regardless of how the caller
        // assembled the chain.
        let mut ordered: Vec<RopeEditDelta> = deltas.to_vec();
        ordered.sort_by(|a, b| b.at.cmp(&a.at));

        let mut dirty: Vec<u32> = Vec::new();
        let mut byte_shift: isize = 0;
        let len = rope_after.len_bytes();
        for delta in &ordered {
            let post_at = ((delta.at as isize) + byte_shift).max(0) as usize;
            let post_end =
                ((delta.at + delta.inserted_bytes) as isize + byte_shift).max(0) as usize;
            byte_shift += delta.shift();
            let clamped_at = post_at.min(len);
            let clamped_end = post_end.min(len);
            let line_lo = rope_after.byte_to_line(clamped_at) as u32;
            let line_hi = rope_after.byte_to_line(clamped_end) as u32;
            let line_count = self.source_line_count();
            for line in line_lo..=line_hi {
                if line < line_count {
                    dirty.push(line);
                }
            }
        }
        dirty.sort_unstable();
        dirty.dedup();
        debug_assert!(
            dirty.windows(2).all(|w| w[0] < w[1]),
            "dirty_after_rope_edits must return sorted, deduplicated entries",
        );
        RowDirty::Lines(dirty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row_index::IndexStamps;

    fn stamps() -> IndexStamps {
        IndexStamps::default()
    }

    #[test]
    fn dirty_after_no_deltas_returns_empty() {
        let index = DisplayRowIndex::from_row_counts(vec![1, 1], stamps());
        let rope = ropey::Rope::from_str("a\nb");
        assert_eq!(
            index.dirty_after_rope_edits(&[], &rope),
            RowDirty::Lines(Vec::new())
        );
    }

    #[test]
    fn dirty_after_within_line_edit_is_single_line() {
        // Buffer "ab\ncd" → 2 lines. Edit at byte 1 inserts 'X':
        // "aXb\ncd". Source line 0 changes; line count unchanged.
        let index = DisplayRowIndex::from_row_counts(vec![1, 1], stamps());
        let rope_after = ropey::Rope::from_str("aXb\ncd");
        let deltas = [RopeEditDelta::insert(1, 1)];
        assert_eq!(
            index.dirty_after_rope_edits(&deltas, &rope_after),
            RowDirty::Lines(vec![0])
        );
    }

    #[test]
    fn dirty_after_single_newline_insert_returns_splice() {
        // ε.3F: single `\n` insert at byte 1 splits source line 0
        // into two halves. The splice replaces the old slot for
        // line 0 with two fresh slots, and the dirty list covers
        // both new lines.
        let index = DisplayRowIndex::from_row_counts(vec![1, 1], stamps());
        let rope_after = ropey::Rope::from_str("a\nb\nc");
        let deltas = [RopeEditDelta::insert(1, 1)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Splice(splice) => {
                assert_eq!(splice.at, 0);
                assert_eq!(splice.removed, 1);
                assert_eq!(splice.inserted, 2);
                assert_eq!(splice.dirty, vec![0, 1]);
                assert_eq!(splice.line_delta(), 1);
            }
            other => panic!("expected Splice, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_single_newline_delete_returns_splice() {
        // ε.3F: deleting the `\n` between line 0 and line 1 merges
        // them. Splice removes two slots, inserts one, dirty list
        // covers the merged line.
        let index = DisplayRowIndex::from_row_counts(vec![1, 1, 1], stamps());
        let rope_after = ropey::Rope::from_str("ab\nc");
        let deltas = [RopeEditDelta::delete(1, 1)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Splice(splice) => {
                assert_eq!(splice.at, 0);
                assert_eq!(splice.removed, 2);
                assert_eq!(splice.inserted, 1);
                assert_eq!(splice.dirty, vec![0]);
                assert_eq!(splice.line_delta(), -1);
            }
            other => panic!("expected Splice, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_multiline_paste_returns_splice_with_n_plus_one_inserted() {
        // ε.3F multi-line-paste extension: an insert with N
        // newlines splits the line containing the insertion point
        // into N + 1 post-edit lines. Pre = "ab\nc" (2 lines).
        // Insert "XY\nZ\n" at byte 1 → post = "aXY\nZ\nb\nc"
        // (4 lines). That's 2 newlines in the paste plus the
        // remainder of the original line 0 still owning post-line 2.
        let index = DisplayRowIndex::from_row_counts(vec![1, 1], stamps());
        let rope_after = ropey::Rope::from_str("aXY\nZ\nb\nc");
        let deltas = [RopeEditDelta::insert(1, 5)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Splice(splice) => {
                assert_eq!(splice.at, 0);
                assert_eq!(splice.removed, 1);
                assert_eq!(splice.inserted, 3);
                assert_eq!(splice.dirty, vec![0, 1, 2]);
                assert_eq!(splice.line_delta(), 2);
            }
            other => panic!("expected Splice, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_multiline_delete_returns_splice_with_n_plus_one_removed() {
        // ε.3F multi-line-delete extension. Pre = "a\nb\nc\nd"
        // (4 lines). Delete bytes 2..6 ("b\nc\n", 2 newlines).
        // Post = "a\nd" (2 lines). N = 2 newlines deleted, so
        // splice removes 3 pre-edit slots and inserts 1.
        let index = DisplayRowIndex::from_row_counts(vec![1, 1, 1, 1], stamps());
        let rope_after = ropey::Rope::from_str("a\nd");
        let deltas = [RopeEditDelta::delete(2, 4)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Splice(splice) => {
                assert_eq!(splice.at, 1);
                assert_eq!(splice.removed, 3);
                assert_eq!(splice.inserted, 1);
                assert_eq!(splice.dirty, vec![1]);
                assert_eq!(splice.line_delta(), -2);
            }
            other => panic!("expected Splice, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_multiline_delete_collapsing_to_single_line_returns_splice() {
        // Whole-buffer multi-line delete: Pre = "abc\ndef\nghi"
        // (3 lines). Delete bytes 2..8 ("c\ndef\n", 2 newlines).
        // Post = "abghi" (1 line). All 3 pre-edit slots collapse
        // into one.
        let index = DisplayRowIndex::from_row_counts(vec![1, 1, 1], stamps());
        let rope_after = ropey::Rope::from_str("abghi");
        let deltas = [RopeEditDelta::delete(2, 6)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Splice(splice) => {
                assert_eq!(splice.at, 0);
                assert_eq!(splice.removed, 3);
                assert_eq!(splice.inserted, 1);
                assert_eq!(splice.dirty, vec![0]);
                assert_eq!(splice.line_delta(), -2);
            }
            other => panic!("expected Splice, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_multiline_delete_at_eof_returns_splice() {
        // EOF-adjacent multi-line delete: Pre = "a\nb\nc\nd\n"
        // (5 lines including the synthetic trailing empty). Delete
        // bytes 2..8 ("b\nc\nd\n", 3 newlines). Post = "a\n"
        // (2 lines: "a", ""). N = 3 newlines removed; 4 slots
        // collapse into 1 at post-line 1.
        let index = DisplayRowIndex::from_row_counts(vec![1, 1, 1, 1, 1], stamps());
        let rope_after = ropey::Rope::from_str("a\n");
        let deltas = [RopeEditDelta::delete(2, 6)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Splice(splice) => {
                assert_eq!(splice.at, 1);
                assert_eq!(splice.removed, 4);
                assert_eq!(splice.inserted, 1);
                assert_eq!(splice.dirty, vec![1]);
                assert_eq!(splice.line_delta(), -3);
            }
            other => panic!("expected Splice, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_single_line_delete_within_a_line_stays_on_lines_path() {
        // Sanity: a within-line delete (no newline removed) keeps
        // the per-line Lines path, not Splice.
        let index = DisplayRowIndex::from_row_counts(vec![1, 1], stamps());
        let rope_after = ropey::Rope::from_str("ac\nde");
        let deltas = [RopeEditDelta::delete(1, 1)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Lines(lines) => {
                assert!(lines.contains(&0));
            }
            other => panic!("expected Lines, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_multiline_edit_spans_affected_lines() {
        let index = DisplayRowIndex::from_row_counts(vec![1, 1], stamps());
        let rope_after = ropey::Rope::from_str("aBCD\ncd");
        let deltas = [RopeEditDelta::replace(1, 1, 3)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Lines(lines) => {
                assert!(lines.contains(&0));
            }
            other => panic!("expected in-place dirty list, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_discontiguous_edits_lists_each_affected_line() {
        // Three source lines, two edits hitting lines 0 and 2 (no
        // newlines added). Source line indices land in the dirty
        // list as sorted unique entries.
        let index = DisplayRowIndex::from_row_counts(vec![1, 1, 1], stamps());
        let rope_after = ropey::Rope::from_str("aX\nb\nc Y");
        // Apply two single-byte inserts. Caller may pass any order;
        // the function canonicalizes internally.
        let deltas = [
            RopeEditDelta::insert(1, 1), // line 0
            RopeEditDelta::insert(6, 1), // line 2 in rope_after
        ];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Lines(lines) => {
                assert!(lines.contains(&0));
                assert!(lines.contains(&2));
                assert!(lines.windows(2).all(|w| w[0] < w[1]));
            }
            other => panic!("expected per-line dirty list, got {other:?}"),
        }
    }
}
