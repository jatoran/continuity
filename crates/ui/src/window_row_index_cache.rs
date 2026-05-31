//! Cross-pane row-index cache.
//!
//! The viewport-bounded `FrameDisplay` build walks every source line
//! once to compute the whole-document `DisplayRowIndex` (this is the
//! "cheap row-count walker" in `crates/display_map/src/builder/row_counts.rs`).
//! On a 9 k-line markdown buffer the walker costs ~400 ms in release
//! builds — and pays that cost on every cold build, including when:
//!
//! - the user switches focus **out** of a large buffer (so the buffer
//!   becomes a spectator in another pane);
//! - Ctrl+Tab brings a new tab with the large buffer into the same
//!   pane (the per-pane spectator cache misses on `document`);
//! - a layout shortcut reshuffles panes (new `PaneId`s evict the
//!   per-pane spectator cache).
//!
//! The per-pane [`crate::window_spectator_cache::SpectatorFrameCache`]
//! does **not** help these cases because its key is `PaneId`. This
//! cache uses the buffer-level geometry instead:
//!
//! ```text
//! key = (BufferId, rope_revision, decoration_revision, wrap_width_dip,
//!        font_state, fold_signature, decoration_row_shape_signature)
//! ```
//!
//! Any future viewport build with the same key reuses the cached
//! `Arc<DisplayRowIndex>` and skips the walker entirely. Spec
//! materialization for the visible viewport (~50 rows + overscan)
//! remains, but that is cheap.
//!
//! A second compatible lookup ignores `decoration_revision` when the
//! decoration row-shape signature matches. The signature hashes only
//! decoration data that can change display row counts (block kind/range,
//! hide/replace inline ranges, inline-color delimiter hides, and table
//! formula replacement ranges), so no-op re-parses or pure styling
//! overlays do not invalidate the expensive row-count table.
//!
//! Thread ownership: UI thread of one window. Pure storage type; no
//! internal locking.

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;

use continuity_decorate::{BlockKind, Decorations, InlineKind, MarkerKind};
use continuity_display_map::DisplayRowIndex;
use continuity_layout::FontStateId;

/// Bound on the number of (buffer, geometry) entries the cache
/// retains. Large enough to cover the buffers the user typically
/// rotates through with Ctrl+Tab plus the panes a grid layout can
/// produce; small enough to avoid unbounded growth in long sessions.
const ROW_INDEX_CACHE_MAX: usize = 32;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x100_0000_01b3;

/// Lookup key for one cached row index. All six fields must match
/// exactly — any difference produces a different display row count
/// for at least one source line.
#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub(crate) struct RowIndexKey {
    pub document: u128,
    pub rope_revision: u64,
    pub decoration_revision: Option<u64>,
    pub wrap_width_dip: u32,
    pub font_state: FontStateId,
    pub fold_signature: u64,
    pub decoration_row_shape_signature: u64,
}

struct Entry {
    key: RowIndexKey,
    row_index: Arc<DisplayRowIndex>,
}

fn is_compatible_key(entry: &RowIndexKey, key: &RowIndexKey) -> bool {
    entry.document == key.document
        && entry.rope_revision == key.rope_revision
        && entry.decoration_revision != key.decoration_revision
        && entry.wrap_width_dip == key.wrap_width_dip
        && entry.font_state == key.font_state
        && entry.fold_signature == key.fold_signature
        && entry.decoration_row_shape_signature == key.decoration_row_shape_signature
}

fn apply_hash_u64(hash: &mut u64, value: u64) {
    *hash ^= value;
    *hash = hash.wrapping_mul(FNV_PRIME);
}

fn apply_hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        apply_hash_u64(hash, u64::from(*byte));
    }
}

fn apply_hash_range(hash: &mut u64, range: Range<usize>) {
    apply_hash_u64(hash, range.start as u64);
    apply_hash_u64(hash, range.end as u64);
}

fn apply_hash_block_kind(hash: &mut u64, kind: BlockKind) {
    let tag = match kind {
        BlockKind::Heading { level } => {
            apply_hash_u64(hash, u64::from(level));
            1
        }
        BlockKind::SetextHeading { level } => {
            apply_hash_u64(hash, u64::from(level));
            2
        }
        BlockKind::Paragraph => 3,
        BlockKind::FencedCodeBlock => 4,
        BlockKind::IndentedCodeBlock => 5,
        BlockKind::List => 6,
        BlockKind::ListItem => 7,
        BlockKind::BlockQuote => 8,
        BlockKind::HorizontalRule => 9,
        BlockKind::PipeTable => 10,
        BlockKind::HtmlBlock => 11,
        BlockKind::Other(name) => {
            apply_hash_bytes(hash, name.as_bytes());
            12
        }
    };
    apply_hash_u64(hash, tag);
}

fn apply_hash_inline_shape(hash: &mut u64, kind: &InlineKind) {
    match kind {
        InlineKind::Marker(marker) => match marker {
            MarkerKind::HeadingHash => apply_hash_u64(hash, 20),
            MarkerKind::ListMarker => apply_hash_u64(hash, 21),
            MarkerKind::FenceTick => apply_hash_u64(hash, 22),
            MarkerKind::BlockquoteCaret => apply_hash_u64(hash, 23),
            MarkerKind::EmphasisDelim => apply_hash_u64(hash, 24),
            MarkerKind::StrikeDelim => apply_hash_u64(hash, 25),
            MarkerKind::CodeDelim => apply_hash_u64(hash, 26),
            MarkerKind::ThematicBreak => apply_hash_u64(hash, 27),
            MarkerKind::TablePipe => {}
        },
        InlineKind::Link {
            text_range,
            url_range,
        } => {
            apply_hash_u64(hash, 30);
            apply_hash_range(hash, text_range.start..text_range.end);
            apply_hash_range(hash, url_range.start..url_range.end);
        }
        InlineKind::FootnoteReference { label } => {
            apply_hash_u64(hash, 31);
            apply_hash_bytes(hash, label.as_bytes());
        }
        InlineKind::ImageRef { alt_range, .. } => {
            apply_hash_u64(hash, 32);
            apply_hash_range(hash, alt_range.start..alt_range.end);
        }
        InlineKind::Checkbox { .. } => apply_hash_u64(hash, 33),
        InlineKind::Strong
        | InlineKind::Emphasis
        | InlineKind::Strikethrough
        | InlineKind::Code
        | InlineKind::FootnoteDefinition { .. } => {}
    }
}

fn should_hash_inline_shape(kind: &InlineKind) -> bool {
    matches!(
        kind,
        InlineKind::Marker(
            MarkerKind::HeadingHash
                | MarkerKind::ListMarker
                | MarkerKind::FenceTick
                | MarkerKind::BlockquoteCaret
                | MarkerKind::EmphasisDelim
                | MarkerKind::StrikeDelim
                | MarkerKind::CodeDelim
                | MarkerKind::ThematicBreak
        ) | InlineKind::Link { .. }
            | InlineKind::FootnoteReference { .. }
            | InlineKind::ImageRef { .. }
            | InlineKind::Checkbox { .. }
    )
}

/// Compute a stable signature for decoration data that can alter
/// display row counts.
#[must_use]
pub(crate) fn compute_decoration_row_shape_signature(decorations: Option<&Decorations>) -> u64 {
    let Some(decorations) = decorations else {
        return 0;
    };
    let mut hash = FNV_OFFSET_BASIS;
    for block in &decorations.blocks {
        apply_hash_u64(&mut hash, 1);
        apply_hash_block_kind(&mut hash, block.kind);
        apply_hash_u64(&mut hash, block.start_byte as u64);
        apply_hash_u64(&mut hash, block.end_byte as u64);
    }
    for span in &decorations.inlines {
        if !should_hash_inline_shape(&span.kind) {
            continue;
        }
        apply_hash_u64(&mut hash, 2);
        apply_hash_range(&mut hash, span.range.start..span.range.end);
        apply_hash_inline_shape(&mut hash, &span.kind);
    }
    for span in &decorations.inline_color_spans {
        apply_hash_u64(&mut hash, 3);
        apply_hash_range(&mut hash, span.outer.clone());
        apply_hash_range(&mut hash, span.inner.clone());
    }
    for table in &decorations.evaluated_tables {
        apply_hash_u64(&mut hash, 4);
        apply_hash_range(&mut hash, table.block_range.clone());
        for cell in &table.overrides {
            apply_hash_u64(&mut hash, 5);
            apply_hash_u64(&mut hash, u64::from(cell.cell.col));
            apply_hash_u64(&mut hash, u64::from(cell.cell.row));
            apply_hash_range(&mut hash, cell.cell_range.clone());
            apply_hash_bytes(&mut hash, cell.display.as_bytes());
        }
    }
    hash
}

/// Cross-pane row-index cache. Bounded LRU; oldest entries evict
/// first.
#[derive(Default)]
pub(crate) struct RowIndexCache {
    entries: VecDeque<Entry>,
    hits: u64,
    misses: u64,
}

impl RowIndexCache {
    /// Empty cache.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Look up a row index by key. Returns `None` on miss.
    pub(crate) fn get(&mut self, key: &RowIndexKey) -> Option<Arc<DisplayRowIndex>> {
        if let Some(pos) = self.entries.iter().position(|e| &e.key == key) {
            self.hits = self.hits.saturating_add(1);
            // Promote to most-recent so LRU eviction drops cold entries.
            let entry = self.entries.remove(pos).expect("position checked above");
            let row_index = entry.row_index.clone();
            self.entries.push_back(entry);
            Some(row_index)
        } else {
            self.misses = self.misses.saturating_add(1);
            None
        }
    }

    /// Look up a row index whose key differs only by decoration
    /// revision while the row-shape signature matches.
    pub(crate) fn get_compatible(&mut self, key: &RowIndexKey) -> Option<Arc<DisplayRowIndex>> {
        if let Some(pos) = self
            .entries
            .iter()
            .position(|entry| is_compatible_key(&entry.key, key))
        {
            self.hits = self.hits.saturating_add(1);
            let entry = self.entries.remove(pos).expect("position checked above");
            let row_index = entry.row_index.clone();
            self.entries.push_back(entry);
            Some(row_index)
        } else {
            self.misses = self.misses.saturating_add(1);
            None
        }
    }

    /// `true` when an exact or compatible entry exists. Does not
    /// update LRU order or hit/miss counters; used only for trace
    /// attribution by callers that will perform the real lookup next.
    #[must_use]
    pub(crate) fn contains_exact_or_compatible(&self, key: &RowIndexKey) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.key == *key || is_compatible_key(&entry.key, key))
    }

    /// Look up a row index whose key matches `key` on every field
    /// except `rope_revision` (and possibly `decoration_revision` /
    /// `decoration_row_shape_signature`, which the splice path
    /// recomputes for the dirty source lines). Returns the most-
    /// recently-inserted candidate so the splice walks the fewest
    /// edits forward.
    ///
    /// Used by the splice fast-path on a `rope_revision` cache miss
    /// in `crates/display_map/src/builder/splice_row_index.rs`: the
    /// caller fetches `rope_deltas_since(stamps().rope_revision)` and
    /// asks the display-map builder to splice the older index forward
    /// into one keyed at `key.rope_revision`. Does not mutate LRU
    /// order; the freshly-spliced index is inserted under the new key
    /// after a successful splice and that insertion handles
    /// promotion.
    pub(crate) fn get_for_splice(&self, key: &RowIndexKey) -> Option<Arc<DisplayRowIndex>> {
        self.entries
            .iter()
            .rev()
            .find(|entry| {
                entry.key.document == key.document
                    && entry.key.wrap_width_dip == key.wrap_width_dip
                    && entry.key.font_state == key.font_state
                    && entry.key.fold_signature == key.fold_signature
                    && entry.key.decoration_row_shape_signature
                        == key.decoration_row_shape_signature
                    && entry.key.rope_revision != key.rope_revision
            })
            .map(|entry| entry.row_index.clone())
    }

    /// On a fresh miss, find the closest same-document entry and
    /// return the first field that differs from `key`. Returns
    /// `"no_entry"` when no entry exists for this document at all,
    /// or `"no_document_entry"` when there are entries but none for
    /// this document. Used by trace attribution to label *why* a
    /// row-index cache miss happened — mirrors the
    /// `StampMismatchField` ladder ε.5d added to projection-worker
    /// stamps.
    #[must_use]
    pub(crate) fn closest_match_diff(&self, key: &RowIndexKey) -> &'static str {
        if self.entries.is_empty() {
            return "no_entry";
        }
        let same_doc: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| e.key.document == key.document)
            .collect();
        if same_doc.is_empty() {
            return "no_document_entry";
        }
        // Most-recent same-document entry; same scan order as `get`.
        let entry = same_doc
            .last()
            .copied()
            .expect("non-empty same_doc checked above");
        if entry.key.rope_revision != key.rope_revision {
            return "rope_revision";
        }
        if entry.key.decoration_revision != key.decoration_revision {
            return "decoration_revision";
        }
        if entry.key.wrap_width_dip != key.wrap_width_dip {
            return "wrap_width_dip";
        }
        if entry.key.font_state != key.font_state {
            return "font_state";
        }
        if entry.key.fold_signature != key.fold_signature {
            return "fold_signature";
        }
        if entry.key.decoration_row_shape_signature != key.decoration_row_shape_signature {
            return "decoration_row_shape_signature";
        }
        "unknown"
    }

    /// Store a row index for `key`. Replaces any existing entry with
    /// the same key; evicts the oldest entry when the cache is full.
    pub(crate) fn insert(&mut self, key: RowIndexKey, row_index: Arc<DisplayRowIndex>) {
        self.entries.retain(|e| e.key != key);
        self.entries.push_back(Entry { key, row_index });
        while self.entries.len() > ROW_INDEX_CACHE_MAX {
            let _ = self.entries.pop_front();
        }
    }

    /// Evict every entry referencing `document`. Called when a buffer
    /// is closed so stale row indexes don't leak.
    #[cfg(test)]
    pub(crate) fn invalidate_document(&mut self, document: u128) {
        self.entries.retain(|e| e.key.document != document);
    }

    /// Hit counter (for tests / perf instrumentation).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn hits(&self) -> u64 {
        self.hits
    }

    /// Miss counter (for tests / perf instrumentation).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn misses(&self) -> u64 {
        self.misses
    }

    /// Number of cached entries.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_buffer::BufferId;
    use continuity_display_map::IndexStamps;

    fn font_state() -> FontStateId {
        FontStateId::from_parts("Cascadia Mono", 14.0, "en-us", 1.0)
    }

    fn empty_index() -> Arc<DisplayRowIndex> {
        Arc::new(DisplayRowIndex::from_row_counts(
            vec![1u16],
            IndexStamps {
                rope_revision: 0,
                decoration_revision: 0,
                wrap_width_dip: 0,
                font_state: 0,
                fold_signature: 0,
            },
        ))
    }

    fn key(buffer_id: BufferId, rope_rev: u64, wrap: u32) -> RowIndexKey {
        let decorations = Decorations::empty(rope_rev);
        RowIndexKey {
            document: buffer_id.as_uuid().as_u128(),
            rope_revision: rope_rev,
            decoration_revision: Some(rope_rev),
            wrap_width_dip: wrap,
            font_state: font_state(),
            fold_signature: 0,
            decoration_row_shape_signature: compute_decoration_row_shape_signature(Some(
                &decorations,
            )),
        }
    }

    fn key_for_decorations(
        buffer_id: BufferId,
        rope_revision: u64,
        wrap: u32,
        decorations: &Decorations,
    ) -> RowIndexKey {
        RowIndexKey {
            document: buffer_id.as_uuid().as_u128(),
            rope_revision,
            decoration_revision: Some(decorations.revision),
            wrap_width_dip: wrap,
            font_state: font_state(),
            fold_signature: 0,
            decoration_row_shape_signature: compute_decoration_row_shape_signature(Some(
                decorations,
            )),
        }
    }

    fn marker_decorations(revision: u64, start: usize, end: usize) -> Decorations {
        let mut decorations = Decorations::empty(revision);
        decorations.inlines.push(continuity_decorate::InlineSpan {
            kind: continuity_decorate::InlineKind::Marker(
                continuity_decorate::MarkerKind::EmphasisDelim,
            ),
            range: continuity_decorate::ByteRange::new(start, end),
        });
        decorations
    }

    #[test]
    fn insert_then_hit() {
        let mut cache = RowIndexCache::new();
        let buffer_id = BufferId::new();
        let k = key(buffer_id, 1, 800);
        cache.insert(k.clone(), empty_index());
        assert!(cache.get(&k).is_some());
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 0);
    }

    #[test]
    fn different_wrap_misses() {
        let mut cache = RowIndexCache::new();
        let buffer_id = BufferId::new();
        cache.insert(key(buffer_id, 1, 800), empty_index());
        assert!(cache.get(&key(buffer_id, 1, 600)).is_none());
    }

    #[test]
    fn compatible_hit_when_only_decoration_revision_differs() {
        let mut cache = RowIndexCache::new();
        let buffer_id = BufferId::new();
        let previous = marker_decorations(1, 0, 2);
        let current = marker_decorations(2, 0, 2);
        let previous_key = key_for_decorations(buffer_id, 9, 800, &previous);
        let current_key = key_for_decorations(buffer_id, 9, 800, &current);
        cache.insert(previous_key, empty_index());

        assert!(cache.get(&current_key).is_none());
        assert!(cache.get_compatible(&current_key).is_some());
    }

    #[test]
    fn compatible_miss_when_row_shape_differs() {
        let mut cache = RowIndexCache::new();
        let buffer_id = BufferId::new();
        let previous = marker_decorations(1, 0, 2);
        let current = marker_decorations(2, 0, 4);
        let previous_key = key_for_decorations(buffer_id, 9, 800, &previous);
        let current_key = key_for_decorations(buffer_id, 9, 800, &current);
        cache.insert(previous_key, empty_index());

        assert!(cache.get_compatible(&current_key).is_none());
    }

    #[test]
    fn invalidate_document_drops_entries() {
        let mut cache = RowIndexCache::new();
        let buffer_a = BufferId::new();
        let buffer_b = BufferId::new();
        cache.insert(key(buffer_a, 1, 800), empty_index());
        cache.insert(key(buffer_b, 1, 800), empty_index());
        assert_eq!(cache.len(), 2);
        cache.invalidate_document(buffer_a.as_uuid().as_u128());
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&key(buffer_a, 1, 800)).is_none());
        assert!(cache.get(&key(buffer_b, 1, 800)).is_some());
    }

    #[test]
    fn get_for_splice_finds_same_geometry_different_revision() {
        let mut cache = RowIndexCache::new();
        let buffer_id = BufferId::new();
        cache.insert(key(buffer_id, 1, 800), empty_index());
        cache.insert(key(buffer_id, 2, 800), empty_index());

        let current = key(buffer_id, 3, 800);
        assert!(cache.get_for_splice(&current).is_some());
    }

    #[test]
    fn get_for_splice_skips_different_wrap_width() {
        let cache = {
            let mut c = RowIndexCache::new();
            let buffer_id = BufferId::new();
            c.insert(key(buffer_id, 1, 800), empty_index());
            (c, buffer_id)
        };
        let (cache, buffer_id) = cache;
        let current = key(buffer_id, 2, 600);
        assert!(cache.get_for_splice(&current).is_none());
    }

    #[test]
    fn get_for_splice_skips_other_documents() {
        let mut cache = RowIndexCache::new();
        let buffer_a = BufferId::new();
        let buffer_b = BufferId::new();
        cache.insert(key(buffer_a, 1, 800), empty_index());
        let current = key(buffer_b, 2, 800);
        assert!(cache.get_for_splice(&current).is_none());
    }

    #[test]
    fn lru_evicts_oldest_past_bound() {
        let mut cache = RowIndexCache::new();
        let buffer_id = BufferId::new();
        for i in 0..(ROW_INDEX_CACHE_MAX as u64 + 5) {
            cache.insert(key(buffer_id, i, 800), empty_index());
        }
        assert_eq!(cache.len(), ROW_INDEX_CACHE_MAX);
        // Earliest entries should be gone.
        assert!(cache.get(&key(buffer_id, 0, 800)).is_none());
        // Most recent retained.
        assert!(cache
            .get(&key(buffer_id, ROW_INDEX_CACHE_MAX as u64 + 4, 800))
            .is_some());
    }
}
