//! Source-line helper functions for display-map building.

use ropey::Rope;

use crate::fold::FoldRange;
use crate::id::{SourceByte, SourceLine};
use crate::line::DisplayLineSpec;

/// `(start_byte, end_byte_excluding_newline)`. For the last synthetic empty
/// line the rope returns `(len, len)`.
pub(in crate::builder) fn source_line_range(rope: &Rope, line: usize) -> (usize, usize) {
    let total_lines = rope.len_lines();
    let start = rope.line_to_byte(line);
    let next = if line + 1 < total_lines {
        rope.line_to_byte(line + 1)
    } else {
        rope.len_bytes()
    };
    let slice = rope.byte_slice(start..next).to_string();
    let mut end = next;
    if slice.ends_with('\n') {
        end -= 1;
        if slice.ends_with("\r\n") {
            end -= 1;
        }
    }
    (start, end)
}

pub(in crate::builder) fn read_line_text(rope: &Rope, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    rope.byte_slice(start..end).to_string()
}

fn fold_covers(folds: &[FoldRange], start: usize, end: usize) -> bool {
    if start >= end {
        return false;
    }
    folds
        .iter()
        .any(|f| f.start.as_usize() <= start && f.end.as_usize() >= end)
}

/// `true` when a source line should be hidden from the display map entirely.
pub(in crate::builder) fn line_is_hidden(
    folds: &[FoldRange],
    line_text: &str,
    line_start: usize,
    line_end: usize,
) -> bool {
    if line_text.is_empty() {
        return false;
    }
    fold_covers(folds, line_start, line_end) || is_continuity_directive_line(line_text)
}

fn is_continuity_directive_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("<!--continuity:") && trimmed.ends_with("-->")
}

/// Construct a zero-byte phantom display row for expanded inline images.
pub(in crate::builder) fn phantom_display_line(
    source_line: SourceLine,
    anchor_byte: usize,
) -> DisplayLineSpec {
    DisplayLineSpec::new(
        source_line,
        SourceByte::from_usize(anchor_byte),
        SourceByte::from_usize(anchor_byte),
        true,
        Vec::new(),
        "",
    )
}
