//! [`EditorHandle`] undo/redo client methods.

use continuity_buffer::{BufferId, Revision};

use crate::handle::EditorHandle;
use crate::message::EditorMessage;
use crate::Error;

impl EditorHandle {
    /// Undo the most-recent group on `buffer_id`.
    ///
    /// # Errors
    ///
    /// Returns whatever the core thread reports.
    pub fn undo(&self, buffer_id: BufferId) -> Result<Option<Revision>, Error> {
        self.round_trip(|reply| EditorMessage::Undo { buffer_id, reply })
    }

    /// Redo the most-recent child of the buffer's current undo head.
    ///
    /// # Errors
    ///
    /// Returns whatever the core thread reports.
    pub fn redo(&self, buffer_id: BufferId) -> Result<Option<Revision>, Error> {
        self.round_trip(|reply| EditorMessage::Redo { buffer_id, reply })
    }

    /// Cycle to (and apply) an alternate sibling of the most-recent redo
    /// target.
    ///
    /// # Errors
    ///
    /// Returns whatever the core thread reports.
    pub fn redo_alternate_branch(&self, buffer_id: BufferId) -> Result<Option<Revision>, Error> {
        self.round_trip(|reply| EditorMessage::RedoAlternateBranch { buffer_id, reply })
    }

    /// Log the current undo head + immediate children. (Phase 8 will front
    /// this with a real palette-style picker.)
    ///
    /// # Errors
    ///
    /// Returns whatever the core thread reports.
    pub fn undo_tree_pick(&self, buffer_id: BufferId) -> Result<(), Error> {
        self.round_trip(|reply| EditorMessage::UndoTreePick { buffer_id, reply })
    }
}
