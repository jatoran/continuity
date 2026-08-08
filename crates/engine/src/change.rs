//! Storage-neutral mutation records returned to host adapters.

use continuity_buffer::{BufferId, Revision, UndoGroupId};
use continuity_text::{EditOp, Selection};

use crate::RopeEditDeltaWithPoints;

/// The high-level operation that produced a [`ChangeBatch`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationKind {
    /// A caller supplied one atomic edit.
    RawEdit,
    /// A caller supplied a grouped list of edits.
    GroupedEdit,
    /// The engine planned a selection-aware edit.
    SelectionEdit,
    /// An undo group was reversed.
    Undo,
    /// An undo group was replayed.
    Redo,
}

/// Metadata for an undo group newly minted by a mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UndoGroupMeta {
    /// Stable group identifier.
    pub id: UndoGroupId,
    /// Parent at the time the group was created.
    pub parent: Option<UndoGroupId>,
    /// Host-supplied wall-clock milliseconds.
    pub timestamp_ms: i64,
    /// Command name shown in history surfaces.
    pub command: String,
}

/// One atomic edit and the information a host persistence adapter needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedChange {
    /// Applied operation.
    pub op: EditOp,
    /// Text removed by the operation, empty for inserts.
    pub removed_text: String,
    /// Revision before the operation.
    pub revision_before: Revision,
    /// Revision after the operation.
    pub revision_after: Revision,
    /// Selection state recorded before the operation.
    pub selections_before: Vec<Selection>,
    /// Selection state recorded after the operation.
    pub selections_after: Vec<Selection>,
    /// Running checksum immediately after this operation.
    pub checksum_after: u64,
}

/// A complete synchronous engine mutation, ready for host-side persistence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeBatch {
    /// Mutated buffer.
    pub buffer_id: BufferId,
    /// High-level mutation kind.
    pub kind: MutationKind,
    /// Host-supplied wall-clock milliseconds.
    pub timestamp_ms: i64,
    /// Command identity for history and host event logs.
    pub command: String,
    /// Revision before the batch.
    pub revision_before: Revision,
    /// Revision after the batch.
    pub revision_after: Revision,
    /// Undo group that owns the edit rows.
    pub undo_group_id: UndoGroupId,
    /// Present only when this batch created the group.
    pub new_undo_group: Option<UndoGroupMeta>,
    /// Selection state before the complete mutation.
    pub selections_before: Vec<Selection>,
    /// Selection state after the complete mutation.
    pub selections_after: Vec<Selection>,
    /// Atomic changes in application order.
    pub changes: Vec<AppliedChange>,
    /// Incrementally maintained checksum after the batch.
    pub checksum_after: u64,
    /// Rope deltas in application order.
    pub deltas: Vec<RopeEditDeltaWithPoints>,
}

/// Lightweight revisioned events emitted after synchronous operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineEvent {
    /// A buffer entered this engine.
    BufferOpened {
        /// Opened buffer.
        id: BufferId,
    },
    /// A text mutation completed.
    Changed {
        /// Mutated buffer.
        id: BufferId,
        /// Revision after the mutation.
        revision: Revision,
        /// Mutation kind.
        kind: MutationKind,
    },
    /// Selections changed without a text mutation.
    SelectionsChanged {
        /// Buffer whose selections changed.
        id: BufferId,
    },
    /// A buffer left this engine.
    BufferClosed {
        /// Closed buffer.
        id: BufferId,
    },
}
