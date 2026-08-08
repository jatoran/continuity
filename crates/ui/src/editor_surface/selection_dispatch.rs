//! Surface-local effects coordinated around one selection edit.

use continuity_buffer::BufferId;
use continuity_core::{EditorSnapshot, SelectionEdit};
use continuity_text::Position;

use super::selection::SelectionState;

/// Data captured before a selection edit enters the native core actor.
///
/// The desktop adapter still owns vault autosave, persistence indicators, and
/// projection-worker scheduling. This value coordinates only surface-local
/// navigation memory and presentation requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SelectionDispatchEffects {
    pre_edit_caret: Option<Position>,
    pre_line_count: Option<usize>,
    should_pulse: bool,
}

impl SelectionDispatchEffects {
    /// Capture surface-local edit context from the pre-edit snapshot.
    pub(crate) fn capture(edit: &SelectionEdit, snapshot: Option<&EditorSnapshot>) -> Self {
        Self {
            pre_edit_caret: snapshot
                .and_then(|value| value.selections().first().map(|selection| selection.head)),
            pre_line_count: snapshot.map(|value| value.rope_snapshot().rope().len_lines()),
            should_pulse: crate::edit_pulse::is_structural_edit(edit),
        }
    }

    /// Apply effects that become valid after the core dispatch succeeds.
    pub(crate) fn apply_to(
        self,
        selection: &mut SelectionState,
        buffer_id: BufferId,
    ) -> Option<EditPulseRequest> {
        if let Some(position) = self.pre_edit_caret {
            selection.remember_edit(buffer_id, position);
        }
        if !self.should_pulse {
            return None;
        }
        Some(EditPulseRequest {
            pre_caret_line: self.pre_edit_caret?.line,
            pre_line_count: self.pre_line_count?,
        })
    }
}

/// Request to resolve and present an edit-region pulse from the post snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EditPulseRequest {
    /// Primary caret line before the edit.
    pub(crate) pre_caret_line: u32,
    /// Source line count before the edit.
    pub(crate) pre_line_count: usize,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use continuity_buffer::{BufferId, Revision, RopeSnapshot};
    use continuity_core::{EditorSnapshot, SelectionEdit};
    use continuity_text::{Position, Selection};
    use ropey::Rope;

    use super::{EditPulseRequest, SelectionDispatchEffects};
    use crate::editor_surface::selection::SelectionState;

    fn snapshot() -> EditorSnapshot {
        EditorSnapshot {
            rope: RopeSnapshot::new(Arc::new(Rope::from_str("first\nsecond")), Revision::INITIAL),
            selections: vec![Selection::caret_at(Position::new(1, 3))],
            file: None,
            is_read_only: false,
        }
    }

    #[test]
    fn structural_edit_remembers_position_and_requests_pulse() {
        let buffer_id = BufferId::new();
        let mut selection = SelectionState::default();
        let snapshot = snapshot();
        let effects =
            SelectionDispatchEffects::capture(&SelectionEdit::DuplicateLine, Some(&snapshot));

        assert_eq!(
            effects.apply_to(&mut selection, buffer_id),
            Some(EditPulseRequest {
                pre_caret_line: 1,
                pre_line_count: 2,
            })
        );
        assert_eq!(
            selection.take_last_edit(buffer_id),
            Some(Position::new(1, 3))
        );
    }

    #[test]
    fn typing_remembers_position_without_pulse() {
        let buffer_id = BufferId::new();
        let mut selection = SelectionState::default();
        let snapshot = snapshot();
        let effects = SelectionDispatchEffects::capture(
            &SelectionEdit::InsertText("x".to_owned()),
            Some(&snapshot),
        );

        assert_eq!(effects.apply_to(&mut selection, buffer_id), None);
        assert_eq!(
            selection.take_last_edit(buffer_id),
            Some(Position::new(1, 3))
        );
    }

    #[test]
    fn missing_snapshot_has_no_surface_effects() {
        let buffer_id = BufferId::new();
        let mut selection = SelectionState::default();
        let effects = SelectionDispatchEffects::capture(&SelectionEdit::DuplicateLine, None);

        assert_eq!(effects.apply_to(&mut selection, buffer_id), None);
        assert_eq!(selection.take_last_edit(buffer_id), None);
    }
}
