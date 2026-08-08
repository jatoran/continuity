//! Region-selecting commands: select word / line / paragraph / all,
//! plus the markdown-aware smart-expand ladder.

use continuity_decorate::{spans::block_spans, MarkdownParser};
use continuity_text::{select, Position, Selection, SelectionKind};
use ropey::Rope;

use crate::Window;

impl Window {
    pub(crate) fn select_word(&mut self) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| select::word_at(rope, selection.head))
                .collect()
        })
    }

    /// Expand ONLY the newest (last) selection to the word under its head,
    /// leaving every prior selection's range untouched.
    ///
    /// Item 2 — Ctrl+double-click appends a fresh caret (via
    /// [`Self::add_cursor_at_pixel`]) and then grows that one caret into a
    /// word range. [`Self::select_word`] cannot be reused here because it
    /// maps over *all* selections and would re-snap the user's existing
    /// ranges (collapsing a deliberate multi-line span back to a single
    /// word). The empty-selection case is a no-op.
    pub(crate) fn select_word_on_last(&mut self) -> bool {
        self.map_selections(|rope, selections| {
            let mut next: Vec<Selection> = selections.to_vec();
            if let Some(last) = next.last_mut() {
                *last = select::word_at(rope, last.head);
            }
            next
        })
    }

    /// Preserve the selected word as the granularity anchor for the active
    /// double-click drag. A tiny pointer move inside the same word therefore
    /// leaves the complete word selected; a real drag grows by whole words.
    pub(crate) fn remember_word_drag_origin(&mut self, use_last_selection: bool) {
        self.surface.pointer.word_drag_origin = self.current_snapshot().and_then(|snapshot| {
            if use_last_selection {
                snapshot.selections().last().copied()
            } else {
                snapshot.selections().first().copied()
            }
        });
    }

    pub(crate) fn extend_word_drag_at_pixel(
        &mut self,
        x: i32,
        y: i32,
        use_last_selection: bool,
    ) -> bool {
        let Some(origin) = self.surface.pointer.word_drag_origin else {
            return false;
        };
        let Some(target) = self.client_to_buffer_position(x, y) else {
            return false;
        };
        let Some(snapshot) = self.current_snapshot() else {
            return false;
        };
        let rope = snapshot.rope_snapshot().rope();
        let target_word = select::word_at(rope, target);
        let next = extend_word_selection(origin, target_word, target);
        let mut selections = snapshot.selections().to_vec();
        let selection = if use_last_selection {
            selections.last_mut()
        } else {
            selections.first_mut()
        };
        let Some(selection) = selection else {
            return false;
        };
        if *selection == next {
            return false;
        }
        *selection = next;
        self.editor
            .set_selections(self.buffer_id, selections)
            .is_ok()
    }

    pub(crate) fn select_line(&mut self) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| select::line_at(rope, selection.head))
                .collect()
        })
    }

    pub(crate) fn select_paragraph(&mut self) -> bool {
        self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| select::paragraph_at(rope, selection.head))
                .collect()
        })
    }

    pub(crate) fn select_all(&mut self) -> bool {
        self.map_selections(|rope, _selections| {
            let last_line = rope.len_lines().saturating_sub(1);
            let end_byte = if rope.len_lines() == 0 {
                0
            } else {
                rope.line(last_line).len_bytes()
            };
            let end = Position::new(last_line as u32, end_byte as u32);
            vec![Selection::new(Position::ZERO, end, SelectionKind::Caret)]
        })
    }

    pub(crate) fn expand_selection_smart(&mut self) -> bool {
        let changed = self.map_selections(|rope, selections| {
            selections
                .iter()
                .map(|selection| {
                    markdown_expand_smart(rope, *selection)
                        .unwrap_or_else(|| select::expand_smart(rope, *selection))
                })
                .collect()
        });
        if changed {
            // α.1 selection-expand bounce — 80 ms tint over the new
            // boundary so the smart-expand ladder feels tactile.
            self.pulse_selection_expand_boundary();
        }
        changed
    }
}

fn extend_word_selection(origin: Selection, target_word: Selection, target: Position) -> Selection {
    let origin_range = origin.ordered_range();
    let target_range = target_word.ordered_range();
    let target_key = (target.line, target.byte_in_line);
    let origin_start_key = (origin_range.start.line, origin_range.start.byte_in_line);
    let origin_end_key = (origin_range.end.line, origin_range.end.byte_in_line);
    if target_key < origin_start_key {
        Selection::new(origin_range.end, target_range.start, SelectionKind::Caret)
    } else if target_key > origin_end_key {
        Selection::new(origin_range.start, target_range.end, SelectionKind::Caret)
    } else {
        origin
    }
}

fn markdown_expand_smart(rope: &Rope, selection: Selection) -> Option<Selection> {
    let text = rope.to_string();
    let mut parser = MarkdownParser::new().ok()?;
    let tree = parser.parse(&text, None)?;
    let current = selection.ordered_range();
    let current_start = current.start.to_byte_offset(rope).ok()?;
    let current_end = current.end.to_byte_offset(rope).ok()?;
    block_spans(&tree)
        .into_iter()
        .filter(|span| {
            span.start_byte <= current_start
                && span.end_byte >= current_end
                && (span.start_byte, span.end_byte) != (current_start, current_end)
        })
        .min_by_key(|span| span.end_byte.saturating_sub(span.start_byte))
        .map(|span| {
            let anchor =
                Position::from_byte_offset(rope, span.start_byte).unwrap_or(Position::ZERO);
            let head = Position::from_byte_offset(rope, span.end_byte).unwrap_or(anchor);
            Selection::new(anchor, head, SelectionKind::Caret)
        })
}

#[cfg(test)]
mod tests {
    use super::extend_word_selection;
    use continuity_text::{Position, Selection, SelectionKind};

    fn selection(start: u32, end: u32) -> Selection {
        Selection::new(
            Position::new(0, start),
            Position::new(0, end),
            SelectionKind::Caret,
        )
    }

    #[test]
    fn pointer_motion_inside_double_clicked_word_preserves_whole_word() {
        let origin = selection(4, 10);
        assert_eq!(
            extend_word_selection(origin, origin, Position::new(0, 7)),
            origin
        );
    }

    #[test]
    fn double_click_drag_extends_by_complete_word_boundaries() {
        let origin = selection(4, 10);
        assert_eq!(
            extend_word_selection(origin, selection(12, 17), Position::new(0, 14)),
            selection(4, 17)
        );
        assert_eq!(
            extend_word_selection(origin, selection(0, 3), Position::new(0, 1)),
            Selection::new(
                Position::new(0, 10),
                Position::new(0, 0),
                SelectionKind::Caret,
            )
        );
    }
}
