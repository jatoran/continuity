use continuity_buffer::Buffer;
use ropey::Rope;

use crate::edit_planning::line_content_end;
/// Sorted, deduplicated list of line numbers covered by the buffer's
/// selections, clipped to the rope length. Shared with
/// `edit_markdown_blocks.rs`.
pub(crate) fn lines_in(buffer: &Buffer) -> Vec<usize> {
    let mut out = Vec::new();
    let len_lines = buffer.rope().len_lines();
    for selection in buffer.selections() {
        let range = selection.ordered_range();
        for line in (range.start.line as usize)..=(range.end.line as usize) {
            if line < len_lines && !out.contains(&line) {
                out.push(line);
            }
        }
    }
    out.sort_unstable();
    out
}

/// Heading level (1..=6) of a single line, or `0` when the line is not a
/// markdown ATX heading. Shared with `edit_markdown_blocks.rs`.
pub(crate) fn heading_level(text: &str) -> u8 {
    let mut level = 0u8;
    for c in text.chars().take(7) {
        if c == '#' && level < 6 {
            level += 1;
        } else if c == ' ' && level > 0 {
            return level;
        } else {
            return 0;
        }
    }
    0
}

/// Strip the `#`-prefix and following space from an ATX heading line. When
/// the line is not a heading, returns the original text. Shared with
/// `edit_markdown_blocks.rs`.
pub(crate) fn strip_heading_prefix(text: &str) -> &str {
    let mut idx = 0;
    let bytes = text.as_bytes();
    while idx < bytes.len() && bytes[idx] == b'#' && idx < 6 {
        idx += 1;
    }
    if idx == 0 {
        return text;
    }
    if idx < bytes.len() && bytes[idx] == b' ' {
        return &text[idx + 1..];
    }
    text
}

pub(crate) fn leading_whitespace_len(text: &str) -> usize {
    text.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(char::len_utf8)
        .sum()
}

/// Read a single line's content (without trailing newline) as an owned
/// string. Shared with `edit_markdown_blocks.rs`.
pub(crate) fn line_text(rope: &Rope, line: usize) -> String {
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    rope.byte_slice(start..end).to_string()
}

/// Walk upward from `line` until we hit an ATX heading; return its line
/// index. Shared with `edit_markdown_blocks.rs`.
pub(crate) fn enclosing_heading_line(rope: &Rope, line: usize) -> Option<usize> {
    let mut probe = line;
    loop {
        let text = line_text(rope, probe);
        if heading_level(&text) > 0 {
            return Some(probe);
        }
        if probe == 0 {
            return None;
        }
        probe -= 1;
    }
}

/// Walk upward from `before_inclusive` to find the previous heading line.
/// Shared with `edit_markdown_blocks.rs`.
pub(crate) fn previous_section_start(rope: &Rope, before_inclusive: usize) -> Option<usize> {
    let mut probe = before_inclusive;
    loop {
        if heading_level(&line_text(rope, probe)) > 0 {
            return Some(probe);
        }
        if probe == 0 {
            return None;
        }
        probe -= 1;
    }
}

/// If the line at `line` is a heading, return its level; otherwise `None`.
/// Shared with `edit_markdown_blocks.rs`.
pub(crate) fn next_heading_level(rope: &Rope, line: usize) -> Option<u8> {
    let lvl = heading_level(&line_text(rope, line));
    if lvl > 0 {
        Some(lvl)
    } else {
        None
    }
}

/// Last line of the section starting at `start` of given `level` —
/// extends until the next heading at the same or lower level (or EOF).
/// Shared with `edit_markdown_blocks.rs`.
pub(crate) fn section_end_line(rope: &Rope, start: usize, level: u8) -> usize {
    let mut last = start;
    let len = rope.len_lines();
    for line in (start + 1)..len {
        let lvl = heading_level(&line_text(rope, line));
        if lvl > 0 && lvl <= level {
            return last;
        }
        last = line;
    }
    last
}
