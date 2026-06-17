//! Paste-time GFM table-block normalization (Phase-D item 30).
//!
//! GFM requires a pipe table to *begin a block*. When a table is pasted
//! directly after a non-blank line with no blank line before it,
//! tree-sitter-md folds the header into the preceding paragraph and the
//! snippet never becomes a `PipeTable`. These pure helpers detect a pasted
//! table and (a) prefix a newline so it starts its own block and (b)
//! synthesize a missing delimiter row.
//!
//! Reuses [`super::is_delimiter_line`] so the paste path agrees with the
//! table-ops parser on what a delimiter row is.

use super::is_delimiter_line;

/// `true` when `line` (trimmed) reads as a pipe-table row: it has a
/// leading `|` after optional indentation.
pub(crate) fn is_pipe_table_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// `true` when `text` is a GFM pipe table — its first non-blank line is a
/// pipe row and the next non-blank line is a delimiter row
/// (`|---|:--:|`). This is the minimum tree-sitter-md needs to classify
/// the block as a `PipeTable`.
pub(crate) fn is_gfm_table_text(text: &str) -> bool {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let Some(header) = lines.next() else {
        return false;
    };
    if !is_pipe_table_row(header) {
        return false;
    }
    match lines.next() {
        Some(second) => is_delimiter_line(second.trim()),
        None => false,
    }
}

/// `true` when `text` looks like a multi-row pipe table that lost its
/// delimiter row — a header pipe row with at least two columns followed by
/// at least one more pipe row, where the second line is NOT a delimiter.
/// Such a paste won't become a `PipeTable`; the normalizer can synthesize
/// the missing delimiter row.
///
/// Deliberately conservative: a lone `|`-prefixed line (e.g. an incidental
/// `| note`) is left alone, so only content that clearly reads as a table
/// body gets a synthesized header rule.
pub(crate) fn is_pipe_table_missing_delimiter(text: &str) -> bool {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let Some(header) = lines.next() else {
        return false;
    };
    if !is_pipe_table_row(header) || pipe_table_column_count(header) < 2 {
        return false;
    }
    let Some(second) = lines.next() else {
        return false;
    };
    // Already a delimiter → not "missing"; a second pipe row → table body.
    !is_delimiter_line(second.trim()) && is_pipe_table_row(second)
}

/// Count the columns in a pipe-table header row by counting unescaped
/// `|` separators. A `| a | b |` header yields 2.
pub(crate) fn pipe_table_column_count(header: &str) -> usize {
    let trimmed = header.trim();
    let bytes = trimmed.as_bytes();
    let mut pipes = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'\\' && idx + 1 < bytes.len() {
            idx += 2;
            continue;
        }
        if bytes[idx] == b'|' {
            pipes += 1;
        }
        idx += 1;
    }
    let leading = trimmed.starts_with('|');
    let trailing = trimmed.ends_with('|');
    match (leading, trailing) {
        (true, true) => pipes.saturating_sub(1),
        (false, false) => pipes + 1,
        _ => pipes,
    }
    .max(1)
}

/// Build a GFM delimiter row (`| --- | --- |`) with `cols` columns.
pub(crate) fn format_delimiter_row(cols: usize) -> String {
    let mut out = String::from("|");
    for _ in 0..cols.max(1) {
        out.push_str(" --- |");
    }
    out
}

/// Normalize a pasted GFM table snippet so tree-sitter-md classifies it as
/// a `PipeTable` when inserted at `insertion_at_blank_line_start`.
///
/// * Synthesizes a delimiter row beneath the header when one is missing.
/// * Prefixes a newline when the insertion point is NOT at column 0 of a
///   blank line, so the table begins its own block.
///
/// Returns the text unchanged when it is not a table.
pub(crate) fn normalize_pasted_table(text: &str, insertion_at_blank_line_start: bool) -> String {
    let is_table = is_gfm_table_text(text);
    let missing_delimiter = !is_table && is_pipe_table_missing_delimiter(text);
    if !is_table && !missing_delimiter {
        return text.to_string();
    }
    let mut body = if missing_delimiter {
        insert_missing_delimiter_row(text)
    } else {
        text.to_string()
    };
    if !insertion_at_blank_line_start {
        body.insert(0, '\n');
    }
    body
}

/// Insert a synthesized delimiter row immediately after the header row of
/// a pipe table that is missing one.
fn insert_missing_delimiter_row(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut header_done = false;
    let mut saw_any = false;
    for line in text.split_inclusive('\n') {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        saw_any = true;
        if !header_done && !bare.trim().is_empty() {
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push('\n');
            }
            let cols = pipe_table_column_count(bare);
            out.push_str(&format_delimiter_row(cols));
            out.push('\n');
            header_done = true;
            continue;
        }
        out.push_str(line);
    }
    if !saw_any {
        return text.to_string();
    }
    // Drop a trailing newline introduced past a source that had none.
    if !text.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_full_gfm_table() {
        let text = "| a | b |\n| --- | --- |\n| 1 | 2 |";
        assert!(is_gfm_table_text(text));
        assert!(!is_pipe_table_missing_delimiter(text));
    }

    #[test]
    fn detects_missing_delimiter() {
        let text = "| a | b |\n| 1 | 2 |";
        assert!(!is_gfm_table_text(text));
        assert!(is_pipe_table_missing_delimiter(text));
    }

    #[test]
    fn lone_header_is_left_alone() {
        // A single pipe-prefixed line is not treated as a table body — too
        // ambiguous (could be incidental prose), so no delimiter is added.
        let text = "| a | b |";
        assert!(!is_pipe_table_missing_delimiter(text));
        assert_eq!(normalize_pasted_table(text, false), text);
    }

    #[test]
    fn single_column_body_not_treated_as_table() {
        // Header with only one column → not enough to infer a table.
        let text = "| just one |\n| value |";
        assert!(!is_pipe_table_missing_delimiter(text));
    }

    #[test]
    fn non_table_text_not_detected() {
        assert!(!is_gfm_table_text("just prose\nmore prose"));
        assert!(!is_pipe_table_missing_delimiter("just prose"));
    }

    #[test]
    fn column_count_from_header() {
        assert_eq!(pipe_table_column_count("| a | b | c |"), 3);
        assert_eq!(pipe_table_column_count("| only |"), 1);
        assert_eq!(pipe_table_column_count("a | b"), 2);
    }

    #[test]
    fn escaped_pipe_not_counted() {
        // `| a \| b |` is a single cell containing a literal pipe.
        assert_eq!(pipe_table_column_count("| a \\| b |"), 1);
    }

    #[test]
    fn delimiter_row_format() {
        assert_eq!(format_delimiter_row(2), "| --- | --- |");
        assert_eq!(format_delimiter_row(1), "| --- |");
    }

    #[test]
    fn normalize_prefixes_newline_when_not_block_start() {
        let text = "| a | b |\n| --- | --- |";
        let out = normalize_pasted_table(text, false);
        assert_eq!(out, "\n| a | b |\n| --- | --- |");
    }

    #[test]
    fn normalize_no_prefix_when_block_start() {
        let text = "| a | b |\n| --- | --- |";
        let out = normalize_pasted_table(text, true);
        assert_eq!(out, text);
    }

    #[test]
    fn normalize_inserts_missing_delimiter() {
        let text = "| a | b |\n| 1 | 2 |";
        let out = normalize_pasted_table(text, true);
        assert_eq!(out, "| a | b |\n| --- | --- |\n| 1 | 2 |");
    }

    #[test]
    fn normalize_inserts_delimiter_and_prefixes_newline() {
        let text = "| a | b |\n| 1 | 2 |";
        let out = normalize_pasted_table(text, false);
        assert_eq!(out, "\n| a | b |\n| --- | --- |\n| 1 | 2 |");
    }

    #[test]
    fn non_table_unchanged() {
        let text = "hello world";
        assert_eq!(normalize_pasted_table(text, false), text);
    }
}
