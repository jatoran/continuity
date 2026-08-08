//! Native host handling after a storage-neutral engine mutation.

use ahash::AHashMap;
use continuity_buffer::{BufferId, Revision};
use continuity_engine::{ChangeBatch, EngineState, MutationKind};
use continuity_persist::PersistClient;

use crate::persistence_bridge::PersistenceBridge;
use crate::policy::{edit_byte_delta, SnapshotPolicy, SnapshotTracker, SnapshotTrigger};
use crate::trace;

/// Persist a completed engine batch and apply the native snapshot policy.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_change_batch(
    state: &mut EngineState,
    trackers: &mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &mut AHashMap<BufferId, String>,
    bridge: &mut PersistenceBridge<'_>,
    persist: &PersistClient,
    policy: SnapshotPolicy,
    batch: &ChangeBatch,
) {
    let _scope = trace::Scope::with_detail(
        "core_persist_change_batch",
        format!("ops={} kind={:?}", batch.changes.len(), batch.kind),
    );
    bridge.persist_batch(batch);
    let byte_delta = if matches!(batch.kind, MutationKind::Undo | MutationKind::Redo) {
        state
            .get(batch.buffer_id)
            .map(|buffer| buffer.rope().len_bytes())
            .unwrap_or(0)
    } else {
        batch
            .changes
            .iter()
            .map(|change| edit_byte_delta(&change.op, change.removed_text.len()))
            .fold(0usize, usize::saturating_add)
    };
    record_snapshot_policy(SnapshotRecordContext {
        state,
        trackers,
        pending_labels,
        persist,
        policy,
        buffer_id: batch.buffer_id,
        revision: batch.revision_after,
        now: batch.timestamp_ms,
        byte_delta,
    });
}

/// On shutdown, persist a final blocking snapshot for every dirty buffer.
pub(crate) fn flush_all_dirty(
    state: &EngineState,
    trackers: &mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &mut AHashMap<BufferId, String>,
    persist: &PersistClient,
) {
    for id in state.ids().collect::<Vec<_>>() {
        let is_dirty = trackers
            .get(&id)
            .is_some_and(|tracker| tracker.edits_since() > 0 || tracker.bytes_since() > 0);
        if !is_dirty {
            continue;
        }
        if let Some(buffer) = state.get(id) {
            let snapshot = buffer.snapshot();
            let revision = snapshot.revision();
            if let Err(error) = persist.save_snapshot_blocking(id, snapshot) {
                eprintln!("continuity-core: shutdown snapshot failed: {error}");
                continue;
            }
            if let Some(label) = pending_labels.remove(&id) {
                let _ = persist.set_snapshot_label(id, revision, Some(label));
            }
        }
    }
}

fn record_snapshot_policy(context: SnapshotRecordContext<'_>) {
    let Some(tracker) = context.trackers.get_mut(&context.buffer_id) else {
        return;
    };
    if !matches!(
        tracker.record_edit(context.byte_delta, context.now, &context.policy),
        SnapshotTrigger::Threshold
    ) {
        return;
    }
    if let Some(buffer) = context.state.get_mut(context.buffer_id) {
        let (observed, computed) = buffer.verify_running_checksum();
        if observed != computed {
            trace::log_event(
                "checksum_drift",
                0,
                &format!(
                    "observed={observed:#x} expected={computed:#x} revision={} \
                     rope_bytes={} trigger=snapshot",
                    context.revision.get(),
                    buffer.rope().len_bytes()
                ),
            );
        }
    }
    if let Some(buffer) = context.state.get(context.buffer_id) {
        let snapshot = buffer.snapshot();
        let snapshot_revision = snapshot.revision();
        let _ = context
            .persist
            .save_snapshot_async(context.buffer_id, snapshot);
        if let Some(label) = context.pending_labels.remove(&context.buffer_id) {
            let _ = context.persist.set_snapshot_label(
                context.buffer_id,
                snapshot_revision,
                Some(label),
            );
        }
        let _ = context
            .persist
            .prune_covered_edits(context.buffer_id, context.revision);
        let _ = context.persist.touch_buffer(context.buffer_id, context.now);
        tracker.reset(context.now);
    }
}

struct SnapshotRecordContext<'a> {
    state: &'a mut EngineState,
    trackers: &'a mut AHashMap<BufferId, SnapshotTracker>,
    pending_labels: &'a mut AHashMap<BufferId, String>,
    persist: &'a PersistClient,
    policy: SnapshotPolicy,
    buffer_id: BufferId,
    revision: Revision,
    now: i64,
    byte_delta: usize,
}
