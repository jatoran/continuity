//! Pipe-table cell parsing utilities for [`super::evaluate_tables`].
//!
//! Walks a `BlockKind::PipeTable` block's source bytes and emits one
//! [`PipeCell`] per pipe-separated slot (including empty cells so the
//! column index stays aligned with the source). The companion
//! [`build_value_matrix`] turns those cells into a `Vec<Vec<Option<f64>>>`
//! literal matrix consumed by the chain formula evaluator.
//!
//! Split out of `table_eval.rs` so the orchestrator stays under the
//! 600-line cap.

use std::ops::Range;

/// Discovered cell within a pipe table.
pub(super) struct PipeCell {
    /// Document-absolute byte range covering the trimmed cell payload.
    pub(super) cell_range: Range<usize>,
    /// 0-indexed column.
    pub(super) col: u32,
    /// 0-indexed body row (header = row 0, delimiter row skipped, first
    /// data row = row 0 of the matrix). Cells in the header row carry
    /// `row = u32::MAX` so the matrix builder can skip them.
    pub(super) row: u32,
    /// `true` for header cells (excluded from the value matrix).
    pub(super) is_header: bool,
    /// `true` for the delimiter row (`---` / `:---:` etc.) — skipped.
    pub(super) is_delimiter: bool,
}

/// Parse a pipe-table block source into a list of [`PipeCell`]s with
/// document-absolute byte ranges. Skips the delimiter row; tags header
/// cells separately. Cells that fail to align are silently dropped.
pub(super) fn parse_pipe_table_cells(block_src: &str, base: usize) -> Vec<PipeCell> {
    let mut out = Vec::new();
    let mut header_seen = false;
    let mut delimiter_seen = false;
    let mut body_row: u32 = 0;
    let bytes = block_src.as_bytes();
    let mut line_start = 0usize;
    while line_start <= bytes.len() {
        // Find the end of this line (exclusive of newline).
        let mut line_end = line_start;
        while line_end < bytes.len() && bytes[line_end] != b'\n' {
            line_end += 1;
        }
        let line = &block_src[line_start..line_end];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if line_end == bytes.len() {
                break;
            }
            line_start = line_end + 1;
            continue;
        }
        if is_delimiter_line(trimmed) {
            delimiter_seen = true;
            if line_end == bytes.len() {
                break;
            }
            line_start = line_end + 1;
            continue;
        }
        let cells = split_pipe_cells(line, line_start);
        let is_header = !header_seen;
        let row_index = if is_header { u32::MAX } else { body_row };
        for (col_index, (start_off, end_off)) in cells.iter().enumerate() {
            out.push(PipeCell {
                cell_range: (base + *start_off)..(base + *end_off),
                col: col_index as u32,
                row: row_index,
                is_header,
                is_delimiter: false,
            });
        }
        if is_header {
            header_seen = true;
        } else if delimiter_seen {
            body_row = body_row.saturating_add(1);
        }
        if line_end == bytes.len() {
            break;
        }
        line_start = line_end + 1;
    }
    out
}

/// `true` when `line` looks like a pipe-table delimiter row (only `-`,
/// `:`, `|`, and whitespace, with at least one `-`).
fn is_delimiter_line(line: &str) -> bool {
    let mut saw_dash = false;
    for byte in line.bytes() {
        match byte {
            b'-' => saw_dash = true,
            b':' | b'|' | b' ' | b'\t' => {}
            _ => return false,
        }
    }
    saw_dash
}

/// Walk a single pipe-table row line and return `(start_offset, end_offset)`
/// pairs covering the trimmed payload of each cell. Offsets are
/// **line-relative** when `line_base` is `0`; the caller adds the block
/// base to make them document-absolute.
fn split_pipe_cells(line: &str, line_base: usize) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    // Skip a leading `|` and surrounding whitespace.
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'|' {
        i += 1;
    }
    while i < bytes.len() {
        // Skip leading whitespace within the cell.
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        let cell_start = i;
        // Scan until the next unescaped pipe or end of line.
        while i < bytes.len() && bytes[i] != b'|' {
            // Permit `\|` as an escaped pipe (rare in formula context, but
            // the parser shouldn't choke on it).
            if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                i += 2;
                continue;
            }
            i += 1;
        }
        let mut cell_end = i;
        // Trim trailing whitespace.
        while cell_end > cell_start && matches!(bytes[cell_end - 1], b' ' | b'\t') {
            cell_end -= 1;
        }
        // Preserve a slot for every pipe-separated cell, including the
        // empty ones — the column index later derived from this `Vec`'s
        // enumeration order is what tells `=A1` apart from `=B1`, and
        // dropping empties shifts those refs to the wrong cell. The
        // final synthetic slot after a trailing `|` (i.e. when we ran
        // off the end of the row with no following pipe) is omitted so
        // a row written as `| 1 | 2 |` produces 2 cells, not 3.
        let trailing_pipe = i < bytes.len() && bytes[i] == b'|';
        let consumed_any = i > cell_start || trailing_pipe;
        if consumed_any {
            out.push((line_base + cell_start, line_base + cell_end));
        }
        if i < bytes.len() {
            i += 1;
        }
    }
    out
}

/// Build the `Vec<Vec<Option<f64>>>` value matrix consumed by the formula
/// evaluator. Header + delimiter rows are excluded; cells whose text does
/// not parse as a number become `None`.
pub(super) fn build_value_matrix(source: &str, cells: &[PipeCell]) -> Vec<Vec<Option<f64>>> {
    let mut max_row: i64 = -1;
    let mut max_col: i64 = -1;
    for c in cells {
        if c.is_header || c.is_delimiter {
            continue;
        }
        max_row = max_row.max(c.row as i64);
        max_col = max_col.max(c.col as i64);
    }
    if max_row < 0 || max_col < 0 {
        return Vec::new();
    }
    let row_count = (max_row + 1) as usize;
    let col_count = (max_col + 1) as usize;
    let mut matrix: Vec<Vec<Option<f64>>> = (0..row_count).map(|_| vec![None; col_count]).collect();
    for c in cells {
        if c.is_header || c.is_delimiter {
            continue;
        }
        let text = source.get(c.cell_range.clone()).unwrap_or("").trim();
        let parsed = text.parse::<f64>().ok();
        if let Some(row) = matrix.get_mut(c.row as usize) {
            if let Some(slot) = row.get_mut(c.col as usize) {
                *slot = parsed;
            }
        }
    }
    matrix
}
