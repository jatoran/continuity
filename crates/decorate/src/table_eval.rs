//! Phase F4 — per-frame table parser + formula re-evaluation pass.
//!
//! Source bytes stay byte-exact markdown (`=SUM(B1:B5)` lives in the cell
//! verbatim); this module produces a list of `TableCellOverride`s that the
//! renderer swaps in for caret-out cells. Editing a cell reveals the raw
//! formula text — the swap-in is a paint-time substitution, not a rope
//! mutation.
//!
//! Thread ownership: pure, callable from any thread. Recomputed once per
//! `(RopeSnapshot, Revision)` by the decoration worker pool.

use std::collections::HashMap;
use std::ops::Range;

use crate::spans::{BlockKind, BlockSpan};
use crate::table_block_fixup::extend_pipe_block_end;
use crate::table_formula::{parse_formula, CellRef, Expr, FormulaError};

mod chain;
mod pipe_cells;

use chain::{ChainEvaluator, FormulaOutcome};
use pipe_cells::{build_value_matrix, parse_pipe_table_cells, PipeCell};

/// Renderer hint: replace the source bytes covering `cell_range` with
/// `display` when painting this cell with no caret inside the enclosing
/// table block. The byte range is **document-absolute**.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableCellOverride {
    /// The cell whose source range receives the override. Both fields are
    /// 0-indexed (column letters in source convert from 1-indexed `A1`).
    pub cell: CellRef,
    /// Document-absolute byte range covering the cell text (after the
    /// leading pipe + whitespace, up to the next pipe). The renderer
    /// substitutes `display` for whatever the source string contains in
    /// this range.
    pub cell_range: Range<usize>,
    /// Substitute text — either the formatted result of the formula or a
    /// stable error sentinel (`#DIV/0!`, `#CIRC`, `#ERR`).
    pub display: String,
}

/// One evaluated pipe-table block: which block it covers, plus the list
/// of overrides per formula cell. The list is empty when no cell in the
/// block carries a formula — in that case the consumer can skip the
/// block entirely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluatedTable {
    /// Document-absolute byte range of the pipe-table block.
    pub block_range: Range<usize>,
    /// One entry per cell whose trimmed text begins with `=` and produced
    /// either a number or an error sentinel.
    pub overrides: Vec<TableCellOverride>,
}

/// Run formula evaluation across every pipe-table block in `source`.
///
/// `blocks` is the block-span list from `block_spans` — typically the same
/// `Vec<BlockSpan>` already cached on `Decorations`. Blocks of any other
/// kind are skipped.
///
/// Returned tables are in document order. An evaluated table with no
/// formula cells is still included (with an empty `overrides` vec) so the
/// consumer can intersect by `block_range` cheaply.
#[must_use]
pub fn evaluate_tables(source: &str, blocks: &[BlockSpan]) -> Vec<EvaluatedTable> {
    let mut out = Vec::new();
    for block in blocks {
        if !matches!(block.kind, BlockKind::PipeTable) {
            continue;
        }
        // tree-sitter-md terminates the pipe-table block at the last
        // row with non-whitespace cell content, so an `insert_table`
        // skeleton (empty-body rows) parses as a short block followed
        // by `Other("unknown")`. Extend forward over any pipe-shaped
        // line so the visible block matches GFM and the pipe-hide /
        // visual-table passes see every row.
        let extended_end = extend_pipe_block_end(source, block.end_byte);
        let block_src = match source.get(block.start_byte..extended_end) {
            Some(s) => s,
            None => continue,
        };
        let cells = parse_pipe_table_cells(block_src, block.start_byte);
        if cells.is_empty() {
            continue;
        }
        let matrix = build_value_matrix(source, &cells);
        let overrides = compute_overrides(source, &cells, &matrix);
        out.push(EvaluatedTable {
            block_range: block.start_byte..extended_end,
            overrides,
        });
    }
    out
}

/// Build the per-cell override list for one pipe-table block. Each
/// formula cell is parsed once; successful parses feed the
/// [`ChainEvaluator`] so a cell referencing another formula cell sees
/// the dependent's computed value (cycles surface as `#CIRC`).
fn compute_overrides(
    source: &str,
    cells: &[PipeCell],
    matrix: &[Vec<Option<f64>>],
) -> Vec<TableCellOverride> {
    let mut formula_entries: Vec<(CellRef, Range<usize>, Result<Expr, FormulaError>)> = Vec::new();
    for cell in cells {
        let cell_text = source.get(cell.cell_range.clone()).unwrap_or("");
        let trimmed = cell_text.trim();
        if !trimmed.starts_with('=') {
            continue;
        }
        let cell_ref = CellRef {
            col: cell.col,
            row: cell.row,
        };
        formula_entries.push((cell_ref, cell.cell_range.clone(), parse_formula(trimmed)));
    }
    let mut formulas_map: HashMap<CellRef, &Expr> = HashMap::new();
    for (cell_ref, _, parse_result) in &formula_entries {
        if let Ok(expr) = parse_result {
            formulas_map.insert(*cell_ref, expr);
        }
    }
    let evaluator = ChainEvaluator::new(matrix, formulas_map);
    let displays: Vec<String> = formula_entries
        .iter()
        .map(|(cell_ref, _, parse_result)| match parse_result {
            Err(_) => "#ERR".to_string(),
            Ok(_) => match evaluator.evaluate_cell(*cell_ref) {
                FormulaOutcome::Value(v) => format_formula_value(v),
                FormulaOutcome::Circular => "#CIRC".to_string(),
                FormulaOutcome::DivByZero => "#DIV/0!".to_string(),
                FormulaOutcome::Error => "#ERR".to_string(),
            },
        })
        .collect();
    drop(evaluator);
    formula_entries
        .into_iter()
        .zip(displays)
        .map(|((cell_ref, cell_range, _), display)| TableCellOverride {
            cell: cell_ref,
            cell_range,
            display,
        })
        .collect()
}

/// Format an evaluator result as a stable, locale-neutral text string:
/// integer-valued floats lose the decimal, everything else uses
/// `%.6g`-style trimming so `0.1 + 0.2` rounds to `"0.3"`.
fn format_formula_value(value: f64) -> String {
    if !value.is_finite() {
        return "#ERR".to_string();
    }
    if value.fract() == 0.0 && value.abs() < 1.0e15 {
        return format!("{:.0}", value);
    }
    let formatted = format!("{:.6}", value);
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

// `PipeCell`, `parse_pipe_table_cells`, `is_delimiter_line`,
// `split_pipe_cells`, and `build_value_matrix` live in the sibling
// `pipe_cells` submodule.

// `TableSource` impl over `&[Vec<Option<f64>>]` is provided by
// `table_formula`. The empty-cell-zero contract is honoured by `eval`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spans::block_spans;
    use crate::{Decorations, MarkdownParser};

    fn parse_blocks(src: &str) -> Vec<BlockSpan> {
        let mut p = MarkdownParser::new().unwrap();
        let tree = p.parse(src, None).unwrap();
        block_spans(&tree)
    }

    #[test]
    fn empty_source_yields_no_tables() {
        assert!(evaluate_tables("", &[]).is_empty());
    }

    #[test]
    fn empty_body_rows_extend_block_range_to_cover_entire_table() {
        // Regression — tree-sitter-md truncates `BlockKind::PipeTable`
        // at the last row with non-whitespace cell content. A skeleton
        // from `format_table_skeleton(3, 3)` therefore parses as
        // PipeTable covering only the header + alignment rows, and the
        // empty body rows fall into `Other("unknown")`. Pre-fix, that
        // left the visual-table renderer + display-map hide pass with
        // a short `block_range`, and the empty body rows rendered as
        // raw markdown text until the user typed into a cell. The
        // `extend_pipe_block_end` walk in `evaluate_tables` must
        // absorb every subsequent pipe-shaped line so the produced
        // `EvaluatedTable.block_range` covers all 5 source lines.
        let src = crate::table_formula::format_table_skeleton(3, 3);
        let blocks = parse_blocks(&src);
        let tables = evaluate_tables(&src, &blocks);
        assert_eq!(tables.len(), 1, "expected one PipeTable");
        let r = &tables[0].block_range;
        let covered: &str = &src[r.start..r.end];
        // Five rows: header + alignment + 3 body rows. Each line ends
        // with `\n`; expect every body row to be included.
        let line_count = covered.matches('\n').count();
        assert_eq!(
            line_count, 5,
            "expected block_range to cover all 5 lines, got {line_count} lines in src \
             {src:?} → block_src {covered:?}"
        );
    }

    #[test]
    fn pipe_block_extension_is_idempotent_when_table_already_complete() {
        // Body rows already have content → tree-sitter-md returns a
        // full PipeTable range and the extension is a no-op.
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables[0].block_range.end, src.len());
    }

    #[test]
    fn skeleton_followed_by_bullet_classifies_list_block_after_table() {
        // Regression — pre-fix, the empty body rows of
        // `format_table_skeleton` confused tree-sitter-md into lumping
        // the empty rows AND every line of user-added markdown into a
        // single `Other("unknown")` block. The downstream bullet's
        // `- list item` therefore never registered as a `List` /
        // `ListItem` block, and the renderer drew the bullet glyph as
        // raw `-` text. After the substitution, the bullet must be
        // classified as `List`.
        let skeleton = crate::table_formula::format_table_skeleton(3, 3);
        let src = format!("{skeleton}- list item\n- another item\n");
        let d = Decorations::compute(&src, 0).expect("compute decorations");
        let saw_list_block = d
            .blocks
            .iter()
            .any(|b| matches!(b.kind, crate::BlockKind::List));
        assert!(
            saw_list_block,
            "bullet content below empty skeleton must classify as List \
             (blocks observed: {:?})",
            d.blocks.iter().map(|b| b.kind).collect::<Vec<_>>()
        );
        // The PipeTable's block_range must still cover the full
        // skeleton, not bleed past it into the list.
        let pipe = d
            .blocks
            .iter()
            .find(|b| matches!(b.kind, crate::BlockKind::PipeTable))
            .expect("PipeTable block missing");
        assert_eq!(pipe.end_byte, skeleton.len());
    }

    #[test]
    fn skeleton_followed_by_heading_classifies_heading_block() {
        let skeleton = crate::table_formula::format_table_skeleton(2, 2);
        let src = format!("{skeleton}# Section after\n");
        let d = Decorations::compute(&src, 0).expect("compute decorations");
        let saw_heading = d
            .blocks
            .iter()
            .any(|b| matches!(b.kind, crate::BlockKind::Heading { .. }));
        assert!(
            saw_heading,
            "heading below skeleton must classify as Heading: {:?}",
            d.blocks.iter().map(|b| b.kind).collect::<Vec<_>>()
        );
    }

    #[test]
    fn table_without_formulas_has_empty_overrides() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables.len(), 1);
        assert!(tables[0].overrides.is_empty());
    }

    #[test]
    fn sum_formula_evaluates() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | =SUM(A1:A2) |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].overrides.len(), 1);
        assert_eq!(tables[0].overrides[0].display, "4");
    }

    #[test]
    fn divide_by_zero_renders_sentinel() {
        let src = "| a | b |\n|---|---|\n| 1 | =1/0 |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables[0].overrides[0].display, "#DIV/0!");
    }

    #[test]
    fn syntax_error_renders_err() {
        let src = "| a |\n|---|\n| =BOGUS(A1) |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables[0].overrides[0].display, "#ERR");
    }

    #[test]
    fn formula_in_header_row_skipped_for_matrix_but_still_overridden() {
        // Formula in a header cell still gets the swap-in; the matrix
        // simply doesn't see header values.
        let src = "| =1+1 | b |\n|---|---|\n| 2 | 3 |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        // Header cell has row==u32::MAX so it's not in the matrix, but the
        // override list still picks it up via the "starts with =" filter.
        assert_eq!(tables[0].overrides.len(), 1);
        assert_eq!(tables[0].overrides[0].display, "2");
    }

    #[test]
    fn fractional_value_trims_trailing_zeros() {
        assert_eq!(format_formula_value(0.5), "0.5");
        assert_eq!(format_formula_value(1.25), "1.25");
        assert_eq!(format_formula_value(3.0), "3");
        assert_eq!(format_formula_value(0.0), "0");
    }

    #[test]
    fn average_formula() {
        let src = "| a |\n|---|\n| 2 |\n| 4 |\n| 6 |\n| =AVG(A1:A3) |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables[0].overrides[0].display, "4");
    }

    #[test]
    fn count_formula_counts_numeric_cells_only() {
        // C1 = COUNT(A1:B2) over a 2-col, 2-row body where one cell is
        // non-numeric and the rest are numeric — the formula cell
        // itself sits outside the counted range, so no cycle.
        let src = "| a | b | c |\n|---|---|---|\n| 1 | foo | =COUNT(A1:B2) |\n| 2 | 4 |   |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        // Numeric cells in A1:B2 are A1=1, A2=2, B2=4 → count is 3.
        assert_eq!(tables[0].overrides[0].display, "3");
    }

    #[test]
    fn chained_formula_picks_up_dependent_value() {
        // Anchor regression for the user's reproducer: B1 = SUM(A1:A3) = 6,
        // B2 = B1 + 3 = 9. Pre-fix B2 rendered as 3 because B1's computed
        // value was invisible to the second evaluation pass.
        let src = "| A | B |\n|---|---|\n| 1 | =SUM(A1:A3) |\n| 2 | =B1+3 |\n| 3 |   |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables[0].overrides.len(), 2);
        // Document order: B1 then B2.
        assert_eq!(tables[0].overrides[0].display, "6");
        assert_eq!(tables[0].overrides[1].display, "9");
    }

    #[test]
    fn lowercase_cell_refs_resolve_same_as_uppercase() {
        // Excel-likeness: case in column letters is irrelevant.
        let src = "| A | B |\n|---|---|\n| 1 | =sum(a1:a3) |\n| 2 | =b1 |\n| 3 |   |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables[0].overrides[0].display, "6");
        assert_eq!(tables[0].overrides[1].display, "6");
    }

    #[test]
    fn circular_reference_emits_circ_sentinel() {
        let src = "| A | B |\n|---|---|\n| =B1+1 | =A1+1 |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables[0].overrides.len(), 2);
        for ov in &tables[0].overrides {
            assert_eq!(ov.display, "#CIRC");
        }
    }

    #[test]
    fn self_reference_emits_circ_sentinel() {
        let src = "| A |\n|---|\n| =A1+1 |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables[0].overrides[0].display, "#CIRC");
    }

    #[test]
    fn whitespace_around_tokens_is_tolerated() {
        let src = "| A | B |\n|---|---|\n| 1 | =  A1   +   3  |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables[0].overrides[0].display, "4");
    }

    #[test]
    fn empty_cell_reference_treated_as_zero() {
        // A1 is empty (no number), so =A1+5 should evaluate to 5.
        let src = "| A | B |\n|---|---|\n|   | =A1+5 |\n";
        let blocks = parse_blocks(src);
        let tables = evaluate_tables(src, &blocks);
        assert_eq!(tables[0].overrides[0].display, "5");
    }
}
