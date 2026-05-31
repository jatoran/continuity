//! Large-dirty-set spill handler. When a
//! [`super::ProjectionBuildKind::Dirty`] carries more source lines
//! than the UI thread can rebuild inline within the frame budget,
//! the inline path defers the rebuild to the projection worker and
//! paints `prev` (or a cached viewport-only cold build) this frame.
//!
//! Origin: 2026-05-17 100 k-line stress test trace showed a single
//! 16.1 s `paint:frame_display:dirty_rebuild` at `lines=107257`
//! after a forced `decoration_parse_full reason=no_prev_tree`. The
//! dirty set was essentially every line; the inline refresh loop in
//! `DisplayMapBuilder::rebuild_dirty` scales linearly with the dirty
//! set. The guard converts that 16-second freeze into a one-frame
//! stale-styling paint plus an off-thread rebuild.
//!
//! Thread ownership: UI thread of one window. The post-paint worker
//! dispatch in `Window::on_paint` reuses the unchanged
//! `projection_kind` and submits the same Dirty plan
//! (`to_worker_plan()`) to the worker thread — the worker's `submit`
//! is latest-wins, so concurrent edits keep updating the target. The
//! worker's delivery is picked up by `try_use_worker_result` on a
//! subsequent paint.

use std::ops::Range;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation};
use continuity_render::FrameDisplay;
use ropey::Rope;

use super::{realized_covers, LARGE_DIRTY_SET_THRESHOLD};
use crate::paint_trace::PaintTrace;
use crate::window::Window;

/// If `dirty` exceeds [`LARGE_DIRTY_SET_THRESHOLD`], emit the
/// `paint:frame_display:dirty_spilled` trace event and return the
/// frame this paint should display while the rebuild runs off
/// thread: `prev` when its realized window still covers the
/// viewport, otherwise a row-index cached viewport-only cold build.
/// Returns `None` when the inline rebuild should proceed normally.
#[allow(clippy::too_many_arguments)]
pub(super) fn spill_if_large(
    window: &Window,
    prev: &FrameDisplay,
    dirty: &[u32],
    rope: &Rope,
    revision: u64,
    decorations: Option<&Decorations>,
    caret_bytes: &[usize],
    folds: &[FoldRange],
    image_reservations: &[ImageRowReservation],
    wrap_width_dip: u32,
    projection_char_width: f32,
    viewport_rows: &Range<u32>,
    trace: &PaintTrace,
) -> Option<FrameDisplay> {
    if dirty.len() <= LARGE_DIRTY_SET_THRESHOLD {
        return None;
    }
    let covers_viewport = realized_covers(prev.realized_row_range(), viewport_rows);
    if crate::paint_trace::is_trace_enabled() {
        let lo = dirty.first().copied().unwrap_or(0);
        let hi = dirty.last().copied().unwrap_or(0);
        let detail = format!(
            "dirty_count={} threshold={} covers_viewport={} dirty_span={lo}..={hi} viewport={}..{}",
            dirty.len(),
            LARGE_DIRTY_SET_THRESHOLD,
            covers_viewport,
            viewport_rows.start,
            viewport_rows.end,
        );
        crate::paint_trace::log_event("paint:frame_display:dirty_spilled", &detail);
    }
    trace.mark("frame_display:dirty_spilled");
    if covers_viewport {
        return Some(prev.clone());
    }
    Some(window.build_frame_display_viewport_cached(
        Some(window.buffer_id),
        rope,
        revision,
        decorations,
        caret_bytes,
        folds,
        image_reservations,
        wrap_width_dip,
        projection_char_width,
        viewport_rows.clone(),
        crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
        continuity_display_map::WalkerCallReason::PaintDirty,
    ))
}
