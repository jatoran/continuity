//! UI-thread cache for outline-sidebar entries.
//!
//! The cache stores the derived row list used by paint and click hit-test.
//! Buffer text remains owned by `core`; this module only keeps cloned
//! heading metadata keyed by rope and decoration revisions.

use continuity_buffer::BufferId;
use continuity_decorate::{Decorations, HeadingEntry};
use continuity_render::OutlineEntry;
use ropey::Rope;

/// Cache key for one buffer's outline rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OutlineEntriesCacheKey {
    pub(crate) buffer_id: BufferId,
    pub(crate) rope_revision: u64,
    pub(crate) decoration_revision: Option<u64>,
}

/// Cached outline rows plus the raw heading metadata used for current-row
/// lookup and click target resolution.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct OutlineEntriesSnapshot {
    pub(crate) entries: Vec<OutlineEntry>,
    pub(crate) headings: Vec<HeadingEntry>,
}

/// Outcome of one cache lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutlineEntriesCacheStatus {
    Hit,
    Miss,
}

impl OutlineEntriesCacheStatus {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
        }
    }
}

#[derive(Clone, Debug)]
struct OutlineEntriesCacheEntry {
    key: OutlineEntriesCacheKey,
    snapshot: OutlineEntriesSnapshot,
}

/// Single-entry outline cache owned by one [`crate::Window`] UI thread.
#[derive(Clone, Debug, Default)]
pub(crate) struct OutlineEntriesCache {
    entry: Option<OutlineEntriesCacheEntry>,
}

impl OutlineEntriesCache {
    pub(crate) fn get_or_build(
        &mut self,
        key: OutlineEntriesCacheKey,
        build: impl FnOnce() -> OutlineEntriesSnapshot,
    ) -> (OutlineEntriesSnapshot, OutlineEntriesCacheStatus) {
        if let Some(entry) = self.entry.as_ref().filter(|entry| entry.key == key) {
            return (entry.snapshot.clone(), OutlineEntriesCacheStatus::Hit);
        }
        let snapshot = build();
        self.entry = Some(OutlineEntriesCacheEntry {
            key,
            snapshot: snapshot.clone(),
        });
        (snapshot, OutlineEntriesCacheStatus::Miss)
    }

    pub(crate) fn clear_for_buffer(&mut self, buffer_id: BufferId) {
        if self
            .entry
            .as_ref()
            .is_some_and(|entry| entry.key.buffer_id == buffer_id)
        {
            self.entry = None;
        }
    }
}

pub(crate) fn build_outline_entries_snapshot(
    rope: &Rope,
    decorations: Option<&Decorations>,
) -> OutlineEntriesSnapshot {
    let Some(decorations) = decorations else {
        return OutlineEntriesSnapshot::default();
    };
    let headings = continuity_decorate::headings(&decorations.blocks, rope);
    let progress =
        continuity_decorate::task_progress_per_heading(&headings, &decorations.inlines, rope);
    let entries = headings
        .iter()
        .enumerate()
        .map(|(idx, heading)| {
            let suffix = progress
                .get(idx)
                .and_then(|progress| progress.format_suffix())
                .map(|suffix| format!(" {suffix}"))
                .unwrap_or_default();
            OutlineEntry {
                text: format!("{}{}", heading.text, suffix),
                level: heading.level,
                target_byte: u32::try_from(heading.start_byte).unwrap_or(u32::MAX),
            }
        })
        .collect();
    OutlineEntriesSnapshot { entries, headings }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(
        buffer_id: BufferId,
        rope_revision: u64,
        decoration_revision: Option<u64>,
    ) -> OutlineEntriesCacheKey {
        OutlineEntriesCacheKey {
            buffer_id,
            rope_revision,
            decoration_revision,
        }
    }

    fn snapshot(text: &str) -> OutlineEntriesSnapshot {
        OutlineEntriesSnapshot {
            entries: vec![OutlineEntry {
                text: text.to_string(),
                level: 1,
                target_byte: 0,
            }],
            headings: vec![HeadingEntry {
                level: 1,
                text: text.to_string(),
                line: 0,
                start_byte: 0,
            }],
        }
    }

    #[test]
    fn same_key_hits_after_first_build() {
        let buffer_id = BufferId::new();
        let mut cache = OutlineEntriesCache::default();
        let key = key(buffer_id, 7, Some(3));

        let (_, first) = cache.get_or_build(key, || snapshot("one"));
        let (second_snapshot, second) = cache.get_or_build(key, || snapshot("two"));

        assert_eq!(first, OutlineEntriesCacheStatus::Miss);
        assert_eq!(second, OutlineEntriesCacheStatus::Hit);
        assert_eq!(second_snapshot.entries[0].text, "one");
    }

    #[test]
    fn decoration_revision_bump_misses() {
        let buffer_id = BufferId::new();
        let mut cache = OutlineEntriesCache::default();

        let _ = cache.get_or_build(key(buffer_id, 7, Some(3)), || snapshot("old"));
        let (next, status) = cache.get_or_build(key(buffer_id, 7, Some(4)), || snapshot("new"));

        assert_eq!(status, OutlineEntriesCacheStatus::Miss);
        assert_eq!(next.entries[0].text, "new");
    }

    #[test]
    fn rope_revision_bump_misses() {
        let buffer_id = BufferId::new();
        let mut cache = OutlineEntriesCache::default();

        let _ = cache.get_or_build(key(buffer_id, 7, Some(3)), || snapshot("old"));
        let (next, status) = cache.get_or_build(key(buffer_id, 8, Some(3)), || snapshot("new"));

        assert_eq!(status, OutlineEntriesCacheStatus::Miss);
        assert_eq!(next.entries[0].text, "new");
    }

    #[test]
    fn empty_snapshot_is_cached() {
        let buffer_id = BufferId::new();
        let mut cache = OutlineEntriesCache::default();
        let key = key(buffer_id, 1, None);

        let (_, first) = cache.get_or_build(key, OutlineEntriesSnapshot::default);
        let (empty, second) = cache.get_or_build(key, || snapshot("unexpected"));

        assert_eq!(first, OutlineEntriesCacheStatus::Miss);
        assert_eq!(second, OutlineEntriesCacheStatus::Hit);
        assert!(empty.entries.is_empty());
    }
}
