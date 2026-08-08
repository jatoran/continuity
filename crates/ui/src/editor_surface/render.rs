//! Renderer resources and derived layout caches for one editor surface.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use continuity_buffer::BufferId;
use continuity_display_map::{SegmentCache, WrapCache};
use continuity_layout::{DWriteFactory, FontStateId, LayoutCache, RunCache};
use continuity_render::{Renderer, TableLayout};
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;

use crate::window_constants::LAYOUT_CACHE_CAPACITY;

/// Render resources and layout caches associated with one editor surface.
///
/// **Thread ownership:** the surface's UI thread is the sole writer. The
/// row-count caches use shared backing stores because the projection worker
/// reads and populates them; DirectWrite and renderer resources never cross
/// the UI-thread boundary.
pub(crate) struct RenderState {
    /// DirectWrite factory used to create formats and layouts.
    pub(crate) dwrite: DWriteFactory,
    /// Device-dependent renderer, installed after HWND creation.
    pub(crate) renderer: Option<Renderer>,
    /// Active prose text format, rebuilt when font or DPI changes.
    pub(crate) text_format: Option<IDWriteTextFormat>,
    /// Bounded LRU cache of text layouts for visible logical lines.
    pub(crate) cache: LayoutCache,
    /// Shared row-count run cache.
    pub(crate) walker_run_cache: Arc<RunCache>,
    /// Shared row-count wrap cache.
    pub(crate) walker_wrap_cache: Arc<WrapCache>,
    /// Shared row-count segment cache.
    pub(crate) walker_segment_cache: Arc<SegmentCache>,
    /// Hash describing the active font and DPI scale.
    pub(crate) font_state: FontStateId,
    /// Most recent non-empty focused-pane visual-table layouts by buffer.
    pub(crate) last_focused_table_layouts: RefCell<HashMap<BufferId, Arc<Vec<TableLayout>>>>,
}

impl RenderState {
    /// Create renderer state before the HWND and device resources exist.
    pub(crate) fn new(dwrite: DWriteFactory, font_state: FontStateId) -> Self {
        Self {
            dwrite,
            renderer: None,
            text_format: None,
            cache: LayoutCache::new(LAYOUT_CACHE_CAPACITY),
            walker_run_cache: Arc::new(RunCache::default()),
            walker_wrap_cache: Arc::new(WrapCache::default()),
            walker_segment_cache: Arc::new(SegmentCache::default()),
            font_state,
            last_focused_table_layouts: RefCell::new(HashMap::new()),
        }
    }
}
