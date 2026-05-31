//! Pre/post-processing that compensates for tree-sitter-md's
//! pipe-table parsing quirks.
//!
//! Two helpers, both consumed by [`crate::table_eval`] /
//! [`crate::Decorations`]:
//!
//! - [`extend_pipe_block_end`] — walk forward from a tree-sitter
//!   `PipeTable` block's `end_byte` and absorb any subsequent
//!   `|…|` lines tree-sitter-md mis-classified as `Other` (happens
//!   when body rows are whitespace-only).
//! - [`fill_empty_pipe_rows_for_parser`] — produce a byte-for-byte
//!   "parse source" with placeholder content in every whitespace-only
//!   pipe row, so tree-sitter-md classifies the full table AND the
//!   markdown content **below it** is freed from the bloated
//!   `Other("unknown")` block that would otherwise swallow it.
//!
//! Sibling of `table_eval.rs` so the latter stays under the 600-line
//! conventions cap.
//!
//! Thread ownership: pure data; callable from any thread (the
//! decoration worker pool consumes both helpers off the UI thread).

/// Walk forward from `end_byte` and return the new end (exclusive)
/// after absorbing every consecutive line that starts with optional
/// whitespace + `|` and contains at least two pipes. Stops at the
/// first line that doesn't look like a pipe-table row, at a blank
/// line, or at EOF.
///
/// Idempotent — calling on an already-complete table returns the input
/// unchanged.
#[must_use]
pub(crate) fn extend_pipe_block_end(source: &str, end_byte: usize) -> usize {
    let bytes = source.as_bytes();
    let mut cursor = end_byte;
    while cursor < bytes.len() {
        let mut line_end = cursor;
        while line_end < bytes.len() && bytes[line_end] != b'\n' {
            line_end += 1;
        }
        let mut trim_end = line_end;
        if trim_end > cursor && bytes[trim_end - 1] == b'\r' {
            trim_end -= 1;
        }
        let line = match source.get(cursor..trim_end) {
            Some(s) => s,
            None => break,
        };
        if !is_pipe_table_row(line) {
            break;
        }
        let advance_to = if line_end < bytes.len() {
            line_end + 1
        } else {
            line_end
        };
        if advance_to <= cursor {
            break;
        }
        cursor = advance_to;
    }
    cursor
}

/// `true` when `line` is shaped like a pipe-table row: starts with
/// optional whitespace + `|`, and the trimmed line has at least two
/// pipes total. Empty / blank lines are rejected — a blank line
/// terminates the table.
fn is_pipe_table_row(line: &str) -> bool {
    let trimmed = line.trim_start();
    let starts_with_pipe = trimmed.starts_with('|');
    if !starts_with_pipe {
        return false;
    }
    trimmed.bytes().filter(|b| *b == b'|').count() >= 2
}

/// Build a "parse source" with placeholder content substituted into
/// every fully-empty pipe-table body row. Returns `None` when no row
/// in `source` matches the empty-body-row pattern, so the caller can
/// keep the original string slice (no allocation in the common case).
///
/// **Why this exists**: tree-sitter-md's GFM pipe-table grammar
/// terminates the table at the last row that contains non-whitespace
/// cell content. A `format_table_skeleton(rows, cols)` skeleton (whose
/// body cells are pure whitespace) therefore parses as PipeTable +
/// alignment-row only, and **every line below** — including the empty
/// body rows AND any bullets / image refs / paragraphs the user
/// added — is lumped into a single sibling `Other("unknown")` block.
/// Content inside an `Other` block never gets classified as `List` /
/// `ImageRef` / `Heading`, so the markdown decoration the user typed
/// below the table renders as raw text until they fill a body cell.
///
/// Filling each empty cell with a single non-whitespace character lets
/// tree-sitter recognise the row, terminates the pipe-table block at
/// the correct line, and frees the lines below to be classified
/// normally. The substitution is **byte-for-byte**: every column's
/// first interior space is replaced with `x`, preserving byte offsets
/// so the resulting `BlockSpan` / `InlineSpan` ranges still address
/// the original rope correctly.
///
/// Rows are only filled when *every* byte between pipes is whitespace
/// (the strict `|\s+\|\s+\|…` shape). Alignment rows (`|---|`) and
/// content-bearing rows are untouched.
#[must_use]
pub(crate) fn fill_empty_pipe_rows_for_parser(source: &str) -> Option<String> {
    let bytes = source.as_bytes();
    let mut out: Option<Vec<u8>> = None;
    let mut i = 0usize;
    while i <= bytes.len() {
        let line_start = i;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        let mut line_end = i;
        if line_end > line_start && bytes[line_end - 1] == b'\r' {
            line_end -= 1;
        }
        let line = match source.get(line_start..line_end) {
            Some(s) => s,
            None => break,
        };
        if is_empty_pipe_row(line) {
            let buf = out.get_or_insert_with(|| bytes.to_vec());
            fill_cells_in_empty_pipe_row(buf, line_start, line_end);
        }
        if i == bytes.len() {
            break;
        }
        i += 1;
    }
    out.and_then(|buf| String::from_utf8(buf).ok())
}

/// `true` when every byte of `line` is either `|`, ASCII space, or tab,
/// and the trimmed line both starts and ends with `|` with at least two
/// pipes total — i.e. a body row whose every cell is empty. An empty
/// string is rejected (a blank line is not a row).
fn is_empty_pipe_row(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return false;
    }
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return false;
    }
    let mut pipe_count = 0usize;
    for byte in trimmed.bytes() {
        match byte {
            b'|' => pipe_count += 1,
            b' ' | b'\t' => {}
            _ => return false,
        }
    }
    pipe_count >= 2
}

/// Replace the first interior byte of every empty pipe cell in
/// `buf[line_start..line_end]` with `x`. The line is assumed to satisfy
/// [`is_empty_pipe_row`] so every cell is whitespace-only. Byte
/// substitution preserves the line length.
fn fill_cells_in_empty_pipe_row(buf: &mut [u8], line_start: usize, line_end: usize) {
    let mut i = line_start;
    while i < line_end && (buf[i] == b' ' || buf[i] == b'\t') {
        i += 1;
    }
    if i < line_end && buf[i] == b'|' {
        i += 1;
    }
    while i < line_end {
        let cell_start = i;
        let mut first_space_offset: Option<usize> = None;
        while i < line_end && buf[i] != b'|' {
            if first_space_offset.is_none() && (buf[i] == b' ' || buf[i] == b'\t') {
                first_space_offset = Some(i);
            }
            i += 1;
        }
        if cell_start < i {
            let slot = first_space_offset.unwrap_or(cell_start);
            buf[slot] = b'x';
        }
        if i < line_end && buf[i] == b'|' {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extend_pipe_block_end_stops_at_first_non_pipe_line() {
        let src = "header\n|   |   |\n|   |   |\nprose paragraph\n";
        let extended = extend_pipe_block_end(src, 7);
        let covered = &src[7..extended];
        assert!(
            covered.contains("|   |   |"),
            "extension should reach the empty body rows: covered={covered:?}"
        );
        assert!(
            !covered.contains("prose"),
            "extension must stop before a non-pipe line: covered={covered:?}"
        );
        let included_rows = covered.matches("|   |   |").count();
        assert_eq!(included_rows, 2);
    }

    #[test]
    fn extend_pipe_block_end_idempotent_at_eof() {
        let src = "| a | b |\n|---|---|\n";
        let extended = extend_pipe_block_end(src, src.len());
        assert_eq!(extended, src.len());
    }

    #[test]
    fn extend_pipe_block_end_stops_at_blank_line() {
        let src = "header\n|   |   |\n\nprose\n";
        let extended = extend_pipe_block_end(src, 7);
        let covered = &src[7..extended];
        // One pipe row, then blank — extension stops at the blank.
        assert_eq!(covered, "|   |   |\n");
    }

    #[test]
    fn fill_empty_pipe_rows_for_parser_substitutes_only_empty_rows() {
        let src = "| a | b |\n|---|---|\n|   |   |\n| x | y |\n";
        let filled = fill_empty_pipe_rows_for_parser(src).expect("empty row should fill");
        assert_eq!(filled.len(), src.len());
        assert!(filled.contains("|x  |x  |") || filled.contains("|x  |"));
        assert!(filled.contains("| a | b |"));
        assert!(filled.contains("|---|---|"));
        assert!(filled.contains("| x | y |"));
    }

    #[test]
    fn fill_empty_pipe_rows_for_parser_returns_none_when_nothing_to_fill() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\nprose below\n";
        assert!(fill_empty_pipe_rows_for_parser(src).is_none());
    }

    #[test]
    fn fill_does_not_touch_alignment_row() {
        let src = "|---|---|\n|   |   |\n";
        let filled = fill_empty_pipe_rows_for_parser(src).expect("empty row should fill");
        assert!(filled.starts_with("|---|---|\n"));
        // Substitution only on the empty row.
        assert!(filled.contains("|x  |x  |") || filled.contains("|x  |"));
    }

    #[test]
    fn is_empty_pipe_row_rejects_lines_with_content() {
        assert!(!is_empty_pipe_row("| a | b |"));
        assert!(!is_empty_pipe_row("|---|---|"));
        assert!(!is_empty_pipe_row(""));
        assert!(!is_empty_pipe_row("no pipes at all"));
        assert!(!is_empty_pipe_row("|"));
        assert!(is_empty_pipe_row("|   |"));
        assert!(is_empty_pipe_row("|       |       |       |"));
        assert!(is_empty_pipe_row("  |   |   |  "));
    }
}
