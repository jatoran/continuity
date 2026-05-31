//! Pipe-table visual rendering — display-map hide pass.
//!
//! The visual table renderer (`continuity_render::table_layout` +
//! `continuity_render::table_paint`) draws each cell with its own
//! borders, header background, and column alignment. For that to look
//! like a real table, the source-level `|` characters, formula source
//! bytes, and the `|---|:---:|---:|` alignment row must not appear as
//! glyphs underneath the visual cell chrome.
//!
//! This module produces the per-line list of source byte ranges that
//! the display-map builder should mark `Hidden` so the renderer never
//! lays them out. Hides are applied unconditionally — tables are
//! always rendered, raw pipes are never shown. Cell-aware caret motion
//! commands and the active-cell visual outline let the user edit
//! cells without needing the raw-markdown reveal mode that previously
//! fired on caret-in-block.
//!
//! The alignment row keeps its source-line vertical slot — bytes are
//! hidden but the row still takes one display line, painted by
//! `continuity_render::table_paint`'s alignment-row chrome path
//! (body_bg fill + per-column borders) so the gutter line numbers and
//! source-line ↔ display-line mapping stay 1:1.
//!
//! Thread ownership: pure data; callable from the display-map worker
//! thread that owns the builder.

use std::ops::Range;

use continuity_decorate::{Decorations, EvaluatedTable};

/// Compute the document-absolute byte ranges to hide on the line
/// covering `[line_start, line_end)` so its pipe-table content renders
/// as visual cells.
///
/// Returns an empty vector when no table block intersects the line.
///
/// `line_text` is the line content without trailing newline, matching
/// the slice already prepared by the display-map builder. Pipe
/// characters are detected directly from `line_text` (cheaper than
/// re-walking `decorations.inlines` for `MarkerKind::TablePipe`);
/// escaped `\|` sequences are preserved.
///
/// `suppressed_table_blocks` lists the document-absolute byte ranges
/// (matching `EvaluatedTable.block_range`) of tables that the user
/// has selected across — a Ctrl+A, a multi-line shift+arrow, a drag
/// across rows, etc. Hides are suppressed for those tables so the
/// raw markdown (pipes, alignment row, formula source) renders and
/// the user can see exactly what their selection covers. The render
/// side consults the same list via `compute_table_layouts` to skip
/// painting visual chrome for those tables.
#[must_use]
pub fn compute_table_hidden_ranges_for_line(
    decorations: &Decorations,
    suppressed_table_blocks: &[Range<usize>],
    line_start: usize,
    line_end: usize,
    line_text: &str,
) -> Vec<Range<usize>> {
    if decorations.evaluated_tables.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    for table in &decorations.evaluated_tables {
        if !line_intersects_block(table, line_start, line_end) {
            continue;
        }
        if suppressed_table_blocks
            .iter()
            .any(|r| r.start == table.block_range.start && r.end == table.block_range.end)
        {
            // Selection covers pipes / alignment row of this table —
            // unrender it. Source bytes render as raw markdown so the
            // user sees the selection clearly. F4 swap painter still
            // shows formula values inline at the source-byte position
            // (it's gated on the absence of a `TableLayout`).
            continue;
        }
        if is_delimiter_line(line_text) {
            // Hide the entire alignment-row content. The display line
            // still occupies one line of vertical space; the visual
            // painter draws a styled divider (body_bg + per-column
            // borders) through that slot so the table reads as one
            // continuous bordered region. Source-line ↔ display-line
            // stays 1:1, so line counts and gutter numbering match.
            push_nonempty(&mut out, line_start..line_end);
            continue;
        }
        push_pipe_ranges(&mut out, line_text, line_start);
        push_formula_override_ranges(&mut out, table, line_start, line_end);
    }
    out
}

fn line_intersects_block(table: &EvaluatedTable, line_start: usize, line_end: usize) -> bool {
    table.block_range.start < line_end && table.block_range.end > line_start
}

/// `true` when every non-whitespace byte of `line` is one of `:`, `-`,
/// or `|`, and at least one `-` is present. Mirrors the delimiter-row
/// detection in `continuity_decorate::table_eval`.
fn is_delimiter_line(line: &str) -> bool {
    let mut saw_dash = false;
    let mut saw_content = false;
    for byte in line.bytes() {
        match byte {
            b'-' => {
                saw_dash = true;
                saw_content = true;
            }
            b':' | b'|' => saw_content = true,
            b' ' | b'\t' => {}
            _ => return false,
        }
    }
    saw_dash && saw_content
}

fn push_nonempty(out: &mut Vec<Range<usize>>, range: Range<usize>) {
    if range.end > range.start {
        out.push(range);
    }
}

fn push_pipe_ranges(out: &mut Vec<Range<usize>>, line_text: &str, line_start: usize) {
    let bytes = line_text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'|' {
            // Escaped pipe: preserve both bytes in the visible run.
            i += 2;
            continue;
        }
        if bytes[i] == b'|' {
            out.push((line_start + i)..(line_start + i + 1));
        }
        i += 1;
    }
}

fn push_formula_override_ranges(
    out: &mut Vec<Range<usize>>,
    table: &EvaluatedTable,
    line_start: usize,
    line_end: usize,
) {
    for override_cell in &table.overrides {
        let start = override_cell.cell_range.start.max(line_start);
        let end = override_cell.cell_range.end.min(line_end);
        push_nonempty(out, start..end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_decorate::{CellRef, Decorations, EvaluatedTable, TableCellOverride};

    fn make_decorations(tables: Vec<EvaluatedTable>) -> Decorations {
        let mut d = Decorations::empty(0);
        d.evaluated_tables = tables;
        d
    }

    #[test]
    fn empty_tables_yields_nothing() {
        let d = make_decorations(Vec::new());
        let out = compute_table_hidden_ranges_for_line(&d, &[], 0, 10, "| a | b |");
        assert!(out.is_empty());
    }

    #[test]
    fn caret_position_does_not_affect_hiding() {
        // Tables render unconditionally — pipes are always hidden,
        // regardless of where the caret sits. This sanity-checks that
        // the function no longer takes a caret arg and that the
        // hide list is stable across calls.
        let d = make_decorations(vec![EvaluatedTable {
            block_range: 0..40,
            overrides: Vec::new(),
        }]);
        let line = "| a | b |";
        let out = compute_table_hidden_ranges_for_line(&d, &[], 0, line.len(), line);
        assert_eq!(out.len(), 3, "every pipe must hide; got {out:?}");
        assert_eq!(out[0], 0..1);
        assert_eq!(out[1], 4..5);
        assert_eq!(out[2], 8..9);
    }

    #[test]
    fn formula_override_payload_is_hidden() {
        let line = "| 3 | =SUM(A1:A3) | =A3+B3 |";
        let line_start = 100;
        let formula_start = line
            .find("=A3+B3")
            .expect("invariant: sample table row contains formula");
        let formula_end = formula_start + "=A3+B3".len();
        let d = make_decorations(vec![EvaluatedTable {
            block_range: line_start..line_start + line.len(),
            overrides: vec![TableCellOverride {
                cell: CellRef { col: 2, row: 3 },
                cell_range: line_start + formula_start..line_start + formula_end,
                display: "9".into(),
            }],
        }]);
        let out = compute_table_hidden_ranges_for_line(
            &d,
            &[],
            line_start,
            line_start + line.len(),
            line,
        );

        assert!(
            out.contains(&(line_start + formula_start..line_start + formula_end)),
            "formula source bytes must be hidden with table chrome active; got {out:?}"
        );
    }

    #[test]
    fn alignment_row_hides_entire_line() {
        let d = make_decorations(vec![EvaluatedTable {
            block_range: 0..40,
            overrides: Vec::new(),
        }]);
        let line = "|---|:---:|---:|";
        let out = compute_table_hidden_ranges_for_line(&d, &[], 10, 10 + line.len(), line);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], 10..10 + line.len());
    }

    #[test]
    fn escaped_pipe_is_preserved() {
        let d = make_decorations(vec![EvaluatedTable {
            block_range: 0..40,
            overrides: Vec::new(),
        }]);
        let line = r"| a \| b | c |";
        let out = compute_table_hidden_ranges_for_line(&d, &[], 0, line.len(), line);
        // Two unescaped pipes (start, mid after the escaped one, end).
        // Offsets in `| a \| b | c |`: 0, 9, 13.
        let unescaped: Vec<Range<usize>> = vec![0..1, 9..10, 13..14];
        assert_eq!(out, unescaped);
    }

    #[test]
    fn line_outside_block_range_is_skipped() {
        let d = make_decorations(vec![EvaluatedTable {
            block_range: 100..200,
            overrides: Vec::new(),
        }]);
        let line = "| a | b |";
        let out = compute_table_hidden_ranges_for_line(&d, &[], 0, line.len(), line);
        assert!(out.is_empty(), "non-intersecting line must yield nothing");
    }

    #[test]
    fn multi_table_doc_both_tables_hide() {
        let table_a = EvaluatedTable {
            block_range: 0..40,
            overrides: Vec::new(),
        };
        let table_b = EvaluatedTable {
            block_range: 60..120,
            overrides: Vec::new(),
        };
        let d = make_decorations(vec![table_a, table_b]);
        let line_b = "| x | y |";
        let out = compute_table_hidden_ranges_for_line(&d, &[], 60, 60 + line_b.len(), line_b);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], 60..61);
    }

    #[test]
    fn alignment_row_with_only_dashes_and_pipes_detected() {
        let d = make_decorations(vec![EvaluatedTable {
            block_range: 0..40,
            overrides: Vec::new(),
        }]);
        let line = "|---|---|";
        let out = compute_table_hidden_ranges_for_line(&d, &[], 0, line.len(), line);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], 0..line.len());
    }

    #[test]
    fn plain_text_line_with_dashes_is_not_a_delim_row() {
        // A line containing only `--` is not a table delimiter (no
        // pipes / colons, but more importantly it has no enclosing
        // table block — so the early-out skips it).
        let d = make_decorations(Vec::new());
        let line = "-- look, dashes --";
        let out = compute_table_hidden_ranges_for_line(&d, &[], 0, line.len(), line);
        assert!(out.is_empty());
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn suppressed_table_emits_no_hides() {
        // When a selection covers any pipe byte in a table, callers
        // include that table's block_range in `suppressed_table_blocks`
        // and the hide provider must emit nothing — raw markdown
        // (pipes + alignment row + formula source) renders so the
        // user can see exactly what's selected.
        let d = make_decorations(vec![EvaluatedTable {
            block_range: 0..40,
            overrides: Vec::new(),
        }]);
        let line = "| a | b |";
        let out = compute_table_hidden_ranges_for_line(&d, &[0..40], 0, line.len(), line);
        assert!(
            out.is_empty(),
            "suppressed table must yield no hides, got {out:?}"
        );
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn suppression_targets_exact_block_range_only() {
        // A suppression entry that doesn't match the table's block
        // range exactly must NOT suppress — the suppression set is
        // keyed by exact `block_range` equality, the cheap unique id
        // for a table in the decoration snapshot.
        let d = make_decorations(vec![EvaluatedTable {
            block_range: 10..50,
            overrides: Vec::new(),
        }]);
        let line = "| a | b |";
        // Same length but different bounds — must not match.
        let out = compute_table_hidden_ranges_for_line(&d, &[0..40], 10, 10 + line.len(), line);
        assert_eq!(out.len(), 3, "non-matching suppression must not skip hides");
    }

    #[test]
    fn empty_body_row_pipes_are_hidden_when_block_range_extends_over_it() {
        // Regression for the markdown.insert_table flow — once
        // decorate's `evaluate_tables` extends the block_range across
        // empty body rows, the hide provider must emit pipe hides for
        // those rows so the visual painter has a clean surface.
        let d = make_decorations(vec![EvaluatedTable {
            block_range: 0..200,
            overrides: Vec::new(),
        }]);
        let line = "|       |       |       |";
        let out = compute_table_hidden_ranges_for_line(&d, &[], 100, 100 + line.len(), line);
        assert_eq!(
            out.len(),
            4,
            "expected 4 pipes (3 cols + leading + trailing share count); got {out:?}"
        );
    }
}
