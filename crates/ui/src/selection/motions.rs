//! Horizontal motions (char / word / paragraph) and line / document
//! bounds motions. None of these touch caret screen-y anchoring —
//! that math lives in [`super::vertical_motion`].

use continuity_text::{Position, Selection};
use ropey::Rope;

use crate::selection_byte_helpers::{
    first_non_ws_byte_in_line, line_content_end, move_paragraph, move_word,
};
use crate::Window;

impl Window {
    pub(crate) fn move_word_selection(&mut self, delta: i32, extend: bool) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| {
                    move_selection(rope, *selection, extend, |rope, head| {
                        move_word(rope, head, delta)
                    })
                })
                .collect()
        })
    }

    pub(crate) fn move_paragraph_selection(&mut self, delta: i32, extend: bool) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| {
                    move_selection(rope, *selection, extend, |rope, head| {
                        move_paragraph(rope, head, delta)
                    })
                })
                .collect()
        })
    }

    pub(crate) fn shrink_selection_smart_at(&mut self) -> bool {
        self.map_selections(|_, selections| {
            selections
                .iter()
                .map(|selection| {
                    if selection.is_collapsed() {
                        *selection
                    } else {
                        Selection::caret_at(selection.head)
                    }
                })
                .collect()
        })
    }

    pub(crate) fn move_char_selection(&mut self, delta: i32, extend: bool) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| {
                    move_selection(rope, *selection, extend, |rope, head| {
                        move_byte(rope, head, delta)
                    })
                })
                .collect()
        })
    }

    pub(crate) fn move_line_start_selection(&mut self, extend: bool) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| {
                    move_selection(rope, *selection, extend, |_rope, head| {
                        Position::new(head.line, 0)
                    })
                })
                .collect()
        })
    }

    pub(crate) fn move_line_end_selection(&mut self, extend: bool) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| {
                    move_selection(rope, *selection, extend, |rope, head| {
                        let line = head.line as usize;
                        Position::from_byte_offset(rope, line_content_end(rope, line))
                            .unwrap_or(head)
                    })
                })
                .collect()
        })
    }

    /// Smart-home: toggle the caret between column 0 and the line's
    /// content start. Content start is the first non-whitespace byte —
    /// or, on a list-item line, the first byte *after* the list marker
    /// (and task checkbox), so `Home` on `- [ ] buy milk` lands on the
    /// `b`. Pressing `Home` again jumps to column 0 — and back. Mirrors
    /// Sublime / VS Code behaviour and spec §12.
    pub(crate) fn smart_home_selection(&mut self, extend: bool) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| {
                    move_selection(rope, *selection, extend, |rope, head| {
                        let line = head.line as usize;
                        let first = first_non_ws_byte_in_line(rope, line);
                        let content = list_item_content_byte_in_line(rope, line, first);
                        let target = if head.byte_in_line as usize == content {
                            0
                        } else {
                            content as u32
                        };
                        Position::new(head.line, target)
                    })
                })
                .collect()
        })
    }

    pub(crate) fn move_doc_start_selection(&mut self, extend: bool) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| {
                    move_selection(rope, *selection, extend, |_rope, _head| Position::ZERO)
                })
                .collect()
        })
    }

    pub(crate) fn move_doc_end_selection(&mut self, extend: bool) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| {
                    move_selection(rope, *selection, extend, |rope, _head| {
                        Position::from_byte_offset(rope, rope.len_bytes()).unwrap_or(Position::ZERO)
                    })
                })
                .collect()
        })
    }
}

/// Byte-in-line where the line's real content begins: the first
/// non-whitespace byte, advanced past a markdown list marker (`- ` /
/// `* ` / `+ ` / `N. ` / `N) `) and an optional task checkbox
/// (`[ ] ` / `[x] `) when present. Smart-home targets this so `Home`
/// on a bullet line lands on the item text first.
fn list_item_content_byte_in_line(rope: &Rope, line: usize, first_non_ws: usize) -> usize {
    if line >= rope.len_lines() {
        return first_non_ws;
    }
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    if start + first_non_ws >= end {
        return first_non_ws;
    }
    let text = rope.byte_slice(start + first_non_ws..end).to_string();
    let bytes = text.as_bytes();
    let marker_len = match bytes.first() {
        Some(b'-' | b'*' | b'+') if bytes.get(1) == Some(&b' ') => 2,
        Some(b'0'..=b'9') => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            match bytes.get(digits) {
                Some(b'.' | b')') if bytes.get(digits + 1) == Some(&b' ') => digits + 2,
                _ => 0,
            }
        }
        _ => 0,
    };
    if marker_len == 0 {
        return first_non_ws;
    }
    let mut content = marker_len;
    for checkbox in ["[ ] ", "[x] ", "[X] "] {
        if text[content..].starts_with(checkbox) {
            content += checkbox.len();
            break;
        }
    }
    first_non_ws + content
}

fn move_selection<F>(rope: &Rope, selection: Selection, extend: bool, f: F) -> Selection
where
    F: FnOnce(&Rope, Position) -> Position,
{
    let head = f(rope, selection.head);
    if extend {
        Selection::new(selection.anchor, head, selection.kind)
    } else {
        Selection::caret_at(head)
    }
}

fn move_byte(rope: &Rope, position: Position, delta: i32) -> Position {
    let byte = position.to_byte_offset(rope).unwrap_or(0);
    let target = if delta < 0 {
        let char_idx = rope.byte_to_char(byte);
        rope.char_to_byte(char_idx.saturating_sub(delta.unsigned_abs() as usize))
    } else {
        let char_idx = rope.byte_to_char(byte);
        let target = (char_idx + delta as usize).min(rope.len_chars());
        rope.char_to_byte(target)
    };
    Position::from_byte_offset(rope, target).unwrap_or(position)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_col(text: &str) -> usize {
        let rope = Rope::from_str(text);
        let first = first_non_ws_byte_in_line(&rope, 0);
        list_item_content_byte_in_line(&rope, 0, first)
    }

    #[test]
    fn content_start_skips_bullet_marker() {
        assert_eq!(content_col("- item"), 2);
        assert_eq!(content_col("* item"), 2);
        assert_eq!(content_col("  - item"), 4);
        assert_eq!(content_col("\t- item"), 3);
    }

    #[test]
    fn content_start_skips_ordered_marker_and_checkbox() {
        assert_eq!(content_col("12. item"), 4);
        assert_eq!(content_col("3) item"), 3);
        assert_eq!(content_col("- [ ] task"), 6);
        assert_eq!(content_col("- [x] task"), 6);
    }

    #[test]
    fn content_start_is_first_non_ws_for_plain_lines() {
        assert_eq!(content_col("plain"), 0);
        assert_eq!(content_col("  plain"), 2);
        assert_eq!(content_col("-not-a-bullet"), 0);
        assert_eq!(content_col("3.14 pi"), 0);
        assert_eq!(content_col(""), 0);
        assert_eq!(content_col("- "), 2);
    }
}
