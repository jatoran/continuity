//! Pure byte/rope helper functions for [`crate::selection`].
//!
//! Extracted from `selection.rs` in Phase 17.9 §J cleanup to keep that
//! file under the 600-line cap. The split is by responsibility: these
//! are stateless, side-effect-free helpers (word-boundary scanning,
//! line slicing, paragraph stepping) — distinct from the `impl Window`
//! selection-mutation methods that consume them.

use continuity_text::{select, Position, Selection};
use ropey::Rope;

pub(crate) fn move_paragraph(rope: &Rope, position: Position, delta: i32) -> Position {
    let total = rope.len_lines();
    if total == 0 {
        return position;
    }
    let mut line = position.line as i64;
    let step = if delta < 0 { -1_i64 } else { 1_i64 };
    let mut remaining = delta.unsigned_abs();
    while remaining > 0 {
        line += step;
        if line < 0 || line as usize >= total {
            line = line.clamp(0, total as i64 - 1);
            break;
        }
        let text = line_text(rope, line as usize);
        if text.trim().is_empty() {
            remaining -= 1;
        }
    }
    let line_idx = line.clamp(0, total as i64 - 1) as usize;
    let start = rope.line_to_byte(line_idx);
    Position::from_byte_offset(rope, start).unwrap_or(position)
}

pub(crate) fn move_word(rope: &Rope, position: Position, delta: i32) -> Position {
    let bytes: Vec<u8> = rope.bytes().collect();
    let start = position.to_byte_offset(rope).unwrap_or(0);
    let target = if delta < 0 {
        word_boundary_before(&bytes, start)
    } else {
        word_boundary_after(&bytes, start)
    };
    Position::from_byte_offset(rope, target).unwrap_or(position)
}

pub(crate) fn word_boundary_before(bytes: &[u8], caret: usize) -> usize {
    let mut i = caret;
    while i > 0 && is_blank(bytes[i - 1]) {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }
    let word = is_word_byte(bytes[i - 1]);
    while i > 0 && is_word_byte(bytes[i - 1]) == word && !is_blank(bytes[i - 1]) {
        i -= 1;
    }
    i
}

pub(crate) fn word_boundary_after(bytes: &[u8], caret: usize) -> usize {
    let len = bytes.len();
    let mut i = caret;
    while i < len && is_blank(bytes[i]) {
        i += 1;
    }
    if i >= len {
        return len;
    }
    let word = is_word_byte(bytes[i]);
    while i < len && is_word_byte(bytes[i]) == word && !is_blank(bytes[i]) {
        i += 1;
    }
    i
}

pub(crate) fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

pub(crate) fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

pub(crate) fn line_text(rope: &Rope, line: usize) -> String {
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    rope.byte_slice(start..end).to_string()
}

/// Byte-in-line offset of the first non-whitespace character on `line`.
/// Returns the line's content length when the line is blank or all
/// whitespace — smart-home's "toggle" then degenerates to a no-op move
/// to column 0, which is the right behaviour.
pub(crate) fn first_non_ws_byte_in_line(rope: &Rope, line: usize) -> usize {
    if line >= rope.len_lines() {
        return 0;
    }
    let slice = rope.line(line);
    let mut offset = 0usize;
    for ch in slice.chars() {
        if ch == '\n' || ch == '\r' {
            break;
        }
        if !ch.is_whitespace() {
            return offset;
        }
        offset += ch.len_utf8();
    }
    offset
}

pub(crate) fn line_content_end(rope: &Rope, line: usize) -> usize {
    let start = rope.line_to_byte(line);
    let next = if line + 1 < rope.len_lines() {
        rope.line_to_byte(line + 1)
    } else {
        rope.len_bytes()
    };
    let mut end = next;
    let text = rope.byte_slice(start..next).to_string();
    if text.ends_with('\n') {
        end = end.saturating_sub(1);
        if text.ends_with("\r\n") {
            end = end.saturating_sub(1);
        }
    }
    end
}

pub(crate) fn primary_match_text(rope: &Rope, selections: &[Selection]) -> Option<(String, usize)> {
    let primary = *selections.first()?;
    let selection = if primary.is_collapsed() {
        select::word_at(rope, primary.head)
    } else {
        primary
    };
    let range = selection.ordered_range();
    let start = range.start.to_byte_offset(rope).ok()?;
    let end = range.end.to_byte_offset(rope).ok()?;
    if start == end {
        return None;
    }
    Some((rope.byte_slice(start..end).to_string(), end))
}
