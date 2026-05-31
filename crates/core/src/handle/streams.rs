//! [`EditorHandle`] event subscriptions, snapshot-policy control, and
//! revision-delta accessors.
//!
//! These methods either fire-and-forget (typed owner messages with no
//! reply) or read derived state (the event channel, the delta history)
//! without mutating the buffer set.

use continuity_buffer::BufferId;
use crossbeam_channel::Receiver;

use crate::handle::EditorHandle;
use crate::message::{CoreMemoryStats, EditEvent, EditorMessage};
use crate::policy::SnapshotPolicy;

impl EditorHandle {
    /// Subscribe to broadcast events.
    #[must_use]
    pub fn events(&self) -> &Receiver<EditEvent> {
        &self.event_rx
    }

    /// Phase-16.5 typed owner message: replace the snapshot policy
    /// without stopping the core thread. Best-effort — silently dropped
    /// when the core thread has already exited.
    pub fn set_snapshot_policy(&self, policy: SnapshotPolicy) {
        let _ = self.cmd_tx.send(EditorMessage::SetSnapshotPolicy(policy));
    }

    /// Edits applied to `buffer_id` strictly after `since_revision`,
    /// in apply order. Second tuple field is `true` when the bounded
    /// history covered every revision in `(since_revision, current]`
    /// — `false` means the requested revision was older than the
    /// oldest tracked entry and the caller cannot safely transform
    /// stale spans through; they must be dropped instead. Returns
    /// `(empty, true)` when the buffer is unknown or the buffer has
    /// no edits past `since_revision`.
    #[must_use]
    pub fn rope_deltas_since(
        &self,
        buffer_id: BufferId,
        since_revision: u64,
    ) -> (Vec<continuity_text::RopeEditDelta>, bool) {
        self.round_trip(|reply| EditorMessage::RopeDeltasSince {
            buffer_id,
            since_revision,
            reply,
        })
    }

    /// Core-owned memory counters for trace attribution.
    #[must_use]
    pub fn memory_stats(&self) -> CoreMemoryStats {
        self.round_trip(|reply| EditorMessage::MemoryStats { reply })
    }

    /// ε.4 — position-augmented companion of [`Self::rope_deltas_since`]
    /// for the decoration worker's incremental tree-sitter parse.
    /// Each returned delta carries `start_point`, `old_end_point`,
    /// and `new_end_point` next to the byte-shift component so the
    /// caller can build a `tree_sitter::InputEdit` directly.
    #[must_use]
    pub fn rope_deltas_with_points_since(
        &self,
        buffer_id: BufferId,
        since_revision: u64,
    ) -> (
        Vec<crate::rope_edit_delta_points::RopeEditDeltaWithPoints>,
        bool,
    ) {
        self.round_trip(|reply| EditorMessage::RopeDeltasWithPointsSince {
            buffer_id,
            since_revision,
            reply,
        })
    }

    /// Phase-I1: stage `label` so it is stamped onto the next snapshot
    /// the snapshot policy commits for `buffer_id`. Pass `None` to
    /// clear any staged label. Fire-and-forget; no reply.
    pub fn set_pending_snapshot_label(&self, buffer_id: BufferId, label: Option<String>) {
        let _ = self
            .cmd_tx
            .send(EditorMessage::SetPendingSnapshotLabel { buffer_id, label });
    }
}
