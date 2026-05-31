//! Focused-pane mouse hit-test frame cache.
//!
//! Hit-testing may need to build a `FrameDisplay` before the next
//! `WM_PAINT` when the focused pane has no compatible last-painted or
//! spectator frame. This cache keeps that frame, its query, and the
//! decoration context that produced it so the following paint can use
//! the same frame as a rebuild source instead of cold-walking the row
//! index a second time.
//!
//! Thread ownership: UI thread of one window. Stored under
//! `Window::mouse_hit_test_frame_cache`.

use std::sync::Arc;

use continuity_decorate::Decorations;
use continuity_render::FrameDisplay;

use crate::display_prewarm_cache::PrewarmQuery;

/// One hit-test fallback frame and the projection context that built it.
#[derive(Clone)]
pub(crate) struct MouseHitTestFrameCacheEntry {
    query: PrewarmQuery,
    frame_display: FrameDisplay,
    decorations: Option<Arc<Decorations>>,
    parse_revision: Option<u64>,
}

impl MouseHitTestFrameCacheEntry {
    /// Build a cache entry from a freshly-realized hit-test fallback.
    #[must_use]
    pub(crate) fn new(
        query: PrewarmQuery,
        frame_display: FrameDisplay,
        decorations: Option<Arc<Decorations>>,
        parse_revision: Option<u64>,
    ) -> Self {
        Self {
            query,
            frame_display,
            decorations,
            parse_revision,
        }
    }

    /// Projection query the frame was built for.
    #[must_use]
    pub(crate) fn query(&self) -> &PrewarmQuery {
        &self.query
    }

    /// Cached frame-display.
    #[must_use]
    pub(crate) fn frame_display(&self) -> &FrameDisplay {
        &self.frame_display
    }

    /// Decorations snapshot consumed by the cached frame, if any.
    #[must_use]
    pub(crate) fn decorations(&self) -> Option<&Arc<Decorations>> {
        self.decorations.as_ref()
    }

    /// Worker parse revision of `decorations`.
    #[must_use]
    pub(crate) fn parse_revision(&self) -> Option<u64> {
        self.parse_revision
    }
}
