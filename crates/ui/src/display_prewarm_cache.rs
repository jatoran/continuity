//! Pure queue/cache state for idle display-map prewarm.
//!
//! Owned by one window's UI thread. The cache stores derived
//! `FrameDisplay` values only; buffer text remains owned by `core`.

use std::collections::VecDeque;

use continuity_buffer::BufferId;
use continuity_display_map::{FoldRange, FoldSignature, ImageRowReservation};
use continuity_layout::FontStateId;
use continuity_render::FrameDisplay;

use crate::projection_worker::ProjectionStamp;

/// Number of MRU-adjacent buffers targeted by prewarm.
pub(crate) const PREWARM_TARGET_BUFFERS: usize = 2;
const PREWARM_MAX_QUEUE: usize = PREWARM_TARGET_BUFFERS * 3;
const PREWARM_MAX_CACHE: usize = PREWARM_TARGET_BUFFERS * 3;

/// One phase of prewarm work for a single buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrewarmStage {
    /// Build around the target buffer's current caret set with no viewport
    /// wrapping or decorations.
    Caret,
    /// Build the exact current viewport projection without decorations.
    Viewport,
    /// Build the exact current viewport projection with current decorations,
    /// or submit decoration work if the parser has not caught up yet.
    Decoration,
}

impl PrewarmStage {
    pub(crate) fn next(self) -> Option<Self> {
        match self {
            Self::Caret => Some(Self::Viewport),
            Self::Viewport => Some(Self::Decoration),
            Self::Decoration => None,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Caret => 0,
            Self::Viewport => 1,
            Self::Decoration => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PrewarmWork {
    pub(crate) buffer_id: BufferId,
    pub(crate) stage: PrewarmStage,
}

/// Cache lookup key for one prewarmed display map.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PrewarmQuery {
    document: u128,
    rope_revision: u64,
    decoration_revision: Option<u64>,
    caret_bytes: Vec<usize>,
    fold_signature: u64,
    /// Image-row-reservation hash (γ). Expanded inline images (and,
    /// later, multi-line table rows) inject phantom display rows that
    /// change row geometry without touching the rope, decorations,
    /// folds, or wrap width. A frame built with one reservation set is
    /// geometrically invalid for another, so this field participates in
    /// **both** motion and hit-test compatibility — mirroring
    /// [`Self::fold_signature`], the other "extra rows per source line"
    /// input. Empty reservations hash to the deterministic empty-slice
    /// signature so the common no-image path stays stable across paints.
    /// Computed via [`ProjectionStamp::image_reservations_signature`] so
    /// the cache key and the worker stamp can never disagree on "same
    /// reservation set". Defaults to the empty-slice signature in
    /// [`Self::new`]; opt a query into a non-empty set with
    /// [`Self::with_image_reservations`].
    image_reservations_signature: u64,
    wrap_width_dip: u32,
    font_state: FontStateId,
}

impl PrewarmQuery {
    /// Font state this query was built against. Used by the deferred
    /// font-swap settle loop ([`crate::window_font_swap`]) to detect
    /// spectator cache entries that haven't yet caught up to the new
    /// font.
    #[must_use]
    pub(crate) fn font_state(&self) -> FontStateId {
        self.font_state
    }

    /// Build a query from the projection inputs used by paint.
    #[must_use]
    pub(crate) fn new(
        buffer_id: BufferId,
        rope_revision: u64,
        decoration_revision: Option<u64>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        wrap_width_dip: u32,
        font_state: FontStateId,
    ) -> Self {
        Self {
            document: buffer_id.as_uuid().as_u128(),
            rope_revision,
            decoration_revision,
            caret_bytes: caret_bytes.to_vec(),
            fold_signature: FoldSignature::compute(folds),
            image_reservations_signature: ProjectionStamp::image_reservations_signature(&[]),
            wrap_width_dip,
            font_state,
        }
    }

    /// Return a query keyed against `reservations` (γ). The base query
    /// from [`Self::new`] carries the empty-slice signature; call this
    /// to fold in a non-empty image-row-reservation set so the cache
    /// only serves a frame to a paint with the identical reservation
    /// geometry. A stable set keeps hitting; expanding / collapsing an
    /// image (or, later, growing a multi-line table row) bumps the
    /// signature and forces exactly one rebuild.
    #[must_use]
    pub(crate) fn with_image_reservations(mut self, reservations: &[ImageRowReservation]) -> Self {
        self.image_reservations_signature =
            ProjectionStamp::image_reservations_signature(reservations);
        self
    }

    /// Image-row-reservation signature this query was built against.
    /// Read by the worker-miss reuse-candidate filters
    /// (`window_paint::frame_resolution::worker_outcome_dispatch`) so a
    /// reservation-mismatched `last_painted` / spectator frame is never
    /// handed to the cold-deferred stub or the live-resize reuse path.
    #[must_use]
    pub(crate) fn image_reservations_signature(&self) -> u64 {
        self.image_reservations_signature
    }

    /// True when `other` shares the same projection geometry (rope,
    /// decorations, folds, wrap, font) as `self`, **ignoring caret
    /// bytes**. Caret position only perturbs the projection through
    /// markdown reveal of the caret line; for pre-move soft-wrap
    /// vertical-row navigation against the last painted frame, that
    /// drift is acceptable — wrap row counts on every other source
    /// line are byte-for-byte identical and the next paint reseeds
    /// the cache with the post-move caret. See
    /// [`crate::Window::last_painted_frame_display`].
    #[must_use]
    pub(crate) fn is_compatible_for_motion(&self, other: &Self) -> bool {
        self.motion_compat_mismatch(other).is_none()
    }

    /// Document hash used as the primary cache-equality field. Exposed
    /// so callers performing a tolerant compat check (e.g. the focused-
    /// pane cold-deferred stub, the wrap-tolerant hit-test fallback)
    /// can confirm two queries name the same buffer without comparing
    /// every other geometry field.
    #[must_use]
    pub(crate) fn document(&self) -> u128 {
        self.document
    }

    /// Caret bytes recorded for this query. Paint reads this off
    /// [`crate::Window::last_painted_frame_display`] to detect
    /// selection-only drift — when the rope/decorations/wrap/font
    /// match but the caret moved between paints, the source lines
    /// containing the old and new carets must refresh their markdown
    /// reveal state before the cached frame is reused. See
    /// `crates/ui/src/window_paint.rs` for the rebuild path.
    #[must_use]
    pub(crate) fn caret_bytes(&self) -> &[usize] {
        &self.caret_bytes
    }

    /// Returns the first field that differs between two queries for
    /// motion-compatibility purposes, or `None` when they match. Used
    /// by the paint trace to explain cache misses; the comparison is
    /// identical to [`Self::is_compatible_for_motion`].
    #[must_use]
    pub(crate) fn motion_compat_mismatch(&self, other: &Self) -> Option<&'static str> {
        if self.document != other.document {
            Some("document")
        } else if self.rope_revision != other.rope_revision {
            Some("rope_revision")
        } else if self.decoration_revision != other.decoration_revision {
            Some("decoration_revision")
        } else if self.fold_signature != other.fold_signature {
            Some("fold_signature")
        } else if self.image_reservations_signature != other.image_reservations_signature {
            Some("image_reservations_signature")
        } else if self.wrap_width_dip != other.wrap_width_dip {
            Some("wrap_width_dip")
        } else if self.font_state != other.font_state {
            Some("font_state")
        } else {
            None
        }
    }

    /// Hit-test compatibility — looser than motion compatibility.
    /// Used when mapping a mouse click to a buffer position: the user
    /// clicked **what they saw**, so a projection at the same
    /// document / wrap / font / fold geometry maps correctly to the
    /// clicked pixel even if the rope or decoration revision has
    /// advanced slightly between the last paint and the click. The
    /// caller clamps the resulting `Position` against the live rope,
    /// so a single-line drift in source bytes between paint and click
    /// resolves harmlessly.
    ///
    /// Returns the first field that prevents hit-test reuse, or
    /// `None` when the cached frame is usable.
    ///
    /// `image_reservations_signature` participates here, like
    /// `fold_signature`: phantom image rows change which source line a
    /// clicked display row maps to (a click below an expanded image
    /// resolves to the image's source line, not the line that would sit
    /// there without the reservation), so a reservation-drifted frame
    /// maps clicks to the wrong source position. This also makes the
    /// frame-promote / rebuild-source consumers that key off hit-test
    /// compat (`lookup_for_focused_paint`,
    /// `compute_compatible_last_painted_frame`) reservation-safe in one
    /// stroke. The genuine click→source path keeps a reservation-blind
    /// fallback via
    /// [`crate::window_spectator_cache::SpectatorFrameCache::lookup_same_document`],
    /// so a rare reservation flip between paint and click still maps to
    /// the pixels the user saw.
    #[must_use]
    pub(crate) fn hit_test_compat_mismatch(&self, other: &Self) -> Option<&'static str> {
        if self.document != other.document {
            Some("document")
        } else if self.fold_signature != other.fold_signature {
            Some("fold_signature")
        } else if self.image_reservations_signature != other.image_reservations_signature {
            Some("image_reservations_signature")
        } else if self.wrap_width_dip != other.wrap_width_dip {
            Some("wrap_width_dip")
        } else if self.font_state != other.font_state {
            Some("font_state")
        } else {
            None
        }
    }

    /// True when `other` is hit-test compatible with `self`. See
    /// [`Self::hit_test_compat_mismatch`] for the field rules.
    #[must_use]
    pub(crate) fn is_compatible_for_hit_test(&self, other: &Self) -> bool {
        self.hit_test_compat_mismatch(other).is_none()
    }
}

struct PrewarmedDisplayMap {
    query: PrewarmQuery,
    stage: PrewarmStage,
    frame_display: FrameDisplay,
}

/// UI-thread cache and bounded work queue for MRU-adjacent display maps.
#[derive(Default)]
pub(crate) struct DisplayMapPrewarm {
    queue: VecDeque<PrewarmWork>,
    cache: VecDeque<PrewarmedDisplayMap>,
    cache_hits: u64,
    cache_misses: u64,
}

impl DisplayMapPrewarm {
    /// Empty queue/cache.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Cache hit count, used by tests and perf instrumentation.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn cache_hits(&self) -> u64 {
        self.cache_hits
    }

    /// Cache miss count, used by tests and perf instrumentation.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn cache_misses(&self) -> u64 {
        self.cache_misses
    }

    /// Drop all queued and cached work for a document. Used on keystrokes
    /// and buffer mutations so prewarm never races active editing.
    pub(crate) fn cancel_document(&mut self, document: u128) {
        self.queue
            .retain(|work| work.buffer_id.as_uuid().as_u128() != document);
        self.cache.retain(|entry| entry.query.document != document);
    }

    /// Keep only entries built against `rope_revision` for `document`.
    pub(crate) fn invalidate_rope_revision(&mut self, document: u128, rope_revision: u64) {
        self.cache.retain(|entry| {
            entry.query.document != document || entry.query.rope_revision == rope_revision
        });
    }

    /// A fresh decoration result supersedes undecorated and older-decorated
    /// maps for the same document.
    pub(crate) fn invalidate_decoration_revision(
        &mut self,
        document: u128,
        decoration_revision: u64,
    ) {
        self.cache.retain(|entry| {
            entry.query.document != document
                || entry.query.decoration_revision == Some(decoration_revision)
        });
    }

    /// Ensure the MRU-adjacent target set has work queued. Targets not in
    /// `buffers` are evicted so the cache stays close to the focused pane.
    pub(crate) fn refresh_targets(&mut self, buffers: &[BufferId]) {
        self.queue.retain(|work| buffers.contains(&work.buffer_id));
        self.cache.retain(|entry| {
            buffers
                .iter()
                .any(|id| id.as_uuid().as_u128() == entry.query.document)
        });
        for buffer_id in buffers.iter().copied().take(PREWARM_TARGET_BUFFERS) {
            if !self.has_stage_or_work(buffer_id, PrewarmStage::Caret) {
                self.push_work(buffer_id, PrewarmStage::Caret);
            }
        }
    }

    /// Pop one queued item.
    pub(crate) fn pop_work(&mut self) -> Option<PrewarmWork> {
        self.queue.pop_front()
    }

    /// Requeue the next stage for the same buffer.
    pub(crate) fn push_next_stage(&mut self, buffer_id: BufferId, stage: PrewarmStage) {
        if let Some(next) = stage.next() {
            self.push_work(buffer_id, next);
        }
    }

    /// Insert a built projection and evict oldest entries past the bound.
    pub(crate) fn insert(
        &mut self,
        query: PrewarmQuery,
        stage: PrewarmStage,
        frame_display: FrameDisplay,
    ) {
        self.cache.retain(|entry| entry.query != query);
        self.cache.push_back(PrewarmedDisplayMap {
            query,
            stage,
            frame_display,
        });
        while self.cache.len() > PREWARM_MAX_CACHE {
            let _ = self.cache.pop_front();
        }
    }

    /// Return the best cached frame matching `query`.
    pub(crate) fn frame_for_query(
        &mut self,
        query: &PrewarmQuery,
        allow_undecorated: bool,
    ) -> Option<FrameDisplay> {
        let mut decorated_hit: Option<usize> = None;
        let mut fallback_hit: Option<usize> = None;
        for (idx, entry) in self.cache.iter().enumerate() {
            if !entry.matches_projection(query) {
                continue;
            }
            if entry.query.decoration_revision == query.decoration_revision {
                decorated_hit = Some(idx);
                break;
            }
            if allow_undecorated
                && entry.stage.rank() >= PrewarmStage::Viewport.rank()
                && entry.query.decoration_revision.is_none()
            {
                fallback_hit = Some(idx);
            }
        }
        let hit = decorated_hit.or(fallback_hit);
        if let Some(idx) = hit {
            self.cache_hits = self.cache_hits.saturating_add(1);
            return self.cache.get(idx).map(|entry| entry.frame_display.clone());
        }
        self.cache_misses = self.cache_misses.saturating_add(1);
        None
    }

    pub(crate) fn push_work(&mut self, buffer_id: BufferId, stage: PrewarmStage) {
        if self
            .queue
            .iter()
            .any(|work| work.buffer_id == buffer_id && work.stage == stage)
        {
            return;
        }
        if self.queue.len() >= PREWARM_MAX_QUEUE {
            let _ = self.queue.pop_back();
        }
        self.queue.push_back(PrewarmWork { buffer_id, stage });
    }

    fn has_stage_or_work(&self, buffer_id: BufferId, stage: PrewarmStage) -> bool {
        let document = buffer_id.as_uuid().as_u128();
        self.queue
            .iter()
            .any(|work| work.buffer_id == buffer_id && work.stage == stage)
            || self
                .cache
                .iter()
                .any(|entry| entry.query.document == document && entry.stage.rank() >= stage.rank())
    }
}

impl PrewarmedDisplayMap {
    fn matches_projection(&self, query: &PrewarmQuery) -> bool {
        self.query.document == query.document
            && self.query.rope_revision == query.rope_revision
            && self.query.caret_bytes == query.caret_bytes
            && self.query.fold_signature == query.fold_signature
            && self.query.image_reservations_signature == query.image_reservations_signature
            && self.query.wrap_width_dip == query.wrap_width_dip
            && self.query.font_state == query.font_state
    }
}

#[cfg(test)]
mod tests;
