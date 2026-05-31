//! [`EditorHandle`] edit and selection-mutation methods.
//!
//! [`EditorHandle::apply_edit`] always starts a fresh undo group;
//! [`EditorHandle::apply_selection_edit`] funnels through the
//! selection-aware planner. Selection mutations ride
//! [`EditorHandle::set_selections`] (replace wholesale) or
//! [`EditorHandle::mutate_selections`] (in-place closure on the core
//! thread).

use continuity_buffer::{BufferId, Revision};
use continuity_text::{EditOp, Selection};

use crate::handle::EditorHandle;
use crate::message::EditorMessage;
use crate::selection_edit::SelectionEdit;
use crate::Error;

impl EditorHandle {
    /// Apply `op` to `buffer_id`. Always starts a new (discrete) undo group.
    ///
    /// # Errors
    ///
    /// Returns whatever the core thread reports.
    pub fn apply_edit(&self, buffer_id: BufferId, op: EditOp) -> Result<Revision, Error> {
        self.apply_edit_with_seq(buffer_id, op, None)
    }

    /// As [`Self::apply_edit`] but tags the cross-thread message with a
    /// caller-provided edit sequence number for trace correlation. The
    /// core thread binds the seq to its `edit_seq` trace thread-local
    /// for the duration of the apply.
    pub fn apply_edit_with_seq(
        &self,
        buffer_id: BufferId,
        op: EditOp,
        edit_seq: Option<u64>,
    ) -> Result<Revision, Error> {
        self.round_trip(|reply| EditorMessage::ApplyEdit {
            buffer_id,
            op,
            edit_seq,
            reply,
        })
    }

    /// Apply a selection-aware edit. Each call lands as one undo group;
    /// rapid `editor.insert_char` typing may extend a prior group per spec
    /// §8 (handled by the core-thread orchestrator).
    ///
    /// # Errors
    ///
    /// Returns whatever the core thread reports.
    pub fn apply_selection_edit(
        &self,
        buffer_id: BufferId,
        edit: SelectionEdit,
    ) -> Result<Option<Revision>, Error> {
        self.apply_selection_edit_with_seq(buffer_id, edit, None)
    }

    /// As [`Self::apply_selection_edit`] but tags the cross-thread
    /// message with a caller-provided edit sequence number for trace
    /// correlation. See [`Self::apply_edit_with_seq`].
    pub fn apply_selection_edit_with_seq(
        &self,
        buffer_id: BufferId,
        edit: SelectionEdit,
        edit_seq: Option<u64>,
    ) -> Result<Option<Revision>, Error> {
        self.round_trip(|reply| EditorMessage::ApplySelectionEdit {
            buffer_id,
            edit,
            edit_seq,
            reply,
        })
    }

    /// Apply `ops` as one discrete undo group.
    ///
    /// # Errors
    ///
    /// Returns whatever the core thread reports.
    pub fn apply_edit_group(
        &self,
        buffer_id: BufferId,
        ops: Vec<EditOp>,
        selections_after: Vec<Selection>,
        command_name: &'static str,
    ) -> Result<Option<Revision>, Error> {
        self.round_trip(|reply| EditorMessage::ApplyEditGroup {
            buffer_id,
            ops,
            selections_after,
            command_name,
            edit_seq: None,
            reply,
        })
    }

    /// Replace a buffer's selection set.
    ///
    /// # Errors
    ///
    /// Returns whatever the core thread reports.
    pub fn set_selections(
        &self,
        buffer_id: BufferId,
        selections: Vec<Selection>,
    ) -> Result<(), Error> {
        self.round_trip(|reply| EditorMessage::SetSelections {
            buffer_id,
            selections,
            reply,
        })
    }

    /// Mutate a buffer's selection set on the core thread.
    ///
    /// # Errors
    ///
    /// Returns whatever the core thread reports.
    pub fn mutate_selections<F>(&self, buffer_id: BufferId, f: F) -> Result<(), Error>
    where
        F: FnOnce(&mut Vec<Selection>) + Send + 'static,
    {
        self.round_trip(|reply| EditorMessage::MutateSelections {
            buffer_id,
            f: Box::new(f),
            reply,
        })
    }
}
