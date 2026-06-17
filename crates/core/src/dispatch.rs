//! Dispatch helpers for the editor core thread.
//!
//! Pulled out of [`crate::handle`] to keep that module focused on the
//! [`EditorHandle`] surface and the message-loop wiring. Every function
//! here runs on the core thread.

use std::collections::VecDeque;

use ahash::AHashMap;
use continuity_buffer::{Buffer, BufferId, Revision};
use continuity_persist::PersistClient;
use continuity_text::{EditOp, RopeEditDelta, Selection};
use crossbeam_channel::Sender;

use crate::clock::Clock;
use crate::message::EditEvent;
use crate::policy::{edit_byte_delta, SnapshotPolicy, SnapshotTracker, SnapshotTrigger};
use crate::rope_edit_delta_points::RopeEditDeltaWithPoints;
use crate::selection_edit::{plan, SelectionEdit};
use crate::trace;
use crate::undo::{CoalesceKind, UndoOrchestrator};
use crate::{EditorState, Error};

/// One revision's worth of byte deltas. A plain `ApplyEdit` produces
/// one entry with a single delta; an `ApplySelectionEdit` plan
/// produces one entry whose deltas are recorded in plan-execution
/// (descending byte) order so a chain walk through them matches the
/// rope's actual state evolution.
#[derive(Clone, Debug)]
pub(crate) struct DeltaHistoryEntry {
    pub(crate) revision: u64,
    /// ε.4 — position-augmented deltas. Byte-only consumers
    /// (`decorations_transform`, `rope_deltas_since`) map through
    /// `.delta`; the decoration worker's incremental tree-sitter
    /// parse uses the full struct.
    pub(crate) deltas: Vec<RopeEditDeltaWithPoints>,
}

/// Per-buffer bounded history. 512 entries is roughly five seconds of
/// sustained typing; if a decoration request is older than that the
/// UI falls back to dropping the cached spans and accepting the
/// undecorated paint for one frame.
pub(crate) const DELTA_HISTORY_CAP: usize = 512;

/// Per-buffer history. Bounded queue + a watermark of the highest
/// revision we've evicted from the front, so the query can report
/// `covered=false` only when the request asks for edits older than
/// what we still have on record.
#[derive(Debug, Default)]
pub(crate) struct BufferDeltaHistory {
    pub(crate) entries: VecDeque<DeltaHistoryEntry>,
    /// Highest revision of an entry that's been evicted from the
    /// front of `entries` so far. `0` ⇒ nothing has ever been
    /// evicted from this buffer's history; the queue is complete
    /// from the start of the buffer's life.
    pub(crate) evicted_revision: u64,
}

pub(crate) type DeltaHistory = AHashMap<BufferId, BufferDeltaHistory>;

/// Convert one [`EditOp`] into a [`RopeEditDelta`] against the
/// **current** state of `buf` (call before the op is applied for
/// single-op edits, or for each plan op in plan-execution order
/// — descending byte position keeps lower ops' positions stable
/// against the original rope).
/// ε.4 — capture an `EditOp`'s byte-shift delta together with the
/// pre-edit and post-edit positions tree-sitter's `InputEdit`
/// requires. Called at edit time so the decoration worker can
/// reconstruct `InputEdit` later without re-walking the pre-edit
/// rope.
pub(crate) fn delta_with_points_for_op(buf: &Buffer, op: &EditOp) -> RopeEditDeltaWithPoints {
    let rope = buf.rope();
    match op {
        EditOp::Insert { at, text } => {
            let byte = at.to_byte_offset(rope).unwrap_or(0);
            let delta = RopeEditDelta::insert(byte, text.len());
            RopeEditDeltaWithPoints::capture(delta, rope, text)
        }
        EditOp::Delete { range } => {
            let start = range.start.to_byte_offset(rope).unwrap_or(0);
            let end = range.end.to_byte_offset(rope).unwrap_or(start);
            let delta = RopeEditDelta::delete(start, end.saturating_sub(start));
            RopeEditDeltaWithPoints::capture(delta, rope, "")
        }
        EditOp::Replace { range, text } => {
            let start = range.start.to_byte_offset(rope).unwrap_or(0);
            let end = range.end.to_byte_offset(rope).unwrap_or(start);
            let delta = RopeEditDelta::replace(start, end.saturating_sub(start), text.len());
            RopeEditDeltaWithPoints::capture(delta, rope, text)
        }
    }
}

pub(crate) fn push_delta_history(
    history: &mut DeltaHistory,
    buffer_id: BufferId,
    entry: DeltaHistoryEntry,
) {
    let h = history.entry(buffer_id).or_default();
    h.entries.push_back(entry);
    while h.entries.len() > DELTA_HISTORY_CAP {
        if let Some(evicted) = h.entries.pop_front() {
            h.evicted_revision = h.evicted_revision.max(evicted.revision);
        }
    }
}

/// Collect deltas for `buffer_id` whose revision is strictly greater
/// than `since`. Second return is `true` when the history covers
/// every revision in `(since, newest]`; `false` when the requested
/// `since` is older than the most-recently-evicted entry — i.e. we
/// no longer have a complete delta chain for that revision and the
/// caller must drop cached decorations rather than transform through
/// a partial chain.
pub(crate) fn deltas_since(
    history: &DeltaHistory,
    buffer_id: BufferId,
    since: u64,
) -> (Vec<RopeEditDelta>, bool) {
    let Some(h) = history.get(&buffer_id) else {
        return (Vec::new(), true);
    };
    if since < h.evicted_revision {
        return (Vec::new(), false);
    }
    let mut out = Vec::new();
    for entry in &h.entries {
        if entry.revision > since {
            out.extend(entry.deltas.iter().map(|d| d.delta));
        }
    }
    (out, true)
}

/// ε.4 — `deltas_since` companion that exposes the position-augmented
/// entries needed by the decoration worker's incremental tree-sitter
/// parse. Same history, same `covered` semantics; just the augmented
/// payload.
pub(crate) fn deltas_with_points_since(
    history: &DeltaHistory,
    buffer_id: BufferId,
    since: u64,
) -> (Vec<RopeEditDeltaWithPoints>, bool) {
    let Some(h) = history.get(&buffer_id) else {
        return (Vec::new(), true);
    };
    if since < h.evicted_revision {
        return (Vec::new(), false);
    }
    let mut out = Vec::new();
    for entry in &h.entries {
        if entry.revision > since {
            out.extend(entry.deltas.iter().copied());
        }
    }
    (out, true)
}

/// Apply a raw single-op `EditorMessage::ApplyEdit` request.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_one_edit(
    state: &mut EditorState,
    trackers: &mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &mut AHashMap<BufferId, String>,
    delta_history: &mut DeltaHistory,
    undo: &mut UndoOrchestrator,
    persist: &PersistClient,
    clock: &dyn Clock,
    policy: SnapshotPolicy,
    buffer_id: BufferId,
    op: EditOp,
) -> Result<Revision, Error> {
    let buf = state.get_mut(buffer_id).ok_or(Error::UnknownBuffer)?;
    let removed_len = removed_byte_count(buf, &op);
    // Capture the delta + positions against the pre-mutation rope so
    // the byte coordinates are valid for `transformed_through` AND
    // the (row, column) anchors are valid for tree-sitter's
    // `InputEdit` construction in the decoration worker.
    let delta = delta_with_points_for_op(buf, &op);
    let now = clock.now_ms();
    let revision = undo.apply_single_op(buf, &op, "editor.apply_edit", now, persist)?;
    push_delta_history(
        delta_history,
        buffer_id,
        DeltaHistoryEntry {
            revision: revision.get(),
            deltas: vec![delta],
        },
    );
    let byte_delta = edit_byte_delta(&op, removed_len);
    record_snapshot_policy(SnapshotRecordContext {
        state,
        trackers,
        pending_labels,
        persist,
        policy,
        buffer_id,
        revision,
        now,
        byte_delta,
    });
    Ok(revision)
}

/// Apply a planner-style `EditorMessage::ApplySelectionEdit` request.
#[allow(clippy::too_many_arguments)]
pub fn apply_selection_edit(
    state: &mut EditorState,
    trackers: &mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &mut AHashMap<BufferId, String>,
    delta_history: &mut DeltaHistory,
    undo: &mut UndoOrchestrator,
    persist: &PersistClient,
    clock: &dyn Clock,
    policy: SnapshotPolicy,
    buffer_id: BufferId,
    edit: SelectionEdit,
) -> Result<Option<Revision>, Error> {
    let coalesce = coalesce_kind_for(&edit);
    let command = command_name_for(&edit);
    let _dispatch_scope =
        trace::Scope::with_detail("core_apply_selection_edit", format!("cmd={command}"));
    let buf = state.get_mut(buffer_id).ok_or(Error::UnknownBuffer)?;
    let plan = {
        let _s = trace::Scope::new("core_selection_edit_plan");
        plan(buf, &edit)?
    };
    let Some(plan) = plan else {
        return Ok(None);
    };
    let now = clock.now_ms();
    let mut total_delta: usize = 0;
    let _capture_scope = trace::Scope::with_detail(
        "core_capture_plan_deltas",
        format!("ops={}", plan.ops.len()),
    );
    // Capture per-op deltas + positions against the pre-mutation
    // rope. Plan ops are in descending byte order so each op's `at`
    // stays valid after the prior (higher-position) ops apply —
    // recording them in the same order keeps the
    // `transform_range_through_chain` semantics matched to the
    // rope's actual evolution. ε.4: positions are captured at the
    // same instant so the decoration worker can build
    // `tree_sitter::InputEdit` from the stored payload without a
    // pre-edit rope snapshot.
    let mut plan_deltas: Vec<RopeEditDeltaWithPoints> = Vec::with_capacity(plan.ops.len());
    for op in &plan.ops {
        let removed_len = removed_byte_count(buf, op);
        total_delta = total_delta.saturating_add(edit_byte_delta(op, removed_len));
        plan_deltas.push(delta_with_points_for_op(buf, op));
    }
    drop(_capture_scope);
    let final_revision = undo.apply_planner_group(
        buf,
        &plan.ops,
        &plan.selections_before,
        &plan.selections_after,
        command,
        coalesce,
        now,
        persist,
    )?;
    if let Some(revision) = final_revision {
        {
            let _s = trace::Scope::new("core_push_delta_history");
            push_delta_history(
                delta_history,
                buffer_id,
                DeltaHistoryEntry {
                    revision: revision.get(),
                    deltas: plan_deltas,
                },
            );
        }
        let _snap_scope = trace::Scope::new("core_record_snapshot_policy");
        record_snapshot_policy(SnapshotRecordContext {
            state,
            trackers,
            pending_labels,
            persist,
            policy,
            buffer_id,
            revision,
            now,
            byte_delta: total_delta,
        });
    }
    Ok(final_revision)
}

/// Apply caller-planned ops as one discrete undo group.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_edit_group(
    state: &mut EditorState,
    trackers: &mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &mut AHashMap<BufferId, String>,
    delta_history: &mut DeltaHistory,
    undo: &mut UndoOrchestrator,
    persist: &PersistClient,
    clock: &dyn Clock,
    policy: SnapshotPolicy,
    buffer_id: BufferId,
    ops: Vec<EditOp>,
    selections_after: Vec<Selection>,
    command_name: &'static str,
) -> Result<Option<Revision>, Error> {
    if ops.is_empty() {
        return Ok(None);
    }
    let buf = state.get_mut(buffer_id).ok_or(Error::UnknownBuffer)?;
    let selections_before = buf.selections().to_vec();
    let selections_after = if selections_after.is_empty() {
        selections_before.clone()
    } else {
        selections_after
    };
    let now = clock.now_ms();
    let mut total_delta: usize = 0;
    let mut plan_deltas = Vec::with_capacity(ops.len());
    for op in &ops {
        let removed_len = removed_byte_count(buf, op);
        total_delta = total_delta.saturating_add(edit_byte_delta(op, removed_len));
        plan_deltas.push(delta_with_points_for_op(buf, op));
    }
    let final_revision = undo.apply_planner_group(
        buf,
        &ops,
        &selections_before,
        &selections_after,
        command_name,
        CoalesceKind::Discrete,
        now,
        persist,
    )?;
    if let Some(revision) = final_revision {
        push_delta_history(
            delta_history,
            buffer_id,
            DeltaHistoryEntry {
                revision: revision.get(),
                deltas: plan_deltas,
            },
        );
        record_snapshot_policy(SnapshotRecordContext {
            state,
            trackers,
            pending_labels,
            persist,
            policy,
            buffer_id,
            revision,
            now,
            byte_delta: total_delta,
        });
    }
    Ok(final_revision)
}

/// Forward a successful undo / redo / redo-alternate result to the
/// broadcast channel.
pub(crate) fn broadcast_revision(
    event_tx: &Sender<EditEvent>,
    buffer_id: BufferId,
    result: &Result<Option<Revision>, Error>,
) {
    if let Ok(Some(rev)) = result {
        let _ = event_tx.send(EditEvent::EditApplied {
            id: buffer_id,
            revision: *rev,
        });
    }
}

/// Run the snapshot-policy tracker after an undo/redo path mutated the
/// buffer. We use the rope size as a conservative byte delta — the inverse
/// edits already counted on the way down are now being re-counted, but this
/// errs toward more-frequent snapshots, not fewer.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_snapshot_policy(
    state: &mut EditorState,
    trackers: &mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &mut AHashMap<BufferId, String>,
    persist: &PersistClient,
    policy: SnapshotPolicy,
    buffer_id: BufferId,
    now: i64,
    result: &Result<Option<Revision>, Error>,
) {
    let Ok(Some(revision)) = result else {
        return;
    };
    let byte_delta = state
        .get(buffer_id)
        .map(|b| b.rope().len_bytes())
        .unwrap_or(0);
    record_snapshot_policy(SnapshotRecordContext {
        state,
        trackers,
        pending_labels,
        persist,
        policy,
        buffer_id,
        revision: *revision,
        now,
        byte_delta,
    });
}

/// On shutdown, persist a final blocking snapshot for every dirty buffer.
/// Phase-I1: any pending snapshot label staged for a flushed buffer is
/// stamped onto the just-written snapshot row before clearing.
pub(crate) fn flush_all_dirty(
    state: &mut EditorState,
    trackers: &mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &mut AHashMap<BufferId, String>,
    persist: &PersistClient,
) {
    for id in state.ids().collect::<Vec<_>>() {
        let dirty = trackers
            .get(&id)
            .is_some_and(|t| t.edits_since() > 0 || t.bytes_since() > 0);
        if !dirty {
            continue;
        }
        if let Some(buf) = state.get(id) {
            let snap = buf.snapshot();
            let revision = snap.revision();
            if let Err(e) = persist.save_snapshot_blocking(id, snap) {
                eprintln!("continuity-core: shutdown snapshot failed: {e}");
                continue;
            }
            if let Some(label) = pending_labels.remove(&id) {
                let _ = persist.set_snapshot_label(id, revision, Some(label));
            }
        }
    }
}

fn record_snapshot_policy(ctx: SnapshotRecordContext<'_>) {
    if let Some(tracker) = ctx.trackers.get_mut(&ctx.buffer_id) {
        if matches!(
            tracker.record_edit(ctx.byte_delta, ctx.now, &ctx.policy),
            SnapshotTrigger::Threshold
        ) {
            // Cross-check the running checksum at the snapshot
            // boundary so a snapshot is never persisted alongside a
            // silently-drifted edit-log checksum. Drift surfaces as
            // `event:checksum_drift` and the running counter is
            // reseated before the snapshot is captured.
            if let Some(buf) = ctx.state.get_mut(ctx.buffer_id) {
                let (observed, computed) = buf.verify_running_checksum();
                if observed != computed {
                    let rope_bytes = buf.rope().len_bytes();
                    trace::log_event(
                        "checksum_drift",
                        0,
                        &format!(
                            "observed={observed:#x} expected={computed:#x} \
                             revision={} rope_bytes={rope_bytes} trigger=snapshot",
                            ctx.revision.get()
                        ),
                    );
                }
            }
            if let Some(buf) = ctx.state.get(ctx.buffer_id) {
                let snap = buf.snapshot();
                let snap_revision = snap.revision();
                let _ = ctx.persist.save_snapshot_async(ctx.buffer_id, snap);
                if let Some(label) = ctx.pending_labels.remove(&ctx.buffer_id) {
                    let _ =
                        ctx.persist
                            .set_snapshot_label(ctx.buffer_id, snap_revision, Some(label));
                }
                let _ = ctx.persist.prune_covered_edits(ctx.buffer_id, ctx.revision);
                let _ = ctx.persist.touch_buffer(ctx.buffer_id, ctx.now);
                tracker.reset(ctx.now);
            }
        }
    }
}

struct SnapshotRecordContext<'a> {
    state: &'a mut EditorState,
    trackers: &'a mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &'a mut AHashMap<BufferId, String>,
    persist: &'a PersistClient,
    policy: SnapshotPolicy,
    buffer_id: BufferId,
    revision: Revision,
    now: i64,
    byte_delta: usize,
}

fn coalesce_kind_for(edit: &SelectionEdit) -> CoalesceKind {
    match edit {
        // Single-character `editor.insert_char` keystrokes (and Backspace /
        // Delete) form a "continuous typing" burst per spec §8. Other
        // selection edits each get a fresh group.
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

fn removed_byte_count(buf: &Buffer, op: &EditOp) -> usize {
    match op {
        EditOp::Insert { .. } => 0,
        EditOp::Delete { range } | EditOp::Replace { range, .. } => {
            let start = range.start.to_byte_offset(buf.rope()).unwrap_or(0);
            let end = range.end.to_byte_offset(buf.rope()).unwrap_or(start);
            end.saturating_sub(start)
        }
    }
}
