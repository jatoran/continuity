//! §H3 — markdown heading-fold provider.
//!
//! Sibling of [`crate::indent_fold_provider`]. Where indent folds
//! collapse the body of an indent subtree, **heading folds** collapse
//! everything below a heading line up to (but excluding) the next
//! heading at the same or shallower level. A folded `## Section`
//! hides every line until the next `## Other` or `# Top`.
//!
//! Both providers feed the same `PaneModesState.folded_lines: Vec<u32>`
//! set on the ui-side. A line is foldable when **either** an indent
//! subtree exists below it **or** a heading sits on it; the
//! caller (see `crates/ui/src/window_paint.rs`) merges the two
//! providers' outputs into one `Vec<FoldRange>` for the display map,
//! coalescing overlaps.
//!
//! Headings take priority on conflict: a heading subtree is usually
//! larger than any indent subtree starting on the same line, so the
//! merge step uses the heading range when both providers claim the
//! same start byte.
//!
//! Lives in `core` because `display_map` does not depend on `core`'s
//! analysis. The wrapping into `display_map::FoldRange` happens at the
//! ui call site.
//!
//! Input shape:
//! - `headings: &[(line, level)]` — `(u32 line, u8 level)` pairs sorted
//!   ascending by `line`. The caller supplies this from
//!   `continuity_decorate::headings`; this module never depends on
//!   `decorate` so the layer graph stays acyclic.
//! - `folded_lines: &[u32]` — the user-toggled set (same set indent
//!   folds use). `u32::MAX` is the "fold all top-level" sentinel.
//!
//! Sentinel semantics: `u32::MAX` expands to every heading at the
//! shallowest level present (H1 if the doc has any H1, otherwise the
//! smallest `level` number that appears).

use ropey::Rope;

use crate::indent_fold_provider::IndentFoldByteRange;

/// Compute the heading-fold byte ranges from a `folded_lines` set.
///
/// Returns half-open `[start_byte, end_byte)` ranges in source-byte
/// space; the caller is expected to wrap each into a
/// `display_map::FoldRange`.
///
/// - Indices in `folded_lines` that don't match a heading are
///   silently skipped (the indent provider handles them).
/// - The `u32::MAX` sentinel expands to every top-level heading
///   (smallest `level` number present in `headings`).
/// - Headings whose body is empty (the heading is the last line, or
///   the next heading is the immediately-following line) produce no
///   range.
/// - Output is sorted ascending by `start_byte` and coalesced.
#[must_use]
pub fn compute_heading_fold_byte_ranges(
    rope: &Rope,
    headings: &[(u32, u8)],
    folded_lines: &[u32],
) -> Vec<IndentFoldByteRange> {
    let total_lines = match u32::try_from(rope.len_lines()) {
        Ok(n) => n,
        Err(_) => return Vec::new(),
    };
    if headings.is_empty() {
        return Vec::new();
    }
    let mut ranges: Vec<IndentFoldByteRange> = Vec::new();
    let emit = |out: &mut Vec<IndentFoldByteRange>, header_line: u32, end_line: u32| {
        if let Some(r) = heading_subtree_to_byte_range(rope, header_line, end_line, total_lines) {
            out.push(r);
        }
    };

    for &line in folded_lines {
        if line == u32::MAX {
            // Sentinel — fold every top-level heading. "Top level" is
            // the shallowest level actually present (smallest `level`).
            let Some(min_level) = headings.iter().map(|(_, l)| *l).min() else {
                continue;
            };
            for (idx, &(hline, hlevel)) in headings.iter().enumerate() {
                if hlevel != min_level {
                    continue;
                }
                let end_line =
                    next_heading_at_or_shallower_line(headings, idx, hlevel, total_lines);
                emit(&mut ranges, hline, end_line);
            }
            continue;
        }
        // Match the folded line to a heading entry. Headings are
        // ascending-sorted by line; a linear scan is fine for typical
        // doc heading counts (< 200).
        let Some(idx) = headings.iter().position(|&(hline, _)| hline == line) else {
            continue;
        };
        let (hline, hlevel) = headings[idx];
        let end_line = next_heading_at_or_shallower_line(headings, idx, hlevel, total_lines);
        emit(&mut ranges, hline, end_line);
    }
    coalesce(&mut ranges);
    ranges
}

/// First heading after `idx` whose level is at or shallower than
/// `level` (i.e. `<= level` numerically — H1 is shallower than H2).
/// Returns that heading's line, or `total_lines` when no such heading
/// follows (the fold extends to EOF).
fn next_heading_at_or_shallower_line(
    headings: &[(u32, u8)],
    idx: usize,
    level: u8,
    total_lines: u32,
) -> u32 {
    for &(line, lvl) in &headings[idx + 1..] {
        if lvl <= level {
            return line;
        }
    }
    total_lines
}

/// Translate a heading's `(header_line, body_end_line_exclusive)` into
/// a half-open byte range covering the **body** lines only (header
/// stays visible). Returns `None` when there's no body.
fn heading_subtree_to_byte_range(
    rope: &Rope,
    header_line: u32,
    end_line: u32,
    total_lines: u32,
) -> Option<IndentFoldByteRange> {
    let body_start_line = header_line.checked_add(1)?;
    if body_start_line >= end_line {
        return None;
    }
    if header_line >= total_lines {
        return None;
    }
    let start_byte = rope.line_to_byte(body_start_line as usize);
    let end_byte = if end_line < total_lines {
        rope.line_to_byte(end_line as usize)
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
    fn empty_headings_yields_empty() {
        let rope = r("# H1\nbody\n");
        assert!(compute_heading_fold_byte_ranges(&rope, &[], &[0]).is_empty());
    }

    #[test]
    fn empty_folded_lines_yields_empty() {
        let rope = r("# H1\nbody\n");
        assert!(compute_heading_fold_byte_ranges(&rope, &[(0, 1)], &[]).is_empty());
    }

    #[test]
    fn folded_h2_collapses_until_next_same_or_higher_heading() {
        // # H1
        // ## A         <- fold this
        // body A
        // ## B         <- next H2 stops the fold
        // body B
        let text = "# H1\n## A\nbody A\n## B\nbody B\n";
        let rope = r(text);
        let headings = vec![(0u32, 1u8), (1, 2), (3, 2)];
        let ranges = compute_heading_fold_byte_ranges(&rope, &headings, &[1]);
        assert_eq!(ranges.len(), 1);
        // Body is lines 2..3 → bytes [line_to_byte(2), line_to_byte(3)).
        assert_eq!(ranges[0].start_byte, "# H1\n## A\n".len());
        assert_eq!(ranges[0].end_byte, "# H1\n## A\nbody A\n".len());
    }

    #[test]
    fn folded_h2_collapses_to_eof_when_no_more_headings() {
        let text = "# H1\n## A\nbody A\nmore\n";
        let rope = r(text);
        let headings = vec![(0u32, 1u8), (1, 2)];
        let ranges = compute_heading_fold_byte_ranges(&rope, &headings, &[1]);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start_byte, "# H1\n## A\n".len());
        assert_eq!(ranges[0].end_byte, rope.len_bytes());
    }

    #[test]
    fn folded_h1_collapses_until_next_h1() {
        // # A
        // ## sub
        // body
        // # B          <- same-level H1 stops the fold
        // body B
        let text = "# A\n## sub\nbody\n# B\nbody B\n";
        let rope = r(text);
        let headings = vec![(0u32, 1u8), (1, 2), (3, 1)];
        let ranges = compute_heading_fold_byte_ranges(&rope, &headings, &[0]);
        assert_eq!(ranges.len(), 1);
        // Body = lines 1..3 → bytes [4, 17).
        assert_eq!(ranges[0].start_byte, "# A\n".len());
        assert_eq!(ranges[0].end_byte, "# A\n## sub\nbody\n".len());
    }

    #[test]
    fn deeper_heading_does_not_terminate_parent_heading_fold() {
        // Folding the H2 must not stop at the deeper H3.
        let text = "## A\n### deep\nbody\n## B\n";
        let rope = r(text);
        let headings = vec![(0u32, 2u8), (1, 3), (3, 2)];
        let ranges = compute_heading_fold_byte_ranges(&rope, &headings, &[0]);
        assert_eq!(ranges.len(), 1);
        // End line = 3 (next H2). Body bytes = [5, line_to_byte(3)).
        assert_eq!(ranges[0].start_byte, "## A\n".len());
        assert_eq!(ranges[0].end_byte, "## A\n### deep\nbody\n".len());
    }

    #[test]
    fn folded_line_not_a_heading_is_skipped() {
        let text = "# H1\nbody\n";
        let rope = r(text);
        let headings = vec![(0u32, 1u8)];
        // Line 1 isn't a heading — heading provider ignores it.
        // (The indent provider handles non-heading folds.)
        assert!(compute_heading_fold_byte_ranges(&rope, &headings, &[1]).is_empty());
    }

    #[test]
    fn sentinel_expands_to_all_top_level_headings() {
        // Two H1s with bodies, both should fold.
        let text = "# A\nbody A\n# B\nbody B\n";
        let rope = r(text);
        let headings = vec![(0u32, 1u8), (2, 1)];
        let ranges = compute_heading_fold_byte_ranges(&rope, &headings, &[u32::MAX]);
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].start_byte, "# A\n".len());
        assert_eq!(ranges[0].end_byte, "# A\nbody A\n".len());
        assert_eq!(ranges[1].start_byte, "# A\nbody A\n# B\n".len());
        assert_eq!(ranges[1].end_byte, rope.len_bytes());
    }

    #[test]
    fn sentinel_uses_highest_level_when_no_h1() {
        // No H1; H2 is the top level present.
        let text = "## A\nbody A\n## B\nbody B\n";
        let rope = r(text);
        let headings = vec![(0u32, 2u8), (2, 2)];
        let ranges = compute_heading_fold_byte_ranges(&rope, &headings, &[u32::MAX]);
        assert_eq!(ranges.len(), 2);
    }

    #[test]
    fn coalesces_overlapping_heading_folds() {
        // Folding H1 line 0 and H2 line 1 — the H1 fold covers the
        // H2's body already, so the merged set should be one range.
        let text = "# A\n## sub\nbody\n# B\n";
        let rope = r(text);
        let headings = vec![(0u32, 1u8), (1, 2), (3, 1)];
        let ranges = compute_heading_fold_byte_ranges(&rope, &headings, &[0, 1]);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start_byte, "# A\n".len());
        assert_eq!(ranges[0].end_byte, "# A\n## sub\nbody\n".len());
    }

    #[test]
    fn heading_with_empty_body_produces_no_range() {
        // # A immediately followed by # B — no body to hide.
        let text = "# A\n# B\n";
        let rope = r(text);
        let headings = vec![(0u32, 1u8), (1, 1)];
        let ranges = compute_heading_fold_byte_ranges(&rope, &headings, &[0]);
        assert!(ranges.is_empty());
    }
}
