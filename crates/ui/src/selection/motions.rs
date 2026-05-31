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

    /// Smart-home: toggle the caret between column 0 and the line's first
    /// non-whitespace byte. Pressing `Home` once lands on first-non-ws;
    /// pressing again jumps to column 0 — and back. Mirrors Sublime /
    /// VS Code behaviour and spec §12.
    pub(crate) fn smart_home_selection(&mut self, extend: bool) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| {
                    move_selection(rope, *selection, extend, |rope, head| {
                        let line = head.line as usize;
                        let first = first_non_ws_byte_in_line(rope, line);
                        let target = if head.byte_in_line as usize == first {
                            0
                        } else {
                            first as u32
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
