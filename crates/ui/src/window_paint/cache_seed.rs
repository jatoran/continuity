//! Post-resolve cache seeding for the focused pane's paint frame.
//!
//! Extracted from `window_paint.rs::on_paint` so the orchestrator
//! stays under the conventions cap. The single responsibility here is
//! deciding when to install the just-painted frame as
//! [`crate::window::Window::last_painted_frame_display`] and into the
//! per-pane spectator cache so the next paint can either motion-reuse
//! the projection or skip cold rebuilds via spectator-promote.
//!
//! Two exit paths:
//!
//! - Reused/deferred paint → leave the prior cache entries untouched.
//!   The painted frame may be a cold-deferred wrap stub or a
//!   scroll-animation reuse of the prior viewport. In both cases the
//!   current [`crate::display_prewarm_cache::PrewarmQuery`] can describe
//!   geometry newer than the frame actually realizes. The worker's real
//!   frame seeds the caches on the next paint.
//! - Otherwise → seed both caches with the painted pair, plus the
//!   `Decorations` and decoration parse revision that produced it
//!   (so a focus switch's spectator-promote can install matching
//!   `last_painted_decorations` and stay on the tight rebuild path
//!   instead of falling to Cold).
//!
//! Thread ownership: UI thread of one window. Mutates
//! [`crate::window::Window::last_painted_frame_display`],
//! [`crate::window::Window::last_painted_decorations`],
//! [`crate::window::Window::last_painted_decoration_parse_revision`],
//! and the [`crate::window_spectator_cache::SpectatorFrameCache`].

use std::sync::Arc;

use continuity_decorate::Decorations;
use continuity_render::FrameDisplay;

use crate::display_prewarm_cache::PrewarmQuery;
use crate::window::Window;

impl Window {
    /// Seed (or invalidate) the per-window paint caches after a paint
    /// frame has been resolved.
    pub(crate) fn seed_paint_caches_after_resolve(
        &mut self,
        display_query: &PrewarmQuery,
        frame_display: &FrameDisplay,
        decorations_owned: Option<&Arc<Decorations>>,
        current_decoration_parse_revision: Option<u64>,
        should_skip_cache_seed: bool,
    ) {
        if should_skip_cache_seed {
            return;
        }
        // γ — reservation-bearing frames are seeded like any other now
        // that `display_query` carries the image-row-reservation
        // signature; a later paint with a different reservation set
        // misses the cache on `motion_compat_mismatch` rather than
        // reusing stale geometry. (Previously this site cleared the
        // caches whenever reservations were active, which guaranteed a
        // cold walk on every subsequent reservation paint.)
        // P18.5b — detect the partial → full transition at the install
        // site so the trace consumer can verify the background fill
        // landed and the scrollbar geometry refined. Emitted here (not
        // in the worker_hit dispatch) because the worker hit path may
        // be bypassed when paint reuses a cached non-partial frame
        // that supersedes the partial — we want the event on every
        // partial → full transition regardless of how the full frame
        // arrived. `background_us` is unknown at this site (cache_seed
        // doesn't see the worker `build_dur_us`), so the trace field
        // is omitted; consumers correlate against
        // `event:projection_worker_result seq=…` for the worker time.
        if crate::paint_trace::is_trace_enabled() {
            let prev_partial = self
                .last_painted_frame_display
                .as_ref()
                .map(|(_, prev)| prev.row_index())
                .filter(|prev_index| prev_index.is_partial());
            if let Some(prev_index) = prev_partial {
                let new_index = frame_display.row_index();
                if !new_index.is_partial() {
                    let estimated = prev_index.estimated_total_rows();
                    let actual = new_index.display_row_count();
                    let delta = (actual as i64) - (estimated as i64);
                    crate::paint_trace::log_event(
                        "event:row_index_complete_fill",
                        &format!("actual_total_rows={actual} estimated_total_rows={estimated}"),
                    );
                    crate::paint_trace::log_event(
                        "event:scrollbar_geometry_refined",
                        &format!("estimated={estimated} actual={actual} delta={delta}"),
                    );
                }
            }
        }
        self.last_painted_frame_display = Some((display_query.clone(), frame_display.clone()));
        self.last_painted_decorations = decorations_owned.cloned();
        self.last_painted_decoration_parse_revision = current_decoration_parse_revision;
        self.spectator_frame_cache.borrow_mut().insert(
            self.tree.focused,
            display_query.clone(),
            frame_display.clone(),
            decorations_owned.cloned(),
            current_decoration_parse_revision,
        );
        if crate::paint_trace::is_trace_enabled() {
            let stamps = frame_display.row_index().stamps();
            crate::paint_trace::log_event(
                "spectator_cache_populate",
                &format!(
                    "pane_id={:032x} document_id={:032x} rope_rev={} \
                     decoration_rev={} source=paint_epilogue elapsed_us=0",
                    self.tree.focused.0 as u128,
                    display_query.document(),
                    stamps.rope_revision,
                    stamps.decoration_revision,
                ),
            );
        }
    }
}
