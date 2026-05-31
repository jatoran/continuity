//! Per-(font_state, wrap_width) bucketed cache for row-count walker
//! soft-wrap results.
//!
//! Stores the row count and break metadata produced by the slow wrap walk
//! for a projected source line. The full display map remains derived from
//! the rope and decorations; this cache only memoizes measurement-derived
//! wrap output.
//!
//! Cache layout: a small bounded set of `(font_state, wrap_width_dip)`
//! buckets. Each bucket owns its own sharded LRU at the configured
//! per-bucket capacity. Entries from one bucket never evict entries from
//! another. When the active bucket set grows past
//! [`WRAP_CACHE_MAX_BUCKETS`] the least-recently-used bucket is dropped
//! wholesale.
//!
//! **Thread ownership.** Shared by the projection worker thread and the UI
//! thread's inline fallback through `Arc<WrapCache>`. The bucket registry
//! sits behind a short-lived `Mutex`; per-bucket sharded `RwLock`s avoid
//! one process-wide lock on row-count walks.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::{Mutex, RwLock};

use ahash::{AHashMap, AHasher};

/// Total entry target for the row-count wrap cache, applied per
/// `(font_state, wrap_width_dip)` bucket.
pub const WRAP_CACHE_CAPACITY: usize = 16_384;

/// Maximum number of distinct `(font_state, wrap_width_dip)` buckets
/// retained simultaneously. Past this point the least-recently-used
/// bucket is evicted wholesale.
pub const WRAP_CACHE_MAX_BUCKETS: usize = 4;

const WRAP_CACHE_SHARDS: usize = 16;

/// Key for one cached soft-wrap result.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WrapCacheKey {
    /// Projection content stamp for the source line being walked.
    pub content_stamp: u64,
    /// `FontStateId` bits from the caller. `display_map` stores the raw
    /// value to preserve the crate import boundary.
    pub font_state: u64,
    /// Hash of the DirectWrite locale.
    pub locale_hash: u64,
    /// Soft-wrap width in DIPs.
    pub wrap_width_dip: u32,
}

impl WrapCacheKey {
    /// Build a key from the stable line stamp plus text layout inputs.
    #[must_use]
    pub fn new(content_stamp: u64, font_state: u64, locale: &str, wrap_width_dip: u32) -> Self {
        Self {
            content_stamp,
            font_state,
            locale_hash: compute_hash(locale),
            wrap_width_dip,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct BucketKey {
    font_state: u64,
    wrap_width_dip: u32,
}

impl BucketKey {
    fn from_wrap_key(key: &WrapCacheKey) -> Self {
        Self {
            font_state: key.font_state,
            wrap_width_dip: key.wrap_width_dip,
        }
    }
}

/// Cached wrap output.
///
/// Row-count walker entries from the slow path (P18.12a) carry a
/// width-independent line-wrap profile: one entry in each of
/// `break_points`, `prefix_advances_bits`, and
/// `pre_whitespace_advances_bits` per soft-wrap break candidate
/// (whitespace boundaries) plus, when the line does not end with
/// whitespace, a sentinel entry at the line's end. The three arrays
/// are always the same length. Legacy callers may still construct a
/// row-count-only shape via [`WrapCacheEntry::row_count_only`] with
/// empty arrays.
///
/// Cache value contract (set by `crates/display_map/src/builder/row_counts.rs`
/// slow path and consumed by `crates/display_map/src/wrap_profile.rs`):
///
/// - `break_points[i]` — display-byte offset *just after* the
///   trailing whitespace of break candidate `i`. For an end-of-line
///   sentinel this is the total display-byte length of the line.
/// - `prefix_advances_bits[i]` — `f32::to_bits` of the cumulative
///   display width from line start to `break_points[i]`, **including**
///   the trailing whitespace's width.
/// - `pre_whitespace_advances_bits[i]` — `f32::to_bits` of the
///   cumulative display width from line start to the byte *just
///   before* the trailing whitespace of break candidate `i`, i.e.
///   **excluding** the trailing whitespace. For an end-of-line
///   sentinel this equals `prefix_advances_bits[i]` (no trailing
///   whitespace to subtract).
///
/// The two advance arrays exist as a pair so
/// `wrap_profile::row_count_from_profile` can disambiguate the slow
/// path's "overshoot" cut (trigger fires at trailing whitespace,
/// producing a row of width slightly greater than `wrap_width_dip`)
/// from the non-overshoot cut (trigger fires before the trailing
/// whitespace at some non-whitespace grapheme).
#[derive(Clone, Debug)]
pub struct WrapCacheEntry {
    /// Number of display rows emitted by the wrap pass.
    pub row_count: u16,
    /// Display-byte offsets where wrap continuations begin.
    pub break_points: Arc<[u32]>,
    /// Cumulative display width at each `break_points[i]`, *including*
    /// the trailing whitespace's width. Stored as raw `f32::to_bits()`
    /// so the entry is `Eq`-free and compact.
    pub prefix_advances_bits: Arc<[u32]>,
    /// Cumulative display width up to but *excluding* the trailing
    /// whitespace at each `break_points[i]`. For an end-of-line
    /// sentinel this equals the matching `prefix_advances_bits[i]`.
    /// Stored as raw `f32::to_bits()`.
    pub pre_whitespace_advances_bits: Arc<[u32]>,
}

impl WrapCacheEntry {
    /// Build the compact row-count-only shape used by the walker.
    #[must_use]
    pub fn row_count_only(row_count: u16) -> Self {
        Self {
            row_count,
            break_points: Arc::from([]),
            prefix_advances_bits: Arc::from([]),
            pre_whitespace_advances_bits: Arc::from([]),
        }
    }
}

/// LRU-bounded wrap cache, partitioned per `(font_state, wrap_width_dip)`
/// bucket.
pub struct WrapCache {
    buckets: Mutex<BucketRegistry>,
    per_bucket_capacity: usize,
    byte_size_estimate: AtomicUsize,
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
    shards: Box<[RwLock<WrapCacheShard>]>,
    bytes: AtomicUsize,
}

struct WrapCacheShard {
    capacity: usize,
    counter: u64,
    entries: AHashMap<WrapCacheKey, WrapCacheValue>,
}

struct WrapCacheValue {
    entry: WrapCacheEntry,
    last_used: u64,
}

impl Default for WrapCache {
    fn default() -> Self {
        Self::new(WRAP_CACHE_CAPACITY)
    }
}

impl WrapCache {
    /// Create a per-bucket LRU cache. Each `(font_state, wrap_width_dip)`
    /// bucket is sized to `capacity` total entries (split across shards
    /// for lock concurrency). At most [`WRAP_CACHE_MAX_BUCKETS`] buckets
    /// are retained simultaneously.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            buckets: Mutex::new(BucketRegistry {
                counter: 0,
                entries: Vec::with_capacity(WRAP_CACHE_MAX_BUCKETS),
            }),
            per_bucket_capacity: capacity.max(1),
            byte_size_estimate: AtomicUsize::new(0),
        }
    }

    /// Look up an entry and update its LRU timestamp.
    #[must_use]
    pub fn get(&self, key: &WrapCacheKey) -> Option<WrapCacheEntry> {
        let bucket_key = BucketKey::from_wrap_key(key);
        let store = self.touch_bucket(&bucket_key)?;
        store.get(key)
    }

    /// P18.12b — look up a populated profile entry for
    /// `(content_stamp, font_state, locale)` across every active
    /// `wrap_width_dip` bucket. Returns the most-recently-used
    /// matching entry whose `break_points` are non-empty (i.e., one
    /// that carries a real width-independent profile, not a legacy
    /// row-count-only shell). Entries from the row-count walker's
    /// slow path (post-P18.12a) always populate the profile fields,
    /// so this routinely succeeds when any sibling-wrap_width bucket
    /// holds the same line.
    ///
    /// On a hit, the donor bucket's LRU timestamp is refreshed so
    /// repeated drag-tick queries do not unfairly age out the bucket
    /// the profile lives in.
    #[must_use]
    pub fn get_any_width(
        &self,
        content_stamp: u64,
        font_state: u64,
        locale: &str,
    ) -> Option<WrapCacheEntry> {
        let locale_hash = compute_hash(locale);
        let candidates: Vec<(u32, Arc<BucketStore>)> = {
            let registry = self.buckets.lock().ok()?;
            let mut matches: Vec<(u32, u64, Arc<BucketStore>)> = registry
                .entries
                .iter()
                .filter(|b| b.key.font_state == font_state)
                .map(|b| (b.key.wrap_width_dip, b.last_used, Arc::clone(&b.store)))
                .collect();
            matches.sort_by(|a, b| b.1.cmp(&a.1));
            matches.into_iter().map(|(w, _, s)| (w, s)).collect()
        };
        for (width, store) in candidates {
            let lookup_key = WrapCacheKey {
                content_stamp,
                font_state,
                locale_hash,
                wrap_width_dip: width,
            };
            if let Some(entry) = store.get(&lookup_key) {
                if !entry.break_points.is_empty() {
                    let _ = self.touch_bucket(&BucketKey {
                        font_state,
                        wrap_width_dip: width,
                    });
                    return Some(entry);
                }
            }
        }
        None
    }

    /// Insert a freshly-computed entry.
    pub fn insert(&self, key: WrapCacheKey, entry: WrapCacheEntry) {
        let bucket_key = BucketKey::from_wrap_key(&key);
        let store = self.touch_or_create_bucket(bucket_key);
        let (removed_bytes, inserted_bytes) = store.insert(key, entry);
        apply_byte_delta(&self.byte_size_estimate, removed_bytes, inserted_bytes);
    }

    /// Insert the compact row-count-only entry shape.
    pub fn insert_row_count_only(&self, key: WrapCacheKey, row_count: u16) {
        self.insert(key, WrapCacheEntry::row_count_only(row_count));
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

    /// Number of active `(font_state, wrap_width_dip)` buckets.
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
        let Ok(mut registry) = self.buckets.lock() else {
            return Arc::new(BucketStore::new(
                self.per_bucket_capacity,
                WRAP_CACHE_SHARDS,
            ));
        };
        if let Some(pos) = registry.entries.iter().position(|b| b.key == key) {
            registry.counter = registry.counter.wrapping_add(1);
            let now = registry.counter;
            registry.entries[pos].last_used = now;
            return Arc::clone(&registry.entries[pos].store);
        }
        if registry.entries.len() >= WRAP_CACHE_MAX_BUCKETS {
            self.evict_oldest_bucket(&mut registry);
        }
        registry.counter = registry.counter.wrapping_add(1);
        let now = registry.counter;
        let store = Arc::new(BucketStore::new(
            self.per_bucket_capacity,
            WRAP_CACHE_SHARDS,
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
        drop(removed.store);
        let _ =
            self.byte_size_estimate
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    Some(current.saturating_sub(bucket_bytes))
                });
    }
}

impl BucketStore {
    fn new(per_bucket_capacity: usize, shard_count: usize) -> Self {
        let per_shard = per_bucket_capacity.div_ceil(shard_count.max(1)).max(1);
        let mut shards = Vec::with_capacity(shard_count);
        for _ in 0..shard_count {
            shards.push(RwLock::new(WrapCacheShard::new(per_shard)));
        }
        Self {
            shards: shards.into_boxed_slice(),
            bytes: AtomicUsize::new(0),
        }
    }

    fn get(&self, key: &WrapCacheKey) -> Option<WrapCacheEntry> {
        let shard_idx = shard_index(key, self.shards.len());
        let shard_lock = self.shards.get(shard_idx)?;
        let mut shard = shard_lock.write().ok()?;
        shard.get(key)
    }

    fn insert(&self, key: WrapCacheKey, entry: WrapCacheEntry) -> (usize, usize) {
        let shard_idx = shard_index(&key, self.shards.len());
        let Some(shard_lock) = self.shards.get(shard_idx) else {
            return (0, 0);
        };
        let Ok(mut shard) = shard_lock.write() else {
            return (0, 0);
        };
        let (removed_bytes, inserted_bytes) = shard.insert(key, entry);
        apply_byte_delta(&self.bytes, removed_bytes, inserted_bytes);
        (removed_bytes, inserted_bytes)
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

impl WrapCacheShard {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            counter: 0,
            entries: AHashMap::with_capacity(capacity),
        }
    }

    fn get(&mut self, key: &WrapCacheKey) -> Option<WrapCacheEntry> {
        self.counter = self.counter.wrapping_add(1);
        let now = self.counter;
        let entry = self.entries.get_mut(key)?;
        entry.last_used = now;
        Some(entry.entry.clone())
    }

    fn insert(&mut self, key: WrapCacheKey, entry: WrapCacheEntry) -> (usize, usize) {
        self.counter = self.counter.wrapping_add(1);
        let now = self.counter;
        let mut removed_bytes: usize = 0;
        if self.entries.len() >= self.capacity && !self.entries.contains_key(&key) {
            removed_bytes = removed_bytes.saturating_add(self.evict_oldest());
        }
        let value = WrapCacheValue {
            entry,
            last_used: now,
        };
        let inserted_bytes = estimate_wrap_value_bytes(&value);
        if let Some(previous) = self.entries.insert(key, value) {
            removed_bytes = removed_bytes.saturating_add(estimate_wrap_value_bytes(&previous));
        }
        (removed_bytes, inserted_bytes)
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
                .map_or(0, |value| estimate_wrap_value_bytes(&value));
        }
        0
    }
}

fn estimate_wrap_value_bytes(value: &WrapCacheValue) -> usize {
    std::mem::size_of::<WrapCacheKey>()
        .saturating_add(std::mem::size_of::<WrapCacheValue>())
        .saturating_add(value.entry.break_points.len() * std::mem::size_of::<u32>())
        .saturating_add(value.entry.prefix_advances_bits.len() * std::mem::size_of::<u32>())
        .saturating_add(value.entry.pre_whitespace_advances_bits.len() * std::mem::size_of::<u32>())
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

fn compute_hash(value: &str) -> u64 {
    let mut hasher = AHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

fn shard_index(key: &WrapCacheKey, shard_count: usize) -> usize {
    if shard_count == 0 {
        return 0;
    }
    let mut hasher = AHasher::default();
    key.hash(&mut hasher);
    (hasher.finish() as usize) % shard_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lookup_hits() {
        let cache = WrapCache::new(8);
        let key = WrapCacheKey::new(10, 20, "en-us", 80);
        assert!(cache.get(&key).is_none());
        cache.insert(
            key,
            WrapCacheEntry {
                row_count: 3,
                break_points: Arc::from(vec![4_u32, 8_u32]),
                prefix_advances_bits: Arc::from(vec![1.0_f32.to_bits(), 2.0_f32.to_bits()]),
                pre_whitespace_advances_bits: Arc::from(vec![0.5_f32.to_bits(), 1.5_f32.to_bits()]),
            },
        );
        assert_eq!(cache.get(&key).map(|entry| entry.row_count), Some(3));
    }

    #[test]
    fn second_width_does_not_evict_first() {
        let cache = WrapCache::new(8);
        let key_a = WrapCacheKey::new(10, 20, "en-us", 80);
        let key_b = WrapCacheKey::new(10, 20, "en-us", 160);
        cache.insert(key_a, WrapCacheEntry::row_count_only(1));
        for content_stamp in 0..32 {
            let extra = WrapCacheKey::new(content_stamp + 100, 20, "en-us", 160);
            cache.insert(extra, WrapCacheEntry::row_count_only(1));
        }
        cache.insert(key_b, WrapCacheEntry::row_count_only(2));
        assert!(
            cache.get(&key_a).is_some(),
            "the wrap=80 bucket should retain its entries when wrap=160 fills"
        );
    }

    #[test]
    fn bucket_count_caps_at_max() {
        let cache = WrapCache::new(4);
        for wrap_width_dip in 0..(WRAP_CACHE_MAX_BUCKETS as u32 + 4) {
            let key = WrapCacheKey::new(0, 0, "en-us", wrap_width_dip * 10);
            cache.insert(key, WrapCacheEntry::row_count_only(1));
        }
        assert_eq!(cache.bucket_count(), WRAP_CACHE_MAX_BUCKETS);
    }

    #[test]
    fn distinct_font_states_use_distinct_buckets() {
        let cache = WrapCache::new(4);
        cache.insert(
            WrapCacheKey::new(1, 100, "en-us", 80),
            WrapCacheEntry::row_count_only(1),
        );
        cache.insert(
            WrapCacheKey::new(1, 200, "en-us", 80),
            WrapCacheEntry::row_count_only(1),
        );
        assert_eq!(cache.bucket_count(), 2);
    }
}
