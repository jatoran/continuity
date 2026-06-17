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
