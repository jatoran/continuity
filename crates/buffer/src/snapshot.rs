//! Cheap, send-able rope snapshots tagged with a revision.
//!
//! `RopeSnapshot` is the only buffer-shaped value that crosses thread
//! boundaries: decoration workers, the persistence thread, and the search
//! indexer all consume snapshots produced on the editor core thread.
//!
//! Every constructed snapshot registers a `Weak<Rope>` with the
//! process-wide [`RopeSnapshotRegistry`] and deregisters on drop. The
//! registry surfaces "how many live snapshots exist" and "how many
//! distinct `Arc<Rope>` heads do they pin" — the two numbers Block 1.2
//! of the memory optimization plan uses to detect divergent-rope
//! generations held by long-lived workers.
//!
//! **Limitation.** We compare `Arc<Rope>` pointer identity, not the
//! inner `Arc<Node>` head ropey uses for structural sharing. Two
//! `RopeSnapshot`s built from independent `Arc::new(rope.clone())`
//! calls count as two distinct generations even when the underlying
//! ropey state is byte-identical. The metric therefore tracks
//! "distinct snapshot rope handles" rather than the structurally-true
//! "distinct rope generations"; a single distinct handle is still the
//! healthy steady state we want to gate on.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use ropey::Rope;

use crate::Revision;

/// Process-wide registry of live `RopeSnapshot`s. Entries are weak
/// references; drops deregister explicitly so the map does not bloat
/// with stale weaks during steady state.
#[derive(Debug, Default)]
pub struct RopeSnapshotRegistry {
    by_id: Mutex<HashMap<u64, Weak<Rope>>>,
}

impl RopeSnapshotRegistry {
    /// Borrow the process-wide registry.
    #[must_use]
    pub fn instance() -> &'static Self {
        static INSTANCE: OnceLock<RopeSnapshotRegistry> = OnceLock::new();
        INSTANCE.get_or_init(RopeSnapshotRegistry::default)
    }

    /// Number of currently-live `RopeSnapshot`s. Strong refs that have
    /// already been dropped are excluded.
    #[must_use]
    pub fn live_snapshot_count(&self) -> usize {
        match self.by_id.lock() {
            Ok(map) => map.values().filter(|weak| weak.strong_count() > 0).count(),
            Err(_) => 0,
        }
    }

    /// Count of distinct `Arc<Rope>` heads across live snapshots. A
    /// healthy editor with N snapshots all built from one source
    /// returns 1; an N-distinct-generations leak returns N.
    #[must_use]
    pub fn distinct_arc_heads(&self) -> usize {
        let Ok(map) = self.by_id.lock() else {
            return 0;
        };
        let mut heads: Vec<*const Rope> = Vec::with_capacity(map.len());
        for weak in map.values() {
            if let Some(strong) = weak.upgrade() {
                heads.push(Arc::as_ptr(&strong));
            }
        }
        heads.sort_unstable();
        heads.dedup();
        heads.len()
    }

    fn register(&self, id: u64, rope: &Arc<Rope>) {
        if let Ok(mut map) = self.by_id.lock() {
            map.insert(id, Arc::downgrade(rope));
        }
    }

    fn deregister(&self, id: u64) {
        if let Ok(mut map) = self.by_id.lock() {
            map.remove(&id);
        }
    }
}

fn next_snapshot_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// An immutable, ref-counted view of a buffer's contents at a specific
/// revision.
#[derive(Debug)]
pub struct RopeSnapshot {
    rope: Arc<Rope>,
    revision: Revision,
    /// Registry slot id; freed on drop.
    registry_id: u64,
}

impl Clone for RopeSnapshot {
    fn clone(&self) -> Self {
        let registry_id = next_snapshot_id();
        RopeSnapshotRegistry::instance().register(registry_id, &self.rope);
        Self {
            rope: Arc::clone(&self.rope),
            revision: self.revision,
            registry_id,
        }
    }
}

impl Drop for RopeSnapshot {
    fn drop(&mut self) {
        RopeSnapshotRegistry::instance().deregister(self.registry_id);
    }
}

impl RopeSnapshot {
    /// Construct a snapshot. Callers should prefer [`crate::Buffer::snapshot`].
    #[must_use]
    pub fn new(rope: Arc<Rope>, revision: Revision) -> Self {
        let registry_id = next_snapshot_id();
        RopeSnapshotRegistry::instance().register(registry_id, &rope);
        Self {
            rope,
            revision,
            registry_id,
        }
    }

    /// Borrow the rope.
    #[must_use]
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Borrow the underlying `Arc<Rope>`. Lets consumers clone the
    /// `Arc` to ship the rope across thread boundaries without
    /// allocating a fresh refcounted wrapper.
    #[must_use]
    pub fn rope_arc(&self) -> &Arc<Rope> {
        &self.rope
    }

    /// The revision this snapshot was taken at.
    #[must_use]
    pub fn revision(&self) -> Revision {
        self.revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_clone_shares_underlying_rope() {
        let rope = Arc::new(Rope::from_str("hello"));
        let s = RopeSnapshot::new(Arc::clone(&rope), Revision(5));
        let s2 = s.clone();
        assert_eq!(s.revision(), s2.revision());
        // Cloning the snapshot must NOT clone the rope — both `&Rope`s point
        // at the same allocation.
        assert!(std::ptr::eq(s.rope(), s2.rope()));
        assert_eq!(s.rope().to_string(), "hello");
    }

    #[test]
    fn snapshot_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RopeSnapshot>();
    }

    #[test]
    fn snapshot_registry_tracks_distinct_heads() {
        // Two snapshots cloned from the same Arc share one head; a
        // separately-constructed snapshot bumps the head count. After
        // every snapshot drops, both counters return to their prior
        // baseline.
        let registry = RopeSnapshotRegistry::instance();
        let heads_before = registry.distinct_arc_heads();
        let live_before = registry.live_snapshot_count();
        let rope_a = Arc::new(Rope::from_str("alpha"));
        let s1 = RopeSnapshot::new(Arc::clone(&rope_a), Revision(1));
        let s2 = s1.clone();
        assert_eq!(registry.live_snapshot_count(), live_before + 2);
        assert_eq!(registry.distinct_arc_heads(), heads_before + 1);
        let rope_b = Arc::new(Rope::from_str("beta"));
        let s3 = RopeSnapshot::new(rope_b, Revision(2));
        assert_eq!(registry.distinct_arc_heads(), heads_before + 2);
        drop(s1);
        drop(s2);
        drop(s3);
        assert_eq!(registry.live_snapshot_count(), live_before);
    }
}
