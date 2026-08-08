//! Public synchronous editor facade.

use continuity_buffer::{Buffer, BufferId, Revision, RopeSnapshot, UndoGroupId};
use continuity_text::{EditOp, Position, Range, Selection};

use crate::change::{ChangeBatch, EngineEvent, MutationKind};
use crate::reconcile_diff::compute_reconcile_splice;
use crate::selection_coalesce::coalesce_selections;
use crate::selection_edit::{plan, SelectionEdit};
use crate::undo::{CoalesceKind, UndoManager};
use crate::{DeltaHistory, EngineState, Error, RopeEditDeltaWithPoints};

/// Identifier source used for undo groups.
///
/// Hosts that need reproducible operation streams can inject a deterministic
/// implementation with [`Engine::with_id_generator`].
pub trait IdGenerator {
    /// Return the next buffer identifier.
    fn next_buffer_id(&mut self) -> BufferId;

    /// Return the next undo-group identifier.
    fn next_undo_group_id(&mut self) -> UndoGroupId;
}

/// UUIDv7 identifier source used by the native application.
#[derive(Debug, Default)]
pub struct SystemIdGenerator;

impl IdGenerator for SystemIdGenerator {
    fn next_buffer_id(&mut self) -> BufferId {
        BufferId::new()
    }

    fn next_undo_group_id(&mut self) -> UndoGroupId {
        UndoGroupId::new()
    }
}

/// An immutable rope and selection view captured in one engine call.
#[derive(Clone)]
pub struct EngineSnapshot {
    /// Immutable rope snapshot.
    pub rope: RopeSnapshot,
    /// Selections captured at the same revision.
    pub selections: Vec<Selection>,
    /// Whether the buffer rejects text mutations.
    pub is_read_only: bool,
}

/// Synchronous, storage-neutral editor state machine.
///
/// **Thread ownership**: the host chooses the owning thread and must keep all
/// mutable access on that thread. `Engine` does not create threads or invoke
/// host callbacks.
pub struct Engine {
    state: EngineState,
    undo: UndoManager,
    delta_history: DeltaHistory,
    events: Vec<EngineEvent>,
    ids: Box<dyn IdGenerator>,
}

impl Engine {
    /// Construct an empty engine using UUIDv7 undo-group identifiers.
    #[must_use]
    pub fn new() -> Self {
        Self::with_id_generator(Box::<SystemIdGenerator>::default())
    }

    /// Construct an empty engine with a host-provided identifier source.
    #[must_use]
    pub fn with_id_generator(ids: Box<dyn IdGenerator>) -> Self {
        Self {
            state: EngineState::new(),
            undo: UndoManager::default(),
            delta_history: DeltaHistory::default(),
            events: Vec::new(),
            ids,
        }
    }

    /// Open text with a newly generated buffer identifier.
    pub fn open_buffer(&mut self, text: &str) -> BufferId {
        let id = self.ids.next_buffer_id();
        self.adopt_buffer(Buffer::from_parts(id, text, Revision::INITIAL))
    }

    /// Load text under a host-provided id and revision.
    pub fn load_buffer(&mut self, id: BufferId, text: &str, revision: Revision) -> BufferId {
        self.adopt_buffer(Buffer::from_parts(id, text, revision))
    }

    /// Adopt a prepared buffer without performing persistence or I/O.
    pub fn adopt_buffer(&mut self, buffer: Buffer) -> BufferId {
        let id = self.state.insert(buffer);
        self.events.push(EngineEvent::BufferOpened { id });
        id
    }

    /// Remove a buffer and its transient engine state.
    pub fn close_buffer(&mut self, id: BufferId) -> Option<Buffer> {
        let buffer = self.state.remove(id)?;
        self.undo.forget(id);
        self.delta_history.forget(id);
        self.events.push(EngineEvent::BufferClosed { id });
        Some(buffer)
    }

    /// Capture a buffer snapshot.
    #[must_use]
    pub fn snapshot(&self, id: BufferId) -> Option<EngineSnapshot> {
        self.state.get(id).map(|buffer| EngineSnapshot {
            rope: buffer.snapshot(),
            selections: buffer.selections().to_vec(),
            is_read_only: buffer.is_read_only(),
        })
    }

    /// Copy a buffer's current text.
    #[must_use]
    pub fn text(&self, id: BufferId) -> Option<String> {
        self.state.get(id).map(|buffer| buffer.rope().to_string())
    }

    /// Borrow a buffer's current selections.
    #[must_use]
    pub fn selections(&self, id: BufferId) -> Option<&[Selection]> {
        self.state.get(id).map(Buffer::selections)
    }

    /// Return a buffer's current revision.
    #[must_use]
    pub fn revision(&self, id: BufferId) -> Option<Revision> {
        self.state.get(id).map(Buffer::revision)
    }

    /// Replace and normalize a buffer's selections.
    pub fn set_selections(
        &mut self,
        id: BufferId,
        mut selections: Vec<Selection>,
    ) -> Result<(), Error> {
        coalesce_selections(&mut selections);
        let buffer = self.state.get_mut(id).ok_or(Error::UnknownBuffer)?;
        buffer.set_selections(selections);
        self.events.push(EngineEvent::SelectionsChanged { id });
        Ok(())
    }

    /// Apply one raw atomic edit as a discrete undo group.
    pub fn apply_edit(
        &mut self,
        id: BufferId,
        op: EditOp,
        timestamp_ms: i64,
    ) -> Result<ChangeBatch, Error> {
        let batch = self.undo.apply_single(
            self.state.get_mut(id).ok_or(Error::UnknownBuffer)?,
            &op,
            "editor.apply_edit",
            timestamp_ms,
            self.ids.as_mut(),
        )?;
        self.record_batch(&batch);
        Ok(batch)
    }

    /// Apply caller-planned operations as one discrete undo group.
    pub fn apply_grouped_edit(
        &mut self,
        id: BufferId,
        ops: &[EditOp],
        selections_after: Vec<Selection>,
        command: &str,
        timestamp_ms: i64,
    ) -> Result<Option<ChangeBatch>, Error> {
        let buffer = self.state.get_mut(id).ok_or(Error::UnknownBuffer)?;
        let selections_before = buffer.selections().to_vec();
        let selections_after = if selections_after.is_empty() {
            selections_before.clone()
        } else {
            selections_after
        };
        let batch = self.undo.apply_group(
            buffer,
            ops,
            &selections_before,
            &selections_after,
            command,
            CoalesceKind::Discrete,
            timestamp_ms,
            MutationKind::GroupedEdit,
            self.ids.as_mut(),
        )?;
        self.record_optional_batch(batch.as_ref());
        Ok(batch)
    }

    /// Plan and apply a selection-aware edit.
    pub fn apply_selection_edit(
        &mut self,
        id: BufferId,
        edit: &SelectionEdit,
        timestamp_ms: i64,
    ) -> Result<Option<ChangeBatch>, Error> {
        let buffer = self.state.get_mut(id).ok_or(Error::UnknownBuffer)?;
        let Some(edit_plan) = plan(buffer, edit)? else {
            return Ok(None);
        };
        let batch = self.undo.apply_group(
            buffer,
            &edit_plan.ops,
            &edit_plan.selections_before,
            &edit_plan.selections_after,
            command_name_for(edit),
            coalesce_kind_for(edit),
            timestamp_ms,
            MutationKind::SelectionEdit,
            self.ids.as_mut(),
        )?;
        self.record_optional_batch(batch.as_ref());
        Ok(batch)
    }

    /// Undo the current group.
    pub fn undo(&mut self, id: BufferId, timestamp_ms: i64) -> Result<Option<ChangeBatch>, Error> {
        let batch = self.undo.undo(
            self.state.get_mut(id).ok_or(Error::UnknownBuffer)?,
            timestamp_ms,
        )?;
        self.record_optional_batch(batch.as_ref());
        Ok(batch)
    }

    /// Replay the most-recent redo child.
    pub fn redo(&mut self, id: BufferId, timestamp_ms: i64) -> Result<Option<ChangeBatch>, Error> {
        self.redo_inner(id, timestamp_ms, false)
    }

    /// Replay an alternate redo branch.
    pub fn redo_alternate(
        &mut self,
        id: BufferId,
        timestamp_ms: i64,
    ) -> Result<Option<ChangeBatch>, Error> {
        self.redo_inner(id, timestamp_ms, true)
    }

    /// Replace the full document only when the caller's revision is current.
    pub fn replace_text_if_revision(
        &mut self,
        id: BufferId,
        expected: Revision,
        text: impl Into<String>,
        timestamp_ms: i64,
    ) -> Result<ChangeBatch, Error> {
        let buffer = self.state.get(id).ok_or(Error::UnknownBuffer)?;
        let actual = buffer.revision();
        if actual != expected {
            return Err(Error::RevisionMismatch { expected, actual });
        }
        let end = Position::from_byte_offset(buffer.rope(), buffer.rope().len_bytes())?;
        let op = EditOp::replace(Range::new(Position::ZERO, end), text);
        self.apply_edit(id, op, timestamp_ms)
    }

    /// Reconcile a full host document when its expected revision is current.
    ///
    /// Returns `None` when the supplied text already matches, so loading or
    /// reconciling current state never emits a phantom edit. A real difference
    /// is applied as the minimal single splice (shared prefix and suffix kept),
    /// so selections and host line mirrors outside the changed range stay put
    /// instead of collapsing to the document end.
    pub fn reconcile_text_if_revision(
        &mut self,
        id: BufferId,
        expected: Revision,
        text: impl Into<String>,
        timestamp_ms: i64,
    ) -> Result<Option<ChangeBatch>, Error> {
        let text = text.into();
        let buffer = self.state.get(id).ok_or(Error::UnknownBuffer)?;
        let actual = buffer.revision();
        if actual != expected {
            return Err(Error::RevisionMismatch { expected, actual });
        }
        if buffer.rope() == text.as_str() {
            return Ok(None);
        }
        let splice = compute_reconcile_splice(buffer.rope(), &text);
        let start = Position::from_byte_offset(buffer.rope(), splice.start)?;
        let end = Position::from_byte_offset(buffer.rope(), splice.old_end)?;
        let replacement = text[splice.start..splice.new_end].to_string();
        let op = EditOp::replace(Range::new(start, end), replacement);
        self.apply_edit(id, op, timestamp_ms).map(Some)
    }

    /// Return byte deltas strictly newer than `since_revision`.
    #[must_use]
    pub fn deltas_since(
        &self,
        id: BufferId,
        since_revision: u64,
    ) -> (Vec<continuity_text::RopeEditDelta>, bool) {
        self.delta_history.since(id, since_revision)
    }

    /// Return position-augmented deltas strictly newer than `since_revision`.
    #[must_use]
    pub fn deltas_with_points_since(
        &self,
        id: BufferId,
        since_revision: u64,
    ) -> (Vec<RopeEditDeltaWithPoints>, bool) {
        self.delta_history.with_points_since(id, since_revision)
    }

    /// Drain events emitted since the previous call.
    pub fn drain_events(&mut self) -> Vec<EngineEvent> {
        std::mem::take(&mut self.events)
    }

    /// Borrow the lower-level state for native host queries during migration.
    #[must_use]
    pub fn state(&self) -> &EngineState {
        &self.state
    }

    /// Mutably borrow lower-level state for native host-only metadata changes.
    pub fn state_mut(&mut self) -> &mut EngineState {
        &mut self.state
    }

    fn redo_inner(
        &mut self,
        id: BufferId,
        timestamp_ms: i64,
        alternate: bool,
    ) -> Result<Option<ChangeBatch>, Error> {
        let batch = self.undo.redo(
            self.state.get_mut(id).ok_or(Error::UnknownBuffer)?,
            timestamp_ms,
            alternate,
        )?;
        self.record_optional_batch(batch.as_ref());
        Ok(batch)
    }

    fn record_optional_batch(&mut self, batch: Option<&ChangeBatch>) {
        if let Some(batch) = batch {
            self.record_batch(batch);
        }
    }

    fn record_batch(&mut self, batch: &ChangeBatch) {
        self.delta_history.push(
            batch.buffer_id,
            batch.revision_after.get(),
            batch.deltas.clone(),
        );
        self.events.push(EngineEvent::Changed {
            id: batch.buffer_id,
            revision: batch.revision_after,
            kind: batch.kind,
        });
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

fn coalesce_kind_for(edit: &SelectionEdit) -> CoalesceKind {
    match edit {
        SelectionEdit::InsertText(text) if text.chars().count() == 1 && text != "\n" => {
            CoalesceKind::Typing
        }
        SelectionEdit::DeleteBack | SelectionEdit::DeleteForward => CoalesceKind::Typing,
        _ => CoalesceKind::Discrete,
    }
}

fn command_name_for(edit: &SelectionEdit) -> &'static str {
    match edit {
        SelectionEdit::InsertText(_) => "editor.insert_text",
        SelectionEdit::DeleteBack => "editor.delete_back",
        SelectionEdit::DeleteForward => "editor.delete_forward",
        _ => "editor.selection_edit",
    }
}
