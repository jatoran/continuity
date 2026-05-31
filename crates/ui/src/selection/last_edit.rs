//! δ.1 last-edit jump stack: per-buffer ring of recent edit positions
//! plus the pop-to-caret entry point.

use continuity_text::{Position, Selection, SelectionKind};

use crate::Window;

impl Window {
    /// δ.1 — push a position onto this buffer's last-edit stack.
    /// Bounded so a burst of single-char inserts in the same place
    /// doesn't flood the stack; deduplicated against the most-recent
    /// entry on the same line.
    pub(crate) fn push_last_edit_position(&mut self, pos: Position) {
        const LAST_EDIT_STACK_CAP: usize = 16;
        let stack = self.last_edit_stack.entry(self.buffer_id).or_default();
        if stack.back().is_some_and(|p| p.line == pos.line) {
            // Same line as last push — overwrite so the stack tracks
            // unique edit lines rather than per-keystroke positions.
            if let Some(last) = stack.back_mut() {
                *last = pos;
            }
            return;
        }
        stack.push_back(pos);
        while stack.len() > LAST_EDIT_STACK_CAP {
            stack.pop_front();
        }
    }

    /// δ.1 — pop the most recent last-edit entry and move the primary
    /// caret to it (collapsing any selection). Returns `true` when an
    /// entry was popped, `false` when the stack was empty.
    pub(crate) fn goto_last_edit_impl(&mut self) -> bool {
        let pos = {
            let Some(stack) = self.last_edit_stack.get_mut(&self.buffer_id) else {
                return false;
            };
            stack.pop_back()
        };
        let Some(pos) = pos else {
            return false;
        };
        let sel = Selection {
            anchor: pos,
            head: pos,
            kind: SelectionKind::Caret,
        };
        let _ = self.editor.set_selections(self.buffer_id, vec![sel]);
        true
    }
}
