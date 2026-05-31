//! Sharded cache for row-count walker text-run measurements.
//!
//! The cache stores measured width advances for display fragments under a
//! `(font_state, locale, fragment, style)` key. Production callers use it
//! from `DirectWriteWidthMeasure` so repeated row-count walks over unchanged
//! complex lines do not recreate DirectWrite layouts for the same fragments.
//!
//! **Thread ownership.** Shared by the projection worker thread and the UI
//! thread's inline fallback through `Arc<RunCache>`. Each shard has its own
//! `RwLock`; no single global lock covers all walker measurements.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::RwLock;

use ahash::{AHashMap, AHasher};

use crate::cache::{line_content_stamp, FontStateId};

/// Total entry target for the row-count run cache.
pub const RUN_CACHE_CAPACITY: usize = 16_384;

const RUN_CACHE_SHARDS: usize = 16;

/// Key for one cached measured fragment.
///
/// The identity is `(font_state, locale, fragment, style)` — deliberately
/// **not** any per-source-line stamp. A fragment's advance is measured in
/// isolation (`DirectWriteWidthMeasure` builds a text layout for just that
/// fragment), so it depends only on its text, font, locale, and style; the
/// source line it happens to sit on is irrelevant. A per-line
/// `content_stamp` was previously part of this key — a cache-key bug that
/// made the same grapheme a distinct entry on every line, so a cold
/// whole-document row-count walk re-measured every grapheme on every line
/// (~468 k `CreateTextLayout` calls, ~0.3–1.0 s, the first-Ctrl+End lag on
/// a large buffer). Keying only on the fragment lets identical fragments
/// share one entry across all lines and across walks. Do **not** re-add a
/// per-line discriminator here.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RunCacheKey {
    /// Font configuration, including DPI scale.
    pub font_state: FontStateId,
    /// Hash of the DirectWrite locale.
    pub locale_hash: u64,
    /// Hash of the measured fragment text.
    pub fragment_stamp: u64,
    /// Hash of the style applied while measuring the fragment.
    pub style_hash: u64,
}

impl RunCacheKey {
    /// Build a key from the fragment text plus font/locale/style attributes.
    #[must_use]
    pub fn new(font_state: FontStateId, locale: &str, fragment: &str, style_hash: u64) -> Self {
        Self {
            font_state,
            locale_hash: compute_hash(locale),
            fragment_stamp: line_content_stamp(fragment),
            style_hash,
        }
    }
}

/// Result of a run-cache lookup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunCacheLookup {
    /// Measured width in DIPs.
    pub width_dip: f32,
    /// `true` when the value came from an existing cache entry.
    pub was_hit: bool,
}

/// LRU-bounded run measurement cache.
pub struct RunCache {
    shards: Box<[RwLock<RunCacheShard>]>,
    byte_size_estimate: AtomicUsize,
}

struct RunCacheShard {
    capacity: usize,
    counter: u64,
    entries: AHashMap<RunCacheKey, RunCacheEntry>,
}

#[derive(Clone, Copy)]
struct RunCacheEntry {
    width_bits: u32,
    text_bytes: usize,
    last_used: u64,
}

impl Default for RunCache {
    fn default() -> Self {
        Self::new(RUN_CACHE_CAPACITY)
    }
}

impl RunCache {
    /// Create a sharded LRU cache with the requested total capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let total = capacity.max(1);
        let per_shard = total.div_ceil(RUN_CACHE_SHARDS).max(1);
        let mut shards = Vec::with_capacity(RUN_CACHE_SHARDS);
        for _ in 0..RUN_CACHE_SHARDS {
            shards.push(RwLock::new(RunCacheShard::new(per_shard)));
        }
        Self {
            shards: shards.into_boxed_slice(),
            byte_size_estimate: AtomicUsize::new(0),
        }
    }

    /// Look up `key`, or insert the width returned by `measure`.
    pub fn get_or_insert_with(
        &self,
        key: RunCacheKey,
        text_bytes: usize,
        measure: impl FnOnce() -> f32,
    ) -> RunCacheLookup {
        let shard_idx = shard_index(&key, self.shards.len());
        let Some(shard_lock) = self.shards.get(shard_idx) else {
            return RunCacheLookup {
                width_dip: measure(),
                was_hit: false,
            };
        };
        if let Ok(mut shard) = shard_lock.write() {
            if let Some(width_dip) = shard.get(&key) {
                return RunCacheLookup {
                    width_dip,
                    was_hit: true,
                };
            }
        }
        let width_dip = measure();
        if let Ok(mut shard) = shard_lock.write() {
            if let Some(width_dip) = shard.get(&key) {
                return RunCacheLookup {
                    width_dip,
                    was_hit: true,
                };
            }
            let (removed_bytes, inserted_bytes) = shard.insert_width(key, text_bytes, width_dip);
            apply_byte_delta(&self.byte_size_estimate, removed_bytes, inserted_bytes);
        }
        RunCacheLookup {
            width_dip,
            was_hit: false,
        }
    }

    /// Number of cached entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shards
            .iter()
            .filter_map(|shard| shard.read().ok().map(|guard| guard.entries.len()))
            .sum()
    }

    /// `true` if the cache holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shards.iter().all(|shard| {
            shard
                .read()
                .ok()
                .is_none_or(|guard| guard.entries.is_empty())
        })
    }

    /// Estimated resident bytes for trace memory attribution.
    #[must_use]
    pub fn byte_size_estimate(&self) -> usize {
        self.byte_size_estimate.load(Ordering::Relaxed)
    }
}

impl RunCacheShard {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            counter: 0,
            entries: AHashMap::with_capacity(capacity),
        }
    }

    fn get(&mut self, key: &RunCacheKey) -> Option<f32> {
        self.counter = self.counter.wrapping_add(1);
        let now = self.counter;
        let entry = self.entries.get_mut(key)?;
        entry.last_used = now;
        Some(f32::from_bits(entry.width_bits))
    }

    fn insert_width(
        &mut self,
        key: RunCacheKey,
        text_bytes: usize,
        width_dip: f32,
    ) -> (usize, usize) {
        self.counter = self.counter.wrapping_add(1);
        let now = self.counter;
        let mut removed_bytes: usize = 0;
        if self.entries.len() >= self.capacity && !self.entries.contains_key(&key) {
            removed_bytes = removed_bytes.saturating_add(self.evict_oldest());
        }
        let entry = RunCacheEntry {
            width_bits: width_dip.to_bits(),
            text_bytes,
            last_used: now,
        };
        let inserted_bytes = estimate_run_entry_bytes(&entry);
        if let Some(previous) = self.entries.insert(key, entry) {
            removed_bytes = removed_bytes.saturating_add(estimate_run_entry_bytes(&previous));
        }
        (removed_bytes, inserted_bytes)
    }

    fn evict_oldest(&mut self) -> usize {
        let mut oldest_key = None;
        let mut oldest_used = u64::MAX;
        for (key, entry) in &self.entries {
            if entry.last_used < oldest_used {
                oldest_used = entry.last_used;
                oldest_key = Some(*key);
            }
        }
        if let Some(key) = oldest_key {
            return self
                .entries
                .remove(&key)
                .map_or(0, |entry| estimate_run_entry_bytes(&entry));
        }
        0
    }
}

fn estimate_run_entry_bytes(entry: &RunCacheEntry) -> usize {
    std::mem::size_of::<RunCacheKey>()
        .saturating_add(std::mem::size_of::<RunCacheEntry>())
        .saturating_add(entry.text_bytes)
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

fn shard_index(key: &RunCacheKey, shard_count: usize) -> usize {
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

    fn key(text: &str) -> RunCacheKey {
        RunCacheKey::new(
            FontStateId::from_parts("Cascadia Mono", 14.0, "en-us", 1.0),
            "en-us",
            text,
            42,
        )
    }

    #[test]
    fn second_lookup_hits() {
        let cache = RunCache::new(8);
        let first = cache.get_or_insert_with(key("alpha"), 5, || 40.0);
        let second = cache.get_or_insert_with(key("alpha"), 5, || 10.0);
        assert!(!first.was_hit);
        assert!(second.was_hit);
        assert_eq!(second.width_dip, 40.0);
    }

    #[test]
    fn lru_bound_caps_entries_per_shard() {
        let cache = RunCache::new(1);
        for i in 0..64 {
            let text = format!("line {i}");
            let _ = cache.get_or_insert_with(key(&text), text.len(), || i as f32);
        }
        assert!(cache.len() <= RUN_CACHE_SHARDS);
    }

    #[test]
    fn identical_fragment_is_measured_once_not_per_line() {
        // Regression guard for the cold-walk perf bug. The key must depend
        // only on (fragment, font, locale, style) and NOT on any per-line
        // identity, so a grapheme like "e" that appears on thousands of
        // source lines is measured exactly once. The previous per-line
        // `content_stamp` in the key forced a re-measure per line — ~468 k
        // `CreateTextLayout` calls on a cold whole-document walk, the
        // first-Ctrl+End lag on a large buffer.
        let cache = RunCache::new(128);
        let mut measure_calls = 0_u32;
        for _line in 0..1000 {
            let lookup = cache.get_or_insert_with(key("e"), 1, || {
                measure_calls += 1;
                7.0
            });
            assert_eq!(lookup.width_dip, 7.0);
        }
        assert_eq!(
            measure_calls, 1,
            "fragment must be measured once across all lines, not per line"
        );
    }
}
