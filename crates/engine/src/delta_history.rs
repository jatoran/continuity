//! Bounded per-buffer rope-delta history.

use std::collections::VecDeque;

use ahash::AHashMap;
use continuity_buffer::BufferId;
use continuity_text::RopeEditDelta;

use crate::RopeEditDeltaWithPoints;

const DELTA_HISTORY_CAP: usize = 512;

#[derive(Clone, Debug)]
struct DeltaHistoryEntry {
    revision: u64,
    deltas: Vec<RopeEditDeltaWithPoints>,
}

#[derive(Debug, Default)]
struct BufferDeltaHistory {
    entries: VecDeque<DeltaHistoryEntry>,
    evicted_revision: u64,
}

/// Bounded delta chains used to project revisioned host data forward.
#[derive(Debug, Default)]
pub struct DeltaHistory {
    buffers: AHashMap<BufferId, BufferDeltaHistory>,
}

impl DeltaHistory {
    pub(crate) fn push(
        &mut self,
        buffer_id: BufferId,
        revision: u64,
        deltas: Vec<RopeEditDeltaWithPoints>,
    ) {
        let history = self.buffers.entry(buffer_id).or_default();
        history
            .entries
            .push_back(DeltaHistoryEntry { revision, deltas });
        while history.entries.len() > DELTA_HISTORY_CAP {
            if let Some(evicted) = history.entries.pop_front() {
                history.evicted_revision = history.evicted_revision.max(evicted.revision);
            }
        }
    }

    pub(crate) fn forget(&mut self, buffer_id: BufferId) {
        self.buffers.remove(&buffer_id);
    }

    /// Return byte deltas strictly newer than `since_revision`.
    #[must_use]
    pub fn since(&self, buffer_id: BufferId, since_revision: u64) -> (Vec<RopeEditDelta>, bool) {
        let (deltas, covered) = self.with_points_since(buffer_id, since_revision);
        (
            deltas.into_iter().map(|delta| delta.delta).collect(),
            covered,
        )
    }

    /// Return position-augmented deltas strictly newer than `since_revision`.
    #[must_use]
    pub fn with_points_since(
        &self,
        buffer_id: BufferId,
        since_revision: u64,
    ) -> (Vec<RopeEditDeltaWithPoints>, bool) {
        let Some(history) = self.buffers.get(&buffer_id) else {
            return (Vec::new(), true);
        };
        if since_revision < history.evicted_revision {
            return (Vec::new(), false);
        }
        let deltas = history
            .entries
            .iter()
            .filter(|entry| entry.revision > since_revision)
            .flat_map(|entry| entry.deltas.iter().copied())
            .collect();
        (deltas, true)
    }
}
