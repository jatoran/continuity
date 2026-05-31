//! Per-pane spectator `FrameDisplay` cache.
//!
//! Non-focused panes paint from their own per-pane projection (see
//! [`crate::window_paint_spectators::build_spectator_pane_data`]).
//! Without caching, every paint cold-builds every spectator's
//! projection — a 9 k-line markdown buffer in a non-focused pane
//! costs ~450 ms per paint while the user types into a small pane.
//! The trace `perf-snapshots/manual-lag_after-coalesce_20260517-212738.tsv`
//! captured a 465 074 µs spectator paint with
//! `spectator_realized_specs=17 574` while the focused pane held a
//! one-line source.
//!
//! This cache stores the most recent painted [`FrameDisplay`] per
//! [`PaneId`] together with its [`PrewarmQuery`]. Subsequent paints
//! reuse it via [`PrewarmQuery::is_compatible_for_motion`] — the same
//! caret-byte-ignoring comparator the focused pane already uses for
//! `Window::last_painted_frame_display`.
//!
//! Image reservations (γ) participate in the key via
//! [`PrewarmQuery`]'s reservation signature, so a spectator with a
//! stable expanded-image set reuses its cached frame and a changed set
//! misses and rebuilds — the same contract the focused pane uses.
//! Frames are skipped by callers when the prior cached entry is
//! incompatible.
//!
//! Thread ownership: UI thread of one window. Pure storage type with
//! no internal locking. Writers are spectator paint, focused-paint
//! cache seeding, and the UI-thread projection-worker drain for
//! spectator-targeted layout prewarms.

use std::collections::HashMap;
use std::sync::Arc;

use continuity_decorate::Decorations;
use continuity_render::FrameDisplay;

use crate::display_prewarm_cache::PrewarmQuery;
use crate::pane_tree::PaneId;

/// One cached spectator entry. Stored per [`PaneId`]; replaced on
/// every paint whose projection inputs change.
///
/// `decorations` and `parse_revision` ride along with the frame so a
/// promote into the focused-paint classify path can install them as
/// `Window::last_painted_decorations` /
/// `Window::last_painted_decoration_parse_revision`. Without that
/// shadowing, `classify_projection_build` rejects the promoted frame
/// with `Cold` at `decoration_advanced` because the OUTGOING focused
/// pane's decorations don't match the PROMOTED frame's stamp.
#[derive(Clone)]
struct Entry {
    query: PrewarmQuery,
    frame_display: FrameDisplay,
    decorations: Option<Arc<Decorations>>,
    parse_revision: Option<u64>,
}

/// Bundle returned by [`SpectatorFrameCache::lookup_for_focused_paint`]
/// when the focused paint can promote a spectator's prior frame.
/// Carries both the frame and the decoration context the classify
/// path needs to compute a tight dirty set instead of falling
/// through to a full cold rebuild.
pub(crate) struct PromotedFrame {
    pub frame_display: FrameDisplay,
    pub decorations: Option<Arc<Decorations>>,
    pub parse_revision: Option<u64>,
}

/// Per-pane storage of the most recently painted spectator
/// `FrameDisplay`. One entry per non-focused pane; entries for panes
/// no longer in the tree are evicted by
/// [`Self::retain_panes`].
#[derive(Default)]
pub(crate) struct SpectatorFrameCache {
    entries: HashMap<PaneId, Entry>,
    hits: u64,
    misses: u64,
}

/// Result of a cache lookup. Carries the cloned frame on a hit, the
/// motion-compat mismatch reason on a stale hit, and the cache-empty
/// signal otherwise. The reason strings are stable trace tokens
/// emitted alongside `paint:spectator_cache` events; perf scripts
/// key on them.
pub(crate) enum SpectatorCacheLookup {
    Hit(FrameDisplay),
    Stale(&'static str),
    Empty,
}

impl SpectatorFrameCache {
    /// Empty cache.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// `true` when any cached spectator entry was built against a
    /// `font_state` other than `current`. Used by the deferred
    /// font-swap settle loop ([`crate::window_font_swap`]) to decide
    /// whether to nudge the message pump for another paint while
    /// spectator workers are still catching up to a freshly-committed
    /// font change.
    #[must_use]
    pub(crate) fn any_entry_lags_font_state(
        &self,
        current: continuity_layout::FontStateId,
    ) -> bool {
        self.entries
            .values()
            .any(|entry| entry.query.font_state() != current)
    }

    /// Look up the cached frame for `pane`. Returns
    /// [`SpectatorCacheLookup::Hit`] only when the cached query is
    /// [`PrewarmQuery::is_compatible_for_motion`] with `query` — the
    /// caret-byte field is intentionally ignored so a caret-only move
    /// in the spectator (rare; spectator selections rarely change
    /// mid-typing) does not invalidate the cache.
    pub(crate) fn lookup(&mut self, pane: PaneId, query: &PrewarmQuery) -> SpectatorCacheLookup {
        match self.entries.get(&pane) {
            None => {
                self.misses = self.misses.saturating_add(1);
                SpectatorCacheLookup::Empty
            }
            Some(entry) => match entry.query.motion_compat_mismatch(query) {
                None => {
                    self.hits = self.hits.saturating_add(1);
                    SpectatorCacheLookup::Hit(entry.frame_display.clone())
                }
                Some(reason) => {
                    self.misses = self.misses.saturating_add(1);
                    SpectatorCacheLookup::Stale(reason)
                }
            },
        }
    }

    /// Miss-attributed hit-test lookup. Returns the cached frame on
    /// hit, or the first compat field that prevented reuse on miss
    /// (`"no_entry"` when the pane has no cached entry at all). Used
    /// by the click hit-test path so the trace event
    /// `click_hit_test_frame_source source=fallback stale_cache=…`
    /// attributes the spectator-cache arm's miss to the right field
    /// instead of swallowing it silently.
    pub(crate) fn lookup_for_hit_test_with_reason(
        &self,
        pane: PaneId,
        query: &PrewarmQuery,
    ) -> Result<FrameDisplay, &'static str> {
        let Some(entry) = self.entries.get(&pane) else {
            return Err("no_entry");
        };
        match entry.query.hit_test_compat_mismatch(query) {
            None => Ok(entry.frame_display.clone()),
            Some(reason) => Err(reason),
        }
    }

    /// Hit-test compatible lookup. Tolerates rope-revision and
    /// decoration-revision drift since the click maps to **what the
    /// user saw**; only document / wrap / font / fold mismatches
    /// reject the cache. Used by the mouse hit-test path after a
    /// `try_pane_body_focus_switch` to reuse the previous
    /// spectator-paint projection on the newly focused pane without
    /// cold-building.
    #[cfg(test)]
    pub(crate) fn lookup_for_hit_test(
        &self,
        pane: PaneId,
        query: &PrewarmQuery,
    ) -> Option<FrameDisplay> {
        self.lookup_for_hit_test_with_reason(pane, query).ok()
    }

    /// Focused-paint lookup. Returns the cached frame whenever its
    /// **geometry** (document / wrap / font / folds) matches `query`,
    /// even if rope or decoration revision has drifted.
    ///
    /// Used by `Window::resolve_paint_frame_display` when the
    /// outgoing focused pane's `last_painted_frame_display` misses
    /// on `document`: the just-focused pane was a spectator on the
    /// prior paint and its frame is still here. Passing the frame
    /// down as `cached_frame` lets `classify_projection_build`
    /// decide the correct rebuild kind — a `CacheHit` when nothing
    /// drifted, a `Dirty` rebuild keyed off the spectator's frame
    /// as `prev` when decorations advanced, and a `Cold` build only
    /// when geometry itself shifted.
    ///
    /// The looser compat is safe because the consumer **never paints
    /// the cached frame verbatim**: classify either returns it as
    /// `CacheHit` (only when revisions also matched), or treats it
    /// as `prev` for a rebuild. The rebuild paths reuse the frame's
    /// row index for unchanged source lines and re-realize specs
    /// for dirty lines under current rope / decoration state.
    ///
    /// Wrap width and font are still strict — both feed into row
    /// counts and y positions; a mismatch would paint the wrong
    /// content at the wrong place.
    pub(crate) fn lookup_for_focused_paint(
        &self,
        pane: PaneId,
        query: &PrewarmQuery,
    ) -> Option<PromotedFrame> {
        let entry = self.entries.get(&pane)?;
        if entry.query.is_compatible_for_hit_test(query) {
            Some(PromotedFrame {
                frame_display: entry.frame_display.clone(),
                decorations: entry.decorations.clone(),
                parse_revision: entry.parse_revision,
            })
        } else {
            None
        }
    }

    /// Wrap-tolerant lookup for the focus-switch hit-test and the
    /// focused-pane cold-deferred stub. Returns the cached frame
    /// whenever its **document** matches `query.document()`, ignoring
    /// every other compat field (wrap, rope/decoration revision, fold,
    /// font, caret bytes). Used in two narrow cases where strict
    /// compat would force an expensive cold rebuild:
    ///
    /// - The post-`try_pane_body_focus_switch` click resolver. The
    ///   just-focused pane's spectator entry was built with the
    ///   *spectator* `wrap_width_dip` formula (see
    ///   `continuity_render::pane_body::spectator_body_text_width_dip`)
    ///   while the click's `hit_test_query` carries the *focused*
    ///   pane's wrap. The painted pixels still come from the spectator
    ///   frame (paint hasn't run at the new wrap yet), so mapping the
    ///   click against that frame matches what the user clicked.
    /// - The focused-pane cold-deferred stub. The cached frame's wrap
    ///   differs from the current paint's; the caller (in
    ///   `window_paint::frame_resolution`) applies a stricter
    ///   rope/decoration-revision check on top of the document match
    ///   before substituting the frame for paint.
    ///
    /// Wrong-document entries are rejected outright — a click into a
    /// pane whose buffer changed since the cache entry was inserted
    /// would otherwise map pixels to the wrong rope.
    pub(crate) fn lookup_same_document(
        &self,
        pane: PaneId,
        query: &PrewarmQuery,
    ) -> Option<FrameDisplay> {
        let entry = self.entries.get(&pane)?;
        if entry.query.document() == query.document() {
            Some(entry.frame_display.clone())
        } else {
            None
        }
    }

    /// γ — document + image-row-reservation match, ignoring wrap /
    /// rope / decoration drift (like [`Self::lookup_same_document`]).
    /// Used by the focused-pane worker-miss reuse path
    /// (`window_paint::frame_resolution::worker_outcome_dispatch`) to
    /// pick the candidate frame that feeds the live-resize reuse and
    /// the cold-deferred stub. Those consumers validate wrap / rope /
    /// decoration against the frame's `IndexStamps` but cannot see the
    /// reservation set, so this lookup enforces reservation equality up
    /// front — a reservation-mismatched frame must never substitute for
    /// a Cold build. The plain [`Self::lookup_same_document`] stays
    /// reservation-blind for the mouse hit-test focus-switch path,
    /// where a click maps to the painted pixels regardless of a later
    /// reservation flip.
    pub(crate) fn lookup_same_document_for_reuse(
        &self,
        pane: PaneId,
        query: &PrewarmQuery,
    ) -> Option<FrameDisplay> {
        let entry = self.entries.get(&pane)?;
        if entry.query.document() == query.document()
            && entry.query.image_reservations_signature() == query.image_reservations_signature()
        {
            Some(entry.frame_display.clone())
        } else {
            None
        }
    }

    /// Replace the cached entry for `pane` with the given frame and
    /// the decorations / parse revision that produced it. Callers
    /// should pass the same `Decorations` Arc they fed to the
    /// builder so the focused-paint promote can hand the classify
    /// path a self-consistent prev frame and prev decorations.
    pub(crate) fn insert(
        &mut self,
        pane: PaneId,
        query: PrewarmQuery,
        frame: FrameDisplay,
        decorations: Option<Arc<Decorations>>,
        parse_revision: Option<u64>,
    ) {
        self.entries.insert(
            pane,
            Entry {
                query,
                frame_display: frame,
                decorations,
                parse_revision,
            },
        );
    }

    /// Drop every entry whose pane is not in `live_panes`. Called
    /// once per paint so closed / collapsed panes leave no
    /// `FrameDisplay` references behind.
    pub(crate) fn retain_panes(&mut self, live_panes: &[PaneId]) {
        self.entries.retain(|pane, _| live_panes.contains(pane));
    }

    /// Display-row count from the cached frame for `pane`.
    #[must_use]
    pub(crate) fn display_line_count(
        &self,
        pane: PaneId,
        buffer_id: continuity_buffer::BufferId,
    ) -> Option<u32> {
        let entry = self.entries.get(&pane)?;
        (entry.query.document() == buffer_id.as_uuid().as_u128())
            .then(|| entry.frame_display.display_line_count())
    }

    /// Cache-hit counter. Read by tests and perf instrumentation.
    #[must_use]
    pub(crate) fn hits(&self) -> u64 {
        self.hits
    }

    /// Cache-miss counter. Read by tests and perf instrumentation.
    #[must_use]
    pub(crate) fn misses(&self) -> u64 {
        self.misses
    }

    /// Number of cached entries (one per pane currently storing a
    /// frame).
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
    use continuity_display_map::FoldRange;
    use continuity_layout::FontStateId;
    use ropey::Rope;

    fn font_state() -> FontStateId {
        FontStateId::from_parts("Cascadia Mono", 14.0, "en-us", 1.0)
    }

    fn query(
        buffer_id: BufferId,
        rope_revision: u64,
        decoration_revision: Option<u64>,
        caret_bytes: &[usize],
        wrap_width_dip: u32,
    ) -> PrewarmQuery {
        PrewarmQuery::new(
            buffer_id,
            rope_revision,
            decoration_revision,
            caret_bytes,
            &[] as &[FoldRange],
            wrap_width_dip,
            font_state(),
        )
    }

    fn frame() -> FrameDisplay {
        let rope = Rope::from_str("hello\n");
        FrameDisplay::build(&rope, 1, None, &[0], 0, 8.0)
    }

    #[test]
    fn empty_lookup_is_empty() {
        let mut cache = SpectatorFrameCache::new();
        let pane = PaneId::fresh();
        let buffer_id = BufferId::new();
        let q = query(buffer_id, 1, None, &[0], 0);
        assert!(matches!(
            cache.lookup(pane, &q),
            SpectatorCacheLookup::Empty
        ));
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.misses(), 1);
    }

    #[test]
    fn hit_ignores_caret_bytes() {
        let mut cache = SpectatorFrameCache::new();
        let pane = PaneId::fresh();
        let buffer_id = BufferId::new();
        let inserted = query(buffer_id, 1, Some(3), &[10], 480);
        cache.insert(pane, inserted, frame(), None, None);
        let same_geom_different_caret = query(buffer_id, 1, Some(3), &[42], 480);
        assert!(matches!(
            cache.lookup(pane, &same_geom_different_caret),
            SpectatorCacheLookup::Hit(_)
        ));
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 0);
    }

    #[test]
    fn revision_advance_invalidates() {
        let mut cache = SpectatorFrameCache::new();
        let pane = PaneId::fresh();
        let buffer_id = BufferId::new();
        cache.insert(
            pane,
            query(buffer_id, 1, Some(3), &[10], 480),
            frame(),
            None,
            None,
        );
        let next = query(buffer_id, 2, Some(3), &[10], 480);
        let lookup = cache.lookup(pane, &next);
        match lookup {
            SpectatorCacheLookup::Stale(reason) => assert_eq!(reason, "rope_revision"),
            _ => panic!("expected stale"),
        }
        assert_eq!(cache.misses(), 1);
    }

    #[test]
    fn wrap_width_advance_invalidates() {
        let mut cache = SpectatorFrameCache::new();
        let pane = PaneId::fresh();
        let buffer_id = BufferId::new();
        cache.insert(
            pane,
            query(buffer_id, 1, None, &[0], 480),
            frame(),
            None,
            None,
        );
        let next = query(buffer_id, 1, None, &[0], 600);
        match cache.lookup(pane, &next) {
            SpectatorCacheLookup::Stale(reason) => assert_eq!(reason, "wrap_width_dip"),
            _ => panic!("expected stale"),
        }
    }

    #[test]
    fn no_wrap_stub_entry_hits_only_no_wrap_query() {
        let mut cache = SpectatorFrameCache::new();
        let pane = PaneId::fresh();
        let buffer_id = BufferId::new();
        cache.insert(
            pane,
            query(buffer_id, 1, None, &[0], 0),
            frame(),
            None,
            None,
        );

        match cache.lookup(pane, &query(buffer_id, 1, None, &[0], 480)) {
            SpectatorCacheLookup::Stale(reason) => assert_eq!(reason, "wrap_width_dip"),
            _ => panic!("expected stale"),
        }
        assert!(matches!(
            cache.lookup(pane, &query(buffer_id, 1, None, &[0], 0)),
            SpectatorCacheLookup::Hit(_)
        ));
    }

    #[test]
    fn retain_panes_evicts_missing() {
        let mut cache = SpectatorFrameCache::new();
        let live = PaneId::fresh();
        let stale = PaneId::fresh();
        let buffer_id = BufferId::new();
        cache.insert(
            live,
            query(buffer_id, 1, None, &[0], 0),
            frame(),
            None,
            None,
        );
        cache.insert(
            stale,
            query(buffer_id, 1, None, &[0], 0),
            frame(),
            None,
            None,
        );
        assert_eq!(cache.len(), 2);
        cache.retain_panes(&[live]);
        assert_eq!(cache.len(), 1);
        assert!(matches!(
            cache.lookup(stale, &query(buffer_id, 1, None, &[0], 0)),
            SpectatorCacheLookup::Empty
        ));
    }

    #[test]
    fn hit_test_lookup_tolerates_revision_drift() {
        let mut cache = SpectatorFrameCache::new();
        let pane = PaneId::fresh();
        let buffer_id = BufferId::new();
        cache.insert(
            pane,
            query(buffer_id, 1, Some(3), &[10], 480),
            frame(),
            None,
            None,
        );
        let drifted = query(buffer_id, 2, Some(4), &[10], 480);
        assert!(
            cache.lookup_for_hit_test(pane, &drifted).is_some(),
            "hit-test lookup must ignore rope/decoration drift"
        );
        let geom_change = query(buffer_id, 1, Some(3), &[10], 600);
        assert!(
            cache.lookup_for_hit_test(pane, &geom_change).is_none(),
            "hit-test lookup must reject wrap_width changes"
        );
    }

    #[test]
    fn reservation_drift_invalidates_motion_lookup() {
        use continuity_display_map::{ImageRowReservation, SourceLine};

        let mut cache = SpectatorFrameCache::new();
        let pane = PaneId::fresh();
        let buffer_id = BufferId::new();
        let reservations = [ImageRowReservation {
            source_line: SourceLine(3),
            reserved_display_rows: 5,
        }];
        cache.insert(
            pane,
            query(buffer_id, 1, Some(3), &[10], 480).with_image_reservations(&reservations),
            frame(),
            None,
            None,
        );
        // Same geometry but the expanded image collapsed (empty set):
        // the spectator must re-render rather than reuse the phantom
        // rows.
        match cache.lookup(pane, &query(buffer_id, 1, Some(3), &[10], 480)) {
            SpectatorCacheLookup::Stale(reason) => {
                assert_eq!(reason, "image_reservations_signature")
            }
            _ => panic!("expected stale on reservation drift"),
        }
        // Same reservation set → hit.
        assert!(matches!(
            cache.lookup(
                pane,
                &query(buffer_id, 1, Some(3), &[42], 480).with_image_reservations(&reservations),
            ),
            SpectatorCacheLookup::Hit(_)
        ));
    }

    #[test]
    fn reuse_lookup_enforces_reservations_but_plain_same_document_does_not() {
        use continuity_display_map::{ImageRowReservation, SourceLine};

        let mut cache = SpectatorFrameCache::new();
        let pane = PaneId::fresh();
        let buffer_id = BufferId::new();
        let reservations = [ImageRowReservation {
            source_line: SourceLine(3),
            reserved_display_rows: 5,
        }];
        cache.insert(
            pane,
            query(buffer_id, 1, Some(3), &[10], 480).with_image_reservations(&reservations),
            frame(),
            None,
            None,
        );
        let empty_query = query(buffer_id, 2, Some(9), &[10], 640);
        // Reuse lookup (feeds cold-deferred / live-resize) rejects a
        // reservation mismatch — those consumers can't see the
        // reservation set on the bare frame.
        assert!(
            cache
                .lookup_same_document_for_reuse(pane, &empty_query)
                .is_none(),
            "reuse lookup must reject a reservation mismatch"
        );
        // The plain document-only lookup stays reservation-blind for
        // the mouse-click focus-switch path (maps to painted pixels).
        assert!(
            cache.lookup_same_document(pane, &empty_query).is_some(),
            "plain same-document lookup stays reservation-blind"
        );
    }
}
