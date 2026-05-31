//! The persistence thread's drain loop.
//!
//! Pulled out of [`crate::handle`] so that file stays under the 600-line
//! cap. **Thread ownership**: every function here runs on the
//! `continuity-persist` thread (the unique owner of the
//! [`crate::Store`] and its `rusqlite::Connection`). The shared
//! `Arc<AtomicUsize>` is the byte-accounting gauge consumed by
//! [`crate::PersistClient::unflushed_bytes`].
//!
//! δ.3 — fire-and-forget write failures now emit a typed
//! [`PersistEvent::WriteFailed`] in addition to the historical
//! `eprintln!`, so the registry can fan a sticky `FileBanner` to every
//! live window. A final [`PersistEvent::ThreadStopped`] is emitted on
//! clean shutdown so the receiver can distinguish a planned exit from a
//! channel-disconnect on panic.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};

use crate::budget::{edit_row_byte_cost, snapshot_byte_cost};
use crate::events::{PersistEvent, PersistOperation};
use crate::message::PersistMessage;
use crate::store::Store;

fn report_write_failure<E: std::fmt::Display>(
    events: &Sender<PersistEvent>,
    kind: PersistOperation,
    err: &E,
) {
    eprintln!("continuity-persist: {} failed: {err}", kind.as_str());
    let _ = events.send(PersistEvent::WriteFailed {
        kind,
        message: err.to_string(),
    });
}

/// Drain `rx`, dispatching each [`PersistMessage`] against `store`, and
/// keeping `unflushed` in sync with the bytes the queue has accepted but
/// not yet committed.
pub(crate) fn persist_loop(
    store: Store,
    rx: &Receiver<PersistMessage>,
    unflushed: &Arc<AtomicUsize>,
    events: Sender<PersistEvent>,
) {
    while let Ok(msg) = rx.recv() {
        match msg {
            PersistMessage::AppendEdit { row, edit_seq } => {
                let _seq_guard = edit_seq.map(crate::trace::bind_edit_seq);
                let cost = edit_row_byte_cost(&row);
                let _s = crate::trace::Scope::with_detail(
                    "persist_loop_append_edit",
                    format!("cost={cost} qlen={}", rx.len()),
                );
                if let Err(e) = store.append_edit(&row) {
                    report_write_failure(&events, PersistOperation::AppendEdit, &e);
                }
                unflushed.fetch_sub(cost, Ordering::AcqRel);
            }
            PersistMessage::SaveSnapshot {
                buffer_id,
                snapshot,
                ack,
            } => {
                let cost = snapshot_byte_cost(&snapshot);
                let _s = crate::trace::Scope::with_detail(
                    "persist_loop_save_snapshot",
                    format!(
                        "cost={cost} rev={} blocking={}",
                        snapshot.revision().get(),
                        ack.is_some(),
                    ),
                );
                let result = store.save_snapshot(buffer_id, &snapshot);
                if let Some(ack) = ack {
                    let _ = ack.send(result);
                } else if let Err(e) = result {
                    report_write_failure(&events, PersistOperation::SaveSnapshot, &e);
                }
                unflushed.fetch_sub(cost, Ordering::AcqRel);
            }
            PersistMessage::UpsertBuffer {
                buffer_id,
                created_at_ms,
                last_touched_ms,
            } => {
                if let Err(e) = store.upsert_buffer(buffer_id, created_at_ms, last_touched_ms) {
                    report_write_failure(&events, PersistOperation::UpsertBuffer, &e);
                }
            }
            PersistMessage::TouchBuffer {
                buffer_id,
                last_touched_ms,
            } => {
                if let Err(e) = store.touch_buffer(buffer_id, last_touched_ms) {
                    report_write_failure(&events, PersistOperation::TouchBuffer, &e);
                }
            }
            PersistMessage::LoadLatestSnapshot { buffer_id, reply } => {
                let _ = reply.send(store.load_latest_valid_snapshot(buffer_id));
            }
            PersistMessage::EditsSince {
                buffer_id,
                after_revision,
                reply,
            } => {
                let _ = reply.send(store.edits_since(buffer_id, after_revision));
            }
            PersistMessage::MostRecentBuffer { reply } => {
                let _ = reply.send(store.most_recent_buffer());
            }
            PersistMessage::PruneCoveredEdits {
                buffer_id,
                at_or_before,
            } => {
                if let Err(e) = store.prune_edits_at_or_before(buffer_id, at_or_before) {
                    report_write_failure(&events, PersistOperation::PruneCoveredEdits, &e);
                }
            }
            PersistMessage::MoveToTrash {
                buffer_id,
                now_ms,
                retention_days,
            } => {
                if let Err(e) = store.move_to_trash(buffer_id, now_ms, retention_days) {
                    report_write_failure(&events, PersistOperation::MoveToTrash, &e);
                }
            }
            PersistMessage::PurgeExpired { now_ms, reply } => {
                let _ = reply.send(store.purge_expired(now_ms));
            }
            PersistMessage::Backup { dest_path, reply } => {
                let result = store.online_backup(&dest_path);
                if let Some(ack) = reply {
                    let _ = ack.send(result);
                } else if let Err(e) = result {
                    report_write_failure(&events, PersistOperation::Backup, &e);
                }
            }
            PersistMessage::SetSynchronous { value } => {
                if let Err(e) = store.set_synchronous(&value) {
                    eprintln!("continuity-persist: set_synchronous({value}) failed: {e}");
                    let _ = events.send(PersistEvent::WriteFailed {
                        kind: PersistOperation::SetSynchronous,
                        message: e.to_string(),
                    });
                }
            }
            PersistMessage::WriteUndoGroup { row } => {
                let _s = crate::trace::Scope::with_detail(
                    "persist_loop_write_undo_group",
                    format!("qlen={}", rx.len()),
                );
                if let Err(e) = store.insert_undo_group(&row) {
                    report_write_failure(&events, PersistOperation::WriteUndoGroup, &e);
                }
            }
            PersistMessage::LoadUndoGroups { buffer_id, reply } => {
                let _ = reply.send(store.load_undo_groups(buffer_id));
            }
            PersistMessage::SaveWindow { row, reply } => {
                let result = crate::window_state::save_window(store.conn(), &row);
                if let Some(ack) = reply {
                    let _ = ack.send(result);
                } else if let Err(e) = result {
                    report_write_failure(&events, PersistOperation::SaveWindow, &e);
                }
            }
            PersistMessage::DeleteWindow { id, now_ms, reply } => {
                let _ = reply.send(crate::window_state::delete_window(store.conn(), id, now_ms));
            }
            PersistMessage::LoadActiveWindows { reply } => {
                let _ = reply.send(crate::window_state::load_active_windows(store.conn()));
            }
            PersistMessage::SetBufferFile {
                buffer_id,
                file,
                reply,
            } => {
                let result =
                    crate::file_assoc::set_buffer_file(store.conn(), buffer_id, file.as_ref());
                if let Some(ack) = reply {
                    let _ = ack.send(result);
                } else if let Err(e) = result {
                    report_write_failure(&events, PersistOperation::SetBufferFile, &e);
                }
            }
            PersistMessage::LoadBufferFile { buffer_id, reply } => {
                let _ = reply.send(crate::file_assoc::load_buffer_file(store.conn(), buffer_id));
            }
            PersistMessage::LoadActiveBufferIds { reply } => {
                let _ = reply.send(crate::file_assoc::load_active_buffer_ids(store.conn()));
            }
            PersistMessage::NextSeq { buffer_id, reply } => {
                let _ = reply.send(store.next_seq(buffer_id));
            }
            PersistMessage::SetSnapshotLabel {
                buffer_id,
                revision,
                label,
                reply,
            } => {
                let result = store.set_snapshot_label(buffer_id, revision, label.as_deref());
                if let Some(ack) = reply {
                    let _ = ack.send(result);
                } else if let Err(e) = result {
                    report_write_failure(&events, PersistOperation::SetSnapshotLabel, &e);
                }
            }
            PersistMessage::ListSnapshotSummaries { buffer_id, reply } => {
                let _ = reply.send(store.list_snapshot_summaries(buffer_id));
            }
            PersistMessage::LoadContentAtRevision {
                buffer_id,
                target_revision,
                reply,
            } => {
                let _ = reply.send(store.load_content_at_revision(buffer_id, target_revision));
            }
            PersistMessage::RecordMetricsDelta { delta } => {
                if let Err(e) = store.record_metrics_delta(&delta) {
                    report_write_failure(&events, PersistOperation::RecordMetricsDelta, &e);
                }
            }
            PersistMessage::LoadMetricsRange {
                start_day_iso,
                end_day_iso,
                reply,
            } => {
                let _ = reply.send(store.load_metrics_range(&start_day_iso, &end_day_iso));
            }
            PersistMessage::PurgeMetrics { reply } => {
                let _ = reply.send(store.purge_metrics());
            }
            PersistMessage::LoadTopBuffersByEdits {
                start_ms,
                end_ms,
                limit,
                reply,
            } => {
                let _ = reply.send(store.load_top_buffers_by_edits(start_ms, end_ms, limit));
            }
            PersistMessage::ListBufferRecords { filter, reply } => {
                let _ = reply.send(store.list_buffer_records(filter));
            }
            PersistMessage::ListBufferHistoryTimeline { filter, reply } => {
                let _ = reply.send(store.load_buffer_history_timeline(filter));
            }
            PersistMessage::PushClosedHistory {
                kind,
                window_id,
                payload_json,
                closed_at_ms,
                reply,
            } => {
                let result = crate::closed_history::push_closed_history(
                    store.conn(),
                    kind,
                    window_id,
                    &payload_json,
                    closed_at_ms,
                );
                let _ = reply.send(result);
            }
            PersistMessage::PopClosedHistory { reply } => {
                let _ = reply.send(crate::closed_history::pop_closed_history(store.conn()));
            }
            PersistMessage::PeekClosedHistory { reply } => {
                let _ = reply.send(crate::closed_history::peek_closed_history(store.conn()));
            }
            PersistMessage::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }
    // Reclaim WAL space on a clean shutdown. Best-effort.
    let _ = store
        .conn()
        .pragma_update(None, "wal_checkpoint", "TRUNCATE");
    // δ.3 — final event so the receiver can distinguish a clean
    // shutdown from a channel-disconnect on panic. Sent *before* the
    // Sender drops on function return.
    let _ = events.send(PersistEvent::ThreadStopped);
}
