//! Pipe-table row parsing helpers — line enumeration, cell tokenization,
//! delimiter detection, formula-override resolution, column-width sizing,
//! and alignment fan-out.
//!
//! Pure-data utilities consumed by [`super::build_one_table_layout`]. No
//! D2D / DirectWrite / Win32 imports; callable from any thread that owns
//! the per-frame snapshot inputs.

use std::ops::Range;

use continuity_decorate::{EvaluatedTable, TableAlignment};

use super::{
    TableCellLayout, DEFAULT_TABLE_COL_WIDTH_MAX_DIP, MIN_TABLE_COL_WIDTH_DIP, TABLE_CELL_PAD_DIP,
};

/// `true` when `line` looks like a pipe-table delimiter row (only `-`,
/// `:`, `|`, and whitespace, with at least one `-`).
pub(super) fn is_delimiter_line(line: &str) -> bool {
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

pub(super) struct LineInfo<'a> {
    /// Offset (in bytes) within the block source where this line starts.
    pub offset_in_block: usize,
    /// Line content with no trailing newline.
    pub text: &'a str,
    /// `true` when `text.trim().is_empty()`.
    pub is_blank: bool,
}

pub(super) fn enumerate_lines(block_src: &str) -> Vec<LineInfo<'_>> {
    let mut out = Vec::new();
    let bytes = block_src.as_bytes();
    let mut i = 0usize;
    while i <= bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        let mut end = i;
        // Strip trailing `\r` from CRLF.
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        let text = &block_src[start..end];
        out.push(LineInfo {
            offset_in_block: start,
            text,
            is_blank: text.trim().is_empty(),
        });
        if i == bytes.len() {
            break;
        }
        i += 1;
    }
    out
}

pub(super) struct ParsedCell<'a> {
    /// Document-absolute byte range of the trimmed cell payload.
    pub doc_range: Range<usize>,
    /// Trimmed cell text borrowed from the row's `line_text`.
    pub text: &'a str,
}

pub(super) fn parse_row_cells<'a>(
    line_text: &'a str,
    line_doc_start: usize,
) -> Vec<ParsedCell<'a>> {
    let bytes = line_text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    // Skip leading whitespace + optional leading pipe.
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    let had_leading_pipe = i < bytes.len() && bytes[i] == b'|';
    if had_leading_pipe {
        i += 1;
    }
    if !had_leading_pipe {
        return out;
    }
    // Empty body rows (`|   |   |   |`) must contribute one
    // `ParsedCell` per column so the visual painter draws a border for
    // every cell — even with no text inside. The earlier "skip
    // zero-length cells" heuristic dropped those, leaving empty body
    // rows as raw markdown text underneath the visual table chrome.
    while i <= bytes.len() {
        // Skip leading whitespace inside the cell.
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        let cell_start = i;
        while i < bytes.len() && bytes[i] != b'|' {
            if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                i += 2;
                continue;
            }
            i += 1;
        }
        let mut cell_end = i;
        while cell_end > cell_start && matches!(bytes[cell_end - 1], b' ' | b'\t') {
            cell_end -= 1;
        }
        // The trailing pipe convention treats `…|` as the row's end;
        // do *not* synthesize a final empty cell past the last pipe.
        let at_eol = i == bytes.len();
        let after_trailing_pipe = at_eol && cell_start == i && cell_end == i;
        if !after_trailing_pipe {
            out.push(ParsedCell {
                doc_range: (line_doc_start + cell_start)..(line_doc_start + cell_end),
                text: &line_text[cell_start..cell_end],
            });
        }
        if i < bytes.len() {
            i += 1;
        } else {
            break;
        }
    }
    out
}

/// Resolve the visible text for one cell. Returns `(display_text,
/// is_formula)`.
///
/// When `caret_in_cell` is `true` the cell shows its raw source
/// bytes — the user is typing the formula and a partial expression
/// like `=` would otherwise render as `#ERR` and hide what they just
/// typed. Once the caret leaves, the next paint picks up the
/// evaluator's override and the cell flips to its computed value.
pub(super) fn resolve_cell_display(
    table: &EvaluatedTable,
    cell_start: usize,
    raw_text: &str,
    caret_in_cell: bool,
) -> (String, bool) {
    if caret_in_cell {
        return (raw_text.to_string(), false);
    }
    for ov in &table.overrides {
        if ov.cell_range.start == cell_start {
            return (ov.display.clone(), true);
        }
    }
    (raw_text.to_string(), false)
}

/// Column-width quantization step in DIPs. Computed column widths are
/// rounded UP to the next multiple of this step, so adding one
/// character to the longest cell in a column doesn't shift the
/// column every keystroke — the column only grows when content
/// crosses the next step boundary. Removes the per-keystroke visual
/// jitter ("flashing") that comes from the active-cell overlay
/// repainting at a slightly-different x each frame, and reduces
/// chrome-cache invalidation traffic since the cache key includes
/// column widths.
const TABLE_COL_WIDTH_STEP_DIP: f32 = 16.0;

/// Compute per-column DIP widths.
///
/// `measurement_texts` runs parallel to `cells` — entry `i` is the
/// "what to measure" string for `cells[i]`. The painter draws
/// `cell.display_text` (which may include `**` markers when the user
/// has the caret in that cell), but width sizing always uses the
/// markers-stripped visible form so columns stay stable across caret
/// transitions. See `build_one_table_layout` for the rationale.
pub(super) fn compute_col_widths_dip(
    cells: &[TableCellLayout],
    measurement_texts: &[String],
    col_count: usize,
    measure: &mut dyn FnMut(&str) -> f32,
) -> Vec<f32> {
    let mut widths = vec![MIN_TABLE_COL_WIDTH_DIP; col_count];
    for (cell, text) in cells.iter().zip(measurement_texts.iter()) {
        let col_index = cell.col as usize;
        if col_index >= col_count {
            continue;
        }
        if text.is_empty() {
            continue;
        }
        // Phase F — cap auto-size at `DEFAULT_TABLE_COL_WIDTH_MAX_DIP`:
        // content wider than a comfortable measure wraps (or clips)
        // instead of stretching the column off the pane edge. An
        // explicit directive width can exceed this (applied in `build`).
        let content_w =
            (measure(text) + 2.0 * TABLE_CELL_PAD_DIP).min(DEFAULT_TABLE_COL_WIDTH_MAX_DIP);
        if content_w > widths[col_index] {
            widths[col_index] = content_w;
        }
    }
    for w in widths.iter_mut() {
        *w = quantize_up(*w, TABLE_COL_WIDTH_STEP_DIP).min(DEFAULT_TABLE_COL_WIDTH_MAX_DIP);
    }
    widths
}

/// Round `value` UP to the next multiple of `step`. Returns `value`
/// unchanged when `step <= 0`.
fn quantize_up(value: f32, step: f32) -> f32 {
    if step <= 0.0 {
        return value;
    }
    (value / step).ceil() * step
}

pub(super) fn build_col_alignments(
    from_delim: &[TableAlignment],
    col_count: usize,
) -> Vec<TableAlignment> {
    let mut out = Vec::with_capacity(col_count);
    for i in 0..col_count {
        out.push(from_delim.get(i).copied().unwrap_or(TableAlignment::Left));
    }
    out
}
