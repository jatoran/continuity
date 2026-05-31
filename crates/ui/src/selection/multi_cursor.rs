//! Multi-cursor commands: add-cursor on lines, add-cursor on
//! match-of-primary, column (block-wise) selection, clear secondaries.
//!
//! Free helpers `match_selection` and `dedupe` are re-used from the
//! parent module so [`crate::selection_arithmetic`] continues to
//! import them at their original path `crate::selection::{...}`.

use continuity_text::{Position, Selection, SelectionKind};
use ropey::Rope;

use crate::selection_byte_helpers::primary_match_text;
use crate::selection_vertical::move_line_with_column;
use crate::Window;

use super::{dedupe, match_selection};

impl Window {
    pub(crate) fn add_cursor_line(&mut self, delta: i32) -> bool {
        self.map_selections(|rope, selections| {
            let mut out = selections.to_vec();
            if let Some(primary) = selections.first() {
                let head = move_line(rope, primary.head, delta);
                if head != primary.head {
                    out.push(Selection::caret_at(head));
                }
            }
            dedupe(out)
        })
    }

    /// G3 — drop the primary cursor and place a new one at the next
    /// occurrence of its text. Wraps past EOF back to the start. No-op
    /// when the primary is a bare caret (no text to match) or the
    /// needle never reoccurs in the buffer.
    pub(crate) fn skip_current_match_at_selection(&mut self) -> bool {
        self.map_selections(|rope, selections| {
            let Some((needle, after)) = primary_match_text(rope, selections) else {
                return selections.to_vec();
            };
            let haystack = rope.to_string();
            // Search after the current primary, then wrap to the start.
            let found_byte = haystack[after..]
                .find(&needle)
                .map(|i| after + i)
                .or_else(|| haystack[..after.saturating_sub(needle.len())].find(&needle));
            let Some(start) = found_byte else {
                return selections.to_vec();
            };
            // Replace the primary (selection 0) with the new match;
            // keep any other secondary cursors intact.
            let mut out = selections.to_vec();
            if out.is_empty() {
                out.push(match_selection(rope, start, needle.len()));
            } else {
                out[0] = match_selection(rope, start, needle.len());
            }
            dedupe(out)
        })
    }

    pub(crate) fn add_cursor_at_next_match(&mut self) -> bool {
        self.map_selections(|rope, selections| {
            let mut out = selections.to_vec();
            if let Some((needle, after)) = primary_match_text(rope, selections) {
                let haystack = rope.to_string();
                if let Some(found) = haystack[after..].find(&needle) {
                    let start = after + found;
                    out.push(match_selection(rope, start, needle.len()));
                }
            }
            dedupe(out)
        })
    }

    pub(crate) fn add_cursor_at_all_matches(&mut self) -> bool {
        self.map_selections(|rope, selections| {
            let mut out = selections.to_vec();
            if let Some((needle, _)) = primary_match_text(rope, selections) {
                let haystack = rope.to_string();
                let mut offset = 0;
                while let Some(found) = haystack[offset..].find(&needle) {
                    let start = offset + found;
                    out.push(match_selection(rope, start, needle.len()));
                    offset = start + needle.len().max(1);
                }
            }
            dedupe(out)
        })
    }

    pub(crate) fn column_select(&mut self, delta: i32) -> bool {
        self.map_selections(|rope, selections| {
            let Some(primary) = selections.first() else {
                return vec![Selection::caret_at(Position::ZERO)];
            };
            let anchor = if primary.kind == SelectionKind::BlockWise {
                primary.anchor
            } else {
                primary.head
            };
            let head = move_line(rope, primary.head, delta);
            vec![Selection::new(anchor, head, SelectionKind::BlockWise)]
        })
    }

    pub(crate) fn clear_secondary_cursors(&mut self) -> bool {
        self.map_selections(|_rope, selections| {
            selections
                .first()
                .copied()
                .map_or_else(|| vec![Selection::caret_at(Position::ZERO)], |s| vec![s])
        })
    }
}

fn move_line(rope: &Rope, position: Position, delta: i32) -> Position {
    move_line_with_column(rope, position, delta, position.byte_in_line)
}
