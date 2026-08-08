//! Core-thread message loop.
//!
//! [`core_loop`] is the sole owner of [`continuity_engine::Engine`], the
//! snapshot trackers, native persistence bridge, and pending
//! snapshot labels. It drains [`EditorMessage`]s on `cmd_rx`, mutates
//! state, broadcasts [`EditEvent`]s on `event_tx`, and persists every
//! edit and policy-driven snapshot via `persist`. On shutdown it flushes
//! a final snapshot for every dirty buffer before returning.

use ahash::AHashMap;
use continuity_buffer::{derive_title, Buffer, BufferId};
use continuity_engine::{ChangeBatch, Engine};
use continuity_persist::PersistClient;
use crossbeam_channel::{Receiver, Sender};

use crate::clock::Clock;
use crate::dispatch::{flush_all_dirty, record_change_batch};
use crate::message::{BufferSummary, CoreMemoryStats, EditEvent, EditorMessage, EditorSnapshot};
use crate::persistence_bridge::PersistenceBridge;
use crate::policy::{SnapshotPolicy, SnapshotTracker};
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
fn apply_history_change(
    result: Result<Option<ChangeBatch>, continuity_engine::Error>,
    engine: &mut Engine,
    trackers: &mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &mut AHashMap<BufferId, String>,
    bridge: &mut PersistenceBridge<'_>,
    persist: &PersistClient,
    policy: SnapshotPolicy,
) -> Result<Option<continuity_buffer::Revision>, Error> {
    result.map_err(Error::from).map(|batch| {
        batch.map(|batch| {
            record_change_batch(
                engine.state_mut(),
                trackers,
                pending_labels,
                bridge,
                persist,
                policy,
                &batch,
            );
            batch.revision_after
        })
    })
}

fn broadcast_revision(
    event_tx: &Sender<EditEvent>,
    buffer_id: BufferId,
    result: &Result<Option<continuity_buffer::Revision>, Error>,
) {
    if let Ok(Some(revision)) = result {
        let _ = event_tx.send(EditEvent::EditApplied {
            id: buffer_id,
            revision: *revision,
        });
    }
}

fn log_undo_tree(buffer: &Buffer) {
    let tree = buffer.undo_tree();
    match tree.current() {
        None => eprintln!("undo_tree_pick: at pre-history state"),
        Some(group) => eprintln!(
            "undo_tree_pick: current group {} `{}` (ts={}ms, ops={})",
            group.id.as_uuid(),
            group.command,
            group.timestamp_ms,
            group.ops.len()
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

#[allow(clippy::too_many_arguments)]
pub(super) fn core_loop(
    engine: &mut Engine,
    trackers: &mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &mut AHashMap<BufferId, String>,
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
    let mut persistence_bridge = PersistenceBridge::new(persist);
    while let Ok(msg) = cmd_rx.recv() {
        match msg {
            EditorMessage::OpenBuffer { content, reply } => {
                let now = clock.now_ms();
                let buf = Buffer::from_text(&content);
                let id = buf.id();
                let snap = buf.snapshot();
                engine.adopt_buffer(buf);
                engine.drain_events();
                trackers.insert(id, SnapshotTracker::starting_at(now));
                persistence_bridge.seed_next_sequence(id, 1);
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
                engine.adopt_buffer(buf);
                engine.drain_events();
                trackers.insert(id, SnapshotTracker::starting_at(now));
                persistence_bridge.seed_next_sequence(id, 1);
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
                engine.adopt_buffer(buffer);
                engine.drain_events();
                trackers.insert(id, SnapshotTracker::starting_at(last_snapshot_at_ms));
                persistence_bridge.seed_next_sequence(id, next_seq);
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
                let now = clock.now_ms();
                let result = engine
                    .apply_edit(buffer_id, op, now)
                    .map_err(Error::from)
                    .map(|batch| {
                        record_change_batch(
                            engine.state_mut(),
                            trackers,
                            pending_labels,
                            &mut persistence_bridge,
                            persist,
                            policy,
                            &batch,
                        );
                        batch.revision_after
                    });
                engine.drain_events();
                if let Ok(revision) = &result {
                    let _ = event_tx.send(EditEvent::EditApplied {
                        id: buffer_id,
                        revision: *revision,
                    });
                }
                let _ = reply.send(result);
            }
            EditorMessage::ApplySelectionEdit {
                buffer_id,
                operation,
                edit_seq,
                reply,
            } => {
                let _seq_guard = edit_seq.map(crate::trace::bind_edit_seq);
                let now = clock.now_ms();
                let request = continuity_host::OperationRequest {
                    buffer_id,
                    expected_revision: None,
                    timestamp_ms: now,
                    operation,
                };
                let result = continuity_host::apply_editor_operation(engine, &request)
                    .map_err(Error::from)
                    .map(|operation_result| {
                        operation_result.change.map(|batch| {
                            record_change_batch(
                                engine.state_mut(),
                                trackers,
                                pending_labels,
                                &mut persistence_bridge,
                                persist,
                                policy,
                                &batch,
                            );
                            batch.revision_after
                        })
                    });
                engine.drain_events();
                if let Ok(Some(revision)) = &result {
                    let _ = event_tx.send(EditEvent::EditApplied {
                        id: buffer_id,
                        revision: *revision,
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
                let now = clock.now_ms();
                let result = engine
                    .apply_grouped_edit(buffer_id, &ops, selections_after, command_name, now)
                    .map_err(Error::from)
                    .map(|batch| {
                        batch.map(|batch| {
                            record_change_batch(
                                engine.state_mut(),
                                trackers,
                                pending_labels,
                                &mut persistence_bridge,
                                persist,
                                policy,
                                &batch,
                            );
                            batch.revision_after
                        })
                    });
                engine.drain_events();
                broadcast_revision(event_tx, buffer_id, &result);
                let _ = reply.send(result);
            }
            EditorMessage::SetSelections {
                buffer_id,
                selections,
                reply,
            } => {
                let result = engine
                    .set_selections(buffer_id, selections)
                    .map_err(Error::from);
                engine.drain_events();
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
                let result = match engine.selections(buffer_id) {
                    None => Err(Error::UnknownBuffer),
                    Some(current) => {
                        let mut selections = current.to_vec();
                        f(&mut selections);
                        engine
                            .set_selections(buffer_id, selections)
                            .map_err(Error::from)
                    }
                };
                engine.drain_events();
                if result.is_ok() {
                    let _ = event_tx.send(EditEvent::SelectionsChanged { id: buffer_id });
                }
                let _ = reply.send(result);
            }
            EditorMessage::Snapshot { buffer_id, reply } => {
                let snap = engine.snapshot(buffer_id).map(|snapshot| EditorSnapshot {
                    rope: snapshot.rope,
                    selections: snapshot.selections,
                    is_read_only: snapshot.is_read_only,
                    file: engine
                        .state()
                        .get(buffer_id)
                        .and_then(|buffer| buffer.file_association().cloned()),
                });
                let _ = reply.send(snap);
            }
            EditorMessage::SetFileAssociation {
                buffer_id,
                file,
                reply,
            } => {
                let result = engine
                    .state_mut()
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
                let result = apply_history_change(
                    engine.undo(buffer_id, now),
                    engine,
                    trackers,
                    pending_labels,
                    &mut persistence_bridge,
                    persist,
                    policy,
                );
                engine.drain_events();
                broadcast_revision(event_tx, buffer_id, &result);
                let _ = reply.send(result);
            }
            EditorMessage::Redo { buffer_id, reply } => {
                let now = clock.now_ms();
                let result = apply_history_change(
                    engine.redo(buffer_id, now),
                    engine,
                    trackers,
                    pending_labels,
                    &mut persistence_bridge,
                    persist,
                    policy,
                );
                engine.drain_events();
                broadcast_revision(event_tx, buffer_id, &result);
                let _ = reply.send(result);
            }
            EditorMessage::RedoAlternateBranch { buffer_id, reply } => {
                let now = clock.now_ms();
                let result = apply_history_change(
                    engine.redo_alternate(buffer_id, now),
                    engine,
                    trackers,
                    pending_labels,
                    &mut persistence_bridge,
                    persist,
                    policy,
                );
                engine.drain_events();
                broadcast_revision(event_tx, buffer_id, &result);
                let _ = reply.send(result);
            }
            EditorMessage::UndoTreePick { buffer_id, reply } => {
                let result = match engine.state().get(buffer_id) {
                    None => Err(Error::UnknownBuffer),
                    Some(buf) => {
                        log_undo_tree(buf);
                        Ok(())
                    }
                };
                let _ = reply.send(result);
            }
            EditorMessage::ListBuffers { reply } => {
                let summaries: Vec<BufferSummary> = engine
                    .state()
                    .ids()
                    .filter_map(|id| engine.state().get(id).map(|buf| summarize(id, buf)))
                    .collect();
                let _ = reply.send(summaries);
            }
            EditorMessage::MemoryStats { reply } => {
                let _ = reply.send(compute_memory_stats(engine.state()));
            }
            EditorMessage::SetSnapshotPolicy(new_policy) => {
                policy = new_policy;
            }
            EditorMessage::RopeDeltasSince {
                buffer_id,
                since_revision,
                reply,
            } => {
                let answer = engine.deltas_since(buffer_id, since_revision);
                let _ = reply.send(answer);
            }
            EditorMessage::RopeDeltasWithPointsSince {
                buffer_id,
                since_revision,
                reply,
            } => {
                let answer = engine.deltas_with_points_since(buffer_id, since_revision);
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
                flush_all_dirty(engine.state(), trackers, pending_labels, persist);
                break;
            }
        }
    }
}
