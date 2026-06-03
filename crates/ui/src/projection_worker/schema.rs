// ε.5 ships the worker foundation only; until the integration slice
// wires `Window::on_paint` to dispatch + validate worker results,
// these types read "never used".
#![allow(dead_code)]
//! Wire schema for the projection worker: the per-request plan, the
//! request envelope, and the result.

use std::sync::Arc;

use ropey::Rope;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation, MarkdownRenderToggles, RowSplice};
use continuity_render::{FrameDisplay, DEFAULT_HEADING_SCALE};
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;

use crate::pane_tree::PaneId;

use super::measure::SendCom;
use super::stamp::ProjectionStamp;

/// What the worker should do with this request.
#[derive(Clone)]
pub(crate) enum ProjectionPlan {
    /// Cold viewport build (no compatible previous frame).
    Cold,
    /// ε.3 dirty rebuild — reuse `prev`'s clean specs, rebuild the
    /// `dirty` source lines.
    Dirty {
        /// Previous frame to reuse clean specs from.
        prev: FrameDisplay,
        /// Source lines whose specs must be rebuilt (sorted, deduped).
        dirty: Arc<[u32]>,
    },
    /// ε.3F splice rebuild — reuse `prev`'s realized specs through
    /// `shift_source_bytes`, splice the row index, materialize only
    /// the splice's new rows.
    Splice {
        /// Previous frame to splice forward from.
        prev: FrameDisplay,
        /// Structural splice descriptor from
        /// [`continuity_display_map::DisplayRowIndex::dirty_after_rope_edits`].
        splice: RowSplice,
    },
}

/// DirectWrite font metrics carried per request so the worker holds no
/// baked font state (RC1: a font-family / font-size change must be
/// reflected on the next build, not frozen at worker spawn). `format`
/// is `None` on paths without a live text format, which selects the
/// fixed-width fallback measurer.
#[derive(Clone)]
pub(crate) struct WorkerFontMetrics {
    /// Current text format (family + base size). Cloned per request —
    /// a single COM `AddRef`.
    pub format: Option<SendCom<IDWriteTextFormat>>,
    /// Base font size in DIPs the renderer paints with this frame.
    pub font_size_dip: f32,
    /// Per-heading-level font scale `[h1..h6]`.
    pub heading_scale: [f32; 6],
}

impl WorkerFontMetrics {
    /// Fixed-width fallback metrics (no DirectWrite format). Used by
    /// tests and any submit path without a live text format.
    #[must_use]
    pub fn fallback(font_size_dip: f32) -> Self {
        Self {
            format: None,
            font_size_dip,
            heading_scale: DEFAULT_HEADING_SCALE,
        }
    }
}

/// One unit of work submitted to the worker.
#[derive(Clone)]
pub(crate) struct ProjectionRequest {
    /// Monotonically increasing per-worker sequence number. Used in
    /// trace events and lets UI-side debugging name a specific
    /// dispatch.
    pub seq: u64,
    /// Pane whose projection cache should receive this result.
    pub target_pane: PaneId,
    /// Stamp of every input the worker will see.
    pub stamp: ProjectionStamp,
    /// Rope to project. UI-thread Arc-shared from the snapshot.
    pub rope: Arc<Rope>,
    /// Decorations (post-transform), or `None` for undecorated paint.
    pub decorations: Option<Arc<Decorations>>,
    /// Absolute caret bytes — must match `stamp.caret_signature`.
    pub caret_bytes: Arc<[usize]>,
    /// Fold ranges — must match `stamp.fold_signature`.
    pub folds: Arc<[FoldRange]>,
    /// Image-row reservations — must match
    /// `stamp.image_reservations_signature`.
    pub image_reservations: Arc<[ImageRowReservation]>,
    /// Suppression set — `EvaluatedTable.block_range`s of tables the
    /// active selection has reached past a single cell of. Threaded
    /// into the display-map builder so the per-line hide pass skips
    /// these tables, and the render side skips painting their
    /// chrome. Empty (`Arc::from(Vec::new())`) means no suppression.
    pub suppressed_table_blocks: Arc<[std::ops::Range<usize>]>,
    /// Fixed-width fallback for [`continuity_render::DirectWriteWidthMeasure`]
    /// when its own DirectWrite call fails. Mirrors the production
    /// call site.
    pub fallback_char_width_dip: f32,
    /// Live DirectWrite font metrics for this build. Carried per
    /// request so the worker measures at the current font — see
    /// [`WorkerFontMetrics`] (RC1 stale-font fix).
    pub font_metrics: WorkerFontMetrics,
    /// Markdown render toggle set at dispatch time. Gates emphasis /
    /// strong styling, the `==` highlight + thematic-break / setext
    /// rendering inside the builder. Carried as data (not just folded
    /// into `stamp.font_state`) because the worker needs the actual
    /// booleans to build segments; the stamp's `font_state` already
    /// discriminates results so a toggle flip rejects stale frames.
    pub markdown_toggles: MarkdownRenderToggles,
    /// Plan classification.
    pub plan: ProjectionPlan,
}

/// What the worker emits.
pub(crate) struct ProjectionResult {
    /// Sequence number of the request that produced this result.
    pub seq: u64,
    /// Pane whose projection request produced this result.
    pub target_pane: PaneId,
    /// Stamp of the inputs the worker actually built against.
    pub stamp: ProjectionStamp,
    /// The built projection. Safe to clone (interior `Arc`).
    pub frame_display: FrameDisplay,
    /// Wall-clock duration (microseconds) the worker thread spent
    /// inside `build_for_request` for this result. Excludes time
    /// waiting on the request channel; includes the entire
    /// `FrameDisplay::build_*_measured` call (walker + materialize for
    /// a `Cold` plan, dirty rebuild for `Dirty`, splice rebuild for
    /// `Splice`). The UI thread reads this on `worker_hit` to emit
    /// `event:projection_worker_result … build_dur_us=…` so the trace
    /// can show how fast the worker is at the current geometry.
    pub build_dur_us: u64,
    /// Number of additional queued requests the worker drained before
    /// keeping this one (latest-wins coalescing). A non-zero value
    /// means the UI thread was producing requests faster than the
    /// worker was building; combined with `build_dur_us` it tells us
    /// whether the worker is the bottleneck.
    pub coalesced_dropped: u32,
}
