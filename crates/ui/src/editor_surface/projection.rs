//! Editor-surface projection worker and derived-frame caches.
//!
//! All state here is derived from immutable buffer/decorations snapshots or
//! schedules work that produces such derived state. It belongs to the
//! reusable surface lifetime, not to desktop placement, panes/tabs, file
//! operations, SQLite durability, or application shutdown.

use std::cell::RefCell;
use std::sync::Arc;

use continuity_decorate::Decorations;
use continuity_render::FrameDisplay;

use crate::display_prewarm_cache::{DisplayMapPrewarm, PrewarmQuery};
use crate::projection_worker::{ProjectionStamp, ProjectionWorker};
use crate::window_mouse_hit_test_cache::MouseHitTestFrameCacheEntry;

/// UI-thread-owned projection state for one editor surface.
pub(crate) struct ProjectionState {
    /// Bounded buffer-switch and layout prewarm cache.
    pub(crate) display_map_prewarm: DisplayMapPrewarm,
    /// Whether the desktop timer is currently driving prewarm work.
    pub(crate) display_prewarm_timer_active: bool,
    /// Most recent focused-pane frame and its compatibility query.
    pub(crate) last_painted_frame_display: Option<(PrewarmQuery, FrameDisplay)>,
    /// Decorations consumed by the most recent focused-pane paint.
    pub(crate) last_painted_decorations: Option<Arc<Decorations>>,
    /// Worker parse revision consumed by the most recent paint.
    pub(crate) last_painted_decoration_parse_revision: Option<u64>,
    /// Optional latest-wins projection worker, joined when the surface drops.
    pub(crate) projection_worker: Option<ProjectionWorker>,
    /// Monotonic request sequence for worker trace correlation.
    pub(crate) projection_request_seq: u64,
    /// Stamp used to deduplicate consecutive early-dispatch requests.
    pub(crate) last_early_dispatch_stamp: Option<ProjectionStamp>,
    /// Per-pane immutable projection cache.
    pub(crate) spectator_frame_cache: RefCell<crate::window_spectator_cache::SpectatorFrameCache>,
    /// Frame built by click hit testing and eligible for paint promotion.
    pub(crate) mouse_hit_test_frame_cache: RefCell<Option<MouseHitTestFrameCacheEntry>>,
    /// Partial display-row indexes shared by paint and input geometry.
    pub(crate) row_index_cache: RefCell<crate::window_row_index_cache::RowIndexCache>,
}

impl ProjectionState {
    pub(crate) fn new() -> Self {
        Self {
            display_map_prewarm: DisplayMapPrewarm::new(),
            display_prewarm_timer_active: false,
            last_painted_frame_display: None,
            last_painted_decorations: None,
            last_painted_decoration_parse_revision: None,
            projection_worker: None,
            projection_request_seq: 0,
            last_early_dispatch_stamp: None,
            spectator_frame_cache: RefCell::new(
                crate::window_spectator_cache::SpectatorFrameCache::new(),
            ),
            mouse_hit_test_frame_cache: RefCell::new(None),
            row_index_cache: RefCell::new(crate::window_row_index_cache::RowIndexCache::new()),
        }
    }
}
