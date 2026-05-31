//! §H3 — translate user-toggled source-line indices into half-open
//! source-byte ranges suitable for `continuity_display_map::FoldRange`.
//!
//! Lives in `core` because `display_map` does not depend on `core`'s
//! indent-subtree analysis. The wrapping into the actual `FoldRange`
//! newtype happens at the call site (`crates/ui/src/window_paint.rs`),
//! which has access to both crates.
//!
//! Sentinel: `u32::MAX` in `folded_lines` means "fold all top-level
//! subtrees" — see `Window::fold_all_impl` in
//! `crates/ui/src/window_pane_modes.rs`. We expand it here against the
//! live rope using [`all_top_level_subtrees`].
//!
//! Each fold conceals the *body* of the indent subtree — i.e. lines
//! `header_line + 1 ..= last_descendant`. The header line itself stays
//! visible so the user can see what is folded.

use ropey::Rope;

use crate::edit_indent_subtree::{all_top_level_subtrees, indent_subtree, IndentRange};

/// Half-open source-byte range `[start_byte, end_byte)`.
///
/// `usize` rather than the `display_map` `SourceByte` newtype so this
/// module stays independent of the `display_map` crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndentFoldByteRange {
    /// Inclusive start byte.
    pub start_byte: usize,
    /// Exclusive end byte.
    pub end_byte: usize,
}

/// Compute the byte ranges that should be hidden given the user-toggled
/// set `folded_lines`.
///
/// Semantics:
/// - `u32::MAX` in `folded_lines` is the "fold all" sentinel; it expands
///   to every top-level subtree returned by [`all_top_level_subtrees`].
/// - Indices past the buffer's last line are dropped silently (the rope
///   may have shrunk under a stored fold).
/// - Single-line subtrees (no deeper descendants) are skipped — there is
///   nothing to hide.
/// - The result is coalesced: overlapping or adjacent ranges merge into
///   one, so the display-map builder sees a tidy non-overlapping list.
/// - Output is sorted ascending by `start_byte`.
#[must_use]
pub fn compute_indent_fold_byte_ranges(
    rope: &Rope,
    folded_lines: &[u32],
) -> Vec<IndentFoldByteRange> {
    let total_lines = match u32::try_from(rope.len_lines()) {
        Ok(n) => n,
        Err(_) => return Vec::new(),
    };
    let mut ranges: Vec<IndentFoldByteRange> = Vec::new();
    for &line in folded_lines {
        if line == u32::MAX {
            for sub in all_top_level_subtrees(rope) {
                if let Some(r) = subtree_to_byte_range(rope, sub, total_lines) {
                    ranges.push(r);
                }
            }
            continue;
        }
        if line >= total_lines {
            continue;
        }
        let Some(sub) = indent_subtree(rope, line) else {
            continue;
        };
        if let Some(r) = subtree_to_byte_range(rope, sub, total_lines) {
            ranges.push(r);
        }
    }
    coalesce(&mut ranges);
    ranges
}

/// Translate an `IndentRange` (header-line + body-lines) into a half-open
/// source-byte range covering only the **body** lines. Returns `None`
/// when the subtree has no body (single line) or when arithmetic would
/// overflow.
fn subtree_to_byte_range(
    rope: &Rope,
    sub: IndentRange,
    total_lines: u32,
) -> Option<IndentFoldByteRange> {
    let body_start_line = sub.start_line.checked_add(1)?;
    if body_start_line >= sub.end_line {
        return None;
    }
    let start_byte = rope.line_to_byte(body_start_line as usize);
    let end_byte = if sub.end_line < total_lines {
        rope.line_to_byte(sub.end_line as usize)
    } else {
        rope.len_bytes()
    };
    if start_byte >= end_byte {
        return None;
    }
    Some(IndentFoldByteRange {
        start_byte,
        end_byte,
    })
}

/// Sort then merge overlapping or adjacent ranges in-place.
fn coalesce(ranges: &mut Vec<IndentFoldByteRange>) {
    if ranges.len() <= 1 {
        return;
    }
    ranges.sort_unstable_by_key(|r| r.start_byte);
    let mut write = 0;
    for read in 1..ranges.len() {
        let cur = ranges[read];
        let prev = ranges[write];
        if cur.start_byte <= prev.end_byte {
            ranges[write] = IndentFoldByteRange {
                start_byte: prev.start_byte,
                end_byte: prev.end_byte.max(cur.end_byte),
            };
        } else {
            write += 1;
            ranges[write] = cur;
        }
    }
    ranges.truncate(write + 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn empty_folded_lines_yields_empty() {
        let rope = r("foo\n  bar\n");
        assert!(compute_indent_fold_byte_ranges(&rope, &[]).is_empty());
    }

    #[test]
    fn single_fold_covers_body_lines_only() {
        // "parent\n  child\n  child2\nsibling\n"
        //  ^0     ^7      ^15      ^24
        let rope = r("parent\n  child\n  child2\nsibling\n");
        let ranges = compute_indent_fold_byte_ranges(&rope, &[0]);
        assert_eq!(ranges.len(), 1);
        // Body starts at line 1 = byte 7. Ends at line 3 = byte 24.
        assert_eq!(ranges[0].start_byte, 7);
        assert_eq!(ranges[0].end_byte, 24);
    }

    #[test]
    fn single_line_subtree_is_skipped() {
        let rope = r("alpha\nbeta\ngamma\n");
        // Line 0 has no deeper descendants — nothing to hide.
        assert!(compute_indent_fold_byte_ranges(&rope, &[0]).is_empty());
    }

    #[test]
    fn stale_index_past_eof_is_dropped() {
        let rope = r("foo\n");
        let ranges = compute_indent_fold_byte_ranges(&rope, &[42]);
        assert!(ranges.is_empty());
    }

    #[test]
    fn sentinel_expands_to_all_top_level() {
        // Two top-level subtrees with bodies.
        let rope = r("alpha\n  a1\n  a2\nbeta\n  b1\n");
        let ranges = compute_indent_fold_byte_ranges(&rope, &[u32::MAX]);
        assert_eq!(ranges.len(), 2);
        // Body of "alpha" starts at line 1 (byte 6) and ends at line 3
        // (byte 16). Body of "beta" starts at line 4 (byte 21) and ends
        // at the rope's end (byte 26).
        assert_eq!(ranges[0].start_byte, 6);
        assert_eq!(ranges[0].end_byte, 16);
        assert_eq!(ranges[1].start_byte, 21);
        assert_eq!(ranges[1].end_byte, 26);
    }

    #[test]
    fn duplicate_folds_coalesce() {
        let rope = r("parent\n  child\n  child2\nsibling\n");
        let ranges = compute_indent_fold_byte_ranges(&rope, &[0, 0]);
        assert_eq!(ranges.len(), 1);
    }

    #[test]
    fn overlapping_folds_coalesce() {
        // Fold parent (covers body lines 1..3) and child (covers nothing
        // since "  child" has no deeper descendant). The child entry is
        // dropped as single-line; only the parent remains.
        let rope = r("parent\n  child\n    grandchild\nsibling\n");
        let ranges = compute_indent_fold_byte_ranges(&rope, &[0, 1]);
        assert_eq!(ranges.len(), 1);
        // Parent body: line 1 → end_line 3, bytes [7, 33).
        assert_eq!(ranges[0].start_byte, 7);
    }

    #[test]
    fn fold_at_end_of_buffer_clamps_to_len_bytes() {
        let rope = r("parent\n  child\n  child2\n");
        let ranges = compute_indent_fold_byte_ranges(&rope, &[0]);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].end_byte, rope.len_bytes());
    }

    #[test]
    fn blank_line_in_subtree_is_absorbed() {
        // Body line 2 is blank but followed by deeper-indented line 3,
        // so indent_subtree absorbs it. The fold covers lines 1..=3.
        let rope = r("parent\n  child\n\n  child2\nsibling\n");
        let ranges = compute_indent_fold_byte_ranges(&rope, &[0]);
        assert_eq!(ranges.len(), 1);
        // Body starts at byte 7 (line 1) and ends at byte 25 (line 4).
        assert_eq!(ranges[0].start_byte, 7);
        assert_eq!(ranges[0].end_byte, 25);
    }
}
