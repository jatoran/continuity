//! Per-spectator cache-outcome resolution.
//!
//! Lifted out of `window_paint_spectators` to keep the orchestrator
//! under the 600-line cap. Holds the wrapping-spectator branch of
//! the main paint loop: motion-compat lookup followed by either a
//! verbatim cached paint, a bounded current-geometry partial, or a
//! cold viewport build. Stub (no-wrap, large) spectators still live
//! in the parent module so the stub-specific cache key stays close
//! to its insert site.
//!
//! Thread ownership: UI thread of one window. Reads
//! `Window::spectator_frame_cache` for the seed-frame lookup; the
//! caller still owns the post-paint cache insert.

use std::ops::Range;

use continuity_buffer::BufferId;
use continuity_decorate::Decorations;
use continuity_display_map::ImageRowReservation;
use continuity_render::FrameDisplay;

use super::log_spectator_cache;
use super::realtime_miss;
use crate::display_prewarm_cache::PrewarmQuery;
use crate::pane_tree::PaneId;
use crate::window::Window;
use crate::window_paint::VIEWPORT_OVERSCAN_ROWS;
use crate::window_projection_plan::realized_covers;
use crate::window_spectator_cache::SpectatorCacheLookup;

/// Inputs for [`resolve_main_cache_outcome`]. Bundles the per-pane
/// projection state so the resolver signature stays callable from
/// inside the parent loop without a giant argument list.
pub(crate) struct SpectatorCacheResolveInputs<'a> {
    pub window: &'a Window,
    pub pane_idx: usize,
    pub pane_id: PaneId,
    pub buffer_id: BufferId,
    pub rope: &'a ropey::Rope,
    pub revision: u64,
    pub decorations: Option<&'a Decorations>,
    pub caret_bytes: &'a [usize],
    pub reservations: &'a [ImageRowReservation],
    pub wrap_width_dip: u32,
    pub projection_char_width: f32,
    pub visible_rows: Range<u32>,
    pub query: &'a PrewarmQuery,
    pub large_spectator_partial_eligible: bool,
    pub worker_pending: bool,
}

/// Outcome returned to the parent loop. `seed_after_paint` controls
/// whether the parent inserts the produced frame back into
/// [`crate::window_spectator_cache::SpectatorFrameCache`]. Stale
/// geometry partials opt out so a viewport-bounded row index never
/// masquerades as a full current-geometry projection. Same-geometry
/// realized-window partials opt in because they extend a compatible
/// cache entry to cover the rows paint needed.
pub(crate) struct SpectatorCacheResolveResult {
    pub frame_display: FrameDisplay,
    pub cache_hit_delta: u32,
    pub cache_miss_delta: u32,
    pub seed_after_paint: bool,
}

/// Resolve one spectator pane's cache outcome to a paintable
/// `FrameDisplay`. Trace tokens emitted here are stable:
///
/// - `hit=true` — cached frame's realized rows cover the current
///   viewport; painted verbatim.
/// - `hit=false miss=realized_miss_partial` — motion-compat matched
///   but the cached frame's realized rows don't cover the current
///   viewport (e.g. scroll or focus return moved the visible window
///   outside the prior realization); rebuild a bounded partial seeded
///   from the cached frame and seed it back into the cache.
/// - `hit=false miss=wrap_width_partial` — `wrap_width_dip` drift on
///   a large pane.
/// - `hit=false miss=placeholder_pending_partial` — a worker
///   submission is pending at the current stamp.
/// - `hit=false miss=live_resize_partial` — large stale during live
///   resize.
/// - `hit=false miss=stale_partial` — large stale (any field)
///   outside live resize, e.g. a `decoration_revision` drift during
///   scroll or an `image_reservations_signature` drift after image /
///   table row reservations change.
/// - `hit=false miss=stale` / `miss=empty` — small-buffer fallback
///   that still pays the full viewport cold build.
pub(crate) fn resolve_main_cache_outcome(
    inputs: SpectatorCacheResolveInputs<'_>,
    cache_outcome: SpectatorCacheLookup,
) -> SpectatorCacheResolveResult {
    let SpectatorCacheResolveInputs {
        window,
        pane_idx,
        pane_id,
        buffer_id,
        rope,
        revision,
        decorations,
        caret_bytes,
        reservations,
        wrap_width_dip,
        projection_char_width,
        visible_rows,
        query,
        large_spectator_partial_eligible,
        worker_pending,
    } = inputs;
    let viewport_payload = |extra: &str| {
        if extra.is_empty() {
            format!(
                "source_lines={} viewport={}..{}",
                rope.len_lines(),
                visible_rows.start,
                visible_rows.end,
            )
        } else {
            format!(
                "{} source_lines={} viewport={}..{}",
                extra,
                rope.len_lines(),
                visible_rows.start,
                visible_rows.end,
            )
        }
    };
    let build_partial = |seed: Option<&FrameDisplay>| {
        realtime_miss::build_spectator_viewport_partial(
            window,
            rope,
            revision,
            decorations,
            caret_bytes,
            &[],
            reservations,
            wrap_width_dip,
            projection_char_width.max(1.0),
            visible_rows.clone(),
            seed,
        )
    };
    let cold_full = || {
        window.build_frame_display_viewport_cached(
            Some(buffer_id),
            rope,
            revision,
            decorations,
            caret_bytes,
            &[],
            reservations,
            wrap_width_dip,
            projection_char_width.max(1.0),
            visible_rows.clone(),
            VIEWPORT_OVERSCAN_ROWS,
            continuity_display_map::WalkerCallReason::PaintCold,
        )
    };
    let seeded_partial = || {
        let seed_frame: Option<FrameDisplay> = window
            .spectator_frame_cache
            .borrow()
            .lookup_same_document(pane_id, query);
        build_partial(seed_frame.as_ref())
    };
    let hit =
        |delta_hit, delta_miss, seed_after_paint, frame_display| SpectatorCacheResolveResult {
            frame_display,
            cache_hit_delta: delta_hit,
            cache_miss_delta: delta_miss,
            seed_after_paint,
        };
    match cache_outcome {
        SpectatorCacheLookup::Hit(fd) => {
            let realized = fd.realized_row_range();
            if realized_covers(realized.clone(), &visible_rows) {
                log_spectator_cache(
                    pane_idx,
                    "hit=true",
                    &viewport_payload(&format!("realized={}..{}", realized.start, realized.end)),
                );
                hit(1, 0, true, fd)
            } else {
                log_spectator_cache(
                    pane_idx,
                    "hit=false miss=realized_miss_partial",
                    &viewport_payload(&format!("realized={}..{}", realized.start, realized.end)),
                );
                let built = build_partial(Some(&fd));
                hit(0, 1, true, built)
            }
        }
        SpectatorCacheLookup::Stale("wrap_width_dip") if large_spectator_partial_eligible => {
            log_spectator_cache(
                pane_idx,
                "hit=false miss=wrap_width_partial",
                &viewport_payload("stale_field=wrap_width_dip"),
            );
            hit(0, 1, false, seeded_partial())
        }
        SpectatorCacheLookup::Stale(reason) if worker_pending => {
            log_spectator_cache(
                pane_idx,
                "hit=false miss=placeholder_pending_partial",
                &viewport_payload(&format!("stale_field={}", reason)),
            );
            hit(0, 1, false, seeded_partial())
        }
        SpectatorCacheLookup::Stale(reason)
            if large_spectator_partial_eligible && window.is_live_resizing =>
        {
            log_spectator_cache(
                pane_idx,
                "hit=false miss=live_resize_partial",
                &viewport_payload(&format!("stale_field={}", reason)),
            );
            hit(0, 1, false, seeded_partial())
        }
        SpectatorCacheLookup::Stale(reason) if large_spectator_partial_eligible => {
            // Non-live-resize large-spectator stale (e.g.
            // `decoration_revision` drift during scroll). Without this
            // arm the cold walker walked the full document and
            // produced ~1 s `row_count_walker` stalls inside
            // `paint:spectators`.
            log_spectator_cache(
                pane_idx,
                "hit=false miss=stale_partial",
                &viewport_payload(&format!("stale_field={}", reason)),
            );
            hit(0, 1, false, seeded_partial())
        }
        SpectatorCacheLookup::Stale(reason) => {
            log_spectator_cache(
                pane_idx,
                "hit=false miss=stale",
                &viewport_payload(&format!("stale_field={}", reason)),
            );
            hit(0, 1, true, cold_full())
        }
        SpectatorCacheLookup::Empty if worker_pending => {
            log_spectator_cache(
                pane_idx,
                "hit=false miss=placeholder_pending_partial",
                &viewport_payload(""),
            );
            hit(0, 1, false, seeded_partial())
        }
        SpectatorCacheLookup::Empty => {
            log_spectator_cache(pane_idx, "hit=false miss=empty", &viewport_payload(""));
            hit(0, 1, true, cold_full())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    #[test]
    fn realized_covers_viewport_is_a_hit() {
        // Sanity check on the gate used by the Hit arm. The shipping
        // resolver depends on `realized_covers` so a regression here
        // would re-introduce the spectator-scroll blank.
        let realized = 0..50;
        let viewport = 10..40;
        assert!(realized_covers(realized, &viewport));
    }

    #[test]
    fn realized_short_of_viewport_misses_coverage() {
        let realized = 0..20;
        let viewport = 10..40;
        assert!(!realized_covers(realized, &viewport));
    }

    #[test]
    fn realized_starts_after_viewport_misses_coverage() {
        let realized = 20..60;
        let viewport = 10..40;
        assert!(!realized_covers(realized, &viewport));
    }

    #[test]
    fn realized_miss_partial_seeds_from_hit_frame() {
        // Touch `compute_spectator_viewport_source_range` through the
        // realtime-miss helper so that a Hit-with-no-coverage produces a
        // partial source range bounded by the seed frame's mapping.
        // The shipping resolver hands `Some(&fd)` to `build_partial`;
        // this lock keeps the seed propagation intact.
        let rope = Rope::from_str("aaaa bbbb cccc dddd eeee ffff\nshort\n");
        let seed = FrameDisplay::build(&rope, 1, None, &[0], 40, 8.0);
        let range = realtime_miss::compute_spectator_viewport_source_range(
            Some(&seed),
            1..2,
            rope.len_lines(),
        );
        assert!(range.start < range.end);
    }
}
