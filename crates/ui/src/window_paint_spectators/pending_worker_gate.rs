//! Pending-worker policy for spectator-pane cache misses.
//!
//! Lifted out of `window_paint_spectators.rs` to keep that file under
//! the 600-line cap. The production match arms in
//! [`super::cache_resolve::resolve_main_cache_outcome`] encode this
//! policy directly via match guards on `realized_covers_viewport`,
//! `worker_pending`, and `large_spectator_partial_eligible`;
//! [`compute_spectator_paint_action`] mirrors that policy so the inline
//! tests below can lock it without a `Window` fixture. If the
//! production arms drift, update this helper in the same diff — the
//! inline tests would otherwise pass with a stale policy.

use crate::window_spectator_cache::SpectatorCacheLookup;

/// Tag describing what `build_spectator_pane_data` should do with a
/// spectator pane's lookup result and the gate-eligibility flags.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SpectatorPaintAction {
    /// `Hit` whose realized row range covers the current spectator
    /// viewport — paint the cached frame as-is.
    UseCachedFrame,
    /// `Hit` whose realized row range does not cover the current
    /// viewport (e.g. hover-routed wheel scroll moved the visible
    /// window outside prior realization). Build a current-geometry
    /// partial frame seeded from the cached hit so the scrolled-in
    /// rows are realized this frame. Because the lookup was otherwise
    /// motion-compatible, the built partial can seed the spectator
    /// cache for the next paint.
    BuildRealizedMissPartialAndSeed,
    /// `Stale(_)` or `Empty` with a worker submission already pending
    /// at the current stamp, or a large-spectator stale that should
    /// bypass the cold walker (`wrap_width_dip`, `decoration_revision`,
    /// `image_reservations_signature`, etc.). Build a current-geometry
    /// partial frame and let the next paint pick up the full worker
    /// result from the drain.
    BuildRealtimePartial,
    /// `Stale(_)` or `Empty` with no eligibility for the bounded
    /// partial path — pay the cold row-count walker inline.
    ColdBuild,
}

/// Pure-function view of the cache-outcome decision used by the
/// production match arms in
/// [`super::cache_resolve::resolve_main_cache_outcome`].
#[cfg_attr(not(test), allow(dead_code))]
#[must_use]
pub(crate) fn compute_spectator_paint_action(
    lookup: &SpectatorCacheLookup,
    realized_covers_viewport: bool,
    worker_pending: bool,
    large_spectator_partial_eligible: bool,
) -> SpectatorPaintAction {
    match lookup {
        SpectatorCacheLookup::Hit(_) if realized_covers_viewport => {
            SpectatorPaintAction::UseCachedFrame
        }
        SpectatorCacheLookup::Hit(_) => SpectatorPaintAction::BuildRealizedMissPartialAndSeed,
        SpectatorCacheLookup::Stale(_) if worker_pending || large_spectator_partial_eligible => {
            SpectatorPaintAction::BuildRealtimePartial
        }
        SpectatorCacheLookup::Empty if worker_pending => SpectatorPaintAction::BuildRealtimePartial,
        SpectatorCacheLookup::Stale(_) | SpectatorCacheLookup::Empty => {
            SpectatorPaintAction::ColdBuild
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_render::FrameDisplay;

    fn dummy_frame() -> FrameDisplay {
        FrameDisplay::placeholder_unrealized(1, 0, None, 0, 0)
    }

    #[test]
    fn covered_hit_paints_cached_frame() {
        assert_eq!(
            compute_spectator_paint_action(
                &SpectatorCacheLookup::Hit(dummy_frame()),
                true,
                false,
                false,
            ),
            SpectatorPaintAction::UseCachedFrame,
        );
    }

    #[test]
    fn uncovered_hit_builds_realized_miss_partial() {
        // Hover-routed wheel scroll on a non-focused pane shifts the
        // visible rows outside the cached frame's realized range. Before
        // P18.12l this returned the cached frame and the scrolled-in
        // rows painted blank until a click forced realization.
        assert_eq!(
            compute_spectator_paint_action(
                &SpectatorCacheLookup::Hit(dummy_frame()),
                false,
                false,
                false,
            ),
            SpectatorPaintAction::BuildRealizedMissPartialAndSeed,
        );
    }

    #[test]
    fn uncovered_hit_with_pending_worker_still_builds_partial_from_hit() {
        // Pending-worker doesn't override a realized-coverage miss; the
        // cached hit is still preferred as a partial seed because it's
        // same-document, same-geometry, just narrower than the new
        // viewport.
        assert_eq!(
            compute_spectator_paint_action(
                &SpectatorCacheLookup::Hit(dummy_frame()),
                false,
                true,
                true,
            ),
            SpectatorPaintAction::BuildRealizedMissPartialAndSeed,
        );
    }

    #[test]
    fn stale_with_pending_submission_builds_realtime_partial() {
        // Tree-sitter decoration delivery shifts the
        // `decoration_revision` cache-key field between the populate
        // and the next paint. With a worker submission already pending
        // at the new stamp, paint stays on the bounded partial path
        // instead of cold-walking the document.
        assert_eq!(
            compute_spectator_paint_action(
                &SpectatorCacheLookup::Stale("decoration_revision"),
                false,
                true,
                false,
            ),
            SpectatorPaintAction::BuildRealtimePartial,
        );
    }

    #[test]
    fn stale_with_large_eligible_builds_realtime_partial_without_live_resize() {
        // P18.12l broadened the live-resize bypass to non-live-resize
        // large-spectator stale misses. The trigger trace
        // (`trace_20260523-115508`) showed a hover-routed
        // wheel-scroll producing a 1.28 s `row_count_walker` inside
        // `paint:spectators` from a `decoration_revision` stale large
        // spectator with no pending worker and no live-resize flag.
        assert_eq!(
            compute_spectator_paint_action(
                &SpectatorCacheLookup::Stale("decoration_revision"),
                false,
                false,
                true,
            ),
            SpectatorPaintAction::BuildRealtimePartial,
        );
    }

    #[test]
    fn reservation_stale_with_large_eligible_builds_realtime_partial() {
        // A 2x2 large-buffer trace showed reservation-bearing
        // spectators falling through to a 330 ms `paint_cold` walk
        // when the worker pending stamp did not include the current
        // reservation signature. Large stale reservation drift must
        // take the same bounded partial path as other stale fields.
        assert_eq!(
            compute_spectator_paint_action(
                &SpectatorCacheLookup::Stale("image_reservations_signature"),
                false,
                false,
                true,
            ),
            SpectatorPaintAction::BuildRealtimePartial,
        );
    }

    #[test]
    fn stale_without_pending_or_large_eligible_falls_through_to_cold() {
        assert_eq!(
            compute_spectator_paint_action(
                &SpectatorCacheLookup::Stale("decoration_revision"),
                false,
                false,
                false,
            ),
            SpectatorPaintAction::ColdBuild,
        );
    }

    #[test]
    fn empty_with_pending_submission_builds_realtime_partial() {
        assert_eq!(
            compute_spectator_paint_action(&SpectatorCacheLookup::Empty, false, true, false),
            SpectatorPaintAction::BuildRealtimePartial,
        );
    }

    #[test]
    fn empty_without_pending_falls_through_to_cold() {
        // Legitimate first paint: no cache, no pending worker. The
        // large-eligible flag intentionally does NOT promote empty to
        // partial because there is no same-document seed frame to map
        // the visible rows back to source lines.
        assert_eq!(
            compute_spectator_paint_action(&SpectatorCacheLookup::Empty, false, false, false),
            SpectatorPaintAction::ColdBuild,
        );
        assert_eq!(
            compute_spectator_paint_action(&SpectatorCacheLookup::Empty, false, false, true),
            SpectatorPaintAction::ColdBuild,
        );
    }

    #[test]
    fn rope_revision_stale_with_pending_builds_realtime_partial() {
        // The pending-worker hoist must cover every `Stale(_)` field,
        // not just `decoration_revision`. Lock the `rope_revision`
        // case so a future cache-key extension cannot accidentally
        // reintroduce a walker stall on rope-only staleness.
        assert_eq!(
            compute_spectator_paint_action(
                &SpectatorCacheLookup::Stale("rope_revision"),
                false,
                true,
                false,
            ),
            SpectatorPaintAction::BuildRealtimePartial,
        );
    }
}
