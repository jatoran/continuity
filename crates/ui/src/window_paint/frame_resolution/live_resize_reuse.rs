//! Live shrink-resize reuse for the focused-pane worker-miss path.
//!
//! The normal wrap-width mismatch path must rebuild because old wrap
//! geometry is visibly wrong. During a live shrink tick, though, the
//! compositor can expose the resized client before the current-wrap
//! partial frame is ready. This helper allows one paint to reuse the
//! previous same-rope/same-decoration frame, keeping the target filled
//! while the existing post-paint worker request catches up.
//!
//! Thread ownership: pure helper; the caller clones UI-thread-owned
//! frame candidates before calling.

use continuity_render::FrameDisplay;

use crate::projection_worker::{ProjectionStamp, StampMismatchField};
use crate::window_projection_plan::ProjectionBuildKind;
use crate::window_projection_worker::WorkerMissReason;

/// Inputs for [`live_resize_reuse_frame`].
pub(super) struct LiveResizeReuseInputs<'a> {
    pub candidate: Option<FrameDisplay>,
    pub is_live_resize_shrink_tick: bool,
    pub projection_kind: &'a ProjectionBuildKind,
    pub worker_miss_reason: WorkerMissReason,
    pub image_reservations_empty: bool,
    pub current_projection_stamp: &'a ProjectionStamp,
}

/// Stable skip reasons for `paint:frame_display:live_resize_reuse_skip`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LiveResizeReuseSkip {
    NotLiveShrink,
    UnsupportedProjection,
    UnsupportedWorkerMiss,
    ImageReservations,
    NoCandidate,
    RopeRevisionDrift,
    DecorationRevisionDrift,
    FoldSignatureDrift,
    WrapWidthMatch,
    ViewportNotCovered,
}

impl LiveResizeReuseSkip {
    #[must_use]
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::NotLiveShrink => "not_live_shrink",
            Self::UnsupportedProjection => "unsupported_projection",
            Self::UnsupportedWorkerMiss => "unsupported_worker_miss",
            Self::ImageReservations => "image_reservations",
            Self::NoCandidate => "no_candidate",
            Self::RopeRevisionDrift => "rope_revision_drift",
            Self::DecorationRevisionDrift => "decoration_revision_drift",
            Self::FoldSignatureDrift => "fold_signature_drift",
            Self::WrapWidthMatch => "wrap_width_match",
            Self::ViewportNotCovered => "viewport_not_covered",
        }
    }
}

pub(super) fn live_resize_reuse_frame(
    inputs: LiveResizeReuseInputs<'_>,
) -> Result<FrameDisplay, LiveResizeReuseSkip> {
    if !inputs.is_live_resize_shrink_tick {
        return Err(LiveResizeReuseSkip::NotLiveShrink);
    }
    if !matches!(
        inputs.projection_kind,
        ProjectionBuildKind::ColdPartial { .. }
    ) {
        return Err(LiveResizeReuseSkip::UnsupportedProjection);
    }
    if !matches!(
        inputs.worker_miss_reason,
        WorkerMissReason::NotReady
            | WorkerMissReason::StampMismatch(
                StampMismatchField::Viewport | StampMismatchField::WrapWidth
            )
    ) {
        return Err(LiveResizeReuseSkip::UnsupportedWorkerMiss);
    }
    if !inputs.image_reservations_empty {
        return Err(LiveResizeReuseSkip::ImageReservations);
    }
    let candidate = inputs.candidate.ok_or(LiveResizeReuseSkip::NoCandidate)?;
    let stamps = candidate.row_index().stamps();
    let current = inputs.current_projection_stamp;
    let current_decoration_revision = current.decoration_revision.unwrap_or(current.rope_revision);
    if stamps.rope_revision != current.rope_revision {
        return Err(LiveResizeReuseSkip::RopeRevisionDrift);
    }
    if stamps.decoration_revision != current_decoration_revision {
        return Err(LiveResizeReuseSkip::DecorationRevisionDrift);
    }
    if stamps.fold_signature != current.fold_signature {
        return Err(LiveResizeReuseSkip::FoldSignatureDrift);
    }
    if stamps.wrap_width_dip == current.wrap_width_dip {
        return Err(LiveResizeReuseSkip::WrapWidthMatch);
    }
    let realized = candidate.realized_row_range();
    if realized.start > current.viewport_rows.start || current.viewport_rows.end > realized.end {
        return Err(LiveResizeReuseSkip::ViewportNotCovered);
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_decorate::Decorations;
    use continuity_layout::FontStateId;
    use ropey::Rope;

    fn cold_partial() -> ProjectionBuildKind {
        ProjectionBuildKind::ColdPartial {
            viewport_source_range: 0..20,
            safety_margin: 20,
        }
    }

    fn frame_at(
        rope_revision: u64,
        decoration_revision: Option<u64>,
        wrap_width: u32,
    ) -> FrameDisplay {
        let rope = Rope::from_str("one two three four five six seven\nsame\nagain\n");
        let decorations = decoration_revision.map(Decorations::empty);
        FrameDisplay::build(
            &rope,
            rope_revision,
            decorations.as_ref(),
            &[0],
            wrap_width,
            8.0,
        )
    }

    fn stamp(wrap_width: u32, viewport_rows: std::ops::Range<u32>) -> ProjectionStamp {
        ProjectionStamp {
            document: 1,
            rope_revision: 7,
            decoration_revision: Some(11),
            decoration_parse_revision: Some(11),
            caret_signature: 0,
            fold_signature: 0,
            image_reservations_signature: 0,
            wrap_width_dip: wrap_width,
            font_state: FontStateId(0),
            viewport_rows,
            overscan: 20,
        }
    }

    #[test]
    fn reuses_stale_wrap_frame_for_live_shrink_cold_partial() {
        let current = stamp(480, 0..3);
        let frame = frame_at(7, Some(11), 640);

        assert!(live_resize_reuse_frame(LiveResizeReuseInputs {
            candidate: Some(frame),
            is_live_resize_shrink_tick: true,
            projection_kind: &cold_partial(),
            worker_miss_reason: WorkerMissReason::NotReady,
            image_reservations_empty: true,
            current_projection_stamp: &current,
        })
        .is_ok());
    }

    #[test]
    fn rejects_non_shrink_tick() {
        let current = stamp(480, 0..3);
        let frame = frame_at(7, Some(11), 640);

        assert_eq!(
            live_resize_reuse_frame(LiveResizeReuseInputs {
                candidate: Some(frame),
                is_live_resize_shrink_tick: false,
                projection_kind: &cold_partial(),
                worker_miss_reason: WorkerMissReason::NotReady,
                image_reservations_empty: true,
                current_projection_stamp: &current,
            })
            .err(),
            Some(LiveResizeReuseSkip::NotLiveShrink),
        );
    }

    #[test]
    fn rejects_same_wrap_candidate() {
        let current = stamp(480, 0..3);
        let frame = frame_at(7, Some(11), 480);

        assert_eq!(
            live_resize_reuse_frame(LiveResizeReuseInputs {
                candidate: Some(frame),
                is_live_resize_shrink_tick: true,
                projection_kind: &cold_partial(),
                worker_miss_reason: WorkerMissReason::NotReady,
                image_reservations_empty: true,
                current_projection_stamp: &current,
            })
            .err(),
            Some(LiveResizeReuseSkip::WrapWidthMatch),
        );
    }

    #[test]
    fn rejects_stale_content_or_style() {
        let current = stamp(480, 0..3);
        let stale_rope = frame_at(6, Some(11), 640);
        let stale_decoration = frame_at(7, Some(10), 640);

        assert_eq!(
            live_resize_reuse_frame(LiveResizeReuseInputs {
                candidate: Some(stale_rope),
                is_live_resize_shrink_tick: true,
                projection_kind: &cold_partial(),
                worker_miss_reason: WorkerMissReason::NotReady,
                image_reservations_empty: true,
                current_projection_stamp: &current,
            })
            .err(),
            Some(LiveResizeReuseSkip::RopeRevisionDrift),
        );
        assert_eq!(
            live_resize_reuse_frame(LiveResizeReuseInputs {
                candidate: Some(stale_decoration),
                is_live_resize_shrink_tick: true,
                projection_kind: &cold_partial(),
                worker_miss_reason: WorkerMissReason::NotReady,
                image_reservations_empty: true,
                current_projection_stamp: &current,
            })
            .err(),
            Some(LiveResizeReuseSkip::DecorationRevisionDrift),
        );
    }

    #[test]
    fn rejects_candidate_that_does_not_cover_viewport() {
        let current = stamp(480, 100..120);
        let frame = frame_at(7, Some(11), 640);

        assert_eq!(
            live_resize_reuse_frame(LiveResizeReuseInputs {
                candidate: Some(frame),
                is_live_resize_shrink_tick: true,
                projection_kind: &cold_partial(),
                worker_miss_reason: WorkerMissReason::NotReady,
                image_reservations_empty: true,
                current_projection_stamp: &current,
            })
            .err(),
            Some(LiveResizeReuseSkip::ViewportNotCovered),
        );
    }
}
