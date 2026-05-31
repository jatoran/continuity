//! Width-independent (shaping-layer) cache for row-count walker segment
//! lists.
//!
//! The row-count walker needs the same projected segment list as full
//! materialization before it can decide whether a wrapped source line fits
//! or needs the slow wrap walk. This cache stores those segment lists for
//! unchanged line projection inputs and returns source-byte-shifted clones
//! when the same content appears at a different absolute byte offset.
//!
//! Segments are projected from decorations, folds, caret, and source text
//! only — none of those inputs depend on `wrap_width_dip`. The cache key
//! is therefore `(content_stamp, font_state)`, and the cache survives
//! soft-wrap-width changes intact. Wrap geometry (row counts, break
//! points) lives in [`crate::wrap_cache::WrapCache`], which keys on the
//! same content stamp plus `wrap_width_dip`.
//!
//! Cache layout: a small bounded set of per-`font_state` buckets. Each
//! bucket owns its own sharded LRU at the configured per-bucket capacity.
//! Entries from one bucket never evict entries from another — switching
//! font configurations (DPI scale flip, font face change) does not
//! pressure the LRU of the prior bucket. When the active bucket set
//! grows past [`SEGMENT_CACHE_MAX_BUCKETS`] the least-recently-used
//! bucket is dropped wholesale.
//!
//! **Thread ownership.** Shared by the projection worker thread and the UI
//! thread's inline fallback through `Arc<SegmentCache>`. The bucket
//! registry sits behind a short-lived `Mutex`; per-bucket sharded
//! `RwLock`s avoid one process-wide lock on row-count walks.

mod relative_stamp;
mod shift;

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::{Mutex, RwLock};

use ahash::{AHashMap, AHasher};
use continuity_decorate::Decorations;

use crate::fold::FoldRange;
use crate::id::SourceByte;
use crate::segment::DisplaySegment;
use relative_stamp::{
    hash_block_relative, hash_color_relative, hash_inline_relative, hash_table_relative,
};
use shift::{estimate_segments_bytes, shift_segment};

/// Total entry target for the row-count segment cache, applied per
/// `font_state` bucket. Sized to cover a 9 k-line markdown workload's
/// complex-script slow-path entries (worst-case observed: ~6 k complex
/// lines × ~16 segments per line, with width-fanout collapsed by P18.3).
/// Pre-P18.3 the effective budget was `4 × 4_096 = 16_384` across width
/// buckets; collapsing the width axis cut that to `1 × 4_096` and the
/// trace showed eviction-driven misses between consecutive cold builds
/// (see `perf-snapshots/trace_20260520-162425.report.md`). The bumped
/// per-bucket capacity restores steady-state hit rates above 95 %.
pub const SEGMENT_CACHE_CAPACITY: usize = 16_384;

/// Maximum number of distinct `font_state` buckets retained
/// simultaneously. Past this point the least-recently-used bucket is
/// evicted wholesale.
pub const SEGMENT_CACHE_MAX_BUCKETS: usize = 4;

const SEGMENT_CACHE_SHARDS: usize = 16;

/// Key for one cached projected segment list. Width-independent: the
/// same key resolves across every soft-wrap width because segments are
/// projected from decorations / folds / caret / source text only.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SegmentCacheKey {
    /// Projection content stamp for the source line being walked.
    pub content_stamp: u64,
    /// `FontStateId` bits from the caller. `display_map` stores the raw
    /// value to preserve the crate import boundary.
    pub font_state: u64,
}

impl SegmentCacheKey {
    /// Build a key from line projection identity and font state.
    #[must_use]
    pub const fn new(content_stamp: u64, font_state: u64) -> Self {
        Self {
            content_stamp,
            font_state,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct BucketKey {
    font_state: u64,
}

impl BucketKey {
    fn from_segment_key(key: &SegmentCacheKey) -> Self {
        Self {
            font_state: key.font_state,
        }
    }
}

/// Compute the content stamp used by [`SegmentCacheKey`] for one
/// projected source line.
#[must_use]
pub fn compute_line_projection_stamp(
    decorations: &Decorations,
    caret_bytes: &[SourceByte],
    folds: &[FoldRange],
    suppressed_table_blocks: &[std::ops::Range<usize>],
    line_start: usize,
    line_end: usize,
    line_text: &str,
) -> u64 {
    let mut hasher = AHasher::default();
    line_text.hash(&mut hasher);
    // The line's absolute start byte is deliberately NOT hashed. The stamp
    // must be offset-independent so a byte-identical line keeps the same key
    // after an edit above shifts it down; `get_shifted` then relocates the
    // cached segments by delta instead of recomputing them. Every offset
    // below is hashed relative to `line_start` for the same reason. Hashing
    // the absolute origin here silently defeats cache reuse — it was the
    // cause of the ~0% hit rate under editing (Block 1.5,
    // `.docs/development/memory_optimization_plan.md`).
    line_end.saturating_sub(line_start).hash(&mut hasher);
    // P18.12e (2026-05-22) — filter most caret input to carets
    // falling inside this line, matching the fold/decoration filters
    // below. Pre-fix
    // every caret position in the buffer contributed to every line's
    // stamp, so a click in pane A invalidated every line of buffer X
    // for pane B's walker (or every line of buffer X for pane A's own
    // next walker, when the caret moved between two consecutive
    // walks). The intended hash semantics: a line's stamp depends on
    // its own content + its own overlapping decorations/folds/carets;
    // edits/clicks elsewhere in the buffer must not invalidate it.
    //
    // Pipe tables are the exception below: table pipe hiding is gated
    // by whether any caret falls inside the whole table block, not
    // just this source line. Hash only that boolean reveal state for
    // tables intersecting the line so entering/leaving a table misses
    // cached row counts without keying on exact off-line caret bytes.
    // See `trace_20260522-190952.report.md` — `wrap_profile_hits=0` /
    // `segment_cache_hits` collapsing despite an unchanged buffer
    // across drag ticks.
    for caret in caret_bytes {
        let pos = caret.as_usize();
        if pos >= line_start && pos <= line_end {
            pos.saturating_sub(line_start).hash(&mut hasher);
        }
    }
    for fold in folds {
        let start = fold.start.as_usize();
        let end = fold.end.as_usize();
        if end > line_start && start < line_end {
            start.saturating_sub(line_start).hash(&mut hasher);
            end.saturating_sub(line_start).hash(&mut hasher);
        }
    }
    for block in &decorations.blocks {
        if block.end_byte > line_start && block.start_byte < line_end {
            hash_block_relative(block, line_start, &mut hasher);
        }
    }
    for inline in &decorations.inlines {
        if inline.range.end > line_start && inline.range.start < line_end {
            hash_inline_relative(inline, line_start, &mut hasher);
        }
    }
    for color in &decorations.inline_color_spans {
        if color.outer.end > line_start && color.outer.start < line_end {
            hash_color_relative(color, line_start, &mut hasher);
        }
    }
    for table in &decorations.evaluated_tables {
        if table.block_range.end > line_start && table.block_range.start < line_end {
            hash_table_relative(table, line_start, &mut hasher);
            // Whether this table is in the active suppression set
            // is the per-line cache discriminator: the hide pass
            // skips suppressed tables, which changes the projected
            // segment list. Was caret-in-block (pre-Phase-A reveal
            // toggle); now the selection-coverage signal feeding
            // the same cache-invalidation slot.
            let is_suppressed = suppressed_table_blocks
                .iter()
                .any(|r| r.start == table.block_range.start && r.end == table.block_range.end);
            is_suppressed.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Hit / miss / eviction counters for [`SegmentCache`].
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct SegmentCacheCounters {
    /// Lookups that returned a shifted segment list.
    pub hits: u64,
    /// Lookups that found no entry for the supplied key.
    pub misses: u64,
    /// Entries removed by the bounded-LRU policy (per-shard or bucket
    /// wholesale).
    pub evictions: u64,
}

/// LRU-bounded segment-list cache, partitioned per `font_state` bucket.
pub struct SegmentCache {
    buckets: Mutex<BucketRegistry>,
    per_bucket_capacity: usize,
    byte_size_estimate: AtomicUsize,
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
    evictions: std::sync::atomic::AtomicU64,
}

struct BucketRegistry {
    counter: u64,
    entries: Vec<BucketEntry>,
}

struct BucketEntry {
    key: BucketKey,
    last_used: u64,
    store: Arc<BucketStore>,
}

struct BucketStore {
    shards: Box<[RwLock<SegmentCacheShard>]>,
    bytes: AtomicUsize,
}

struct SegmentCacheShard {
    capacity: usize,
    counter: u64,
    entries: AHashMap<SegmentCacheKey, SegmentCacheValue>,
}

struct SegmentCacheValue {
    source_start: u32,
    segments: Arc<[DisplaySegment]>,
    byte_size: usize,
    last_used: u64,
}

impl Default for SegmentCache {
    fn default() -> Self {
        Self::new(SEGMENT_CACHE_CAPACITY)
    }
}

impl SegmentCache {
    /// Create a per-bucket LRU cache. Each `font_state` bucket is sized
    /// to `capacity` total entries (split across shards for lock
    /// concurrency). At most [`SEGMENT_CACHE_MAX_BUCKETS`] buckets are
    /// retained simultaneously.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            buckets: Mutex::new(BucketRegistry {
                counter: 0,
                entries: Vec::with_capacity(SEGMENT_CACHE_MAX_BUCKETS),
            }),
            per_bucket_capacity: capacity.max(1),
            byte_size_estimate: AtomicUsize::new(0),
            hits: std::sync::atomic::AtomicU64::new(0),
            misses: std::sync::atomic::AtomicU64::new(0),
            evictions: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Snapshot the cache's hit / miss / eviction counters.
    #[must_use]
    pub fn counters(&self) -> SegmentCacheCounters {
        SegmentCacheCounters {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }

    /// Look up a segment list and shift absolute source bytes to `line_start`.
    #[must_use]
    pub fn get_shifted(
        &self,
        key: &SegmentCacheKey,
        line_start: usize,
    ) -> Option<Vec<DisplaySegment>> {
        let bucket_key = BucketKey::from_segment_key(key);
        let result = self
            .touch_bucket(&bucket_key)
            .and_then(|store| store.get_shifted(key, line_start));
        // Wiring verified 2026-05-28: hit/miss bumps are exactly
        // symmetric (one per call, on the side matching the return
        // value); traces showing `segment_cache_hits=0` with non-zero
        // entry count reflect content-stamp churn (see
        // `compute_line_projection_stamp`), not lost counter wiring.
        // Regression-tested by
        // `basic_tests::counters_increment_on_hit_and_miss`.
        if result.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// Insert a freshly-built segment list.
    pub fn insert(&self, key: SegmentCacheKey, line_start: usize, segments: &[DisplaySegment]) {
        let bucket_key = BucketKey::from_segment_key(&key);
        let store = self.touch_or_create_bucket(bucket_key);
        let (removed_bytes, inserted_bytes, evicted) = store.insert(key, line_start, segments);
        apply_byte_delta(&self.byte_size_estimate, removed_bytes, inserted_bytes);
        if evicted > 0 {
            self.evictions.fetch_add(evicted, Ordering::Relaxed);
        }
    }

    /// Number of cached entries across every bucket.
    #[must_use]
    pub fn len(&self) -> usize {
        match self.buckets.lock() {
            Ok(registry) => registry.entries.iter().map(|b| b.store.len()).sum(),
            Err(_) => 0,
        }
    }

    /// `true` if every bucket is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self.buckets.lock() {
            Ok(registry) => registry.entries.iter().all(|b| b.store.is_empty()),
            Err(_) => true,
        }
    }

    /// Estimated resident bytes for trace memory attribution.
    #[must_use]
    pub fn byte_size_estimate(&self) -> usize {
        self.byte_size_estimate.load(Ordering::Relaxed)
    }

    /// Number of active `font_state` buckets.
    #[must_use]
    pub fn bucket_count(&self) -> usize {
        self.buckets.lock().map(|r| r.entries.len()).unwrap_or(0)
    }

    fn touch_bucket(&self, key: &BucketKey) -> Option<Arc<BucketStore>> {
        let mut registry = self.buckets.lock().ok()?;
        let pos = registry.entries.iter().position(|b| &b.key == key)?;
        registry.counter = registry.counter.wrapping_add(1);
        let now = registry.counter;
        registry.entries[pos].last_used = now;
        Some(Arc::clone(&registry.entries[pos].store))
    }

    fn touch_or_create_bucket(&self, key: BucketKey) -> Arc<BucketStore> {
        // Mutex is poisoned only after a panic on another thread; in that
        // case fall back to a detached store so the caller still
        // makes progress with no cross-thread cache sharing.
        let Ok(mut registry) = self.buckets.lock() else {
            return Arc::new(BucketStore::new(
                self.per_bucket_capacity,
                SEGMENT_CACHE_SHARDS,
            ));
        };
        if let Some(pos) = registry.entries.iter().position(|b| b.key == key) {
            registry.counter = registry.counter.wrapping_add(1);
            let now = registry.counter;
            registry.entries[pos].last_used = now;
            return Arc::clone(&registry.entries[pos].store);
        }
        if registry.entries.len() >= SEGMENT_CACHE_MAX_BUCKETS {
            self.evict_oldest_bucket(&mut registry);
        }
        registry.counter = registry.counter.wrapping_add(1);
        let now = registry.counter;
        let store = Arc::new(BucketStore::new(
            self.per_bucket_capacity,
            SEGMENT_CACHE_SHARDS,
        ));
        registry.entries.push(BucketEntry {
            key,
            last_used: now,
            store: Arc::clone(&store),
        });
        store
    }

    fn evict_oldest_bucket(&self, registry: &mut BucketRegistry) {
        let Some((oldest_pos, _)) = registry
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, b)| b.last_used)
        else {
            return;
        };
        let removed = registry.entries.swap_remove(oldest_pos);
        let bucket_bytes = removed.store.bytes.load(Ordering::Relaxed);
        let bucket_entries = removed.store.len();
        // Drop the Arc; once the projection-worker thread releases its
        // outstanding handle, the store memory is freed.
        drop(removed.store);
        let _ =
            self.byte_size_estimate
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    Some(current.saturating_sub(bucket_bytes))
                });
        if bucket_entries > 0 {
            self.evictions
                .fetch_add(bucket_entries as u64, Ordering::Relaxed);
        }
    }
}

impl BucketStore {
    fn new(per_bucket_capacity: usize, shard_count: usize) -> Self {
        let per_shard = per_bucket_capacity.div_ceil(shard_count.max(1)).max(1);
        let mut shards = Vec::with_capacity(shard_count);
        for _ in 0..shard_count {
            shards.push(RwLock::new(SegmentCacheShard::new(per_shard)));
        }
        Self {
            shards: shards.into_boxed_slice(),
            bytes: AtomicUsize::new(0),
        }
    }

    fn get_shifted(&self, key: &SegmentCacheKey, line_start: usize) -> Option<Vec<DisplaySegment>> {
        let shard_idx = shard_index(key, self.shards.len());
        let shard_lock = self.shards.get(shard_idx)?;
        let mut shard = shard_lock.write().ok()?;
        shard.get_shifted(key, line_start)
    }

    fn insert(
        &self,
        key: SegmentCacheKey,
        line_start: usize,
        segments: &[DisplaySegment],
    ) -> (usize, usize, u64) {
        let shard_idx = shard_index(&key, self.shards.len());
        let Some(shard_lock) = self.shards.get(shard_idx) else {
            return (0, 0, 0);
        };
        let Ok(mut shard) = shard_lock.write() else {
            return (0, 0, 0);
        };
        let (removed_bytes, inserted_bytes, evicted) = shard.insert(key, line_start, segments);
        apply_byte_delta(&self.bytes, removed_bytes, inserted_bytes);
        (removed_bytes, inserted_bytes, evicted)
    }

    fn len(&self) -> usize {
        self.shards
            .iter()
            .filter_map(|shard| shard.read().ok().map(|guard| guard.entries.len()))
            .sum()
    }

    fn is_empty(&self) -> bool {
        self.shards.iter().all(|shard| {
            shard
                .read()
                .ok()
                .is_none_or(|guard| guard.entries.is_empty())
        })
    }
}

impl SegmentCacheShard {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            counter: 0,
            entries: AHashMap::with_capacity(capacity),
        }
    }

    fn get_shifted(
        &mut self,
        key: &SegmentCacheKey,
        line_start: usize,
    ) -> Option<Vec<DisplaySegment>> {
        self.counter = self.counter.wrapping_add(1);
        let now = self.counter;
        let value = self.entries.get_mut(key)?;
        value.last_used = now;
        let delta = line_start as i64 - i64::from(value.source_start);
        Some(
            value
                .segments
                .iter()
                .map(|segment| shift_segment(segment, delta))
                .collect(),
        )
    }

    fn insert(
        &mut self,
        key: SegmentCacheKey,
        line_start: usize,
        segments: &[DisplaySegment],
    ) -> (usize, usize, u64) {
        self.counter = self.counter.wrapping_add(1);
        let now = self.counter;
        let mut removed_bytes: usize = 0;
        let mut evicted: u64 = 0;
        if self.entries.len() >= self.capacity && !self.entries.contains_key(&key) {
            let bytes = self.evict_oldest();
            if bytes > 0 {
                removed_bytes = removed_bytes.saturating_add(bytes);
                evicted = evicted.saturating_add(1);
            }
        }
        let value = SegmentCacheValue {
            source_start: u32::try_from(line_start).unwrap_or(u32::MAX),
            segments: Arc::from(segments.to_vec()),
            byte_size: estimate_segments_bytes(segments),
            last_used: now,
        };
        let inserted_bytes = estimate_segment_value_bytes(&value);
        if let Some(previous) = self.entries.insert(key, value) {
            removed_bytes = removed_bytes.saturating_add(estimate_segment_value_bytes(&previous));
        }
        (removed_bytes, inserted_bytes, evicted)
    }

    fn evict_oldest(&mut self) -> usize {
        let mut oldest_key = None;
        let mut oldest_used = u64::MAX;
        for (key, value) in &self.entries {
            if value.last_used < oldest_used {
                oldest_used = value.last_used;
                oldest_key = Some(*key);
            }
        }
        if let Some(key) = oldest_key {
            return self
                .entries
                .remove(&key)
                .map_or(0, |value| estimate_segment_value_bytes(&value));
        }
        0
    }
}

fn estimate_segment_value_bytes(value: &SegmentCacheValue) -> usize {
    std::mem::size_of::<SegmentCacheKey>()
        .saturating_add(std::mem::size_of::<SegmentCacheValue>())
        .saturating_add(value.byte_size)
}

fn apply_byte_delta(counter: &AtomicUsize, removed_bytes: usize, inserted_bytes: usize) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(
            current
                .saturating_sub(removed_bytes)
                .saturating_add(inserted_bytes),
        )
    });
}

fn shard_index(key: &SegmentCacheKey, shard_count: usize) -> usize {
    if shard_count == 0 {
        return 0;
    }
    let mut hasher = AHasher::default();
    key.hash(&mut hasher);
    (hasher.finish() as usize) % shard_count
}

#[cfg(test)]
mod basic_tests;
#[cfg(test)]
mod capacity_workload_test;
#[cfg(test)]
mod reveal_stamp_tests;
#[cfg(test)]
mod stamp_offset_tests;
