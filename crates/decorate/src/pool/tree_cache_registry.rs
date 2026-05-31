//! Cross-worker registry for [`BufferTreeCache`] instrumentation.
//!
//! Each decoration worker owns its own [`BufferTreeCache`], so any UI-side
//! "how big are all tree caches?" query has to gather state across worker
//! threads without locking the worker hot path. This registry gives each
//! worker a per-slot [`WorkerTreeCacheSlot`] it updates after every
//! cache-touching operation; the UI thread reads the slots through
//! [`TreeCacheRegistry::aggregate_bytes`] /
//! [`TreeCacheRegistry::any_worker_has_buffer`].
//!
//! Slots are split into two halves: a single `AtomicUsize` for the
//! byte-size estimate (lock-free) and a `Mutex<Vec<u128>>` for the live
//! buffer-id set (very low contention — the UI thread reads at trace
//! cadence, workers write after every parse). Buffer-id lists are short
//! by construction ([`crate::tree_cache::BUFFER_TREE_CACHE_CAP`] entries
//! at most per worker).
//!
//! **Thread ownership.** Registry: shared across the pool and every
//! worker via `Arc<TreeCacheRegistry>`. Each [`WorkerTreeCacheSlot`] is
//! mutated by exactly one worker thread; all threads read all slots.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::tree_cache::BufferTreeCache;

/// One worker's published view of its [`BufferTreeCache`].
#[derive(Debug, Default)]
pub(crate) struct WorkerTreeCacheSlot {
    byte_size_estimate: AtomicUsize,
    buffer_ids: Mutex<Vec<u128>>,
}

impl WorkerTreeCacheSlot {
    fn new() -> Self {
        Self::default()
    }

    /// Publish the worker's current tree-cache state. Cheap: one atomic
    /// store plus one short `Vec` write under a private mutex.
    pub(crate) fn publish(&self, cache: &BufferTreeCache) {
        self.byte_size_estimate
            .store(cache.byte_size_estimate(), Ordering::Relaxed);
        if let Ok(mut ids) = self.buffer_ids.lock() {
            ids.clear();
            ids.extend(cache.buffer_ids_for_registry());
        }
    }
}

/// Pool-wide registry of per-worker [`WorkerTreeCacheSlot`]s.
#[derive(Debug, Default)]
pub struct TreeCacheRegistry {
    slots: Vec<WorkerTreeCacheSlot>,
}

impl TreeCacheRegistry {
    /// Construct a registry with one empty slot per worker.
    #[must_use]
    pub fn new(worker_count: usize) -> Self {
        let mut slots = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            slots.push(WorkerTreeCacheSlot::new());
        }
        Self { slots }
    }

    /// Borrow the slot for `worker_id`. Returns `None` when `worker_id`
    /// is out of bounds for the configured worker count.
    #[must_use]
    pub(crate) fn slot(&self, worker_id: usize) -> Option<&WorkerTreeCacheSlot> {
        self.slots.get(worker_id)
    }

    /// Sum the byte-size-estimate fields across every worker slot. This
    /// over-counts only when more than one worker happens to hold a tree
    /// for the same buffer — which is the case we want to surface, so
    /// the over-count is the intended signal.
    #[must_use]
    pub fn aggregate_bytes(&self) -> usize {
        self.slots
            .iter()
            .map(|slot| slot.byte_size_estimate.load(Ordering::Relaxed))
            .sum()
    }

    /// `true` iff any worker's published buffer-id set contains
    /// `buffer_id`. Used by `event:buffer_focus_change` emission to
    /// answer "would a focus switch to this buffer hit a worker's tree
    /// cache?".
    #[must_use]
    pub fn any_worker_has_buffer(&self, buffer_id: u128) -> bool {
        self.slots.iter().any(|slot| {
            slot.buffer_ids
                .lock()
                .map(|ids| ids.contains(&buffer_id))
                .unwrap_or(false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_bytes_sums_published_slots() {
        let registry = TreeCacheRegistry::new(3);
        registry.slots[0]
            .byte_size_estimate
            .store(100, Ordering::Relaxed);
        registry.slots[1]
            .byte_size_estimate
            .store(250, Ordering::Relaxed);
        registry.slots[2]
            .byte_size_estimate
            .store(40, Ordering::Relaxed);
        assert_eq!(registry.aggregate_bytes(), 390);
    }

    #[test]
    fn any_worker_has_buffer_scans_every_slot() {
        let registry = TreeCacheRegistry::new(2);
        {
            let mut ids = registry.slots[1].buffer_ids.lock().unwrap();
            ids.push(0xfeed);
        }
        assert!(registry.any_worker_has_buffer(0xfeed));
        assert!(!registry.any_worker_has_buffer(0xbad));
    }
}
