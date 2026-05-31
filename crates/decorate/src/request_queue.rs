//! Latest-wins request queue for the decoration worker pool.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

use crate::pool::{DecorateRequest, PoolShutdown};

/// Internal queue: at most one pending request per `buffer_id`.
///
/// Newer revisions overwrite older. ε.4 partitions by worker so a buffer's
/// requests always land on the same worker, keeping that worker's
/// `BufferTreeCache` coherent for incremental tree-sitter parse.
#[derive(Default)]
pub(crate) struct LatestQueue {
    /// One slot per worker.
    slots: Vec<HashMap<u128, DecorateRequest>>,
    /// Per-worker drop-buffer mailboxes.
    drop_buffers: Vec<Vec<u128>>,
    shutdown: bool,
}

impl LatestQueue {
    /// Build a queue with at least one worker slot.
    pub(crate) fn new(worker_count: usize) -> Self {
        let n = worker_count.max(1);
        Self {
            slots: (0..n).map(|_| HashMap::new()).collect(),
            drop_buffers: (0..n).map(|_| Vec::new()).collect(),
            shutdown: false,
        }
    }

    /// Pop the highest-revision pending request from `worker_id`'s slot.
    pub(crate) fn pop_one_for_worker(&mut self, worker_id: usize) -> Option<DecorateRequest> {
        let slot = self.slots.get_mut(worker_id)?;
        let key = slot
            .iter()
            .max_by_key(|(_, request)| request.revision)
            .map(|(key, _)| *key)?;
        slot.remove(&key)
    }

    /// Drain `worker_id`'s pending drop-buffer ids.
    pub(crate) fn take_drops_for_worker(&mut self, worker_id: usize) -> Vec<u128> {
        self.drop_buffers
            .get_mut(worker_id)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Number of pending requests across all worker slots.
    pub(crate) fn pending(&self) -> usize {
        self.slots.iter().map(HashMap::len).sum()
    }

    /// `true` once shutdown starts.
    pub(crate) fn is_shutdown(&self) -> bool {
        self.shutdown
    }

    /// Mark shutdown and drop every queued request.
    pub(crate) fn shutdown_and_clear(&mut self) {
        self.shutdown = true;
        for slot in &mut self.slots {
            slot.clear();
        }
    }

    /// Push a closed-buffer cleanup marker to every worker mailbox.
    pub(crate) fn broadcast_drop_buffer(&mut self, buffer_id: u128) {
        for slot in &mut self.drop_buffers {
            slot.push(buffer_id);
        }
    }
}

/// Stable worker assignment for a buffer.
///
/// Decorating multiple requests for the same buffer on the same worker keeps
/// the worker's cached `Tree` coherent. Different workers have independent
/// caches, and cross-worker handoff would invalidate the cached tree's
/// revision against the producer's `prev_revision` hint.
#[inline]
pub(crate) fn route_worker_for_buffer(buffer_id: u128, worker_count: usize) -> usize {
    if worker_count <= 1 {
        return 0;
    }
    let mixed = (buffer_id as u64) ^ ((buffer_id >> 64) as u64);
    (mixed as usize) % worker_count
}

pub(crate) fn enqueue_request(
    queue: &Arc<(Mutex<LatestQueue>, Condvar)>,
    req: DecorateRequest,
) -> Result<(), PoolShutdown> {
    let (lock, cvar) = &**queue;
    let mut guard = lock.lock().map_err(|_| PoolShutdown)?;
    if guard.is_shutdown() {
        return Err(PoolShutdown);
    }
    let worker_id = route_worker_for_buffer(req.buffer_id, guard.slots.len());
    let slot = guard
        .slots
        .get_mut(worker_id)
        .expect("invariant: route_worker_for_buffer returns a valid slot index");
    match slot.get(&req.buffer_id) {
        Some(existing) if existing.revision >= req.revision => {}
        _ => {
            slot.insert(req.buffer_id, req);
        }
    }
    cvar.notify_all();
    Ok(())
}

pub(crate) fn is_shutdown(queue: &Arc<(Mutex<LatestQueue>, Condvar)>) -> bool {
    queue
        .0
        .lock()
        .map(|guard| guard.is_shutdown())
        .unwrap_or(true)
}
