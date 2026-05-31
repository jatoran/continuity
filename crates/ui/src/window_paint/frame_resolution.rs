//! Resolve `FrameDisplay` for the current paint — picks the cached
//! frame to motion-reuse (if any), classifies the projection build
//! plan, polls the projection worker for a stamp-matched result, and
//! falls back to an inline realization when the worker missed.
//!
//! Every trace label this module emits is consumed by perf scripts in
//! `.trash/analyze_trace.py` — the ordering and the label strings are
//! part of the public contract and must not drift.

mod live_resize_reuse;
mod scroll_anim_action;
mod worker_outcome_dispatch;

#[cfg(test)]
mod tests;

use std::ops::Range;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation};
use continuity_render::FrameDisplay;
use ropey::Rope;

use crate::display_prewarm_cache::PrewarmQuery;
use crate::paint_trace::PaintTrace;
use crate::projection_worker::ProjectionStamp;
use crate::window::Window;
use crate::window_paint::VIEWPORT_OVERSCAN_ROWS;
use crate::window_paint_selection_reveal::{
    compute_selection_reveal_dirty_lines, CachedFrameSource,
};
use crate::window_projection_plan::ProjectionBuildKind;

use worker_outcome_dispatch::{WorkerOutcomeDispatchInputs, WorkerOutcomeDispatchOutputs};

fn compute_compatible_last_painted_frame<'a>(
    candidate: Option<(&'a PrewarmQuery, &'a FrameDisplay)>,
    display_query: &PrewarmQuery,
) -> Option<(&'a PrewarmQuery, &'a FrameDisplay)> {
    candidate.and_then(|(query, frame)| {
        query
            .hit_test_compat_mismatch(display_query)
            .is_none()
            .then_some((query, frame))
    })
}

/// Inputs threaded into [`Window::resolve_paint_frame_display`].
pub(crate) struct FrameResolutionInputs<'a> {
    pub rope_for_projection: &'a Rope,
    pub revision_for_projection: u64,
    pub decorations: Option<&'a Decorations>,
    pub caret_bytes_for_projection: &'a [usize],
    pub folds_for_projection: &'a [FoldRange],
    pub image_reservations: &'a [ImageRowReservation],
    pub viewport_rows: Range<u32>,
    pub display_query: &'a PrewarmQuery,
    pub prewarmed_frame_display: Option<FrameDisplay>,
    pub wrap_width_dip: u32,
    pub projection_char_width: f32,
    pub decoration_parse_revision: Option<u64>,
    pub decoration_parse_advanced: bool,
}

/// Outputs from [`Window::resolve_paint_frame_display`].
pub(crate) struct FrameResolutionOutputs {
    pub frame_display: FrameDisplay,
    pub frame_source: &'static str,
    pub worker_miss_reason: Option<&'static str>,
    pub projection_kind: ProjectionBuildKind,
    pub current_projection_stamp: ProjectionStamp,
    pub selection_reveal_dirty: Vec<u32>,
    /// `true` when `frame_display` is a reused/deferred frame that
    /// kept paint off an inline cold/partial path. Callers must NOT
    /// seed `Window::last_painted_frame_display` or the spectator
    /// cache with this frame because the current paint query may
    /// describe a newer viewport than the reused frame realizes. The
    /// next paint after the worker delivers the real frame re-seeds
    /// the caches with current data.
    pub should_skip_cache_seed: bool,
    /// Display-row count synchronously realized by the section-10
    /// scroll-tick strip-realize path. Non-zero only when
    /// `frame_source == "scroll_anim_strip_realize"`. Surfaced through
    /// [`Renderer::set_last_scroll_strip_rows`] for the
    /// `event:scroll_path` trace.
    pub scroll_strip_rows: u32,
}

impl Window {
    pub(crate) fn resolve_paint_frame_display(
        &mut self,
        inputs: FrameResolutionInputs<'_>,
        trace: &PaintTrace,
    ) -> FrameResolutionOutputs {
        let FrameResolutionInputs {
            rope_for_projection,
            revision_for_projection,
            decorations,
            caret_bytes_for_projection,
            folds_for_projection,
            image_reservations,
            viewport_rows,
            display_query,
            mut prewarmed_frame_display,
            wrap_width_dip,
            projection_char_width,
            decoration_parse_revision,
            decoration_parse_advanced,
        } = inputs;

        // Paint-time fast path: when nothing about the projection
        // geometry has changed since the prior paint, reuse that
        // frame's projection without rebuilding. Caret motion alone
        // does *not* invalidate this — the prior paint's caret bytes
        // are ignored via `is_compatible_for_motion`. The trade-off
        // is that markdown markers around the caret line stay revealed
        // (or hidden) for one frame after the caret crosses a span
        // boundary; the next paint catches up with a fresh projection
        // because that paint sees the new caret position and either
        // hits the cache again (caret stayed put) or cold-builds
        // through a revision/decoration change.
        // γ — `motion_compat_mismatch` now compares the
        // image-row-reservation signature, so a stable reservation set
        // (expanded inline image, later multi-line table row) reuses
        // the prior frame and a changed set surfaces as
        // `miss=field_image_reservations_signature`. The old blanket
        // `!image_reservations.is_empty()` bypass cold-walked every
        // paint for any buffer with an expanded image — the latent
        // pre-existing cliff that reverted Phase F.
        let last_paint_candidate = match self.last_painted_frame_display.as_ref() {
            None => {
                crate::paint_trace::log_event("last_painted_frame_display", "miss=cache_empty");
                None
            }
            Some((cached_query, cached_fd)) => {
                match cached_query.motion_compat_mismatch(display_query) {
                    None => Some(cached_fd.clone()),
                    Some(reason) => {
                        crate::paint_trace::log_event(
                            "last_painted_frame_display",
                            &format!("miss=field_{reason}"),
                        );
                        None
                    }
                }
            }
        };
        // After a focus switch into a previously-spectator pane,
        // `last_painted_frame_display` holds the *outgoing* focused
        // buffer's frame and misses on `field_document`. The
        // spectator cache, however, still carries the *incoming*
        // focused pane's last-painted frame (it was a spectator on
        // the prior paint). Reusing it skips the otherwise-required
        // dirty rebuild over thousands of source lines — the manual
        // trace captured a 461 ms inline rebuild after a click into
        // a large spectator (`dirty_spilled dirty_count=7740` with
        // `covers_viewport=false`, so spilling fell back to inline).
        //
        // Critically the promote also installs the spectator's
        // **decorations** and **parse-revision** on `Window` so
        // `classify_projection_build`'s `decoration_advanced` arm
        // has matching prev decorations and emits a tight
        // `Dirty` rebuild instead of falling through to `Cold`
        // (see classify.rs:218 — `last_painted_decorations` mismatch
        // forces Cold otherwise). Without this shadowing the
        // promote-then-Cold pattern was the dominant remaining
        // stall on focus switches (`perf-snapshots/manual-lag_after-coalesce_20260518-002820.tsv`
        // captured 5 cold builds with `hit=spectator_promote`
        // already logged in the same paint).
        //
        // The spectator lookup still checks the full focused-paint
        // compatibility signature, including image-row reservations.
        // Do not blanket-skip reservation-bearing buffers here: the
        // trace from a large-to-large focus switch showed the target
        // content visible as a spectator, then blanking after focus
        // because promotion was disabled before the compatible-cache
        // check could accept that same rendered frame.
        let last_paint_candidate = last_paint_candidate.or_else(|| {
            let cache = self.spectator_frame_cache.borrow();
            let promoted = cache.lookup_for_focused_paint(self.tree.focused, display_query)?;
            let promoted_fd = promoted.frame_display.clone();
            let promoted_decorations = promoted.decorations.clone();
            let promoted_parse_revision = promoted.parse_revision;
            drop(cache);
            // Only shadow `last_painted_decorations` when the
            // promoted frame's rope revision matches the current
            // paint's. With matching ropes the stored decorations'
            // byte ranges remain char-aligned in the current rope —
            // safe to feed into `classify_projection_build`'s
            // `decoration_advanced` diff. When ropes differ
            // (spectator buffer was edited between caching and
            // promote — rare but possible on cross-window save),
            // skip the shadow so classify falls to `Cold` instead of
            // running `diff_dirty_lines` against ranges that might
            // not align after transform. The promoted frame itself
            // is still returned as the candidate — the worst case is
            // classify decides `Cold` and we cold-rebuild as before
            // the promote landed.
            let stamps = promoted_fd.row_index().stamps();
            if stamps.rope_revision == revision_for_projection {
                self.last_painted_decorations = promoted_decorations;
                self.last_painted_decoration_parse_revision = promoted_parse_revision;
                crate::paint_trace::log_event(
                    "last_painted_frame_display",
                    "hit=spectator_promote shadowed=true",
                );
            } else if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event(
                    "last_painted_frame_display",
                    &format!(
                        "hit=spectator_promote shadowed=false prev_rope_rev={} cur_rope_rev={}",
                        stamps.rope_revision, revision_for_projection,
                    ),
                );
            }
            Some(promoted_fd)
        });
        // γ — prewarm cache is keyed without reservations, so it can
        // only serve frames when the reservation set is empty;
        // otherwise the cold-build branch (with `image_reservations`)
        // fires.
        let prewarm_candidate = image_reservations
            .is_empty()
            .then(|| {
                prewarmed_frame_display.take().or_else(|| {
                    self.display_map_prewarm
                        .frame_for_query(display_query, decorations.is_none())
                })
            })
            .flatten();
        let mouse_paint_candidate = if image_reservations.is_empty()
            && last_paint_candidate.is_none()
            && prewarm_candidate.is_none()
        {
            self.mouse_hit_test_paint_candidate(
                display_query,
                revision_for_projection,
                rope_for_projection.len_lines() as u32,
            )
        } else {
            None
        };
        let (cached_frame_display, cached_frame_source) = match last_paint_candidate {
            Some(fd) => (Some(fd), CachedFrameSource::LastPaint),
            None => match prewarm_candidate {
                Some(fd) => (Some(fd), CachedFrameSource::Prewarm),
                None => match mouse_paint_candidate.as_ref() {
                    Some(entry) => (
                        Some(entry.frame_display().clone()),
                        CachedFrameSource::MouseHitTest,
                    ),
                    None => (None, CachedFrameSource::None),
                },
            },
        };
        let last_painted_rebuild_source = compute_compatible_last_painted_frame(
            self.last_painted_frame_display
                .as_ref()
                .map(|(query, frame)| (query, frame)),
            display_query,
        );
        // Selection-only markdown reveal: when the prior painted
        // frame matches the current geometry but was built with a
        // different caret-byte set (the caret moved without a rope
        // edit), the old and new caret source lines — plus any
        // code block whose unit-reveal containment flipped between
        // the two caret sets — need their segments rebuilt before
        // reuse. Without this, clicking a rendered markdown line
        // does not reveal raw markers, and leaving that line does
        // not re-render. Computed once so both the covering-cache
        // path and the dirty/cold rebuild path can union these
        // source lines into their dirty sets.
        //
        // A tab switch can leave `last_painted_frame_display`
        // pointing at the outgoing document. Never diff caret-reveal
        // state or dirty-rebuild against that frame: the document ids
        // differ, so its source lines are meaningless for the
        // incoming buffer.
        let prior_carets_for_reveal =
            if matches!(cached_frame_source, CachedFrameSource::MouseHitTest) {
                mouse_paint_candidate
                    .as_ref()
                    .map(|entry| entry.query().caret_bytes().to_vec())
            } else {
                last_painted_rebuild_source.map(|(query, _)| query.caret_bytes().to_vec())
            };
        let selection_reveal_dirty: Vec<u32> = match prior_carets_for_reveal.as_deref() {
            Some(prior) if prior != caret_bytes_for_projection => {
                compute_selection_reveal_dirty_lines(
                    rope_for_projection,
                    decorations,
                    prior,
                    caret_bytes_for_projection,
                )
            }
            _ => Vec::new(),
        };
        // ε.5c — classify the projection build *once* per paint. The
        // inline realization (worker miss) and the post-paint worker
        // dispatch consume the same kind so they cannot disagree on
        // what frame this paint should produce. Pre-fetch rope deltas
        // against whichever frame the classifier will compare to.
        let rebuild_source_frame_revision = cached_frame_display
            .as_ref()
            .or_else(|| last_painted_rebuild_source.map(|(_, fd)| fd))
            .map(|fd| fd.row_index().stamps().rope_revision);
        let (rope_deltas_for_classify, rope_history_covered) = match rebuild_source_frame_revision {
            Some(prev_rev) if prev_rev < revision_for_projection => {
                self.editor.rope_deltas_since(self.buffer_id, prev_rev)
            }
            _ => (Vec::new(), true),
        };
        let prior_carets_for_trace = prior_carets_for_reveal.unwrap_or_default();
        let last_painted_frame_ref = last_painted_rebuild_source.map(|(_, frame)| frame);
        let projection_kind = crate::window_projection_plan::classify_projection_build(
            crate::window_projection_plan::ProjectionClassifyInputs {
                document: self.buffer_id.as_uuid().as_u128(),
                rope: rope_for_projection,
                revision: revision_for_projection,
                wrap_width_dip,
                current_decorations: decorations,
                last_painted_decorations: self.last_painted_decorations.as_deref(),
                cached_frame: cached_frame_display.as_ref(),
                cached_frame_source,
                last_painted_frame: last_painted_frame_ref,
                viewport_rows: viewport_rows.clone(),
                rope_deltas: &rope_deltas_for_classify,
                rope_history_covered,
                selection_reveal_dirty: &selection_reveal_dirty,
                decoration_parse_advanced,
            },
        );
        // ε.5b — compute the worker stamp for the current paint inputs
        // and poll the worker. A stamp-matched result skips the inline
        // realization entirely.
        //
        // P18.10 — paint never blocks on the worker. Every dispatch
        // path has a fast inline fallback (cache hit replays the
        // cached frame, dirty / splice rebuild incrementally, cold on
        // a small buffer is microseconds, large cold routes through
        // `ColdPartial` which is ~10–25 ms). All are faster than the
        // 8 ms / 500 ms bounded-wait budgets used to be. The initial
        // poll is the only worker interaction — a stamp-matched result
        // is taken when present, otherwise paint falls through to the
        // inline realization immediately.
        let projection_inputs = crate::window_projection_worker::PaintProjectionInputs {
            buffer_id: self.buffer_id,
            rope_revision: revision_for_projection,
            decoration_revision: decorations.map(|d| d.revision),
            decoration_parse_revision,
            caret_bytes: caret_bytes_for_projection,
            folds: folds_for_projection,
            image_reservations,
            wrap_width_dip,
            // Deferred font-swap: while `pending_font_change` is Some,
            // stamp with the *pending* font_state so the worker rebuilds
            // for the new font in the background. The live
            // `prose_font_family`/`text_format` keep painting the prior
            // font against the prior frame_display until
            // `try_apply_pending_font_swap` lands. See `window_font_swap`.
            font_state: self.effective_font_state(),
            viewport_rows: viewport_rows.clone(),
            overscan: VIEWPORT_OVERSCAN_ROWS,
        };
        let current_projection_stamp =
            crate::window_projection_worker::current_projection_stamp(&projection_inputs);
        let worker_has_pending_for_stamp = self.projection_worker.as_ref().is_some_and(|worker| {
            worker.has_pending_target_stamp(self.tree.focused, &current_projection_stamp)
                || worker.has_pending_partial_fill_same_or_older_stamp(&current_projection_stamp)
        });
        let scroll_anim_worker_pending =
            trace.invalidate_reason() == Some("scroll_anim") && worker_has_pending_for_stamp;
        // Fix A — a Ctrl+End / Ctrl+Home (or far reveal) jump armed the
        // off-thread poll; while a matching worker build is in flight,
        // reuse the prior frame + placeholder strip instead of inline-
        // walking the destination on the UI thread.
        let jump_offthread_pending = self.jump_offthread_polls > 0 && worker_has_pending_for_stamp;
        let worker_outcome = crate::window_projection_worker::try_use_worker_result_rich(
            self.projection_worker.as_ref(),
            self.tree.focused,
            &current_projection_stamp,
            image_reservations,
            !self.inited,
        );
        super::projection_stale_trace::log_projection_worker_stale_result(
            &worker_outcome,
            &current_projection_stamp,
        );
        let WorkerOutcomeDispatchOutputs {
            frame_display,
            frame_source,
            worker_miss_reason,
            should_skip_cache_seed,
            scroll_strip_rows,
        } = self.dispatch_worker_outcome_to_frame_display(
            WorkerOutcomeDispatchInputs {
                worker_outcome,
                viewport_rows: viewport_rows.clone(),
                projection_kind: &projection_kind,
                rope_for_projection,
                revision_for_projection,
                decorations,
                caret_bytes_for_projection,
                folds_for_projection,
                image_reservations,
                wrap_width_dip,
                projection_char_width,
                prior_carets_for_trace: &prior_carets_for_trace,
                selection_reveal_dirty: &selection_reveal_dirty,
                display_query,
                scroll_anim_worker_pending,
                jump_offthread_pending,
                current_projection_stamp: &current_projection_stamp,
            },
            trace,
        );
        FrameResolutionOutputs {
            frame_display,
            frame_source,
            worker_miss_reason,
            projection_kind,
            current_projection_stamp,
            selection_reveal_dirty,
            should_skip_cache_seed,
            scroll_strip_rows,
        }
    }
}
