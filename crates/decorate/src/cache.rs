//! Per-buffer revision-keyed [`Decorations`] cache.
//!
//! **Thread ownership**: a single UI thread per cache instance. The cache
//! holds at most one `Arc<Decorations>` per buffer id (the latest one
//! whose revision is `<=` the buffer's current revision). Stale results
//! from the decoration worker pool are discarded on insert.
//!
//! Mirrors the Phase 9 `LayoutCache` ownership model — created and read
//! exclusively from the UI thread that owns the `Window`.
//!
//! Storage wraps each entry in `Arc` so that hot-path reads (e.g.
//! `decoration_cache.get_arc(id).cloned()` on the early-dispatch and
//! paint-decoration-resolve paths) are a refcount bump rather than a
//! deep clone of multi-KB `Vec<BlockSpan>` / `Vec<InlineSpan>` /
//! `Vec<InlineColorSpan>` / `Vec<EvaluatedTable>` fields.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ahash::AHashMap;

use crate::decorations::Decorations;

/// Monotonic counters for [`DecorationCache`] activity.
///
/// `hits` and `misses` are bumped from
/// [`DecorationCache::get`] / [`DecorationCache::get_arc`]. `evictions`
/// is reserved for the bounded LRU policy introduced by Block 1.4 of the
/// memory optimization plan and stays at 0 until that lands; the field
/// is still surfaced now so the trace schema is stable.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct DecorationCacheCounters {
    /// Lookups that returned a cached entry.
    pub hits: u64,
    /// Lookups that found no cached entry.
    pub misses: u64,
    /// Entries removed by the bounded-LRU policy.
    pub evictions: u64,
}

/// Per-buffer cache of the latest accepted [`Decorations`] snapshot.
#[derive(Default)]
pub struct DecorationCache {
    by_buffer: AHashMap<u128, Arc<Decorations>>,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

impl DecorationCache {
    /// Construct an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_buffer: AHashMap::new(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// Snapshot the cache's hit / miss / eviction counters.
    #[must_use]
    pub fn counters(&self) -> DecorationCacheCounters {
        DecorationCacheCounters {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }

    /// Return the top-`n` cached entries by estimated heap bytes,
    /// descending. Cheap when `n` is small (full scan + partial sort).
    #[must_use]
    pub fn largest_entries(&self, n: usize) -> Vec<(u128, usize)> {
        if n == 0 {
            return Vec::new();
        }
        let mut sized: Vec<(u128, usize)> = self
            .by_buffer
            .iter()
            .map(|(buffer_id, arc)| (*buffer_id, arc.byte_size_estimate()))
            .collect();
        sized.sort_by(|a, b| b.1.cmp(&a.1));
        sized.truncate(n);
        sized
    }

    /// Number of buffers with a cached snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_buffer.len()
    }

    /// `true` iff no buffer has a cached snapshot.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_buffer.is_empty()
    }

    /// Estimated heap bytes retained by cached decoration snapshots.
    #[must_use]
    pub fn byte_size_estimate(&self) -> usize {
        self.by_buffer
            .values()
            .map(|arc| arc.byte_size_estimate())
            .sum()
    }

    /// Look up the cached decoration for `buffer_id`. Returns a borrow
    /// through the storage `Arc`; read-only consumers should prefer
    /// this. Callers that need a clonable handle (refcount bump) should
    /// call [`DecorationCache::get_arc`] instead.
    #[must_use]
    pub fn get(&self, buffer_id: u128) -> Option<&Decorations> {
        let found = self.by_buffer.get(&buffer_id).map(Arc::as_ref);
        if found.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        found
    }

    /// Look up the cached decoration for `buffer_id` as the stored
    /// `Arc`. Callers that need to hold the result past the borrow can
    /// call `.cloned()` (or `Arc::clone`) to obtain a cheap refcount
    /// bump instead of a deep clone of the underlying `Decorations`.
    #[must_use]
    pub fn get_arc(&self, buffer_id: u128) -> Option<&Arc<Decorations>> {
        let found = self.by_buffer.get(&buffer_id);
        if found.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        found
    }

    /// Insert `value` for `buffer_id` if its revision is at least as fresh
    /// as anything already cached and its content changed. Returns `true`
    /// when the cache was updated.
    pub fn insert(&mut self, buffer_id: u128, value: Decorations) -> bool {
        match self.by_buffer.get(&buffer_id) {
            Some(existing) if existing.revision >= value.revision => false,
            Some(existing)
                if existing.blocks == value.blocks
                    && existing.inlines == value.inlines
                    && existing.highlights == value.highlights
                    && existing.inline_color_spans == value.inline_color_spans
                    && existing.evaluated_tables == value.evaluated_tables =>
            {
                false
            }
            _ => {
                self.by_buffer.insert(buffer_id, Arc::new(value));
                true
            }
        }
    }

    /// Drop the snapshot for `buffer_id`.
    pub fn evict(&mut self, buffer_id: u128) {
        self.by_buffer.remove(&buffer_id);
    }

    /// Drop every cached snapshot.
    pub fn clear(&mut self) {
        self.by_buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spans::{BlockKind, BlockSpan};

    fn decorations_with_block(revision: u64, start_byte: usize, end_byte: usize) -> Decorations {
        let mut decorations = Decorations::empty(revision);
        decorations.blocks.push(BlockSpan {
            kind: BlockKind::Paragraph,
            start_byte,
            end_byte,
        });
        decorations
    }

    #[test]
    fn newer_revision_replaces_older() {
        let mut c = DecorationCache::new();
        let _ = c.insert(1, decorations_with_block(0, 0, 4));
        let inserted = c.insert(1, decorations_with_block(1, 0, 8));
        assert!(inserted);
        assert_eq!(c.get(1).unwrap().revision, 1);
    }

    #[test]
    fn older_revision_is_discarded() {
        let mut c = DecorationCache::new();
        let _ = c.insert(1, Decorations::empty(5));
        let inserted = c.insert(1, Decorations::empty(2));
        assert!(!inserted);
        assert_eq!(c.get(1).unwrap().revision, 5);
    }

    #[test]
    fn equal_revision_is_discarded() {
        let mut c = DecorationCache::new();
        let _ = c.insert(1, Decorations::empty(5));
        let again = c.insert(1, Decorations::empty(5));
        assert!(!again);
    }

    #[test]
    fn newer_revision_with_same_content_is_discarded() {
        let mut c = DecorationCache::new();
        let _ = c.insert(1, decorations_with_block(5, 0, 4));
        let inserted = c.insert(1, decorations_with_block(6, 0, 4));
        assert!(!inserted);
        assert_eq!(c.get(1).unwrap().revision, 5);
    }

    #[test]
    fn evict_removes() {
        let mut c = DecorationCache::new();
        let _ = c.insert(1, Decorations::empty(0));
        c.evict(1);
        assert!(c.get(1).is_none());
    }

    #[test]
    fn get_arc_returns_same_arc_across_reads() {
        // The whole point of the Arc-wrap: two reads of the same cache
        // entry hand back pointers to the same allocation, so a
        // `.cloned()` on the result is a refcount bump rather than a
        // deep clone of the underlying `Decorations`.
        let mut c = DecorationCache::new();
        let _ = c.insert(1, decorations_with_block(5, 0, 4));
        let a = c.get_arc(1).expect("inserted").clone();
        let b = c.get_arc(1).expect("inserted").clone();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn equal_revision_insert_preserves_arc_identity() {
        // A rejected insert (same/older revision, or matching content
        // at a higher revision) MUST leave the existing `Arc` in place
        // — callers may be holding clones and the rejected payload
        // contributes no new content.
        let mut c = DecorationCache::new();
        let _ = c.insert(1, decorations_with_block(5, 0, 4));
        let before = c.get_arc(1).expect("inserted").clone();
        let again = c.insert(1, decorations_with_block(5, 0, 4));
        assert!(!again);
        let after = c.get_arc(1).expect("still present").clone();
        assert!(Arc::ptr_eq(&before, &after));
    }

    #[test]
    fn accepted_insert_replaces_arc_identity() {
        // An accepted insert must publish a fresh `Arc`; clones taken
        // before the insert still point at the prior `Decorations`,
        // and the cache now hands out a different allocation.
        let mut c = DecorationCache::new();
        let _ = c.insert(1, decorations_with_block(5, 0, 4));
        let before = c.get_arc(1).expect("inserted").clone();
        let inserted = c.insert(1, decorations_with_block(6, 0, 8));
        assert!(inserted);
        let after = c.get_arc(1).expect("still present").clone();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(before.revision, 5);
        assert_eq!(after.revision, 6);
    }

    #[test]
    fn evict_drops_only_target_buffer() {
        // Eviction semantics are unchanged by the Arc-wrap: evicting
        // one buffer leaves other entries' `Arc`s identity-stable.
        let mut c = DecorationCache::new();
        let _ = c.insert(1, decorations_with_block(5, 0, 4));
        let _ = c.insert(2, decorations_with_block(7, 0, 4));
        let two_before = c.get_arc(2).expect("inserted").clone();
        c.evict(1);
        assert!(c.get(1).is_none());
        let two_after = c.get_arc(2).expect("survived").clone();
        assert!(Arc::ptr_eq(&two_before, &two_after));
    }

    #[test]
    fn transformed_through_does_not_mutate_cached_arc() {
        // Stamp-drift path: callers that need a rev-shifted view call
        // `Decorations::transformed_through(&self, ...)`, which
        // returns a fresh owned `Decorations`. The cache entry's
        // `Arc<Decorations>` is unchanged — both its identity and its
        // contents survive the call.
        let mut c = DecorationCache::new();
        let _ = c.insert(1, decorations_with_block(5, 0, 4));
        let cached = c.get_arc(1).expect("inserted").clone();
        let drifted = cached.transformed_through(&[], 9);
        assert_eq!(drifted.revision, 9);
        let after = c.get_arc(1).expect("still present").clone();
        assert!(Arc::ptr_eq(&cached, &after));
        assert_eq!(after.revision, 5);
    }
}
