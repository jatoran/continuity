//! Compute which pipe tables a selection has "reached past a single
//! cell" — those tables must unrender so the user sees raw markdown
//! (pipes, alignment row, formula source) under the selection.
//!
//! Predicate: a table is suppressed when any selection's ordered
//! (start, end) range covers at least one pipe byte inside the
//! table's `block_range`. Pipe positions are scanned directly from
//! the rope so the result stays correct even when decoration data
//! lags the rope by a revision.
//!
//! Why "pipes" specifically? The intra-cell editing UX is built on
//! selections that span exactly a cell's trimmed content range — no
//! pipe bytes. Any selection touching a pipe (including the alignment
//! row, which is all pipes + dashes) is "wider than a cell" by
//! construction, which is the signal we want.
//!
//! Cheap: O(selections × tables × pipes-per-row); typical buffers
//! have one selection, few tables, and ~10 pipes per row. The render
//! crate caller passes the same byte ranges into both
//! [`crate::table_layout::compute_table_layouts`] (to skip painting
//! the chrome) and the display-map builder (to skip emitting hides).
//!
//! Thread ownership: pure data, callable from the UI thread.

use std::ops::Range;

use continuity_decorate::EvaluatedTable;
use continuity_text::Selection;
use ropey::Rope;

/// Return the `block_range`s of every table in `tables` covered by
/// any selection past the single-cell point. Result is empty when
/// every selection is either a caret (collapsed) or matches a cell's
/// content range exactly.
#[must_use]
pub fn compute_suppressed_table_blocks(
    rope: &Rope,
    selections: &[Selection],
    tables: &[EvaluatedTable],
) -> Vec<Range<usize>> {
    if tables.is_empty() {
        return Vec::new();
    }
    // Pre-compute selection byte ranges once (cheaper than re-deriving
    // per table). Collapsed selections (carets) contribute nothing.
    let selection_byte_ranges: Vec<(usize, usize)> = selections
        .iter()
        .filter_map(|sel| {
            let range = sel.ordered_range();
            let start = range.start.to_byte_offset(rope).ok()?;
            let end = range.end.to_byte_offset(rope).ok()?;
            if start >= end {
                None
            } else {
                Some((start, end))
            }
        })
        .collect();
    if selection_byte_ranges.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    for table in tables {
        if any_selection_covers_pipe(&selection_byte_ranges, rope, &table.block_range) {
            out.push(table.block_range.clone());
        }
    }
    out
}

/// `true` when any selection's ordered range contains at least one
/// `|` byte inside `block_range`. Escaped `\|` sequences don't count
/// (they aren't structural).
fn any_selection_covers_pipe(
    selection_byte_ranges: &[(usize, usize)],
    rope: &Rope,
    block_range: &Range<usize>,
) -> bool {
    // Find every selection that overlaps the block. For each overlap,
    // scan the overlapping bytes for a non-escaped pipe.
    for &(sel_start, sel_end) in selection_byte_ranges {
        let overlap_start = sel_start.max(block_range.start);
        let overlap_end = sel_end.min(block_range.end);
        if overlap_start >= overlap_end {
            continue;
        }
        if rope.try_byte_to_char(overlap_start).is_err()
            || rope.try_byte_to_char(overlap_end).is_err()
        {
            // Decorations transiently lag the rope across a multi-byte
            // edit. Treat misaligned ranges as "no pipe found" — the
            // next frame will re-evaluate against a synced state.
            continue;
        }
        let slice: String = rope.byte_slice(overlap_start..overlap_end).into();
        let bytes = slice.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                i += 2;
                continue;
            }
            if bytes[i] == b'|' {
                return true;
            }
            i += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_text::{Position, SelectionKind};

    fn caret_at(line: u32, col: u32) -> Selection {
        let pos = Position::new(line, col);
        Selection::new(pos, pos, SelectionKind::Caret)
    }

    fn range_sel(start: (u32, u32), end: (u32, u32)) -> Selection {
        Selection::new(
            Position::new(start.0, start.1),
            Position::new(end.0, end.1),
            SelectionKind::Caret,
        )
    }

    fn one_table(block_range: Range<usize>) -> Vec<EvaluatedTable> {
        vec![EvaluatedTable {
            block_range,
            overrides: Vec::new(),
        }]
    }

    #[test]
    fn empty_inputs_yield_empty() {
        let rope = Rope::from_str("");
        let out = compute_suppressed_table_blocks(&rope, &[], &[]);
        assert!(out.is_empty());
    }

    #[test]
    fn caret_only_does_not_suppress() {
        // Caret on a pipe byte is still a collapsed selection (no
        // range to "cover" anything). Must not suppress.
        let src = "| a | b |\n|---|---|\n";
        let rope = Rope::from_str(src);
        let tables = one_table(0..src.len());
        let out = compute_suppressed_table_blocks(&rope, &[caret_at(0, 4)], &tables);
        assert!(out.is_empty(), "caret-only must never suppress");
    }

    #[test]
    fn selection_inside_one_cell_content_does_not_suppress() {
        // `| a | b |` — cell B's trimmed content is byte 6..7 ('b').
        // A selection that fits inside touches no pipe.
        let src = "| a | b |\n|---|---|\n";
        let rope = Rope::from_str(src);
        let tables = one_table(0..src.len());
        let sel = range_sel((0, 6), (0, 7));
        let out = compute_suppressed_table_blocks(&rope, &[sel], &tables);
        assert!(
            out.is_empty(),
            "cell-content-only selection must not suppress, got {out:?}"
        );
    }

    #[test]
    fn selection_crossing_a_pipe_suppresses() {
        // Selection from cell A content into cell B content — crosses
        // the middle `|` at byte 4. Must suppress.
        let src = "| a | b |\n|---|---|\n";
        let rope = Rope::from_str(src);
        let tables = one_table(0..src.len());
        let sel = range_sel((0, 2), (0, 7));
        let out = compute_suppressed_table_blocks(&rope, &[sel], &tables);
        assert_eq!(out.len(), 1, "selection crossing a pipe must suppress");
        assert_eq!(out[0], 0..src.len());
    }

    #[test]
    fn selection_spanning_whole_buffer_suppresses_all_tables() {
        // Ctrl+A equivalent — selection covers everything; both
        // tables suppress.
        let src = "| a | b |\n|---|---|\nbreak\n| c | d |\n|---|---|\n";
        let rope = Rope::from_str(src);
        let tables = vec![
            EvaluatedTable {
                block_range: 0..20,
                overrides: Vec::new(),
            },
            EvaluatedTable {
                block_range: 26..src.len(),
                overrides: Vec::new(),
            },
        ];
        let sel = range_sel((0, 0), (5, 0));
        let out = compute_suppressed_table_blocks(&rope, &[sel], &tables);
        assert_eq!(out.len(), 2, "Ctrl+A must suppress every covered table");
    }

    #[test]
    fn selection_outside_any_table_does_not_suppress() {
        // Selection in plain text — no table overlap, no suppression.
        let src = "before\n| a | b |\n|---|---|\nafter text\n";
        let rope = Rope::from_str(src);
        let tables = one_table(7..27);
        let sel = range_sel((3, 0), (3, 5));
        let out = compute_suppressed_table_blocks(&rope, &[sel], &tables);
        assert!(
            out.is_empty(),
            "non-overlapping selection must not suppress, got {out:?}"
        );
    }

    #[test]
    fn escaped_pipe_inside_selection_does_not_suppress() {
        // `\|` is not a structural pipe — a selection that only
        // covers escaped pipes inside cell content (no unescaped
        // pipes) must not suppress.
        let src = r"| a \| b | c |";
        let rope = Rope::from_str(src);
        let tables = one_table(0..src.len());
        // Select just `a \| b` — the cell content (no unescaped pipe).
        let sel = range_sel((0, 2), (0, 8));
        let out = compute_suppressed_table_blocks(&rope, &[sel], &tables);
        assert!(
            out.is_empty(),
            "selection over escaped-only pipes must not suppress, got {out:?}"
        );
    }
}
