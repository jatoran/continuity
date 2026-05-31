//! Messages and events that flow between the editor core thread and its
//! clients.

use continuity_buffer::{Buffer, BufferId, FileAssociation, Revision, RopeSnapshot};
use continuity_text::{EditOp, Selection};
use crossbeam_channel::Sender;

use crate::policy::SnapshotPolicy;
use crate::{Error, SelectionEdit};

/// Mutation closure used by [`EditorMessage::MutateSelections`].
pub(crate) type SelectionMutation = Box<dyn FnOnce(&mut Vec<Selection>) + Send>;

/// A consistent immutable view of a buffer at one core-thread turn.
#[derive(Clone)]
pub struct EditorSnapshot {
    /// Immutable rope snapshot.
    pub rope: RopeSnapshot,
    /// Selection set captured with the rope snapshot.
    pub selections: Vec<Selection>,
    /// File association captured with the rope snapshot.
    pub file: Option<FileAssociation>,
}

/// Lightweight per-buffer summary for switcher / quick-open UIs.
///
/// **Thread ownership**: produced on the core thread under the
/// `EditorMessage::ListBuffers` reply path; safe to send to any UI thread.
#[derive(Clone, Debug)]
pub struct BufferSummary {
    /// Buffer id.
    pub id: BufferId,
    /// Derived title — first non-empty trimmed line, clipped — or `None` for
    /// empty / pure-whitespace buffers.
    pub title: Option<String>,
    /// First non-empty line, full-length, trimmed of leading/trailing
    /// whitespace. Empty when the buffer is empty.
    pub first_line: String,
    /// Buffer revision at the time of the listing.
    pub revision: Revision,
    /// Total line count at the time of the listing.
    pub line_count: usize,
    /// File association when this buffer is tied to a path.
    pub file: Option<FileAssociation>,
}

/// Core-owned memory counters for trace snapshots.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CoreMemoryStats {
    /// Sum of current rope byte lengths across open buffers.
    pub rope_bytes: usize,
    /// Estimated in-memory snapshot-history bytes retained by core,
    /// excluding the live rope. Zero means no cached snapshot chain is
    /// retained beyond the current buffer contents.
    pub snapshot_history_bytes: usize,
    /// Estimated heap bytes retained by undo trees across open buffers.
    pub undo_tree_bytes: usize,
    /// Total `UndoGroup` count across every open buffer.
    pub undo_tree_groups: usize,
    /// Total `EditRecord` count across every open buffer.
    pub undo_tree_records: usize,
}

impl EditorSnapshot {
    /// Borrow the underlying rope snapshot.
    #[must_use]
    pub fn rope_snapshot(&self) -> &RopeSnapshot {
        &self.rope
    }

    /// Borrow the captured selections.
    #[must_use]
    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }
}

/// A request sent to the editor core thread.
pub enum EditorMessage {
    /// Open a fresh buffer with the given content. The core thread persists
    /// an initial snapshot at revision 0 before replying.
    OpenBuffer {
        /// Initial content.
        content: String,
        /// Reply channel for the new buffer's id.
        reply: Sender<BufferId>,
    },
    /// Open a fresh file-associated buffer with the given content.
    OpenFileBuffer {
        /// Initial content.
        content: String,
        /// File association metadata.
        file: FileAssociation,
        /// Reply channel for the new buffer's id.
        reply: Sender<BufferId>,
    },
    /// Adopt an already-constructed buffer (e.g. one rebuilt from a
    /// persisted snapshot + replayed edit log). Inserts it into state with
    /// no further persistence work — the buffer is by definition already
    /// represented on disk.
    AdoptBuffer {
        /// The buffer to adopt.
        buffer: Buffer,
        /// `seq` value to assign to the next [`Self::ApplyEdit`] for this
        /// buffer. Should be `last_seq_in_db + 1`.
        next_seq: u64,
        /// Wall-clock millis the snapshot was taken/loaded at — seeds the
        /// snapshot policy tracker so freshly-adopted buffers don't fire
        /// a snapshot immediately.
        last_snapshot_at_ms: i64,
        /// Reply channel for the buffer id (matches `buffer.id()`).
        reply: Sender<BufferId>,
    },
    /// Apply an edit op to an existing buffer.
    ApplyEdit {
        /// Target buffer.
        buffer_id: BufferId,
        /// The op to apply.
        op: EditOp,
        /// Optional UI-thread edit sequence for cross-thread trace
        /// correlation. The core thread binds this to its `edit_seq`
        /// thread-local for the duration of the apply so every emitted
        /// trace line carries the same `edit_seq=` field.
        edit_seq: Option<u64>,
        /// Reply channel for the resulting revision (or error).
        reply: Sender<Result<Revision, Error>>,
    },
    /// Apply a selection-aware edit to all selections in the target buffer.
    ApplySelectionEdit {
        /// Target buffer.
        buffer_id: BufferId,
        /// Selection-aware edit request.
        edit: SelectionEdit,
        /// Optional UI-thread edit sequence for cross-thread trace
        /// correlation. See [`Self::ApplyEdit::edit_seq`].
        edit_seq: Option<u64>,
        /// Reply channel for the final revision. `None` means no edit was
        /// applied (for example, deleting backward at document start).
        reply: Sender<Result<Option<Revision>, Error>>,
    },
    /// Apply preplanned edit ops as one undo group.
    ApplyEditGroup {
        /// Target buffer.
        buffer_id: BufferId,
        /// Ops in execution order.
        ops: Vec<EditOp>,
        /// Selection set after the grouped edit.
        selections_after: Vec<Selection>,
        /// Command name stored on the undo group.
        command_name: &'static str,
        /// Optional UI-thread edit sequence for trace correlation.
        edit_seq: Option<u64>,
        /// Reply channel for the final revision.
        reply: Sender<Result<Option<Revision>, Error>>,
    },
    /// Replace the selection set on an existing buffer.
    SetSelections {
        /// Target buffer.
        buffer_id: BufferId,
        /// New selection set. Empty input is normalized to one caret at the
        /// start of the buffer.
        selections: Vec<Selection>,
        /// Reply channel for success or failure.
        reply: Sender<Result<(), Error>>,
    },
    /// Mutate the selection set on the core thread.
    MutateSelections {
        /// Target buffer.
        buffer_id: BufferId,
        /// Mutation closure.
        f: SelectionMutation,
        /// Reply channel for success or failure.
        reply: Sender<Result<(), Error>>,
    },
    /// Take a snapshot of an existing buffer.
    Snapshot {
        /// Target buffer.
        buffer_id: BufferId,
        /// Reply channel; `None` if the buffer is unknown.
        reply: Sender<Option<EditorSnapshot>>,
    },
    /// Update a buffer's file association.
    SetFileAssociation {
        /// Target buffer.
        buffer_id: BufferId,
        /// New association, or `None` to clear.
        file: Option<FileAssociation>,
        /// Reply channel.
        reply: Sender<Result<(), Error>>,
    },
    /// Undo the most-recent group on the buffer's undo tree.
    Undo {
        /// Target buffer.
        buffer_id: BufferId,
        /// Reply channel for the new revision (or `None` if there was
        /// nothing to undo).
        reply: Sender<Result<Option<Revision>, Error>>,
    },
    /// Re-apply the most-recent child of the buffer's current undo head.
    Redo {
        /// Target buffer.
        buffer_id: BufferId,
        /// Reply channel.
        reply: Sender<Result<Option<Revision>, Error>>,
    },
    /// Cycle through siblings of the most-recent redo target.
    RedoAlternateBranch {
        /// Target buffer.
        buffer_id: BufferId,
        /// Reply channel.
        reply: Sender<Result<Option<Revision>, Error>>,
    },
    /// Log the buffer's current undo head + immediate children. The
    /// palette-style picker UI lands in Phase 8.
    UndoTreePick {
        /// Target buffer.
        buffer_id: BufferId,
        /// Reply channel for success or failure.
        reply: Sender<Result<(), Error>>,
    },
    /// Enumerate every open buffer, returning a [`BufferSummary`] per buffer.
    /// Order is unspecified; callers should sort by their own MRU policy.
    ListBuffers {
        /// Reply channel.
        reply: Sender<Vec<BufferSummary>>,
    },
    /// Summarize core-owned memory for trace attribution.
    MemoryStats {
        /// Reply channel.
        reply: Sender<CoreMemoryStats>,
    },
    /// Phase-16.5 typed owner message: replace the running snapshot
    /// policy. Takes effect for the *next* edit; the in-flight tracker
    /// values are preserved (lower thresholds may immediately fire on
    /// the next edit, which is the desired behavior).
    SetSnapshotPolicy(SnapshotPolicy),
    /// Return every rope-edit delta applied to `buffer_id` strictly
    /// after `since_revision`, in chronological (apply) order. Used by
    /// the UI to transform stale [`continuity_decorate::Decorations`]
    /// byte ranges forward through edits the worker hasn't reflected
    /// yet, so painting with a stale snapshot doesn't misalign
    /// markdown styling against the current rope. Returns an empty
    /// vec if the buffer is unknown, the requested revision is too
    /// old to be in the bounded history, or no edits have happened
    /// since.
    RopeDeltasSince {
        /// Target buffer.
        buffer_id: BufferId,
        /// Edits strictly after this revision are returned.
        since_revision: u64,
        /// Reply channel. The boolean is `true` when the history
        /// contained every revision since `since_revision` (the
        /// returned vec is complete), `false` when the requested
        /// revision was older than the oldest tracked entry — in
        /// that case the caller cannot safely transform and must
        /// drop the cached decorations.
        reply: Sender<(Vec<continuity_text::RopeEditDelta>, bool)>,
    },
    /// ε.4 — position-augmented variant of [`Self::RopeDeltasSince`]
    /// for the decoration worker's incremental tree-sitter parse.
    /// Same `covered=true|false` semantics; the returned `Vec`
    /// carries `(start_point, old_end_point, new_end_point)` next to
    /// the byte-shift component.
    RopeDeltasWithPointsSince {
        /// Target buffer.
        buffer_id: BufferId,
        /// Edits strictly after this revision are returned.
        since_revision: u64,
        /// Reply channel. The boolean follows
        /// [`Self::RopeDeltasSince`]'s convention.
        reply: Sender<(
            Vec<crate::rope_edit_delta_points::RopeEditDeltaWithPoints>,
            bool,
        )>,
    },
    /// Phase-I1: stage a label that the next snapshot committed for
    /// `buffer_id` will carry. `None` clears any staged label. The
    /// label remains pending until the snapshot policy actually fires
    /// (or shutdown flushes a final snapshot for this buffer); after
    /// the snapshot is queued, core fires `PersistClient::set_snapshot_label`
    /// for the just-written revision and clears the staged label.
    SetPendingSnapshotLabel {
        /// Target buffer.
        buffer_id: BufferId,
        /// Label to stamp onto the next snapshot, or `None` to clear.
        label: Option<String>,
    },
    /// Stop the core thread cleanly. The thread flushes a final snapshot
    /// for every dirty buffer (via blocking persist sends) before exiting.
    Shutdown,
}

/// A broadcast event from the editor core thread.
#[derive(Debug, Clone)]
pub enum EditEvent {
    /// A new buffer was opened.
    BufferOpened {
        /// The new buffer's id.
        id: BufferId,
    },
    /// An edit was applied to a buffer.
    EditApplied {
        /// The buffer's id.
        id: BufferId,
        /// The revision after the edit.
        revision: Revision,
    },
    /// A buffer's selections changed without a text edit.
    SelectionsChanged {
        /// The buffer's id.
        id: BufferId,
    },
    /// A buffer was closed.
    BufferClosed {
        /// The closed buffer's id.
        id: BufferId,
    },
    /// The core thread is shutting down.
    Shutdown,
}
