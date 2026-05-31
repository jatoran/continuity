//! ε.5c — projection-plan classification shared by the inline paint
//! path and the off-thread projection worker dispatch.
//!
//! `Window::on_paint` calls [`classify_projection_build`] once per
//! paint. The returned [`ProjectionBuildKind`] tells the inline path
//! how to realize the frame when the worker misses, and is mapped to
//! a matching [`crate::projection_worker::ProjectionPlan`] for the
//! post-paint worker submission. Without this, the worker always
//! received `ProjectionPlan::Cold` and only helped idle/redraw frames
//! (the cold build duplicated the work inline had already finished
//! cheaply via Dirty/Splice).
//!
//! Sole source of truth for the inline-and-worker plan: changing this
//! function changes both paths in lockstep so a worker result can
//! never disagree with what the inline path would have built.
//!
//! Module layout:
//! - [`classify`] — pure [`classify_projection_build`] + its inputs
//!   struct.
//! - [`realize`] — `Window::realize_projection_build_kind` and the
//!   per-arm dispatch to `rebuild_*` / `build_*` helpers.
//! - [`dirty_spill`] — the large-dirty-set spill handler invoked
//!   from `realize`'s Dirty arm.
//!
//! Thread ownership: UI thread of one window. The classifier itself
//! is pure over caller-provided inputs; the realize impl runs on the
//! UI thread because it touches DirectWrite state via
//! `Window::rebuild_*`.

use std::ops::Range;
use std::sync::Arc;

use continuity_display_map::RowSplice;
use continuity_render::FrameDisplay;
use continuity_text::RopeEditDelta;

use crate::projection_worker::ProjectionPlan;

mod classify;
mod dirty_spill;
mod realize;

pub(crate) use classify::{classify_projection_build, ProjectionClassifyInputs};

// Re-exported for `tests.rs` which uses `super::*` and references
// `CachedFrameSource` unqualified.
#[cfg(test)]
pub(crate) use crate::window_paint_selection_reveal::CachedFrameSource;

/// How the current paint should produce its `FrameDisplay`. Mirrors
/// the rebuild ladder the inline path implemented before ε.5c
/// (covering cache → splice → dirty → cold), plus an explicit
/// "selection-only reveal" variant for the caret-only motion case.
#[derive(Clone)]
pub(crate) enum ProjectionBuildKind {
    /// The motion-fast-path or prewarm cache already realizes the
    /// viewport at the current rope revision. No rebuild needed.
    CacheHit(FrameDisplay),
    /// Covering cache hit reused via `LastPaint`, but the caret moved
    /// and a small set of source lines need their markdown reveal
    /// state refreshed before paint.
    SelectionRebuild { prev: FrameDisplay, dirty: Vec<u32> },
    /// Rope and/or decoration drift — rebuild the listed source lines
    /// against `prev`, reuse the rest of its realized specs.
    Dirty { prev: FrameDisplay, dirty: Vec<u32> },
    /// Line-count edit that the row index can splice in place against
    /// `prev`.
    Splice {
        prev: FrameDisplay,
        splice: RowSplice,
        deltas: Arc<[RopeEditDelta]>,
    },
    /// ε.3F++ (2026-05-17): same-revision viewport miss. `prev`
    /// already has a row index built against the current rope and
    /// decoration revisions; only the requested viewport's specs
    /// need to be materialised. `dirty` carries selection-reveal
    /// flips so caret-only motion within the new viewport rebuilds
    /// the affected source lines instead of falling back to stale
    /// reveal state. Pre-ε.3F++ this case routed to `Cold` —
    /// 15 / 30 of the cold builds in the second manual trace were
    /// this exact shape (`viewport=10606..10655`,
    /// `realized=10585..10654`, same rope_revision).
    ViewportRealize { prev: FrameDisplay, dirty: Vec<u32> },
    /// P18.5b — large-buffer cold path that builds a *partial* row
    /// index covering only the requested viewport (plus a safety
    /// margin) so paint does not synchronously walk the entire
    /// document. After paint installs the partial frame, the post-paint
    /// dispatch submits a regular `ProjectionPlan::Cold` worker request
    /// with reason `paint_partial_fill`; the worker's eventual full
    /// frame replaces the partial frame on the next paint epilogue.
    ColdPartial {
        /// Source-line range the partial walker should produce real
        /// counts for. Mapped from the viewport's display-row range
        /// via the 1:1 source-line ≈ display-row heuristic the
        /// classifier uses on the first paint.
        viewport_source_range: Range<u32>,
        /// Safety margin to pad `viewport_source_range` on each side.
        safety_margin: u32,
    },
    /// P18.6 — large dirty rebuild that should paint through the
    /// viewport-priority partial walker and let the worker fill in the
    /// whole-document row index after paint. The previous frame is
    /// carried for exact row-count reuse inside the walked viewport.
    DirtyPartial {
        /// Previous frame whose clean row counts can be reused when
        /// they are known exactly.
        prev: FrameDisplay,
        /// Source-line range the partial walker should cover with real
        /// counts, padded by `safety_margin`.
        viewport_source_range: Range<u32>,
        /// Safety margin to pad `viewport_source_range` on each side.
        safety_margin: u32,
        /// Dirty source-line ranges in post-edit coordinates. Ranges
        /// outside the viewport-priority window stay placeholders until
        /// the background fill completes.
        dirty_source_ranges: Arc<[Range<u32>]>,
    },
    /// P18.6 — splice rebuild that should paint through the
    /// viewport-priority partial walker. `deltas` are used by the
    /// partial walker to map clean viewport lines back to `prev`;
    /// `splice` is retained so a full previous index can still dispatch
    /// a regular Splice background fill.
    SplicePartial {
        /// Previous frame whose row counts are exact within its walked
        /// range (or everywhere when non-partial).
        prev: FrameDisplay,
        /// Source-line range the partial walker should cover with real
        /// counts, padded by `safety_margin`.
        viewport_source_range: Range<u32>,
        /// Safety margin to pad `viewport_source_range` on each side.
        safety_margin: u32,
        /// Rope edits between `prev` and the live rope.
        deltas: Arc<[RopeEditDelta]>,
        /// Structural splice derived from `deltas`.
        splice: RowSplice,
    },
    /// No reusable previous frame, or a change the incremental paths
    /// cannot safely handle. Cold-build the viewport from scratch.
    Cold,
}

impl ProjectionBuildKind {
    /// Map the build kind to a worker [`ProjectionPlan`]. Returns
    /// `None` for [`Self::CacheHit`] — dispatching that to the
    /// worker would only duplicate the cached frame.
    ///
    /// [`Self::DirtyPartial`] and [`Self::SplicePartial`] always map to
    /// the incremental [`ProjectionPlan::Dirty`] / [`ProjectionPlan::Splice`]
    /// even when `prev`'s row index is partial. `rebuild_dirty` /
    /// `rebuild_spliced` both clone prev's `DisplayRowIndex` before
    /// patching it, so the `partial_state` flag is preserved on the
    /// worker's result — paint receives a frame still tagged as partial
    /// and the partial-aware code paths apply. Falling back to
    /// [`ProjectionPlan::Cold`] here is what saturated the worker queue
    /// during typing-on-partial-prev: every keystroke after the first
    /// partial paint kept submitting full-document Cold walks. The full
    /// row index is upgraded by the post-paint `paint_partial_fill`
    /// path on [`Self::ColdPartial`], or by an idle-time fill (P18.9).
    #[must_use]
    pub(crate) fn to_worker_plan(&self) -> Option<ProjectionPlan> {
        match self {
            Self::CacheHit(_) => None,
            Self::SelectionRebuild { prev, dirty }
            | Self::Dirty { prev, dirty }
            | Self::ViewportRealize { prev, dirty } => Some(ProjectionPlan::Dirty {
                prev: prev.clone(),
                dirty: Arc::from(dirty.clone()),
            }),
            Self::Splice { prev, splice, .. } => Some(ProjectionPlan::Splice {
                prev: prev.clone(),
                splice: splice.clone(),
            }),
            Self::DirtyPartial {
                prev,
                dirty_source_ranges,
                ..
            } => Some(ProjectionPlan::Dirty {
                prev: prev.clone(),
                dirty: flatten_source_ranges(dirty_source_ranges),
            }),
            Self::SplicePartial { prev, splice, .. } => Some(ProjectionPlan::Splice {
                prev: prev.clone(),
                splice: splice.clone(),
            }),
            // P18.5b — the background fill is a regular Cold worker
            // build of the full row index; the partial walker ran
            // inline on the paint thread.
            Self::ColdPartial { .. } | Self::Cold => Some(ProjectionPlan::Cold),
        }
    }

    /// Stable trace label for the inline path (the worker dispatch
    /// trace prints a worker-plan label via [`worker_plan_label`]).
    /// Currently used only by tests; production code emits per-branch
    /// `paint:frame_display:*` events from
    /// `Window::realize_projection_build_kind`.
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn trace_label(&self) -> &'static str {
        match self {
            Self::CacheHit(_) => "cache_hit",
            Self::SelectionRebuild { .. } => "selection_reveal",
            Self::Dirty { .. } => "dirty",
            Self::Splice { .. } => "splice",
            Self::ViewportRealize { .. } => "viewport_realize",
            Self::ColdPartial { .. } => "cold_partial",
            Self::DirtyPartial { .. } => "dirty_partial",
            Self::SplicePartial { .. } => "splice_partial",
            Self::Cold => "cold",
        }
    }

    /// `true` when this paint will run any viewport-priority partial
    /// walker variant and defer the full-document row index to the
    /// background fill. Paint uses this to skip bounded waits and tag
    /// the post-paint worker request as `paint_partial_fill`.
    #[must_use]
    pub(crate) fn is_partial_variant(&self) -> bool {
        matches!(
            self,
            Self::ColdPartial { .. } | Self::DirtyPartial { .. } | Self::SplicePartial { .. }
        )
    }
}

fn flatten_source_ranges(ranges: &[Range<u32>]) -> Arc<[u32]> {
    let mut lines: Vec<u32> = ranges
        .iter()
        .flat_map(|range| range.start..range.end)
        .collect();
    lines.sort_unstable();
    lines.dedup();
    Arc::from(lines)
}

/// P18.5b — minimum source-line count for the classifier to choose
/// [`ProjectionBuildKind::ColdPartial`] over [`ProjectionBuildKind::Cold`].
/// Below this threshold the cold full walker is cheap enough that the
/// partial walker's overhead + background-fill round-trip is net
/// negative; above it, the first walk is a multi-hundred-millisecond
/// stall that paint can no longer afford to block on synchronously.
///
/// `1000` is conservative: the substrate's exit-criterion target is
/// "first paint of a never-walked 9 k-line buffer completes in < 50 ms".
/// At 1 k source lines the worst-case shaping cost on the trace
/// snapshot stays well inside the 8 ms steady-state budget, so the
/// partial path only kicks in where it pays for itself.
pub(crate) const COLD_PARTIAL_MIN_SOURCE_LINES: u32 = 1000;

/// Stable trace spelling for a worker [`ProjectionPlan`].
#[must_use]
pub(crate) fn worker_plan_label(plan: &ProjectionPlan) -> &'static str {
    match plan {
        ProjectionPlan::Cold => "cold",
        ProjectionPlan::Dirty { .. } => "dirty",
        ProjectionPlan::Splice { .. } => "splice",
    }
}

/// `true` when `realized` covers every row in `viewport`. Shared by
/// the classifier (covering-cache fast path) and the dirty-spill
/// handler (deciding whether `prev` can be painted as-is while the
/// rebuild runs off thread).
#[inline]
pub(super) fn realized_covers(realized: Range<u32>, viewport: &Range<u32>) -> bool {
    realized.start <= viewport.start && viewport.end <= realized.end
}

/// Maximum dirty source-line count the UI thread will rebuild inline
/// inside one `paint:frame_display:dirty_rebuild`. Above this the
/// rebuild spills to the projection worker (which is already
/// dispatched by the post-paint path) and this paint reuses the
/// previous frame (or a viewport-only cold build when the previous
/// frame doesn't cover the viewport).
///
/// Selected after the 2026-05-17 100 k-line stress test surfaced
/// a 16.1 s `paint:frame_display:dirty_rebuild` (full document
/// re-styled by a fresh tree-sitter parse). At ~150 µs per dirty
/// line the threshold caps the inline worst case at ~150 ms (1500
/// × 100 µs typical; 1500 × 150 µs worst-case big-doc), which the
/// frame budget can absorb without freezing input. Below the
/// threshold the inline rebuild is well-suited (no worker hop, no
/// stale-styling frame); above it the worker latency is worth it.
pub(crate) const LARGE_DIRTY_SET_THRESHOLD: usize = 1500;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod typing_tests;
