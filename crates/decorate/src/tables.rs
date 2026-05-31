//! Pipe-table column alignment + per-row pipe metadata.
//!
//! The block grammar identifies the table; this module extracts the
//! delimiter row (`---` / `:---` / `---:` / `:---:`) and produces a vector
//! of column alignments. Pipe-byte ranges fall out of the inline scanner
//! (`MarkerKind::TablePipe`) so the renderer already knows to hide them
//! when the caret is outside the table block.

/// Column alignment per the table's delimiter row.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TableAlignment {
    /// `---` — default left alignment.
    Left,
    /// `:---:` — explicit center.
    Center,
    /// `---:` — explicit right.
    Right,
}

/// Extract column alignments for one pipe-table block whose source spans
/// `block_src`. Returns one alignment per delimiter cell; an empty vector
/// when no delimiter row is present (malformed table).
///
/// `block_src` is the raw block source (newlines included). The delimiter
/// row is conventionally the second line.
#[must_use]
pub fn column_alignments(block_src: &str) -> Vec<TableAlignment> {
    let lines: Vec<&str> = block_src.lines().collect();
    if lines.len() < 2 {
        return Vec::new();
    }
    let delim_row = lines[1];
    let mut out = Vec::new();
    for cell in delim_row.split('|') {
        let trimmed = cell.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.chars().all(|c| matches!(c, '-' | ':' | ' ')) {
            return Vec::new();
        }
        let starts = trimmed.starts_with(':');
        let ends = trimmed.ends_with(':');
        let align = match (starts, ends) {
            (true, true) => TableAlignment::Center,
            (false, true) => TableAlignment::Right,
            _ => TableAlignment::Left,
        };
        out.push(align);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_default() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert_eq!(
            column_alignments(src),
            vec![TableAlignment::Left, TableAlignment::Left]
        );
    }

    #[test]
    fn center_and_right() {
        let src = "| a | b | c |\n|:---:|---:|:---|\n| 1 | 2 | 3 |\n";
        assert_eq!(
            column_alignments(src),
            vec![
                TableAlignment::Center,
                TableAlignment::Right,
                TableAlignment::Left
            ]
        );
    }

    #[test]
    fn malformed_returns_empty() {
        let src = "| a |\nnotadelim\n";
        assert!(column_alignments(src).is_empty());
    }

    #[test]
    fn missing_delim_row_returns_empty() {
        let src = "| only one row |\n";
        assert!(column_alignments(src).is_empty());
    }
}
