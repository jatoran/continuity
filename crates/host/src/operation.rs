//! Typed engine operations shared by bindings and native adapters.

use continuity_buffer::{BufferId, Revision};
use continuity_engine::{ChangeBatch, Engine, SelectionEdit};
use continuity_text::{select, EditOp, Selection};

use crate::Error;

/// An editor-owned operation. Desktop files, panes, tabs, windows, and
/// settings commands are deliberately absent from this enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorOperation {
    /// Apply one atomic source edit.
    ApplyEdit(EditOp),
    /// Plan and apply one selection-aware edit.
    ApplySelectionEdit(SelectionEdit),
    /// Replace the active selection set.
    SetSelections(Vec<Selection>),
    /// Select the word at every active selection head.
    SelectWord,
    /// Select the source line at every active selection head.
    SelectLine,
    /// Undo the current group.
    Undo,
    /// Redo the preferred child group.
    Redo,
    /// Redo an alternate child group.
    RedoAlternate,
    /// Reconcile the complete host document when it differs.
    ReconcileText(String),
}

/// Revisioned operation envelope supplied by a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationRequest {
    /// Target buffer.
    pub buffer_id: BufferId,
    /// Optional optimistic-concurrency guard.
    pub expected_revision: Option<Revision>,
    /// Host-supplied wall-clock milliseconds.
    pub timestamp_ms: i64,
    /// Typed operation.
    pub operation: EditorOperation,
}

/// Result of one typed engine operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationResult {
    /// Text change, if the operation mutated source text.
    pub change: Option<ChangeBatch>,
    /// Whether selection state changed without a text batch.
    pub selections_changed: bool,
    /// Revision after the operation.
    pub revision: Revision,
}

/// Apply one revision-checked operation to a synchronous engine.
///
/// # Errors
///
/// Returns [`Error::RevisionMismatch`] for stale guarded requests, or wraps
/// the underlying engine error.
pub fn apply_editor_operation(
    engine: &mut Engine,
    request: &OperationRequest,
) -> Result<OperationResult, Error> {
    let actual = engine
        .revision(request.buffer_id)
        .ok_or(Error::UnknownBuffer)?;
    if let Some(expected) = request.expected_revision {
        if expected != actual {
            return Err(Error::RevisionMismatch { expected, actual });
        }
    }

    let (change, selections_changed) = match &request.operation {
        EditorOperation::ApplyEdit(op) => (
            Some(engine.apply_edit(request.buffer_id, op.clone(), request.timestamp_ms)?),
            false,
        ),
        EditorOperation::ApplySelectionEdit(edit) => (
            engine.apply_selection_edit(request.buffer_id, edit, request.timestamp_ms)?,
            false,
        ),
        EditorOperation::SetSelections(selections) => {
            engine.set_selections(request.buffer_id, selections.clone())?;
            (None, true)
        }
        EditorOperation::SelectWord => {
            select_words(engine, request.buffer_id)?;
            (None, true)
        }
        EditorOperation::SelectLine => {
            select_lines(engine, request.buffer_id)?;
            (None, true)
        }
        EditorOperation::Undo => (engine.undo(request.buffer_id, request.timestamp_ms)?, false),
        EditorOperation::Redo => (engine.redo(request.buffer_id, request.timestamp_ms)?, false),
        EditorOperation::RedoAlternate => (
            engine.redo_alternate(request.buffer_id, request.timestamp_ms)?,
            false,
        ),
        EditorOperation::ReconcileText(text) => (
            engine.reconcile_text_if_revision(
                request.buffer_id,
                actual,
                text,
                request.timestamp_ms,
            )?,
            false,
        ),
    };
    let revision = engine
        .revision(request.buffer_id)
        .ok_or(Error::UnknownBuffer)?;
    Ok(OperationResult {
        change,
        selections_changed,
        revision,
    })
}

fn select_words(engine: &mut Engine, buffer_id: BufferId) -> Result<(), Error> {
    let snapshot = engine.snapshot(buffer_id).ok_or(Error::UnknownBuffer)?;
    let selections = snapshot
        .selections
        .iter()
        .map(|selection| select::word_at(snapshot.rope.rope(), selection.head))
        .collect();
    engine.set_selections(buffer_id, selections)?;
    Ok(())
}

fn select_lines(engine: &mut Engine, buffer_id: BufferId) -> Result<(), Error> {
    let snapshot = engine.snapshot(buffer_id).ok_or(Error::UnknownBuffer)?;
    let selections = snapshot
        .selections
        .iter()
        .map(|selection| select::line_at(snapshot.rope.rope(), selection.head))
        .collect();
    engine.set_selections(buffer_id, selections)?;
    Ok(())
}
