//! In-memory undo grouping, coalescing, branching, and replay.

use ahash::AHashMap;
use continuity_buffer::{compute_inverse_op, Buffer, BufferId, EditRecord, UndoGroupId};
use continuity_text::{EditOp, Selection};

use crate::change::{AppliedChange, ChangeBatch, MutationKind, UndoGroupMeta};
use crate::engine::IdGenerator;
use crate::{Error, RopeEditDeltaWithPoints};

/// Continuous typing window.
pub const COALESCE_WINDOW_MS: i64 = 500;

/// Whether a mutation can join the current typing burst.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CoalesceKind {
    /// Join a compatible group within the coalescing window.
    Typing,
    /// Always create a new undo group.
    Discrete,
}

#[derive(Debug)]
struct CoalesceState {
    group_id: UndoGroupId,
    command: String,
    last_timestamp_ms: i64,
    selections_after: Vec<Selection>,
}

/// Caller-thread-owned in-memory undo state.
#[derive(Debug, Default)]
pub(crate) struct UndoManager {
    coalesce: AHashMap<BufferId, CoalesceState>,
}

impl UndoManager {
    pub(crate) fn forget(&mut self, buffer_id: BufferId) {
        self.coalesce.remove(&buffer_id);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_group(
        &mut self,
        buffer: &mut Buffer,
        ops: &[EditOp],
        selections_before: &[Selection],
        selections_after: &[Selection],
        command: &str,
        coalesce_kind: CoalesceKind,
        timestamp_ms: i64,
        kind: MutationKind,
        ids: &mut dyn IdGenerator,
    ) -> Result<Option<ChangeBatch>, Error> {
        if ops.is_empty() {
            return Ok(None);
        }
        let buffer_id = buffer.id();
        let revision_before = buffer.revision();
        let (group_id, new_undo_group) = self.mint_or_coalesce_group(
            buffer,
            command,
            coalesce_kind,
            selections_before,
            timestamp_ms,
            ids,
        );
        let mut changes = Vec::with_capacity(ops.len());
        let mut deltas = Vec::with_capacity(ops.len());
        for op in ops {
            deltas.push(delta_with_points_for_op(buffer, op));
            changes.push(apply_recorded_op(
                buffer,
                op,
                selections_before,
                selections_after,
                group_id,
            )?);
        }
        buffer.set_selections(selections_after.to_vec());
        let selections_after = buffer.selections().to_vec();
        self.coalesce.insert(
            buffer_id,
            CoalesceState {
                group_id,
                command: command.to_string(),
                last_timestamp_ms: timestamp_ms,
                selections_after: selections_after.clone(),
            },
        );
        Ok(Some(ChangeBatch {
            buffer_id,
            kind,
            timestamp_ms,
            command: command.to_string(),
            revision_before,
            revision_after: buffer.revision(),
            undo_group_id: group_id,
            new_undo_group,
            selections_before: selections_before.to_vec(),
            selections_after,
            changes,
            checksum_after: buffer.running_checksum(),
            deltas,
        }))
    }

    pub(crate) fn apply_single(
        &mut self,
        buffer: &mut Buffer,
        op: &EditOp,
        command: &str,
        timestamp_ms: i64,
        ids: &mut dyn IdGenerator,
    ) -> Result<ChangeBatch, Error> {
        let selections_before = buffer.selections().to_vec();
        let revision_before = buffer.revision();
        let buffer_id = buffer.id();
        let (group_id, new_undo_group) = self.mint_or_coalesce_group(
            buffer,
            command,
            CoalesceKind::Discrete,
            &selections_before,
            timestamp_ms,
            ids,
        );
        let delta = delta_with_points_for_op(buffer, op);
        let removed_text = buffer.capture_removed_text(op)?;
        let op_revision_before = buffer.revision();
        let revision_after = buffer.apply(op)?;
        let selections_after = buffer.selections().to_vec();
        let inverse_op = compute_inverse_op(op, &removed_text, buffer.rope())?;
        buffer.undo_tree_mut().append_record(
            group_id,
            EditRecord {
                op: op.clone(),
                inverse_op,
                revision_before: op_revision_before,
                revision_after,
                selections_before: selections_before.clone(),
                selections_after: selections_after.clone(),
            },
        );
        let checksum_after = current_checksum(buffer);
        self.coalesce.remove(&buffer_id);
        Ok(ChangeBatch {
            buffer_id,
            kind: MutationKind::RawEdit,
            timestamp_ms,
            command: command.to_string(),
            revision_before,
            revision_after,
            undo_group_id: group_id,
            new_undo_group,
            selections_before: selections_before.clone(),
            selections_after: selections_after.clone(),
            changes: vec![AppliedChange {
                op: op.clone(),
                removed_text,
                revision_before: op_revision_before,
                revision_after,
                selections_before,
                selections_after,
                checksum_after,
            }],
            checksum_after,
            deltas: vec![delta],
        })
    }

    pub(crate) fn undo(
        &mut self,
        buffer: &mut Buffer,
        timestamp_ms: i64,
    ) -> Result<Option<ChangeBatch>, Error> {
        let Some(group) = buffer.undo_tree().group_to_undo().cloned() else {
            return Ok(None);
        };
        if group.ops.is_empty() {
            return Ok(None);
        }
        let target_selections = group.ops[0].selections_before.clone();
        let ops: Vec<EditOp> = group
            .ops
            .iter()
            .rev()
            .map(|record| record.inverse_op.clone())
            .collect();
        let batch = apply_replay_ops(
            buffer,
            &ops,
            group.id,
            &group.command,
            target_selections.clone(),
            timestamp_ms,
            MutationKind::Undo,
        )?;
        buffer.set_selections(target_selections);
        buffer.undo_tree_mut().set_current(group.parent);
        self.coalesce.remove(&buffer.id());
        Ok(Some(batch))
    }

    pub(crate) fn redo(
        &mut self,
        buffer: &mut Buffer,
        timestamp_ms: i64,
        alternate: bool,
    ) -> Result<Option<ChangeBatch>, Error> {
        let target = if alternate {
            buffer.undo_tree().group_to_redo_alternate().cloned()
        } else {
            buffer.undo_tree().group_to_redo().cloned()
        };
        let Some(group) = target else {
            return Ok(None);
        };
        if group.ops.is_empty() {
            return Ok(None);
        }
        let target_selections = group
            .ops
            .last()
            .expect("invariant: non-empty undo group")
            .selections_after
            .clone();
        let ops: Vec<EditOp> = group.ops.iter().map(|record| record.op.clone()).collect();
        let batch = apply_replay_ops(
            buffer,
            &ops,
            group.id,
            &group.command,
            target_selections.clone(),
            timestamp_ms,
            MutationKind::Redo,
        )?;
        buffer.set_selections(target_selections);
        buffer.undo_tree_mut().set_current(Some(group.id));
        self.coalesce.remove(&buffer.id());
        Ok(Some(batch))
    }

    #[allow(clippy::too_many_arguments)]
    fn mint_or_coalesce_group(
        &self,
        buffer: &mut Buffer,
        command: &str,
        kind: CoalesceKind,
        selections_before: &[Selection],
        timestamp_ms: i64,
        ids: &mut dyn IdGenerator,
    ) -> (UndoGroupId, Option<UndoGroupMeta>) {
        if matches!(kind, CoalesceKind::Typing) {
            if let Some(state) = self.coalesce.get(&buffer.id()) {
                let is_same_command = state.command == command;
                let is_in_window =
                    timestamp_ms.saturating_sub(state.last_timestamp_ms) <= COALESCE_WINDOW_MS;
                let has_no_jump = state.selections_after.as_slice() == selections_before;
                if is_same_command && is_in_window && has_no_jump {
                    return (state.group_id, None);
                }
            }
        }
        let parent = buffer.undo_tree().current_id();
        let group_id = ids.next_undo_group_id();
        buffer
            .undo_tree_mut()
            .insert_group(group_id, parent, timestamp_ms, command);
        (
            group_id,
            Some(UndoGroupMeta {
                id: group_id,
                parent,
                timestamp_ms,
                command: command.to_string(),
            }),
        )
    }
}

fn apply_recorded_op(
    buffer: &mut Buffer,
    op: &EditOp,
    selections_before: &[Selection],
    selections_after: &[Selection],
    group_id: UndoGroupId,
) -> Result<AppliedChange, Error> {
    let removed_text = buffer.capture_removed_text(op)?;
    let revision_before = buffer.revision();
    let revision_after = buffer.apply(op)?;
    let inverse_op = compute_inverse_op(op, &removed_text, buffer.rope())?;
    buffer.undo_tree_mut().append_record(
        group_id,
        EditRecord {
            op: op.clone(),
            inverse_op,
            revision_before,
            revision_after,
            selections_before: selections_before.to_vec(),
            selections_after: selections_after.to_vec(),
        },
    );
    Ok(AppliedChange {
        op: op.clone(),
        removed_text,
        revision_before,
        revision_after,
        selections_before: selections_before.to_vec(),
        selections_after: selections_after.to_vec(),
        checksum_after: current_checksum(buffer),
    })
}

fn apply_replay_ops(
    buffer: &mut Buffer,
    ops: &[EditOp],
    group_id: UndoGroupId,
    command: &str,
    selections_after: Vec<Selection>,
    timestamp_ms: i64,
    kind: MutationKind,
) -> Result<ChangeBatch, Error> {
    let buffer_id = buffer.id();
    let revision_before = buffer.revision();
    let selections_before = buffer.selections().to_vec();
    let mut changes = Vec::with_capacity(ops.len());
    let mut deltas = Vec::with_capacity(ops.len());
    for op in ops {
        deltas.push(delta_with_points_for_op(buffer, op));
        let removed_text = buffer.capture_removed_text(op)?;
        let op_revision_before = buffer.revision();
        let revision_after = buffer.apply(op)?;
        changes.push(AppliedChange {
            op: op.clone(),
            removed_text,
            revision_before: op_revision_before,
            revision_after,
            selections_before: Vec::new(),
            selections_after: Vec::new(),
            checksum_after: current_checksum(buffer),
        });
    }
    Ok(ChangeBatch {
        buffer_id,
        kind,
        timestamp_ms,
        command: command.to_string(),
        revision_before,
        revision_after: buffer.revision(),
        undo_group_id: group_id,
        new_undo_group: None,
        selections_before,
        selections_after,
        changes,
        checksum_after: buffer.running_checksum(),
        deltas,
    })
}

fn delta_with_points_for_op(buffer: &Buffer, op: &EditOp) -> RopeEditDeltaWithPoints {
    use continuity_text::RopeEditDelta;

    let rope = buffer.rope();
    match op {
        EditOp::Insert { at, text } => {
            let byte = at.to_byte_offset(rope).unwrap_or(0);
            RopeEditDeltaWithPoints::capture(RopeEditDelta::insert(byte, text.len()), rope, text)
        }
        EditOp::Delete { range } => {
            let start = range.start.to_byte_offset(rope).unwrap_or(0);
            let end = range.end.to_byte_offset(rope).unwrap_or(start);
            RopeEditDeltaWithPoints::capture(
                RopeEditDelta::delete(start, end.saturating_sub(start)),
                rope,
                "",
            )
        }
        EditOp::Replace { range, text } => {
            let start = range.start.to_byte_offset(rope).unwrap_or(0);
            let end = range.end.to_byte_offset(rope).unwrap_or(start);
            RopeEditDeltaWithPoints::capture(
                RopeEditDelta::replace(start, end.saturating_sub(start), text.len()),
                rope,
                text,
            )
        }
    }
}

fn current_checksum(buffer: &mut Buffer) -> u64 {
    if buffer.edits_since_verify() >= continuity_buffer::CHECKSUM_VERIFY_INTERVAL {
        buffer.verify_running_checksum().1
    } else {
        buffer.running_checksum()
    }
}
