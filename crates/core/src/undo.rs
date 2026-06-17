//! Undo/redo execution: in-memory tree mutation, persistence of group rows
//! and inverse-op edit-log rows, plus typing-edit coalescing per spec §8.
//!
//! **Thread ownership**: every function in this module runs on the editor
//! core thread. The [`UndoOrchestrator`] is owned exclusively by
//! [`crate::handle`]'s core_loop and accumulates per-buffer state across
//! calls.

use std::time::Instant;

use ahash::AHashMap;
use continuity_buffer::{
    compute_inverse_op, Buffer, BufferId, EditRecord, Revision, UndoGroupId,
    CHECKSUM_VERIFY_INTERVAL,
};
use continuity_persist::{encode_edit, PersistClient, UndoGroupRow};
use continuity_text::{EditOp, Selection};

use crate::dispatch::{
    delta_with_points_for_op, push_delta_history, DeltaHistory, DeltaHistoryEntry,
};
use crate::rope_edit_delta_points::RopeEditDeltaWithPoints;
use crate::trace;
use crate::Error;

/// Continuous-typing window per spec §8 ("within 500ms").
pub const COALESCE_WINDOW_MS: i64 = 500;

/// Per-buffer coalescing state: links typing-style edits within the
/// 500ms window into one undo group instead of one-per-keystroke.
#[derive(Debug)]
struct CoalesceState {
    group_id: UndoGroupId,
    command: &'static str,
    last_ts_ms: i64,
    /// Selections at the END of the last recorded edit; matched against the
    /// next edit's `selections_before` to detect "no caret jump".
    selections_after: Vec<Selection>,
}

/// Whether an edit is part of a coalesce-eligible typing burst.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CoalesceKind {
    /// Coalesce with the previous group when the rules in spec §8 hold.
    Typing,
    /// Always start a new group.
    Discrete,
}

/// State the editor core thread carries between command applications to
/// drive undo coalescing, inverse-op recording, and per-buffer edit-log
/// sequencing.
#[derive(Debug, Default)]
pub struct UndoOrchestrator {
    coalesce: AHashMap<BufferId, CoalesceState>,
    next_seq: AHashMap<BufferId, u64>,
}

impl UndoOrchestrator {
    /// Construct an empty orchestrator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inject a buffer's persisted next-seq counter — used during recovery
    /// hand-off so the first new edit's seq doesn't collide with the
    /// replayed log.
    pub(crate) fn seed_next_seq(&mut self, buffer_id: BufferId, next_seq: u64) {
        self.next_seq.insert(buffer_id, next_seq);
    }

    /// Drop all per-buffer state for `buffer_id`.
    pub fn forget(&mut self, buffer_id: BufferId) {
        self.coalesce.remove(&buffer_id);
        self.next_seq.remove(&buffer_id);
    }

    /// Apply a planner-style multi-op edit (every op already shares one
    /// `selections_before` / `selections_after` snapshot). Mints (or
    /// coalesces into) one undo group, persists the edit rows, and appends
    /// `EditRecord`s to the buffer's undo tree.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_planner_group(
        &mut self,
        buf: &mut Buffer,
        ops: &[EditOp],
        selections_before: &[Selection],
        selections_after: &[Selection],
        command: &'static str,
        coalesce_kind: CoalesceKind,
        ts_ms: i64,
        persist: &PersistClient,
    ) -> Result<Option<Revision>, Error> {
        if ops.is_empty() {
            return Ok(None);
        }
        let buffer_id = buf.id();
        let _planner_scope = trace::Scope::with_detail(
            "core_apply_planner_group",
            format!("ops={} kind={:?} cmd={command}", ops.len(), coalesce_kind),
        );
        let group_id = {
            let _s = trace::Scope::new("core_mint_or_coalesce_group");
            self.mint_or_coalesce_group(
                buffer_id,
                buf,
                command,
                coalesce_kind,
                selections_before,
                ts_ms,
                persist,
            )
        };
        let mut final_revision = None;
        let _ops_scope =
            trace::Scope::with_detail("core_planner_op_loop", format!("ops={}", ops.len()));
        for op in ops {
            let revision = self.apply_op_into_group(
                buf,
                buffer_id,
                op,
                selections_before,
                selections_after,
                group_id,
                ts_ms,
                persist,
            )?;
            final_revision = Some(revision);
        }
        drop(_ops_scope);
        buf.set_selections(selections_after.to_vec());
        self.coalesce.insert(
            buffer_id,
            CoalesceState {
                group_id,
                command,
                last_ts_ms: ts_ms,
                selections_after: selections_after.to_vec(),
            },
        );
        Ok(final_revision)
    }

    /// Apply a raw single op, recording it into a discrete (non-coalesced)
    /// undo group. The buffer's auto-transformed post-apply selections are
    /// used as `selections_after`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_single_op(
        &mut self,
        buf: &mut Buffer,
        op: &EditOp,
        command: &'static str,
        ts_ms: i64,
        persist: &PersistClient,
    ) -> Result<Revision, Error> {
        let buffer_id = buf.id();
        let selections_before = buf.selections().to_vec();
        let group_id = self.mint_or_coalesce_group(
            buffer_id,
            buf,
            command,
            CoalesceKind::Discrete,
            &selections_before,
            ts_ms,
            persist,
        );
        let removed_text = buf.capture_removed_text(op).map_err(Error::from)?;
        let revision_before = buf.revision();
        let revision = buf.apply(op).map_err(Error::from)?;
        let selections_after = buf.selections().to_vec();
        let inverse_op = compute_inverse_op(op, &removed_text, buf.rope()).map_err(Error::from)?;
        let record = EditRecord {
            op: op.clone(),
            inverse_op,
            revision_before,
            revision_after: revision,
            selections_before: selections_before.clone(),
            selections_after: selections_after.clone(),
        };
        buf.undo_tree_mut().append_record(group_id, record);
        self.persist_edit_row(
            buf,
            buffer_id,
            revision,
            op,
            &removed_text,
            &selections_before,
            &selections_after,
            Some(group_id),
            ts_ms,
            persist,
        );
        // Discrete edits always break the typing window for this buffer.
        self.coalesce.remove(&buffer_id);
        Ok(revision)
    }

    /// Apply the most-recent group's inverse ops in reverse order. Returns
    /// the new revision after the inverses have all been applied.
    ///
    /// The byte deltas of every applied inverse op are recorded into
    /// `delta_history` against the resulting revision — exactly as the
    /// forward edit paths do in [`crate::dispatch`]. Without this,
    /// `rope_deltas_since` would report `(empty, covered=true)` after an
    /// undo (the rope advanced but no deltas exist), and the UI's
    /// projection classifier would reuse the pre-undo display map against
    /// the post-undo rope — slicing stale, out-of-bounds byte ranges
    /// during paint.
    pub(crate) fn undo(
        &mut self,
        buf: &mut Buffer,
        ts_ms: i64,
        persist: &PersistClient,
        delta_history: &mut DeltaHistory,
    ) -> Result<Option<Revision>, Error> {
        let buffer_id = buf.id();
        let Some(group) = buf.undo_tree().group_to_undo().cloned() else {
            return Ok(None);
        };
        if group.ops.is_empty() {
            return Ok(None);
        }
        let parent = group.parent;
        let target_selections = group
            .ops
            .first()
            .expect("invariant: group_to_undo non-empty")
            .selections_before
            .clone();
        let group_id = group.id;
        let mut final_revision = None;
        let inverses: Vec<EditOp> = group
            .ops
            .iter()
            .rev()
            .map(|r| r.inverse_op.clone())
            .collect();
        let mut deltas: Vec<RopeEditDeltaWithPoints> = Vec::with_capacity(inverses.len());
        for op in &inverses {
            // Capture the delta + positions against the pre-apply rope so
            // the byte coordinates are valid for the consumers' chain
            // walk, matching the forward edit paths' ordering invariant.
            deltas.push(delta_with_points_for_op(buf, op));
            let removed_text = buf.capture_removed_text(op).map_err(Error::from)?;
            let revision = buf.apply(op).map_err(Error::from)?;
            self.persist_edit_row(
                buf,
                buffer_id,
                revision,
                op,
                &removed_text,
                &[],
                &[],
                Some(group_id),
                ts_ms,
                persist,
            );
            final_revision = Some(revision);
        }
        buf.set_selections(target_selections);
        buf.undo_tree_mut().set_current(parent);
        self.coalesce.remove(&buffer_id);
        if let Some(revision) = final_revision {
            push_delta_history(
                delta_history,
                buffer_id,
                DeltaHistoryEntry {
                    revision: revision.get(),
                    deltas,
                },
            );
        }
        Ok(final_revision)
    }

    /// Re-apply the most-recent child of the current pointer.
    pub(crate) fn redo(
        &mut self,
        buf: &mut Buffer,
        ts_ms: i64,
        persist: &PersistClient,
        delta_history: &mut DeltaHistory,
    ) -> Result<Option<Revision>, Error> {
        let Some(target) = buf.undo_tree().group_to_redo().cloned() else {
            return Ok(None);
        };
        self.replay_group(buf, target.id, ts_ms, persist, delta_history)
    }

    /// Re-apply an alternate child of the current pointer (a sibling of
    /// the most-recent redo target). The user's typical flow: after undoing
    /// past the divergence point, redo into a different branch than the
    /// most-recent one.
    pub(crate) fn redo_alternate(
        &mut self,
        buf: &mut Buffer,
        ts_ms: i64,
        persist: &PersistClient,
        delta_history: &mut DeltaHistory,
    ) -> Result<Option<Revision>, Error> {
        let Some(alt) = buf.undo_tree().group_to_redo_alternate().cloned() else {
            return Ok(None);
        };
        self.replay_group(buf, alt.id, ts_ms, persist, delta_history)
    }

    /// Best-effort log of current group + immediate children. Phase 8 will
    /// front this with a real palette-style picker.
    pub(crate) fn log_tree_pick(&self, buf: &Buffer) {
        let tree = buf.undo_tree();
        match tree.current() {
            None => eprintln!("undo_tree_pick: at pre-history state"),
            Some(g) => eprintln!(
                "undo_tree_pick: current group {} `{}` (ts={}ms, ops={})",
                g.id.as_uuid(),
                g.command,
                g.timestamp_ms,
                g.ops.len()
            ),
        }
        for child in tree.children(tree.current_id()) {
            eprintln!(
                "  child {} `{}` (ts={}ms, ops={})",
                child.id.as_uuid(),
                child.command,
                child.timestamp_ms,
                child.ops.len()
            );
        }
    }

    fn replay_group(
        &mut self,
        buf: &mut Buffer,
        group_id: UndoGroupId,
        ts_ms: i64,
        persist: &PersistClient,
        delta_history: &mut DeltaHistory,
    ) -> Result<Option<Revision>, Error> {
        let Some(group) = buf.undo_tree().get(group_id).cloned() else {
            return Ok(None);
        };
        if group.ops.is_empty() {
            return Ok(None);
        }
        let buffer_id = buf.id();
        let target_selections = group
            .ops
            .last()
            .expect("invariant: group has ops")
            .selections_after
            .clone();
        let mut final_revision = None;
        let mut deltas: Vec<RopeEditDeltaWithPoints> = Vec::with_capacity(group.ops.len());
        for record in &group.ops {
            let op = record.op.clone();
            // Capture the delta against the pre-apply rope (see `undo`).
            deltas.push(delta_with_points_for_op(buf, &op));
            let removed_text = buf.capture_removed_text(&op).map_err(Error::from)?;
            let revision = buf.apply(&op).map_err(Error::from)?;
            self.persist_edit_row(
                buf,
                buffer_id,
                revision,
                &op,
                &removed_text,
                &[],
                &[],
                Some(group_id),
                ts_ms,
                persist,
            );
            final_revision = Some(revision);
        }
        buf.set_selections(target_selections);
        buf.undo_tree_mut().set_current(Some(group_id));
        self.coalesce.remove(&buffer_id);
        if let Some(revision) = final_revision {
            push_delta_history(
                delta_history,
                buffer_id,
                DeltaHistoryEntry {
                    revision: revision.get(),
                    deltas,
                },
            );
        }
        Ok(final_revision)
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_op_into_group(
        &mut self,
        buf: &mut Buffer,
        buffer_id: BufferId,
        op: &EditOp,
        selections_before: &[Selection],
        selections_after: &[Selection],
        group_id: UndoGroupId,
        ts_ms: i64,
        persist: &PersistClient,
    ) -> Result<Revision, Error> {
        let removed_text = {
            let _s = trace::Scope::new("core_capture_removed_text");
            buf.capture_removed_text(op).map_err(Error::from)?
        };
        let revision_before = buf.revision();
        let revision = {
            let _s = trace::Scope::new("core_buffer_apply");
            buf.apply(op).map_err(Error::from)?
        };
        let inverse_op = {
            let _s = trace::Scope::new("core_compute_inverse_op");
            compute_inverse_op(op, &removed_text, buf.rope()).map_err(Error::from)?
        };
        let record = EditRecord {
            op: op.clone(),
            inverse_op,
            revision_before,
            revision_after: revision,
            selections_before: selections_before.to_vec(),
            selections_after: selections_after.to_vec(),
        };
        {
            let _s = trace::Scope::new("core_undo_tree_append");
            buf.undo_tree_mut().append_record(group_id, record);
        }
        self.persist_edit_row(
            buf,
            buffer_id,
            revision,
            op,
            &removed_text,
            selections_before,
            selections_after,
            Some(group_id),
            ts_ms,
            persist,
        );
        Ok(revision)
    }

    #[allow(clippy::too_many_arguments)]
    fn mint_or_coalesce_group(
        &mut self,
        buffer_id: BufferId,
        buf: &mut Buffer,
        command: &'static str,
        kind: CoalesceKind,
        selections_before: &[Selection],
        ts_ms: i64,
        persist: &PersistClient,
    ) -> UndoGroupId {
        if matches!(kind, CoalesceKind::Typing) {
            if let Some(state) = self.coalesce.get(&buffer_id) {
                let same_command = state.command == command;
                let in_window = ts_ms.saturating_sub(state.last_ts_ms) <= COALESCE_WINDOW_MS;
                let no_jump = state.selections_after.as_slice() == selections_before;
                if same_command && in_window && no_jump {
                    return state.group_id;
                }
            }
        }
        let parent = buf.undo_tree().current_id();
        let group_id = UndoGroupId::new();
        buf.undo_tree_mut()
            .insert_group(group_id, parent, ts_ms, command);
        let _s = trace::Scope::new("core_write_undo_group_send");
        let _ = persist.write_undo_group(UndoGroupRow {
            id: group_id,
            buffer_id,
            command_name: command.to_string(),
            ts_ms,
            parent_group_id: parent,
        });
        group_id
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_edit_row(
        &mut self,
        buf: &mut Buffer,
        buffer_id: BufferId,
        revision: Revision,
        op: &EditOp,
        removed_text: &str,
        selections_before: &[Selection],
        selections_after: &[Selection],
        undo_group_id: Option<UndoGroupId>,
        ts_ms: i64,
        persist: &PersistClient,
    ) {
        let rope_bytes = buf.rope().len_bytes();
        let _row_scope =
            trace::Scope::with_detail("core_persist_edit_row", format!("rope_bytes={rope_bytes}"));
        let checksum = compute_persisted_checksum(buf, rope_bytes, revision);
        let seq_entry = self.next_seq.entry(buffer_id).or_insert(1);
        let seq = *seq_entry;
        *seq_entry = seq.saturating_add(1);
        let removed_opt = if removed_text.is_empty() {
            None
        } else {
            Some(removed_text)
        };
        let row = {
            let _s = trace::Scope::new("core_encode_edit");
            encode_edit(
                buffer_id,
                seq,
                revision,
                ts_ms,
                op,
                removed_opt,
                selections_before,
                selections_after,
                undo_group_id,
                checksum,
            )
        };
        {
            let _s = trace::Scope::new("core_append_edit_send");
            let edit_seq = trace::current_edit_seq();
            if let Err(e) = persist.append_edit_with_seq(row, edit_seq) {
                eprintln!("continuity-core: append_edit failed: {e}");
            }
        }
    }
}

/// Look up the running FNV-1a checksum kept on the buffer (already
/// updated incrementally by [`Buffer::apply`]). Every
/// [`CHECKSUM_VERIFY_INTERVAL`] persisted edits, re-walk the rope as a
/// cross-check; any divergence is surfaced through `event:checksum_drift`
/// and the running counter is reseated to the freshly-computed value
/// before it is persisted. Bounds drift damage to one verification
/// interval.
fn compute_persisted_checksum(buf: &mut Buffer, rope_bytes: usize, revision: Revision) -> u64 {
    if buf.edits_since_verify() >= CHECKSUM_VERIFY_INTERVAL {
        let started = Instant::now();
        let (observed, computed) = buf.verify_running_checksum();
        let elapsed_us = started.elapsed().as_micros();
        if observed != computed {
            trace::log_event(
                "checksum_drift",
                0,
                &format!(
                    "observed={observed:#x} expected={computed:#x} \
                     revision={} rope_bytes={rope_bytes} trigger=interval",
                    revision.get()
                ),
            );
        }
        trace::log_event(
            "edit_checksum",
            elapsed_us,
            &format!("path=full rope_bytes={rope_bytes}"),
        );
        computed
    } else {
        trace::log_event(
            "edit_checksum",
            0,
            &format!("path=incremental rope_bytes={rope_bytes}"),
        );
        buf.running_checksum()
    }
}
