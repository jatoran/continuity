//! Core-thread message loop.
//!
//! [`core_loop`] is the sole owner of [`EditorState`], the per-buffer
//! [`UndoOrchestrator`] state, the snapshot trackers, and the pending
//! snapshot labels. It drains [`EditorMessage`]s on `cmd_rx`, mutates
//! state, broadcasts [`EditEvent`]s on `event_tx`, and persists every
//! edit and policy-driven snapshot via `persist`. On shutdown it flushes
//! a final snapshot for every dirty buffer before returning.

use ahash::AHashMap;
use continuity_buffer::{derive_title, Buffer, BufferId};
use continuity_persist::PersistClient;
use crossbeam_channel::{Receiver, Sender};

use crate::clock::Clock;
use crate::dispatch::{
    apply_edit_group, apply_one_edit, apply_selection_edit, broadcast_revision, flush_all_dirty,
    run_snapshot_policy,
};
use crate::message::{BufferSummary, CoreMemoryStats, EditEvent, EditorMessage, EditorSnapshot};
use crate::policy::{SnapshotPolicy, SnapshotTracker};
use crate::undo::UndoOrchestrator;
use crate::{EditorState, Error};

fn summarize(id: BufferId, buf: &Buffer) -> BufferSummary {
    let rope = buf.rope();
    let first_line = (0..rope.len_lines())
        .find_map(|line_idx| {
            let mut line: String = rope.line(line_idx).to_string();
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .unwrap_or_default();
    BufferSummary {
        id,
        title: derive_title(rope, 80),
        first_line,
        revision: buf.revision(),
        line_count: rope.len_lines(),
        file: buf.file_association().cloned(),
    }
}

fn compute_memory_stats(state: &EditorState) -> CoreMemoryStats {
    let mut stats = CoreMemoryStats::default();
    for id in state.ids() {
        let Some(buf) = state.get(id) else {
            continue;
        };
        let rope_bytes = buf.rope().len_bytes();
        stats.rope_bytes = stats.rope_bytes.saturating_add(rope_bytes);
        let undo_tree = buf.undo_tree();
        stats.undo_tree_bytes = stats
            .undo_tree_bytes
            .saturating_add(undo_tree.byte_size_estimate());
        stats.undo_tree_groups = stats
            .undo_tree_groups
            .saturating_add(undo_tree.group_count());
        stats.undo_tree_records = stats
            .undo_tree_records
            .saturating_add(undo_tree.record_count());
    }
    stats
}

#[allow(clippy::too_many_arguments)]
pub(super) fn core_loop(
    state: &mut EditorState,
    trackers: &mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &mut AHashMap<BufferId, String>,
    delta_history: &mut crate::dispatch::DeltaHistory,
    undo: &mut UndoOrchestrator,
    cmd_rx: &Receiver<EditorMessage>,
    event_tx: &Sender<EditEvent>,
    persist: &PersistClient,
    clock: &dyn Clock,
    initial_policy: SnapshotPolicy,
) {
    // Phase 16.5: snapshot policy is now mutable so settings.toml
    // updates routed via [`EditorMessage::SetSnapshotPolicy`] take
    // effect at runtime.
    let mut policy = initial_policy;
    while let Ok(msg) = cmd_rx.recv() {
        match msg {
            EditorMessage::OpenBuffer { content, reply } => {
                let now = clock.now_ms();
                let buf = Buffer::from_text(&content);
                let id = buf.id();
                let snap = buf.snapshot();
                state.insert(buf);
                trackers.insert(id, SnapshotTracker::starting_at(now));
                undo.seed_next_seq(id, 1);
                let _ = persist.upsert_buffer(id, now, now);
                let _ = persist.save_snapshot_async(id, snap);
                let _ = reply.send(id);
                let _ = event_tx.send(EditEvent::BufferOpened { id });
            }
            EditorMessage::OpenFileBuffer {
                content,
                file,
                reply,
            } => {
                let now = clock.now_ms();
                let file = file.with_content_hash(continuity_persist::fnv1a_64(content.as_bytes()));
                let mut buf = Buffer::from_text(&content);
                buf.set_file_association(Some(file.clone()));
                let id = buf.id();
                let snap = buf.snapshot();
                state.insert(buf);
                trackers.insert(id, SnapshotTracker::starting_at(now));
                undo.seed_next_seq(id, 1);
                let _ = persist.upsert_buffer(id, now, now);
                let _ = persist.set_buffer_file_async(id, Some(file));
                let _ = persist.save_snapshot_async(id, snap);
                let _ = reply.send(id);
                let _ = event_tx.send(EditEvent::BufferOpened { id });
            }
            EditorMessage::AdoptBuffer {
                buffer,
                next_seq,
                last_snapshot_at_ms,
                reply,
            } => {
                let id = buffer.id();
                let is_synthetic = buffer.is_synthetic();
                state.insert(buffer);
                trackers.insert(id, SnapshotTracker::starting_at(last_snapshot_at_ms));
                undo.seed_next_seq(id, next_seq);
                if !is_synthetic {
                    let _ = persist.touch_buffer(id, clock.now_ms());
                }
                let _ = reply.send(id);
                let _ = event_tx.send(EditEvent::BufferOpened { id });
            }
            EditorMessage::ApplyEdit {
                buffer_id,
                op,
                edit_seq,
                reply,
            } => {
                let _seq_guard = edit_seq.map(crate::trace::bind_edit_seq);
                let result = apply_one_edit(
                    state,
                    trackers,
                    pending_labels,
                    delta_history,
                    undo,
                    persist,
                    clock,
                    policy,
                    buffer_id,
                    op,
                );
                if let Ok(rev) = &result {
                    let _ = event_tx.send(EditEvent::EditApplied {
                        id: buffer_id,
                        revision: *rev,
                    });
                }
                let _ = reply.send(result);
            }
            EditorMessage::ApplySelectionEdit {
                buffer_id,
                edit,
                edit_seq,
                reply,
            } => {
                let _seq_guard = edit_seq.map(crate::trace::bind_edit_seq);
                let result = apply_selection_edit(
                    state,
                    trackers,
                    pending_labels,
                    delta_history,
                    undo,
                    persist,
                    clock,
                    policy,
                    buffer_id,
                    edit,
                );
                if let Ok(Some(rev)) = &result {
                    let _ = event_tx.send(EditEvent::EditApplied {
                        id: buffer_id,
                        revision: *rev,
                    });
                }
                let _ = reply.send(result);
            }
            EditorMessage::ApplyEditGroup {
                buffer_id,
                ops,
                selections_after,
                command_name,
                edit_seq,
                reply,
            } => {
                let _seq_guard = edit_seq.map(crate::trace::bind_edit_seq);
                let result = apply_edit_group(
                    state,
                    trackers,
                    pending_labels,
                    delta_history,
                    undo,
                    persist,
                    clock,
                    policy,
                    buffer_id,
                    ops,
                    selections_after,
                    command_name,
                );
                broadcast_revision(event_tx, buffer_id, &result);
                let _ = reply.send(result);
            }
            EditorMessage::SetSelections {
                buffer_id,
                mut selections,
                reply,
            } => {
                crate::selection_coalesce::coalesce_selections(&mut selections);
                let result = state
                    .get_mut(buffer_id)
                    .ok_or(Error::UnknownBuffer)
                    .map(|buf| buf.set_selections(selections));
                if result.is_ok() {
                    let _ = event_tx.send(EditEvent::SelectionsChanged { id: buffer_id });
                }
                let _ = reply.send(result);
            }
            EditorMessage::MutateSelections {
                buffer_id,
                f,
                reply,
            } => {
                let result = state
                    .get_mut(buffer_id)
                    .ok_or(Error::UnknownBuffer)
                    .map(|buf| {
                        let mut selections = buf.selections().to_vec();
                        f(&mut selections);
                        crate::selection_coalesce::coalesce_selections(&mut selections);
                        buf.set_selections(selections);
                    });
                if result.is_ok() {
                    let _ = event_tx.send(EditEvent::SelectionsChanged { id: buffer_id });
                }
                let _ = reply.send(result);
            }
            EditorMessage::Snapshot { buffer_id, reply } => {
                let snap = state.get(buffer_id).map(|buf| EditorSnapshot {
                    rope: buf.snapshot(),
                    selections: buf.selections().to_vec(),
                    file: buf.file_association().cloned(),
                });
                let _ = reply.send(snap);
            }
            EditorMessage::SetFileAssociation {
                buffer_id,
                file,
                reply,
            } => {
                let result = state
                    .get_mut(buffer_id)
                    .ok_or(Error::UnknownBuffer)
                    .map(|buf| {
                        buf.set_file_association(file.clone());
                    })
                    .and_then(|()| {
                        persist
                            .set_buffer_file_async(buffer_id, file)
                            .map_err(Error::from)
                    });
                let _ = reply.send(result);
            }
            EditorMessage::Undo { buffer_id, reply } => {
                let now = clock.now_ms();
                let result = match state.get_mut(buffer_id) {
                    None => Err(Error::UnknownBuffer),
                    Some(buf) => undo.undo(buf, now, persist),
                };
                broadcast_revision(event_tx, buffer_id, &result);
                run_snapshot_policy(
                    state,
                    trackers,
                    pending_labels,
                    persist,
                    policy,
                    buffer_id,
                    now,
                    &result,
                );
                let _ = reply.send(result);
            }
            EditorMessage::Redo { buffer_id, reply } => {
                let now = clock.now_ms();
                let result = match state.get_mut(buffer_id) {
                    None => Err(Error::UnknownBuffer),
                    Some(buf) => undo.redo(buf, now, persist),
                };
                broadcast_revision(event_tx, buffer_id, &result);
                run_snapshot_policy(
                    state,
                    trackers,
                    pending_labels,
                    persist,
                    policy,
                    buffer_id,
                    now,
                    &result,
                );
                let _ = reply.send(result);
            }
            EditorMessage::RedoAlternateBranch { buffer_id, reply } => {
                let now = clock.now_ms();
                let result = match state.get_mut(buffer_id) {
                    None => Err(Error::UnknownBuffer),
                    Some(buf) => undo.redo_alternate(buf, now, persist),
                };
                broadcast_revision(event_tx, buffer_id, &result);
                run_snapshot_policy(
                    state,
                    trackers,
                    pending_labels,
                    persist,
                    policy,
                    buffer_id,
                    now,
                    &result,
                );
                let _ = reply.send(result);
            }
            EditorMessage::UndoTreePick { buffer_id, reply } => {
                let result = match state.get(buffer_id) {
                    None => Err(Error::UnknownBuffer),
                    Some(buf) => {
                        undo.log_tree_pick(buf);
                        Ok(())
                    }
                };
                let _ = reply.send(result);
            }
            EditorMessage::ListBuffers { reply } => {
                let summaries: Vec<BufferSummary> = state
                    .ids()
                    .filter_map(|id| state.get(id).map(|buf| summarize(id, buf)))
                    .collect();
                let _ = reply.send(summaries);
            }
            EditorMessage::MemoryStats { reply } => {
                let _ = reply.send(compute_memory_stats(state));
            }
            EditorMessage::SetSnapshotPolicy(new_policy) => {
                policy = new_policy;
            }
            EditorMessage::RopeDeltasSince {
                buffer_id,
                since_revision,
                reply,
            } => {
                let answer =
                    crate::dispatch::deltas_since(delta_history, buffer_id, since_revision);
                let _ = reply.send(answer);
            }
            EditorMessage::RopeDeltasWithPointsSince {
                buffer_id,
                since_revision,
                reply,
            } => {
                let answer = crate::dispatch::deltas_with_points_since(
                    delta_history,
                    buffer_id,
                    since_revision,
                );
                let _ = reply.send(answer);
            }
            EditorMessage::SetPendingSnapshotLabel { buffer_id, label } => match label {
                Some(s) if !s.is_empty() => {
                    pending_labels.insert(buffer_id, s);
                }
                _ => {
                    pending_labels.remove(&buffer_id);
                }
            },
            EditorMessage::Shutdown => {
                flush_all_dirty(state, trackers, pending_labels, persist);
                break;
            }
        }
    }
}
