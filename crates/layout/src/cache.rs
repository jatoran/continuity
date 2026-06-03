//! LRU-bounded cache of `IDWriteTextLayout` keyed per spec §5 by
//! `(document, line, content_stamp, font_state, soft_wrap_width)`.
//!
//! **Thread ownership**: the UI thread that owns the renderer. The cached
//! `IDWriteTextLayout` objects are not `Send` — never pass them across
//! threads.
//!
//! Use `content_stamp` (a hash of the line bytes) instead of a buffer-wide
//! revision number so a single keystroke only invalidates the line that
//! actually changed: identical content always hashes to the same stamp.

use std::hash::{Hash, Hasher};

use ahash::{AHashMap, AHasher};
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;

/// Conservative resident-size estimate for one cached
/// `IDWriteTextLayout`, excluding the line text mirrored in Rust.
pub const ESTIMATED_TEXT_LAYOUT_ENTRY_BYTES: usize = 4 * 1024;

/// Identifier for a (font_family, size, locale, DPI scale, …) tuple.
/// Hash-derived so callers don't need to track changes themselves — change
/// any input and the id changes, which evicts every layout built against the
/// old state.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct FontStateId(pub u64);

impl FontStateId {
    /// Build a [`FontStateId`] by hashing the parts of a font configuration
    /// that affect glyph metrics.
    #[must_use]
    pub fn from_parts(family: &str, size_dip: f32, locale: &str, dpi_scale: f32) -> Self {
        let mut h = AHasher::default();
        family.hash(&mut h);
        size_dip.to_bits().hash(&mut h);
        locale.hash(&mut h);
        dpi_scale.to_bits().hash(&mut h);
        Self(h.finish())
    }

    /// Fold the configured tab width into the id. A literal tab's
    /// rendered advance is pinned per-format by the renderer's
    /// `SetIncrementalTabStop`, so two layouts that differ only in
    /// `tab_width` are genuinely different glyph runs and must not
    /// collide in the layout cache. Callers chain this onto
    /// [`Self::from_parts`] so a `tab_width` change evicts the stale
    /// layouts (the cache key changes) and the worker rebuilds them
    /// against the new stop. `tab_width == 0` is a no-op (the font's
    /// default tab stop is in effect — pre-settings behaviour).
    #[must_use]
    pub fn with_tab_width(self, tab_width: u32) -> Self {
        if tab_width == 0 {
            return self;
        }
        let mut h = AHasher::default();
        self.0.hash(&mut h);
        tab_width.hash(&mut h);
        Self(h.finish())
    }

    /// Fold a markdown render-toggle discriminator (an opaque
    /// 64-bit hash from `MarkdownRenderToggles::hash_key`) into the id.
    /// Flipping any markdown render toggle changes the projected
    /// segment list and soft-wrap row counts, so every layout / frame /
    /// segment / wrap cache keyed on this id must be invalidated.
    /// Callers chain this after [`Self::with_tab_width`]. A `0`
    /// discriminator is a no-op, preserving the pre-toggle id for
    /// callers that do not gate markdown rendering.
    #[must_use]
    pub fn with_markdown_toggles(self, toggles_hash: u64) -> Self {
        if toggles_hash == 0 {
            return self;
        }
        let mut h = AHasher::default();
        self.0.hash(&mut h);
        toggles_hash.hash(&mut h);
        Self(h.finish())
    }
}

/// Compute the content stamp for one line. Defined here so the cache and the
/// renderer agree on the hash function and seed.
#[must_use]
pub fn line_content_stamp(text: &str) -> u64 {
    let mut h = AHasher::default();
    text.hash(&mut h);
    h.finish()
}

/// Lookup key for one laid-out logical line.
///
/// `wrap_width_dip` is rounded to the nearest device-independent pixel; a
/// value of `0` disables soft wrap so the layout is built with infinite max
/// width and identical layouts share the key across viewport widths.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct LineLayoutKey {
    /// Document identifier (e.g. `BufferId.as_uuid().as_u128()`).
    pub document: u128,
    /// Line index within the document (0-based).
    pub line: u32,
    /// Hash of the line bytes — invalidates only when the line content
    /// actually changes, not on every buffer-wide revision bump.
    pub content_stamp: u64,
    /// Font configuration in effect when the layout was built.
    pub font_state: FontStateId,
    /// Soft-wrap width in DIPs, rounded; `0` means no wrap.
    pub wrap_width_dip: u32,
}

/// LRU-bounded cache of [`IDWriteTextLayout`] objects.
pub struct LayoutCache {
    capacity: usize,
    counter: u64,
    counters: LayoutCacheCounters,
    entries: AHashMap<LineLayoutKey, Entry>,
}

/// Monotonic counters for `IDWriteTextLayout` cache activity.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct LayoutCacheCounters {
    /// Cache lookups that found an existing layout.
    pub hits: u64,
    /// Cache lookups that did not find a layout.
    pub misses: u64,
    /// Layouts inserted into the cache after DirectWrite creation.
    pub layouts_created: u64,
    /// Inserts that displaced a different LRU entry to make room — a
    /// signal that the cache is under capacity pressure.
    pub layouts_created_after_evict: u64,
}

impl LayoutCacheCounters {
    /// Return `self - earlier`, saturating each field independently.
    #[must_use]
    pub fn saturating_delta(self, earlier: Self) -> Self {
        Self {
            hits: self.hits.saturating_sub(earlier.hits),
            misses: self.misses.saturating_sub(earlier.misses),
            layouts_created: self.layouts_created.saturating_sub(earlier.layouts_created),
            layouts_created_after_evict: self
                .layouts_created_after_evict
                .saturating_sub(earlier.layouts_created_after_evict),
        }
    }
}

struct Entry {
    text: Box<str>,
    layout: IDWriteTextLayout,
    last_used: u64,
}

/// Borrowed view of a cached layout plus its source text.
pub struct CachedLine<'a> {
    /// Source text the layout was built from (newline trimmed).
    pub text: &'a str,
    /// The cached `IDWriteTextLayout`. Borrowed under the same lifetime as
    /// the cache to avoid an extra COM ref-count bump per draw call.
    pub layout: &'a IDWriteTextLayout,
}

impl LayoutCache {
    /// Create an empty cache bounded to `capacity` entries. Per spec §5,
    /// `~10× visible lines per pane` is the recommended bound.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            capacity: cap,
            counter: 0,
            counters: LayoutCacheCounters::default(),
            entries: AHashMap::with_capacity(cap),
        }
    }

    /// Cache capacity. Inserts beyond it evict the least-recently-used
    /// entry first.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Adjust the LRU bound; existing entries beyond the new bound are
    /// evicted oldest-first.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.entries.len() > self.capacity {
            self.evict_oldest();
        }
    }

    /// Number of entries currently in the cache.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Estimated resident bytes held by cached DirectWrite layouts.
    #[must_use]
    pub fn byte_size_estimate(&self) -> usize {
        self.entries
            .values()
            .map(|entry| ESTIMATED_TEXT_LAYOUT_ENTRY_BYTES + entry.text.len())
            .sum()
    }

    /// Number of entries built against `font_state`. Used by Win32
    /// integration tests to prove a reflow stopped using an old font/DPI
    /// key.
    #[must_use]
    pub fn entry_count_for_font_state(&self, font_state: FontStateId) -> usize {
        self.entries
            .keys()
            .filter(|key| key.font_state == font_state)
            .count()
    }

    /// Snapshot the cache counters for trace deltas around one paint.
    #[must_use]
    pub fn counters(&self) -> LayoutCacheCounters {
        self.counters
    }

    /// `true` if the cache holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Look up an existing entry. Updates its LRU timestamp on hit.
    pub fn get(&mut self, key: &LineLayoutKey) -> Option<CachedLine<'_>> {
        let now = self.next_counter();
        let Some(entry) = self.entries.get_mut(key) else {
            self.counters.misses = self.counters.misses.wrapping_add(1);
            return None;
        };
        self.counters.hits = self.counters.hits.wrapping_add(1);
        entry.last_used = now;
        Some(CachedLine {
            text: &entry.text,
            layout: &entry.layout,
        })
    }

    /// Insert a freshly-built layout. If the cache is full, the least-
    /// recently-used entry is evicted first. Returns `true` when the
    /// insert displaced a different entry to make room (i.e. the cache
    /// was at capacity), so callers can attribute the new entry to
    /// "miss after evict" rather than "miss into empty slot".
    pub fn insert(
        &mut self,
        key: LineLayoutKey,
        text: Box<str>,
        layout: IDWriteTextLayout,
    ) -> bool {
        let now = self.next_counter();
        let mut evicted = false;
        if self.entries.len() >= self.capacity && !self.entries.contains_key(&key) {
            self.evict_oldest();
            evicted = true;
        }
        self.counters.layouts_created = self.counters.layouts_created.wrapping_add(1);
        if evicted {
            self.counters.layouts_created_after_evict =
                self.counters.layouts_created_after_evict.wrapping_add(1);
        }
        self.entries.insert(
            key,
            Entry {
                text,
                layout,
                last_used: now,
            },
        );
        evicted
    }

    /// Drop every entry whose `document` matches.
    pub fn invalidate_document(&mut self, document: u128) {
        self.entries.retain(|k, _| k.document != document);
    }

    /// Drop every entry whose font state differs from `font_state` — used
    /// when zoom or font family changes for the active pane (spec §5: the
    /// cache invalidates for that pane only).
    pub fn invalidate_other_font_states(&mut self, font_state: FontStateId) {
        self.entries.retain(|_, _| true);
        self.entries.retain(|k, _| k.font_state == font_state);
    }

    /// Drop every entry whose wrap width differs from `width`.
    ///
    /// **Caution.** The focused-pane wrap-paint path
    /// (`crates/render/src/wrap_paint.rs::paint_display_lines`) keys
    /// its entries with `wrap_width_dip = 0` because every line builds
    /// its own `DisplayLineSpec` and feeds the layout an infinite
    /// `max_layout_width`. Calling this method with a non-zero focused
    /// viewport width therefore *evicts* the focused-pane entries
    /// rather than preserving them. Only call this when the caller has
    /// actually invalidated keys at the supplied width — e.g. on a
    /// soft-wrap toggle or a per-pane wrap-width recompute. It is not
    /// the right tool for every WM_SIZE tick; the LRU bound handles
    /// stale spectator entries without per-tick churn.
    pub fn invalidate_other_wrap_widths(&mut self, width: u32) {
        self.entries.retain(|k, _| k.wrap_width_dip == width);
    }

    fn next_counter(&mut self) -> u64 {
        self.counter = self.counter.wrapping_add(1);
        self.counter
    }

    fn evict_oldest(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let mut oldest_key: Option<LineLayoutKey> = None;
        let mut oldest_t = u64::MAX;
        for (k, v) in &self.entries {
            if v.last_used < oldest_t {
                oldest_t = v.last_used;
                oldest_key = Some(*k);
            }
        }
        if let Some(k) = oldest_key {
            self.entries.remove(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DWriteFactory;

    fn make_layout(f: &DWriteFactory, text: &str) -> IDWriteTextLayout {
        let fmt = f.text_format("Cascadia Mono", 14.0, "en-us").unwrap();
        f.text_layout(text, &fmt, f32::INFINITY, f32::INFINITY)
            .unwrap()
    }

    fn key(document: u128, line: u32, content: &str) -> LineLayoutKey {
        LineLayoutKey {
            document,
            line,
            content_stamp: line_content_stamp(content),
            font_state: FontStateId::from_parts("Cascadia Mono", 14.0, "en-us", 1.0),
            wrap_width_dip: 0,
        }
    }

    #[test]
    fn font_state_id_is_stable() {
        let a = FontStateId::from_parts("Cascadia Mono", 14.0, "en-us", 1.0);
        let b = FontStateId::from_parts("Cascadia Mono", 14.0, "en-us", 1.0);
        assert_eq!(a, b);
        assert_ne!(
            a,
            FontStateId::from_parts("Cascadia Mono", 14.5, "en-us", 1.0)
        );
        assert_ne!(a, FontStateId::from_parts("Inter", 14.0, "en-us", 1.0));
        assert_ne!(
            a,
            FontStateId::from_parts("Cascadia Mono", 14.0, "en-us", 1.5)
        );
    }

    #[test]
    fn content_stamp_is_stable_and_distinguishes() {
        assert_eq!(line_content_stamp("hello"), line_content_stamp("hello"));
        assert_ne!(line_content_stamp("hello"), line_content_stamp("hellp"));
        // Empty stamps are deterministic.
        assert_eq!(line_content_stamp(""), line_content_stamp(""));
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let f = DWriteFactory::new().unwrap();
        let mut cache = LayoutCache::new(2);
        cache.insert(key(1, 0, "a"), "a".into(), make_layout(&f, "a"));
        cache.insert(key(1, 1, "b"), "b".into(), make_layout(&f, "b"));
        // Touch entry 0 so entry 1 becomes the oldest.
        let _ = cache.get(&key(1, 0, "a"));
        cache.insert(key(1, 2, "c"), "c".into(), make_layout(&f, "c"));
        assert!(cache.get(&key(1, 1, "b")).is_none());
        assert!(cache.get(&key(1, 0, "a")).is_some());
        assert!(cache.get(&key(1, 2, "c")).is_some());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn invalidate_document_removes_only_matching() {
        let f = DWriteFactory::new().unwrap();
        let mut cache = LayoutCache::new(4);
        cache.insert(key(1, 0, "a"), "a".into(), make_layout(&f, "a"));
        cache.insert(key(2, 0, "b"), "b".into(), make_layout(&f, "b"));
        cache.invalidate_document(1);
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&key(2, 0, "b")).is_some());
    }

    #[test]
    fn invalidate_other_font_states_keeps_only_active() {
        let f = DWriteFactory::new().unwrap();
        let mut cache = LayoutCache::new(4);
        let small = FontStateId::from_parts("Cascadia Mono", 14.0, "en-us", 1.0);
        let big = FontStateId::from_parts("Cascadia Mono", 18.0, "en-us", 1.0);
        cache.insert(
            LineLayoutKey {
                font_state: small,
                ..key(1, 0, "a")
            },
            "a".into(),
            make_layout(&f, "a"),
        );
        cache.insert(
            LineLayoutKey {
                font_state: big,
                ..key(1, 0, "a")
            },
            "a".into(),
            make_layout(&f, "a"),
        );
        cache.invalidate_other_font_states(small);
        assert_eq!(cache.len(), 1);
        assert!(cache
            .get(&LineLayoutKey {
                font_state: small,
                ..key(1, 0, "a")
            })
            .is_some());
    }

    #[test]
    fn invalidate_other_wrap_widths_evicts_focused_zero_key_entries() {
        // Regression: `Window::refresh_client_size` used to call
        // `invalidate_other_wrap_widths(view.wrap_width_key())` on
        // every WM_SIZE tick. Focused-pane wrap-paint entries are
        // keyed with `wrap_width_dip = 0`, so passing a non-zero
        // focused viewport width wiped exactly the entries the next
        // paint was about to reuse. This test documents the
        // eviction semantics that motivated dropping the call from
        // the resize path.
        let f = DWriteFactory::new().unwrap();
        let mut cache = LayoutCache::new(4);
        let focused = LineLayoutKey {
            wrap_width_dip: 0,
            ..key(1, 0, "a")
        };
        let spectator = LineLayoutKey {
            wrap_width_dip: 600,
            ..key(1, 0, "a")
        };
        cache.insert(focused, "a".into(), make_layout(&f, "a"));
        cache.insert(spectator, "a".into(), make_layout(&f, "a"));
        // Simulate the old resize-tick call with the new focused
        // viewport width.
        cache.invalidate_other_wrap_widths(600);
        assert!(
            cache.get(&focused).is_none(),
            "focused (wrap_width_dip=0) entries are evicted"
        );
        assert!(
            cache.get(&spectator).is_some(),
            "matching-width entries survive"
        );
    }

    #[test]
    fn set_capacity_evicts_overflow() {
        let f = DWriteFactory::new().unwrap();
        let mut cache = LayoutCache::new(4);
        for i in 0..4 {
            cache.insert(key(1, i, "x"), "x".into(), make_layout(&f, "x"));
        }
        cache.set_capacity(2);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn layout_cache_counters_track_hits_misses_and_creations() {
        let f = DWriteFactory::new().unwrap();
        let mut cache = LayoutCache::new(2);
        let before = cache.counters();
        let key = key(1, 0, "a");

        assert!(cache.get(&key).is_none());
        cache.insert(key, "a".into(), make_layout(&f, "a"));
        assert!(cache.get(&key).is_some());

        let delta = cache.counters().saturating_delta(before);
        assert_eq!(
            delta,
            LayoutCacheCounters {
                hits: 1,
                misses: 1,
                layouts_created: 1,
                layouts_created_after_evict: 0,
            }
        );
    }
}
