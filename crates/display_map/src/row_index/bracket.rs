//! ε.3F+ bracket-splice classifier for multi-delta chains that drift
//! the document's line count. Used by [`super::dirty`] when the
//! single-delta fast paths bail (chain has more than one delta or the
//! shapes don't match).
//!
//! The bracket computes the smallest contiguous post-edit byte range
//! that contains every delta in the chain, maps it to a contiguous
//! post-edit source-line range, and emits one [`RowSplice`] covering
//! the whole region — strictly more dirty lines than a per-delta
//! minimum but still bounded by the actual edited region, which keeps
//! realisation O(visible window) even when the bracket spans thousands
//! of source lines.

use continuity_text::RopeEditDelta;
use ropey::Rope;

use super::splice::RowSplice;

/// ε.3F+ (2026-05-17) — bracket-splice classifier for multi-delta
/// chains that drift the line count. Computes the smallest contiguous
/// post-edit byte range that contains every delta in the chain, maps
/// it to a contiguous post-edit source-line range, and translates the
/// pre↔post line-count delta into a single [`RowSplice`].
///
/// Returns `None` (caller falls back to `FullRebuild`) when:
/// - Any pair of deltas overlaps in a way that
///   [`map_position_through_chain`] can't unambiguously resolve.
/// - The resulting splice would remove fewer than one pre-edit slot
///   (i.e. line-count drift exactly matches the bracketed post line
///   count — appendable-only splice the in-place row index doesn't
///   currently model).
/// - The pre-edit slot range the splice would replace extends past
///   the pre-edit source-line count.
///
/// The bracket is wider than the single-delta fast paths above
/// produce (the dirty list covers every post-edit line in
/// `[post_min_line..=post_max_line]`), but every line outside the
/// bracket is still reused from `prev` — this stays O(visible window)
/// for the realisation step even when the bracket spans thousands of
/// source lines.
pub(super) fn bracket_splice(
    deltas: &[RopeEditDelta],
    rope_after: &Rope,
    old_lines: u32,
    new_lines: u32,
) -> Option<RowSplice> {
    if deltas.is_empty() {
        return None;
    }
    let rope_len = rope_after.len_bytes();
    let mut min_post: usize = usize::MAX;
    let mut max_post: usize = 0;
    for (i, delta) in deltas.iter().enumerate() {
        let start = map_position_through_chain(deltas, i, delta.at)?;
        // For pure deletes (`inserted_bytes == 0`) the end-of-insert
        // collapses to `start`. For inserts/replaces it brackets the
        // newly-inserted bytes in post-of-all coords.
        let end = map_position_through_chain(deltas, i, delta.at + delta.inserted_bytes)?;
        let lo = start.min(end).min(rope_len);
        let hi = start.max(end).min(rope_len);
        if lo < min_post {
            min_post = lo;
        }
        if hi > max_post {
            max_post = hi;
        }
    }
    if min_post == usize::MAX {
        return None;
    }
    let post_min_line = rope_after.byte_to_line(min_post) as u32;
    let post_max_line = rope_after.byte_to_line(max_post) as u32;
    let line_diff = new_lines as i64 - old_lines as i64;
    let post_count = post_max_line as i64 - post_min_line as i64 + 1;
    let removed = post_count - line_diff;
    // A bracket splice must replace at least one pre-edit slot —
    // otherwise the dirty list would touch lines that have no
    // counterpart in `prev` and the realisation would have to
    // materialise content the splice contract doesn't promise.
    if removed < 1 {
        return None;
    }
    let at = post_min_line;
    if (at as u64) + (removed as u64) > old_lines as u64 {
        return None;
    }
    let inserted = post_count as u32;
    let dirty: Vec<u32> = (post_min_line..=post_max_line).collect();
    Some(RowSplice {
        at,
        removed: removed as u32,
        inserted,
        dirty,
    })
}

/// Walk `deltas` from `from_index + 1` to the end and translate
/// `pos` (a byte position expressed in post-of-`deltas[..=from_index]`
/// coordinates) into post-of-all-deltas coordinates.
///
/// Returns `None` if any later delta overlaps `pos` in a way the
/// bracket classifier can't safely resolve — that bails out to
/// `FullRebuild` rather than emitting an ambiguous splice.
fn map_position_through_chain(
    deltas: &[RopeEditDelta],
    from_index: usize,
    pos: usize,
) -> Option<usize> {
    let mut current = pos as isize;
    for delta in &deltas[(from_index + 1)..] {
        let at = delta.at as isize;
        let removed_end = at + delta.removed_bytes as isize;
        if removed_end <= current {
            // delta operates entirely below `current` — its inserted
            // bytes push everything above by `shift()`.
            current += delta.shift();
        } else if at >= current {
            // delta operates entirely above `current` — no shift.
        } else {
            // overlap: later delta touches the byte we're tracking.
            return None;
        }
    }
    if current < 0 {
        return None;
    }
    Some(current as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row_index::dirty::RowDirty;
    use crate::row_index::{DisplayRowIndex, IndexStamps};

    fn stamps() -> IndexStamps {
        IndexStamps::default()
    }

    #[test]
    fn dirty_after_multidelta_insert_chain_returns_bracket_splice() {
        // ε.3F+ (2026-05-17): two single-byte newline inserts. Pre
        // rope "ab\nc" (2 lines). First insert at pre-byte 1 ('\n')
        // → "a\nb\nc". Second insert at post-of-d0 byte 3 ('\n'
        // before 'c') → "a\nb\n\nc" (4 lines). The bracket
        // classifier finds the post-edit byte range [1, 4] covers
        // both deltas, maps to post lines [0, 2], and emits one
        // splice: replace pre line 0 with post lines 0/1/2; pre
        // line 1 ("c") reused as post line 3.
        let index = DisplayRowIndex::from_row_counts(vec![1, 1], stamps());
        let rope_after = ropey::Rope::from_str("a\nb\n\nc");
        let deltas = [RopeEditDelta::insert(1, 1), RopeEditDelta::insert(3, 1)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Splice(splice) => {
                assert_eq!(splice.at, 0);
                assert_eq!(splice.removed, 1);
                assert_eq!(splice.inserted, 3);
                assert_eq!(splice.dirty, vec![0, 1, 2]);
                assert_eq!(splice.line_delta(), 2);
            }
            other => panic!("expected bracket Splice, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_multidelta_chain_within_same_pre_line_brackets_to_one_slot() {
        // ε.3F+ Enter-burst on a small buffer: pre "abc" (1 line).
        // Insert '\n' at byte 3 (end) → "abc\n" (2 lines). Insert
        // '\n' at byte 4 of post-d0 → "abc\n\n" (3 lines). Bracket:
        // post bytes [3, 5] → post lines [0, 2]. Replaces pre line 0
        // with post lines 0/1/2.
        let index = DisplayRowIndex::from_row_counts(vec![1], stamps());
        let rope_after = ropey::Rope::from_str("abc\n\n");
        let deltas = [RopeEditDelta::insert(3, 1), RopeEditDelta::insert(4, 1)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Splice(splice) => {
                assert_eq!(splice.at, 0);
                assert_eq!(splice.removed, 1);
                assert_eq!(splice.inserted, 3);
                assert_eq!(splice.dirty, vec![0, 1, 2]);
            }
            other => panic!("expected bracket Splice, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_multidelta_mixed_text_and_newline_inserts_brackets() {
        // ε.3F+ realistic typing burst: a within-line char insert
        // followed by an Enter on a multi-line buffer. Pre
        // "alpha\nbeta\n" (3 lines). delta 0: insert "X" at byte 5
        // (after "alpha"). post-d0 = "alphaX\nbeta\n" (3 lines).
        // delta 1: insert '\n' at byte 6 of post-d0 (right after
        // 'X') → "alphaX\n\nbeta\n" (4 lines).
        let index = DisplayRowIndex::from_row_counts(vec![1, 1, 1], stamps());
        let rope_after = ropey::Rope::from_str("alphaX\n\nbeta\n");
        let deltas = [RopeEditDelta::insert(5, 1), RopeEditDelta::insert(6, 1)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Splice(splice) => {
                assert_eq!(splice.at, 0, "splice anchored at the first edited pre line");
                assert_eq!(splice.removed, 1);
                assert_eq!(splice.inserted, 2);
                assert_eq!(splice.dirty, vec![0, 1]);
                assert_eq!(splice.line_delta(), 1);
            }
            other => panic!("expected bracket Splice, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_multidelta_chain_of_three_newline_inserts_returns_splice() {
        // Three rapid Enter presses coalesced into one paint. Pre
        // "abc" (1 line). After three '\n' inserts at end:
        // "abc\n\n\n" (4 lines). Each delta is `insert(at, 1)` in
        // its own post-of-prev frame: 3, 4, 5.
        let index = DisplayRowIndex::from_row_counts(vec![1], stamps());
        let rope_after = ropey::Rope::from_str("abc\n\n\n");
        let deltas = [
            RopeEditDelta::insert(3, 1),
            RopeEditDelta::insert(4, 1),
            RopeEditDelta::insert(5, 1),
        ];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Splice(splice) => {
                assert_eq!(splice.at, 0);
                assert_eq!(splice.removed, 1);
                assert_eq!(splice.inserted, 4);
                assert_eq!(splice.dirty, vec![0, 1, 2, 3]);
                assert_eq!(splice.line_delta(), 3);
            }
            other => panic!("expected bracket Splice, got {other:?}"),
        }
    }

    #[test]
    fn dirty_after_multidelta_delete_chain_returns_bracket_splice() {
        // Two consecutive Backspace-over-newline keystrokes.
        // Pre "a\nb\nc" (3 lines). delta 0: delete byte 3 ('\n') →
        // "a\nbc" (2 lines). delta 1: delete byte 1 ('\n') of
        // post-d0 → "abc" (1 line).
        let index = DisplayRowIndex::from_row_counts(vec![1, 1, 1], stamps());
        let rope_after = ropey::Rope::from_str("abc");
        let deltas = [RopeEditDelta::delete(3, 1), RopeEditDelta::delete(1, 1)];
        match index.dirty_after_rope_edits(&deltas, &rope_after) {
            RowDirty::Splice(splice) => {
                assert_eq!(splice.at, 0);
                assert_eq!(splice.removed, 3);
                assert_eq!(splice.inserted, 1);
                assert_eq!(splice.dirty, vec![0]);
                assert_eq!(splice.line_delta(), -2);
            }
            other => panic!("expected bracket Splice, got {other:?}"),
        }
    }
}
