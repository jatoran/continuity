//! Selection-session memory for one editor surface.

use std::collections::{HashMap, VecDeque};

use continuity_buffer::BufferId;
use continuity_text::Position;

const LAST_EDIT_STACK_CAPACITY: usize = 16;

/// Selection state derived from editor interaction rather than host chrome.
///
/// **Thread ownership:** the surface's UI thread is the sole writer. Buffer
/// selections remain engine-owned; this state only remembers UI motion intent
/// and navigation history.
#[derive(Debug, Default)]
pub(crate) struct SelectionState {
    /// Sticky source columns for vertical selection motion.
    pub(crate) intended_columns: Vec<u32>,
    /// Sticky display columns for soft-wrapped vertical motion.
    pub(crate) intended_display_columns: Vec<u32>,
    /// Selection-head fingerprint associated with sticky columns.
    pub(crate) intended_columns_for: Vec<Position>,
    /// Recent edit positions per buffer, newest at the back.
    pub(crate) last_edit_stack: HashMap<BufferId, VecDeque<Position>>,
}

impl SelectionState {
    /// Remember the newest edit position for one buffer.
    pub(crate) fn remember_edit(&mut self, buffer_id: BufferId, position: Position) {
        let stack = self.last_edit_stack.entry(buffer_id).or_default();
        if stack
            .back()
            .is_some_and(|previous| previous.line == position.line)
        {
            if let Some(previous) = stack.back_mut() {
                *previous = position;
            }
            return;
        }
        stack.push_back(position);
        while stack.len() > LAST_EDIT_STACK_CAPACITY {
            stack.pop_front();
        }
    }

    /// Take the newest remembered edit position for one buffer.
    pub(crate) fn take_last_edit(&mut self, buffer_id: BufferId) -> Option<Position> {
        self.last_edit_stack.get_mut(&buffer_id)?.pop_back()
    }
}

#[cfg(test)]
mod tests {
    use super::{SelectionState, LAST_EDIT_STACK_CAPACITY};
    use continuity_buffer::BufferId;
    use continuity_text::Position;

    #[test]
    fn last_edit_memory_deduplicates_lines_and_stays_bounded() {
        let buffer_id = BufferId::new();
        let mut state = SelectionState::default();
        state.remember_edit(buffer_id, Position::new(0, 1));
        state.remember_edit(buffer_id, Position::new(0, 4));
        for line in 1..=(LAST_EDIT_STACK_CAPACITY as u32 + 2) {
            state.remember_edit(buffer_id, Position::new(line, 0));
        }

        assert_eq!(
            state
                .last_edit_stack
                .get(&buffer_id)
                .map(|stack| stack.len()),
            Some(LAST_EDIT_STACK_CAPACITY)
        );
        assert_eq!(
            state.take_last_edit(buffer_id),
            Some(Position::new(LAST_EDIT_STACK_CAPACITY as u32 + 2, 0))
        );
    }

    #[test]
    fn last_edit_memory_is_isolated_per_buffer() {
        let first = BufferId::new();
        let second = BufferId::new();
        let mut state = SelectionState::default();
        state.remember_edit(first, Position::new(3, 1));
        state.remember_edit(second, Position::new(8, 2));

        assert_eq!(state.take_last_edit(first), Some(Position::new(3, 1)));
        assert_eq!(state.take_last_edit(second), Some(Position::new(8, 2)));
    }
}
