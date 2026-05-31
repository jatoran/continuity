//! Focus-change full-index prewarm policy.
//!
//! Thread ownership: UI thread of one window. This module is pure over
//! the already-classified projection kind.

use crate::projection_worker::ProjectionPlan;
use crate::window_projection_plan::ProjectionBuildKind;

pub(super) fn plan_for_focus_change(
    projection_kind: &ProjectionBuildKind,
    submit_reason: &'static str,
) -> Option<ProjectionPlan> {
    if submit_reason != "focus_change" {
        return None;
    }
    match projection_kind {
        ProjectionBuildKind::CacheHit(frame) if frame.row_index().is_partial() => {
            Some(ProjectionPlan::Cold)
        }
        ProjectionBuildKind::ColdPartial { .. }
        | ProjectionBuildKind::DirtyPartial { .. }
        | ProjectionBuildKind::SplicePartial { .. } => Some(ProjectionPlan::Cold),
        _ => None,
    }
}
