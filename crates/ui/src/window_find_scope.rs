//! Window helpers for find-bar selection scope.

use crate::Window;

impl Window {
    /// Current editor selections converted to byte ranges for find scope.
    pub(crate) fn current_find_selection_ranges(&self) -> Vec<(usize, usize)> {
        self.editor
            .snapshot(self.buffer_id)
            .map(|snap| {
                crate::find_scope::selected_byte_ranges(
                    snap.rope_snapshot().rope(),
                    snap.selections(),
                )
            })
            .unwrap_or_default()
    }
}
