//! Last-edit jump entry point backed by surface-owned navigation memory.

use continuity_text::{Selection, SelectionKind};

use crate::Window;

impl Window {
    /// δ.1 — pop the most recent last-edit entry and move the primary
    /// caret to it (collapsing any selection). Returns `true` when an
    /// entry was popped, `false` when the stack was empty.
    pub(crate) fn goto_last_edit_impl(&mut self) -> bool {
        let pos = self.surface.selection.take_last_edit(self.buffer_id);
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
