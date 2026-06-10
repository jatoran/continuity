//! Idle display-map prewarm for MRU-adjacent tabs.
//!
//! The queue and cache are owned by one [`crate::Window`] on its UI
//! thread. They contain only derived `FrameDisplay` projections, keyed by
//! source/decoration revisions and viewport inputs; the core thread remains
//! the sole writer of buffer state.
//!
//! ## Submodule layout
//!
//! - [`tick`] — trigger detection: the UI-thread timer, idle gate,
//!   MRU-adjacent buffer selection, and big-buffer cap.
//! - [`stage`] — the per-tick warm-cache build step
//!   ([`Window::process_one_display_prewarm_stage`]).
//! - [`projection_inputs`] — projection inputs shared by paint and
//!   prewarm so they agree on what `FrameDisplay::build*` would
//!   produce for a given snapshot. Exports
//!   [`DisplayProjectionMetrics`].
//! - [`frame_build`] — `FrameDisplay::build*` wrappers used by both
//!   paint and prewarm; picks the DirectWrite measurer when the
//!   window has finished render setup and falls back otherwise.
//! - [`row_index_direct`] — direct input-path helpers that refresh a
//!   small row-index line set or materialize from an existing index
//!   without invoking the whole-document walker.
//!
//! Invalidation/lookup live in this root file because they are
//! one-liners that delegate straight into
//! [`crate::display_prewarm_cache`].

use continuity_buffer::BufferId;
use continuity_render::FrameDisplay;

use crate::display_prewarm_cache::PrewarmQuery;
use crate::window::Window;

mod frame_build;
mod frame_build_cold;
mod frame_build_partial;
mod frame_build_splice;
mod frame_build_stats_emit;
pub(crate) mod projection_inputs;
mod row_index_direct;
mod stage;
mod tick;

impl Window {
    /// Cancel queued/cached prewarm for the currently active buffer.
    pub(crate) fn cancel_active_display_prewarm(&mut self) {
        let document = self.buffer_id.as_uuid().as_u128();
        self.display_map_prewarm.cancel_document(document);
    }

    /// Cancel queued/cached prewarm for `buffer_id`.
    pub(crate) fn cancel_display_prewarm_for_buffer(&mut self, buffer_id: BufferId) {
        self.display_map_prewarm
            .cancel_document(buffer_id.as_uuid().as_u128());
    }

    /// Get a cached prewarm frame for paint, if available.
    pub(crate) fn prewarmed_frame_for_query(
        &mut self,
        query: &PrewarmQuery,
        allow_undecorated: bool,
    ) -> Option<FrameDisplay> {
        self.display_map_prewarm
            .frame_for_query(query, allow_undecorated)
    }
}
