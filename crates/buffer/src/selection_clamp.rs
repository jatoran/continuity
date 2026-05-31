//! Selection canonicalization against a buffer rope.
//!
//! `core` is the only writer of `Buffer`, but UI hit-tests and restored
//! view state can still hand it stale source positions. Clamp at the
//! buffer boundary so every stored selection remains valid for the rope.

use continuity_text::{Position, Selection};
use ropey::Rope;

pub(crate) fn clamp_selection_to_rope(rope: &Rope, selection: Selection) -> Selection {
    Selection::new(
        clamp_position_to_rope(rope, selection.anchor),
        clamp_position_to_rope(rope, selection.head),
        selection.kind,
    )
}

fn clamp_position_to_rope(rope: &Rope, position: Position) -> Position {
    let total_lines = rope.len_lines();
    if total_lines == 0 {
        return Position::ZERO;
    }
    let line = (position.line as usize).min(total_lines - 1);
    let line_start = rope.line_to_byte(line);
    let line_end = line_content_end_byte(rope, line);
    let byte = line_start
        .saturating_add(position.byte_in_line as usize)
        .min(line_end)
        .min(rope.len_bytes());
    let byte = snap_byte_to_char_boundary(rope, line_start, line_end, byte);
    Position::from_byte_offset(rope, byte).unwrap_or(Position::ZERO)
}

fn line_content_end_byte(rope: &Rope, line: usize) -> usize {
    let start = rope.line_to_byte(line);
    let next = if line + 1 < rope.len_lines() {
        rope.line_to_byte(line + 1)
    } else {
        rope.len_bytes()
    };
    let mut end = next;
    let slice = rope.byte_slice(start..next).to_string();
    if slice.ends_with('\n') {
        end = end.saturating_sub(1);
        if slice.ends_with("\r\n") {
            end = end.saturating_sub(1);
        }
    }
    end
}

fn snap_byte_to_char_boundary(
    rope: &Rope,
    line_start: usize,
    line_end: usize,
    byte: usize,
) -> usize {
    let Some(line_text) = rope.get_byte_slice(line_start..line_end) else {
        return line_start;
    };
    let line_text = line_text.to_string();
    let mut local = byte.saturating_sub(line_start).min(line_text.len());
    while local > 0 && !line_text.is_char_boundary(local) {
        local = local.saturating_sub(1);
    }
    line_start + local
}

#[cfg(test)]
mod tests {
    use continuity_text::{Position, Selection, SelectionKind};

    use super::*;

    #[test]
    fn clamp_snaps_interior_utf8_byte_to_same_line_boundary() {
        let rope = Rope::from_str("a\né");
        let selection = Selection::new(
            Position::new(1, 1),
            Position::new(1, 1),
            SelectionKind::Caret,
        );
        let clamped = clamp_selection_to_rope(&rope, selection);
        assert_eq!(clamped.head, Position::new(1, 0));
    }
}
