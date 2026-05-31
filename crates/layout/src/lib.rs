#![warn(missing_docs)]
//! DirectWrite text layout cache, soft-wrap, and bidirectional source-to-glyph
//! hit-testing.

pub mod cache;
pub mod error;
pub mod factory;
pub mod run_cache;
pub mod view_state;

pub use cache::{
    line_content_stamp, CachedLine, FontStateId, LayoutCache, LayoutCacheCounters, LineLayoutKey,
};
pub use error::Error;
pub use factory::DWriteFactory;
pub use run_cache::{RunCache, RunCacheKey, RunCacheLookup, RUN_CACHE_CAPACITY};
pub use view_state::{ViewState, DEFAULT_SCROLL_ANIM_MS, DEFAULT_ZOOM_STEP, MAX_ZOOM, MIN_ZOOM};
