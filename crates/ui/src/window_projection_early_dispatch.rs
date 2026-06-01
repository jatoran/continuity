//! Early projection-worker dispatch immediately after a buffer edit lands.
//!
//! The post-paint dispatch in `window_paint.rs` only gives the worker
//! a head start *after* the current frame has finished. The next
//! `WM_PAINT` can still arrive before the worker produces the new
//! revision, causing `stamp_mismatch_rope_revision` misses. This module
//! hooks the edit-completion paths
//! (`Window::dispatch_selection_edit`, `insert_text_at_selections`,
//! `delete_back_at_selections`, `delete_forward_at_selections`) and
//! submits a projection request for the just-landed revision before
//! the next paint runs.
//!
//! **Same-contract guarantee.** Early dispatch builds its
//! [`PaintProjectionInputs`] and runs them through the same
//! [`classify_projection_build`] paint uses, so the worker can never
//! be asked to build something paint wouldn't have. Identical stamps
//! are deduplicated so a burst of edit hooks plus the post-paint
//! dispatch only ever submits one request per stamp.
//!
//! **Non-mutating.** Early dispatch does not touch
//! `prewarmed_frame_display`, `last_painted_frame_display`,
//! `last_painted_decorations`, scroll state, selections, or
//! decorations. It reads UI-thread state and submits one fire-and-
//! forget request.
//!
//! Thread ownership: UI thread of one window.

use crate::display_prewarm_cache::PrewarmQuery;
use crate::projection_worker::{ProjectionPlan, ProjectionStamp};
use crate::window::Window;
use crate::window_paint::{visible_display_row_range, VIEWPORT_OVERSCAN_ROWS};
use crate::window_paint_selection_reveal::{
    compute_selection_reveal_dirty_lines, CachedFrameSource,
};
use crate::window_projection_plan::{
    classify_projection_build, worker_plan_label, ProjectionBuildKind, ProjectionClassifyInputs,
};
use crate::window_projection_worker::{
    build_projection_request, current_projection_stamp, PaintProjectionInputs,
};

mod focus_prewarm;
mod previous_frame;

/// Why an early-dispatch attempt did not actually submit a request.
///
/// Stable strings are exposed through [`Self::as_str`] so the trace
/// event carries a stable grep target.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum EarlyDispatchSkip {
    /// Worker has not been spawned yet (no `text_format` at hook time).
    WorkerAbsent,
    /// Renderer not initialized — `compute_focused_pane_image_reservations`
    /// would return empty but other inputs (body rect) may be unsafe.
    RendererAbsent,
    /// DirectWrite text format missing — paint cannot build either.
    TextFormatAbsent,
    /// Core thread has no snapshot for this buffer (during shutdown
    /// races, or before the first buffer adopt).
    NoSnapshot,
    /// Classifier said the next paint will hit a covering cache; the
    /// worker would only duplicate the cached frame.
    CacheHit,
    /// This exact stamp was already submitted by a prior early
    /// dispatch (back-to-back edits in the same paint cycle, or the
    /// motion-timer firing redundantly).
    Dedupe,
    /// `try_send` on the worker channel reported the bounded queue is
    /// full. Worker is far behind; the next submit reseeds the target.
    SubmitFailed,
    /// Edit-burst coalescing: another edit since the last
    /// `WM_PAINT` already triggered an early dispatch this paint
    /// cycle. Skipping keeps the UI thread free for the next paint
    /// — the worker's latest-wins channel means a missed dispatch
    /// only means the next paint builds inline. Counts as evidence
    /// for the "input burst starves paint" hypothesis (the gate
    /// converts it into a measured intervention).
    EditBurstCoalesce,
}

impl EarlyDispatchSkip {
    /// Stable trace spelling matched in tests and grep'd in dev logs.
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::WorkerAbsent => "skip_worker_absent",
            Self::RendererAbsent => "skip_renderer_absent",
            Self::TextFormatAbsent => "skip_text_format_absent",
            Self::NoSnapshot => "skip_no_snapshot",
            Self::CacheHit => "skip_cache_hit",
            Self::Dedupe => "skip_dedupe",
            Self::SubmitFailed => "skip_submit_failed",
            Self::EditBurstCoalesce => "skip_edit_burst_coalesce",
        }
    }
}

/// Outcome of an early-dispatch attempt. Trace tags + tests consume
/// this; production code on the call site ignores the value.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum EarlyDispatchOutcome {
    /// A request was placed on the worker channel for `plan_label`.
    Submitted {
        /// `cold` / `dirty` / `splice` (matches
        /// [`worker_plan_label`]).
        plan_label: &'static str,
    },
    /// Nothing was submitted; carries the precondition / coalescing
    /// reason for the skip.
    Skipped(EarlyDispatchSkip),
}

/// Decide whether to submit `plan` for `stamp`, given the most-recent
/// previously-submitted stamp. Pure; tested without a real `Window`.
///
/// The early-dispatch helper has already run all the per-paint input
/// gathering at this point — this function exists so the dedupe and
/// cache-hit short-circuit can be exercised without DirectWrite.
pub(crate) fn decide_early_dispatch_action(
    projection_kind: &ProjectionBuildKind,
    stamp: &ProjectionStamp,
    last_stamp: Option<&ProjectionStamp>,
) -> Result<ProjectionPlan, EarlyDispatchSkip> {
    let Some(plan) = projection_kind.to_worker_plan() else {
        return Err(EarlyDispatchSkip::CacheHit);
    };
    if last_stamp == Some(stamp) {
        return Err(EarlyDispatchSkip::Dedupe);
    }
    Ok(plan)
}

impl Window {
    /// Edit-burst coalescing gate around
    /// [`Self::try_dispatch_projection_worker_early`]. Only the
    /// *first* edit in a paint cycle pays for the synchronous
    /// input-gathering inside the helper; the remaining edits in a
    /// burst record a `skip_edit_burst_coalesce` event and let the
    /// next paint do the work inline. See `selection_dispatch.rs`
    /// for the rationale citation.
    pub(crate) fn maybe_dispatch_projection_worker_early(
        &mut self,
        is_first_edit_since_paint: bool,
        reason: &'static str,
    ) -> EarlyDispatchOutcome {
        let _scope = crate::paint_trace::is_trace_enabled().then(|| {
            crate::paint_trace::EventScope::with_detail(
                "projection_worker_early_dispatch_total",
                format!("reason={reason} first_edit_since_paint={is_first_edit_since_paint}",),
            )
        });
        if !is_first_edit_since_paint {
            return log_early_dispatch(reason, "skip", EarlyDispatchSkip::EditBurstCoalesce, 0);
        }
        // Edits use the original `early_dispatch` submit category.
        self.try_dispatch_projection_worker_early(reason, "early_dispatch")
    }

    /// ε.5e — submit a projection request for the just-landed buffer
    /// revision before the next `WM_PAINT`. Latest-wins via the
    /// worker's coalescing recv; fire-and-forget — never blocks the
    /// UI thread. Called from edit-completion paths via
    /// [`Self::maybe_dispatch_projection_worker_early`] (which adds
    /// the edit-burst coalescing gate), and from
    /// P0.8.2 layout-change / focus-change hook sites directly.
    ///
    /// `reason` is the fine-grained call-site label that surfaces in
    /// `event:projection_worker_early_dispatch reason=…`. `submit_reason`
    /// is the coarse category routed through to
    /// [`ProjectionWorker::submit_with_reason`] — one of
    /// `early_dispatch` (edit hooks), `layout_change` (pane / split /
    /// grid / resize / WM_SIZE), or `focus_change` (tab switch / pane
    /// focus / file open). Trace consumers grep
    /// `event:projection_worker_queue_depth reason=<submit_reason>`
    /// for the category breakdown.
    pub(crate) fn try_dispatch_projection_worker_early(
        &mut self,
        reason: &'static str,
        submit_reason: &'static str,
    ) -> EarlyDispatchOutcome {
        self.try_dispatch_projection_worker_early_with_viewport(reason, submit_reason, None)
    }

    /// Variant of [`Self::try_dispatch_projection_worker_early`] that
    /// optionally overrides the viewport-rows range used to build the
    /// projection request. P0.8.4 scroll-landing prewarm calls this
    /// with the *projected* landing viewport (current scroll_y +
    /// velocity * tau) so the worker can start realising rows the
    /// inertia is about to expose, before the scroll-tick paint cycles
    /// catch up. `viewport_rows_override = None` matches the existing
    /// early-dispatch behaviour exactly.
    pub(crate) fn try_dispatch_projection_worker_early_with_viewport(
        &mut self,
        reason: &'static str,
        submit_reason: &'static str,
        viewport_rows_override: Option<std::ops::Range<u32>>,
    ) -> EarlyDispatchOutcome {
        // Preconditions — every one matches a paint-time guard. Skip
        // quietly when paint itself would not yet have a worker to
        // dispatch to.
        if self.projection_worker.is_none() {
            return log_early_dispatch(reason, "absent", EarlyDispatchSkip::WorkerAbsent, 0);
        }
        if self.renderer.is_none() {
            return log_early_dispatch(reason, "absent", EarlyDispatchSkip::RendererAbsent, 0);
        }
        if self.text_format.is_none() {
            return log_early_dispatch(reason, "absent", EarlyDispatchSkip::TextFormatAbsent, 0);
        }
        let snap = {
            let _s = crate::paint_trace::is_trace_enabled()
                .then(|| crate::paint_trace::EventScope::new("early_dispatch_snapshot"));
            self.editor.snapshot(self.buffer_id)
        };
        let Some(snap) = snap else {
            return log_early_dispatch(reason, "absent", EarlyDispatchSkip::NoSnapshot, 0);
        };

        let rope = snap.rope_snapshot().rope();
        let revision = snap.rope_snapshot().revision().0;
        let caret_bytes = Self::caret_bytes_for_projection(rope, snap.selections());

        // Decoration snapshot + stale-byte-range transform. Mirrors
        // `decorations_owned` construction in `on_paint`; both paths
        // produce the same `Arc<Decorations>` for the same inputs.
        let decoration_id = self.buffer_id.as_uuid().as_u128();
        // Worker parse revision sampled BEFORE `transformed_through`
        // overwrites the revision label. See the matching site in
        // `on_paint` and `Window::last_painted_decoration_parse_revision`
        // for the full rationale.
        let current_decoration_parse_revision: Option<u64> =
            self.decoration_cache.get(decoration_id).map(|d| d.revision);
        let decorations_owned: Option<std::sync::Arc<continuity_decorate::Decorations>> = {
            let _s = crate::paint_trace::is_trace_enabled().then(|| {
                crate::paint_trace::EventScope::new("early_dispatch_decoration_transform")
            });
            match self.decoration_cache.get_arc(decoration_id).cloned() {
                None => None,
                Some(d) if d.revision == revision => Some(d),
                Some(d) => {
                    let (deltas, covered) =
                        self.editor.rope_deltas_since(self.buffer_id, d.revision);
                    if !covered {
                        None
                    } else if deltas.is_empty() {
                        Some(d)
                    } else {
                        Some(std::sync::Arc::new(
                            d.transformed_through(&deltas, revision),
                        ))
                    }
                }
            }
        };
        let decorations = decorations_owned.as_deref();

        let heading_lines = {
            let _s = crate::paint_trace::is_trace_enabled()
                .then(|| crate::paint_trace::EventScope::new("early_dispatch_heading_lines"));
            self.cached_heading_lines_for_projection(self.buffer_id, rope, revision, decorations)
        };
        let folds = {
            let _s = crate::paint_trace::is_trace_enabled()
                .then(|| crate::paint_trace::EventScope::new("early_dispatch_folds"));
            self.display_projection_folds(rope, &heading_lines, &caret_bytes)
        };
        let body_rect = self.focused_body_rect();
        let image_reservations = {
            let _s = crate::paint_trace::is_trace_enabled()
                .then(|| crate::paint_trace::EventScope::new("early_dispatch_image_reservations"));
            self.compute_focused_pane_image_reservations(
                decorations,
                rope,
                self.effective_line_height(),
                body_rect.w.max(1.0),
            )
        };
        let projection_metrics =
            self.display_projection_metrics(self.current_search_minimap_active(), rope.len_lines());
        let line_height = self.effective_line_height();
        let viewport_rows = viewport_rows_override.unwrap_or_else(|| {
            visible_display_row_range(
                self.view.scroll_y_dip,
                self.view.viewport_height_dip,
                line_height,
            )
        });

        let projection_inputs = PaintProjectionInputs {
            buffer_id: self.buffer_id,
            rope_revision: revision,
            decoration_revision: decorations.map(|d| d.revision),
            decoration_parse_revision: current_decoration_parse_revision,
            caret_bytes: &caret_bytes,
            folds: &folds,
            image_reservations: &image_reservations,
            wrap_width_dip: projection_metrics.wrap_width_dip,
            // Deferred font-swap (see `window_font_swap`): stamp early
            // dispatch with the pending target font_state so the worker
            // starts building for the new font right away.
            font_state: self.effective_font_state(),
            viewport_rows: viewport_rows.clone(),
            overscan: VIEWPORT_OVERSCAN_ROWS,
        };
        let stamp = current_projection_stamp(&projection_inputs);
        let display_query = PrewarmQuery::new(
            self.buffer_id,
            revision,
            decorations.map(|d| d.revision),
            &caret_bytes,
            &folds,
            projection_metrics.wrap_width_dip,
            self.font_state,
        )
        .with_image_reservations(&image_reservations);

        let last_painted_frame = previous_frame::compatible_last_painted_frame(
            self.last_painted_frame_display.as_ref(),
            &display_query,
        );
        let last_painted_frame_ref = last_painted_frame.map(|(_, frame)| frame);
        let rebuild_source_frame_revision =
            last_painted_frame_ref.map(|fd| fd.row_index().stamps().rope_revision);
        let (rope_deltas, rope_history_covered) = match rebuild_source_frame_revision {
            Some(prev_rev) if prev_rev < revision => {
                self.editor.rope_deltas_since(self.buffer_id, prev_rev)
            }
            _ => (Vec::new(), true),
        };
        let selection_reveal_dirty: Vec<u32> =
            match last_painted_frame.map(|(q, _)| q.caret_bytes()) {
                Some(prior) if prior != caret_bytes.as_slice() => {
                    compute_selection_reveal_dirty_lines(rope, decorations, prior, &caret_bytes)
                }
                _ => Vec::new(),
            };
        let _classify_scope = crate::paint_trace::is_trace_enabled()
            .then(|| crate::paint_trace::EventScope::new("early_dispatch_classify"));
        let projection_kind = classify_projection_build(ProjectionClassifyInputs {
            document: self.buffer_id.as_uuid().as_u128(),
            rope,
            revision,
            wrap_width_dip: projection_metrics.wrap_width_dip,
            current_decorations: decorations,
            last_painted_decorations: self.last_painted_decorations.as_deref(),
            cached_frame: None,
            cached_frame_source: CachedFrameSource::None,
            last_painted_frame: last_painted_frame_ref,
            viewport_rows: viewport_rows.clone(),
            rope_deltas: &rope_deltas,
            rope_history_covered,
            selection_reveal_dirty: &selection_reveal_dirty,
            // Early dispatch tracks the same parse-revision flag as
            // paint so a worker submission triggered by a fresh parse
            // delivery doesn't get short-circuited to `CacheHit`.
            decoration_parse_advanced: current_decoration_parse_revision
                != self.last_painted_decoration_parse_revision,
        });

        let plan = match focus_prewarm::plan_for_focus_change(&projection_kind, submit_reason) {
            Some(_) if self.last_early_dispatch_stamp.as_ref() == Some(&stamp) => {
                return log_early_dispatch(reason, "skip", EarlyDispatchSkip::Dedupe, revision);
            }
            Some(plan) => plan,
            None => match decide_early_dispatch_action(
                &projection_kind,
                &stamp,
                self.last_early_dispatch_stamp.as_ref(),
            ) {
                Ok(plan) => plan,
                Err(skip) => return log_early_dispatch(reason, "skip", skip, revision),
            },
        };
        drop(_classify_scope);
        let plan_label = worker_plan_label(&plan);

        let seq = self.next_projection_request_seq();
        let suppressed_table_blocks = self.compute_suppressed_table_blocks();
        let request = {
            let _s = crate::paint_trace::is_trace_enabled()
                .then(|| crate::paint_trace::EventScope::new("early_dispatch_build_request"));
            build_projection_request(
                seq,
                self.tree.focused,
                stamp.clone(),
                rope,
                decorations_owned.clone(),
                &caret_bytes,
                &folds,
                &image_reservations,
                &suppressed_table_blocks,
                projection_metrics.char_width_dip,
                self.projection_font_metrics(),
                plan,
            )
        };
        let worker = self
            .projection_worker
            .as_ref()
            .expect("invariant: worker presence checked above");
        let _submit_scope = crate::paint_trace::is_trace_enabled()
            .then(|| crate::paint_trace::EventScope::new("early_dispatch_worker_submit"));
        let submitted = worker.submit_with_reason(request, submit_reason);
        if !submitted {
            return log_early_dispatch(reason, "skip", EarlyDispatchSkip::SubmitFailed, revision);
        }
        self.last_early_dispatch_stamp = Some(stamp);
        if crate::paint_trace::is_trace_enabled() {
            let detail = format!(
                "reason={reason} submitted=true plan={plan_label} stamp_rev={revision} seq={seq}",
            );
            crate::paint_trace::log_event("event:projection_worker_early_dispatch", &detail);
        }
        EarlyDispatchOutcome::Submitted { plan_label }
    }
}

fn log_early_dispatch(
    reason: &'static str,
    outcome_tag: &'static str,
    skip: EarlyDispatchSkip,
    stamp_rev: u64,
) -> EarlyDispatchOutcome {
    if crate::paint_trace::is_trace_enabled() {
        let detail = format!(
            "reason={reason} submitted=false plan={outcome_tag} stamp_rev={stamp_rev} skip={}",
            skip.as_str(),
        );
        crate::paint_trace::log_event("event:projection_worker_early_dispatch", &detail);
    }
    EarlyDispatchOutcome::Skipped(skip)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use continuity_buffer::BufferId;
    use continuity_display_map::wrap::FixedCharWidth;
    use continuity_display_map::{FoldRange, ImageRowReservation, RowSplice};
    use continuity_layout::FontStateId;
    use continuity_render::FrameDisplay;
    use ropey::Rope;

    use crate::projection_worker::ProjectionStamp;

    fn make_stamp(revision: u64) -> ProjectionStamp {
        let caret: Vec<usize> = vec![0];
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        ProjectionStamp {
            document: 0,
            rope_revision: revision,
            decoration_revision: None,
            decoration_parse_revision: None,
            caret_signature: ProjectionStamp::caret_signature(&caret),
            fold_signature: ProjectionStamp::fold_signature(&folds),
            image_reservations_signature: ProjectionStamp::image_reservations_signature(
                &reservations,
            ),
            wrap_width_dip: 0,
            font_state: FontStateId::default(),
            viewport_rows: 0..8,
            overscan: 20,
        }
    }

    fn tiny_frame_display(revision: u64) -> FrameDisplay {
        let rope = Rope::from_str("a\nb\nc\n");
        let mut measure = FixedCharWidth::new(8.0);
        FrameDisplay::build_viewport_measured(
            &rope,
            revision,
            None,
            &[0usize],
            &[],
            &[],
            0,
            &mut measure,
            0..3,
            0,
        )
    }

    #[test]
    fn decide_submits_cold_plan_when_reservations_present() {
        let stamp = make_stamp(1);
        let kind = ProjectionBuildKind::Cold;
        let plan = decide_early_dispatch_action(&kind, &stamp, None)
            .expect("image reservations ride through the worker request");
        assert!(matches!(plan, ProjectionPlan::Cold));
    }

    #[test]
    fn decide_returns_cache_hit_when_classifier_returns_cache_hit() {
        let frame = tiny_frame_display(1);
        let stamp = make_stamp(1);
        let kind = ProjectionBuildKind::CacheHit(frame);
        let outcome = decide_early_dispatch_action(&kind, &stamp, None);
        assert_eq!(outcome.err(), Some(EarlyDispatchSkip::CacheHit));
    }

    #[test]
    fn decide_returns_dedupe_when_stamp_matches_last() {
        let stamp = make_stamp(7);
        let prev_frame = tiny_frame_display(6);
        let kind = ProjectionBuildKind::Dirty {
            prev: prev_frame,
            dirty: vec![0u32],
        };
        let last = stamp.clone();
        let outcome = decide_early_dispatch_action(&kind, &stamp, Some(&last));
        assert_eq!(outcome.err(), Some(EarlyDispatchSkip::Dedupe));
    }

    #[test]
    fn decide_submits_dirty_plan_when_classifier_returns_dirty() {
        let stamp = make_stamp(2);
        let prev_frame = tiny_frame_display(1);
        let kind = ProjectionBuildKind::Dirty {
            prev: prev_frame,
            dirty: vec![3u32, 7u32],
        };
        let plan =
            decide_early_dispatch_action(&kind, &stamp, None).expect("dirty kind must submit");
        match plan {
            ProjectionPlan::Dirty { dirty, .. } => {
                assert_eq!(&*dirty, &[3u32, 7u32]);
            }
            other => panic!(
                "expected Dirty plan, got {:?}",
                core::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn decide_submits_splice_plan_when_classifier_returns_splice() {
        let stamp = make_stamp(2);
        let prev_frame = tiny_frame_display(1);
        let splice = RowSplice {
            at: 1,
            removed: 1,
            inserted: 2,
            dirty: vec![1, 2],
        };
        let kind = ProjectionBuildKind::Splice {
            prev: prev_frame,
            splice,
            deltas: Arc::from(Vec::<continuity_text::RopeEditDelta>::new()),
        };
        let plan =
            decide_early_dispatch_action(&kind, &stamp, None).expect("splice kind must submit");
        assert!(matches!(plan, ProjectionPlan::Splice { .. }));
    }

    #[test]
    fn decide_submits_cold_plan_when_classifier_returns_cold() {
        let stamp = make_stamp(1);
        let kind = ProjectionBuildKind::Cold;
        let plan =
            decide_early_dispatch_action(&kind, &stamp, None).expect("cold kind must submit");
        assert!(matches!(plan, ProjectionPlan::Cold));
    }

    #[test]
    fn decide_does_not_dedupe_when_revision_changed() {
        let stamp = make_stamp(2);
        let last = make_stamp(1);
        let kind = ProjectionBuildKind::Cold;
        let plan = decide_early_dispatch_action(&kind, &stamp, Some(&last))
            .expect("different revision must submit");
        assert!(matches!(plan, ProjectionPlan::Cold));
    }

    #[test]
    fn skip_reason_strings_are_stable() {
        // Trace string format is part of the diagnostics contract;
        // dev logs and follow-up roadmap items grep these.
        assert_eq!(
            EarlyDispatchSkip::WorkerAbsent.as_str(),
            "skip_worker_absent"
        );
        assert_eq!(EarlyDispatchSkip::CacheHit.as_str(), "skip_cache_hit");
        assert_eq!(EarlyDispatchSkip::Dedupe.as_str(), "skip_dedupe");
        assert_eq!(
            EarlyDispatchSkip::EditBurstCoalesce.as_str(),
            "skip_edit_burst_coalesce"
        );
        // Silence unused-import warnings when fields stay private.
        let _ = Arc::<u8>::new(0);
        let _: BufferId = BufferId::new();
    }
}
