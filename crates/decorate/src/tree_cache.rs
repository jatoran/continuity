//! Per-worker tree-sitter parse tree cache for decoration workers.

use std::collections::VecDeque;

use tree_sitter::Tree;

/// Cap on per-worker cached trees.
///
/// Bounded LRU eviction prevents pathological accumulation in long sessions
/// where a buffer-close hook might lag or the user opens many transient
/// buffers between close events.
pub const BUFFER_TREE_CACHE_CAP: usize = 32;

#[derive(Clone)]
struct BufferTreeEntry {
    buffer_id: u128,
    revision: u64,
    source_len: usize,
    tree: Tree,
}

/// Owned cached tree snapshot returned to the worker loop.
#[derive(Clone)]
pub struct CachedBufferTree {
    /// Revision the cached tree was parsed against.
    pub revision: u64,
    /// Source byte length at that revision.
    pub source_len: usize,
    /// Cached tree for `revision`.
    pub tree: Tree,
}

/// ε.4 — per-worker incremental-parse state.
///
/// Each worker thread owns one cache, holding the persistent tree-sitter
/// `Tree` for every buffer the worker has parsed at least once. On watchdog
/// restart the cache is dropped, so the first request after restart
/// full-reparses, which is the correct safety floor.
#[derive(Default)]
pub struct BufferTreeCache {
    /// LRU order: front = least recently used, back = most recent.
    entries: VecDeque<BufferTreeEntry>,
}

impl BufferTreeCache {
    /// Construct an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    /// Borrow the cached tree for `buffer_id`, if any. Moves the matched
    /// entry to the back so later evictions target the true oldest entry.
    pub fn get(&mut self, buffer_id: u128) -> Option<&Tree> {
        let pos = self
            .entries
            .iter()
            .position(|entry| entry.buffer_id == buffer_id)?;
        let entry = self.entries.remove(pos)?;
        self.entries.push_back(entry);
        self.entries.back().map(|entry| &entry.tree)
    }

    /// Clone the cached tree only if it belongs to `revision`.
    pub fn get_for_revision(&mut self, buffer_id: u128, revision: u64) -> Option<CachedBufferTree> {
        let pos = self
            .entries
            .iter()
            .position(|entry| entry.buffer_id == buffer_id && entry.revision == revision)?;
        let entry = self.entries.remove(pos)?;
        let cached = CachedBufferTree {
            revision: entry.revision,
            source_len: entry.source_len,
            tree: entry.tree.clone(),
        };
        self.entries.push_back(entry);
        Some(cached)
    }

    /// Borrow without touching LRU order (test instrumentation).
    #[must_use]
    pub fn peek(&self, buffer_id: u128) -> Option<&Tree> {
        self.entries
            .iter()
            .find(|entry| entry.buffer_id == buffer_id)
            .map(|entry| &entry.tree)
    }

    /// Store the newly parsed tree for `buffer_id`.
    pub fn insert(&mut self, buffer_id: u128, revision: u64, source_len: usize, tree: Tree) {
        if let Some(pos) = self
            .entries
            .iter()
            .position(|entry| entry.buffer_id == buffer_id)
        {
            self.entries.remove(pos);
        }
        self.entries.push_back(BufferTreeEntry {
            buffer_id,
            revision,
            source_len,
            tree,
        });
        while self.entries.len() > BUFFER_TREE_CACHE_CAP {
            self.entries.pop_front();
        }
    }

    /// Drop the tree for `buffer_id`.
    pub fn drop_buffer(&mut self, buffer_id: u128) {
        if let Some(pos) = self
            .entries
            .iter()
            .position(|entry| entry.buffer_id == buffer_id)
        {
            self.entries.remove(pos);
        }
    }

    /// Number of buffers tracked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when no buffers are cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `true` iff a cached entry exists for `buffer_id` (does not touch
    /// LRU order — intended for instrumentation queries from outside the
    /// worker).
    #[must_use]
    pub fn contains_buffer(&self, buffer_id: u128) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.buffer_id == buffer_id)
    }

    /// Iterate the cached buffer ids in LRU order (least recent first).
    /// Used by the pool's tree-cache registry to publish the worker's
    /// current cached-buffer set to the UI thread.
    pub fn buffer_ids_for_registry(&self) -> impl Iterator<Item = u128> + '_ {
        self.entries.iter().map(|entry| entry.buffer_id)
    }

    /// Estimated bytes retained by cached parse trees.
    ///
    /// **This is a lower-bound proxy.** For each cached entry we charge
    /// `descendant_count * 64` bytes — a coarse approximation of
    /// `sizeof(TSSubtree)` per node. tree-sitter packs short leaves
    /// inline and shares subtree pools, so the real heap footprint is
    /// typically **1.5×-3× higher** than what this method reports. Use
    /// the figure as a trend signal across traces, not an absolute heap
    /// number.
    #[must_use]
    pub fn byte_size_estimate(&self) -> usize {
        const APPROX_BYTES_PER_NODE: usize = 64;
        self.entries
            .iter()
            .map(|entry| entry.tree.root_node().descendant_count() * APPROX_BYTES_PER_NODE)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MarkdownParser;

    #[test]
    fn insert_get_drop_round_trip() {
        let mut cache = BufferTreeCache::new();
        assert!(cache.is_empty());
        assert!(cache.get(42).is_none());
        let mut parser = MarkdownParser::new().expect("grammar loads");
        let tree = parser.parse("# heading\n", None).expect("parse ok");
        cache.insert(42, 3, "# heading\n".len(), tree);
        assert_eq!(cache.len(), 1);
        assert!(cache.get(42).is_some());
        cache.drop_buffer(42);
        assert!(cache.is_empty());
    }
}
