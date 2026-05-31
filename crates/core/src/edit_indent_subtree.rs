//! §H3 / §H3a — indent-subtree range computation.
//!
//! The "indent subtree" of a line `n` is the half-open line range
//! `[n, end)` covering `n` plus every following line whose leading
//! indent is **deeper than** `n`'s. Blank lines are absorbed: they
//! belong to the subtree only when followed by a line still indented
//! deeper than `n`. The first line with indent `<= n` ends the
//! subtree.
//!
//! Two consumers (this module ships the analysis side; the planner
//! `SelectionEdit::MoveIndentSubtreeUp/Down` wiring is queued for a
//! follow-up):
//! - §H3 folding: the foldable region geometry.
//! - §H3a `MoveIndentSubtreeUp/Down`: swap the subtree with its
//!   nearest equal-or-shallower sibling.
//!
//! The `Buffer` ↔ `EditOp` glue lives upstream of this module so the
//! analysis stays pure-function over `&Rope`.

use ropey::Rope;

/// Half-open source-line range `[start_line, end_line)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndentRange {
    /// First line (inclusive).
    pub start_line: u32,
    /// Last-line-exclusive end.
    pub end_line: u32,
}

impl IndentRange {
    /// `true` when this range spans only a single line.
    #[must_use]
    pub(crate) fn is_single_line(&self) -> bool {
        self.end_line.saturating_sub(self.start_line) <= 1
    }
}

/// Leading-indent column of `line` (tabs counted as 4 columns).
/// Blank lines return `u32::MAX` so they never terminate a subtree.
#[must_use]
pub fn line_indent(rope: &Rope, line: u32) -> u32 {
    let total = rope.len_lines();
    if (line as usize) >= total {
        return 0;
    }
    let start = rope.line_to_byte(line as usize);
    let end = if (line as usize) + 1 < total {
        rope.line_to_byte((line as usize) + 1)
    } else {
        rope.len_bytes()
    };
    let slice = rope.byte_slice(start..end);
    let mut indent = 0u32;
    let mut any_non_ws = false;
    for ch in slice.chars() {
        match ch {
            ' ' => indent += 1,
            '\t' => indent += 4,
            '\r' | '\n' => break,
            _ => {
                any_non_ws = true;
                break;
            }
        }
    }
    if any_non_ws {
        indent
    } else {
        u32::MAX
    }
}

/// Compute the indent subtree of `line`. Returns `None` when `line`
/// is past the buffer's last source line.
///
/// The subtree includes `line` itself plus every following line whose
/// indent is strictly deeper than `line`'s. Blank lines are absorbed.
#[must_use]
pub fn indent_subtree(rope: &Rope, line: u32) -> Option<IndentRange> {
    let total = u32::try_from(rope.len_lines()).ok()?;
    if total == 0 || line >= total {
        return None;
    }
    let base = line_indent(rope, line);
    if base == u32::MAX {
        return Some(IndentRange {
            start_line: line,
            end_line: line + 1,
        });
    }
    let mut end = line + 1;
    while end < total {
        let i = line_indent(rope, end);
        if i == u32::MAX {
            // Blank line — peek forward. If the next non-blank line
            // is still deeper than base, absorb; else stop.
            let mut peek = end + 1;
            while peek < total && line_indent(rope, peek) == u32::MAX {
                peek += 1;
            }
            if peek < total && line_indent(rope, peek) > base {
                end = peek + 1;
                continue;
            }
            break;
        }
        if i <= base {
            break;
        }
        end += 1;
    }
    Some(IndentRange {
        start_line: line,
        end_line: end,
    })
}

/// Find the previous sibling of the subtree starting at `line`. A
/// sibling is the closest preceding line with indent equal to or
/// shallower than `line`'s. Returns the `IndentRange` of that
/// sibling, or `None` if no sibling exists at or above this depth.
#[must_use]
pub fn previous_sibling_subtree(rope: &Rope, line: u32) -> Option<IndentRange> {
    if line == 0 {
        return None;
    }
    let base = line_indent(rope, line);
    if base == u32::MAX {
        return None;
    }
    let mut probe = line;
    while probe > 0 {
        probe -= 1;
        let i = line_indent(rope, probe);
        if i == u32::MAX {
            continue;
        }
        if i <= base {
            return indent_subtree(rope, probe);
        }
    }
    None
}

/// Find the next sibling of the subtree starting at `line`. Returns
/// `None` when the subtree is the last one at its depth.
#[must_use]
pub fn next_sibling_subtree(rope: &Rope, line: u32) -> Option<IndentRange> {
    let me = indent_subtree(rope, line)?;
    let total = u32::try_from(rope.len_lines()).ok()?;
    let base = line_indent(rope, line);
    if base == u32::MAX {
        return None;
    }
    let mut probe = me.end_line;
    while probe < total {
        let i = line_indent(rope, probe);
        if i == u32::MAX {
            probe += 1;
            continue;
        }
        if i <= base {
            return indent_subtree(rope, probe);
        }
        probe += 1;
    }
    None
}

/// Enumerate every top-level (column-0) indent subtree in the buffer.
/// Useful for §H3 `view.fold_all`.
#[must_use]
pub fn all_top_level_subtrees(rope: &Rope) -> Vec<IndentRange> {
    let total = match u32::try_from(rope.len_lines()) {
        Ok(n) => n,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut i = 0u32;
    while i < total {
        let indent = line_indent(rope, i);
        if indent != 0 {
            i += 1;
            continue;
        }
        if let Some(r) = indent_subtree(rope, i) {
            if !r.is_single_line() {
                out.push(r);
            }
            i = r.end_line.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn r(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn line_indent_counts_spaces_and_tabs() {
        let rope = r("    hello\n\tworld\nnone\n");
        assert_eq!(line_indent(&rope, 0), 4);
        assert_eq!(line_indent(&rope, 1), 4);
        assert_eq!(line_indent(&rope, 2), 0);
    }

    #[test]
    fn line_indent_blank_lines_return_max() {
        let rope = r("foo\n\n   \nbar\n");
        assert_eq!(line_indent(&rope, 0), 0);
        assert_eq!(line_indent(&rope, 1), u32::MAX);
        assert_eq!(line_indent(&rope, 2), u32::MAX);
        assert_eq!(line_indent(&rope, 3), 0);
    }

    #[test]
    fn indent_subtree_single_line_when_no_deeper_lines() {
        let rope = r("alpha\nbeta\ngamma\n");
        let r = indent_subtree(&rope, 1).unwrap();
        assert_eq!(r.start_line, 1);
        assert_eq!(r.end_line, 2);
        assert!(r.is_single_line());
    }

    #[test]
    fn indent_subtree_absorbs_deeper_descendants() {
        let rope = r("parent\n  child a\n  child b\n    grandchild\n  child c\nsibling\n");
        let r = indent_subtree(&rope, 0).unwrap();
        assert_eq!(r.start_line, 0);
        assert_eq!(r.end_line, 5);
    }

    #[test]
    fn indent_subtree_terminates_on_shallower_or_equal_indent() {
        // line 0 indent 4, line 1 deeper (8), line 2 shallower (2).
        // Subtree of line 0 must include line 1 and stop at line 2.
        let rope = r("    a\n        deeper\n  shallower\n    d\n");
        let r = indent_subtree(&rope, 0).unwrap();
        assert_eq!(r.end_line, 2);
    }

    #[test]
    fn indent_subtree_absorbs_blank_lines_inside_subtree() {
        let rope = r("parent\n  child\n\n  child2\nsibling\n");
        let r = indent_subtree(&rope, 0).unwrap();
        assert_eq!(r.end_line, 4);
    }

    #[test]
    fn indent_subtree_past_eof_returns_none() {
        let rope = r("only\n");
        assert!(indent_subtree(&rope, 5).is_none());
    }

    #[test]
    fn next_sibling_finds_following_equal_depth_line() {
        let rope = r("first\n  child\nsecond\n  child2\nthird\n");
        let s = next_sibling_subtree(&rope, 0).unwrap();
        assert_eq!(s.start_line, 2);
    }

    #[test]
    fn previous_sibling_finds_prior_equal_depth_line() {
        let rope = r("first\n  child\nsecond\n  child2\nthird\n");
        let s = previous_sibling_subtree(&rope, 2).unwrap();
        assert_eq!(s.start_line, 0);
    }

    #[test]
    fn previous_sibling_returns_none_at_top() {
        let rope = r("first\n  child\nsecond\n");
        assert!(previous_sibling_subtree(&rope, 0).is_none());
    }

    #[test]
    fn next_sibling_returns_none_at_bottom() {
        let rope = r("first\nsecond\nlast\n");
        assert!(next_sibling_subtree(&rope, 2).is_none());
    }

    #[test]
    fn all_top_level_subtrees_returns_each_block() {
        let rope = r("alpha\n  a1\n  a2\nbeta\n  b1\ngamma\n");
        let subs = all_top_level_subtrees(&rope);
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].start_line, 0);
        assert_eq!(subs[0].end_line, 3);
        assert_eq!(subs[1].start_line, 3);
        assert_eq!(subs[1].end_line, 5);
    }
}
