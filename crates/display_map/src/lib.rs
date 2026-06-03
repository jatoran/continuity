#![warn(missing_docs)]
//! Source ↔ display separation for the rendering pipeline.
//!
//! A [`DisplayMap`] is an immutable, ref-counted snapshot describing what the
//! editor *paints* for a given source revision. Source bytes still live in the
//! rope (single source of truth, undo, persistence, search all unchanged); the
//! display map answers "what does the user see for this line?".
//!
//! Per spec §5 (rendering) and §9 (markdown live preview), every `IDWriteTextLayout`
//! is built from a *display string*, with per-segment styles applied once at build
//! time. The renderer never sees source bytes that aren't supposed to be visible —
//! `**`, `## `, fence ticks, fold ranges, and the literal `- ` of a bullet are all
//! either `Hidden` or replaced by a `Replace` segment.
//!
//! ## Thread ownership
//!
//! A [`DisplayMap`] is built on a worker thread (typically the decoration worker)
//! from a `RopeSnapshot` + `Decorations` + caret + folds + wrap config, then
//! handed to the UI thread as `Arc<DisplayMap>`. The map is immutable; a caret
//! move, fold registration, or wrap-width change produces a *new* snapshot. In-
//! flight paint frames keep their old snapshot until they release it.
//!
//! ## Layer position
//!
//! ```text
//! display_map ← buffer · decorate · text
//! render      ← display_map · layout · …
//! ui          ← display_map · render · …
//! ```

pub mod backslash_escape_provider;
pub mod builder;
pub mod error;
pub mod fold;
pub mod id;
pub mod image_row_reservation_provider;
pub mod line;
pub mod map;
pub mod markdown_toggles;
pub mod row_index;
mod row_index_fenwick;
pub mod segment;
pub mod segment_cache;
pub mod style;
pub mod table_hide_provider;
pub mod test_support;
pub mod wrap;
pub mod wrap_cache;
pub mod wrap_profile;

pub use builder::progressive_walker::{
    PartialWalkOutcome, PARTIAL_WALK_SAFETY_MARGIN, UNWALKED_PLACEHOLDER_ROW_COUNT,
};
pub use builder::splice_row_index::RowIndexSpliceStats;
pub use builder::stats::WalkerStats;
pub use builder::{DisplayMapBuilder, WalkerCallReason};
pub use error::Error;
pub use fold::{FoldRange, FoldSignature};
pub use id::{DisplayByte, DisplayLine, DisplayUtf16, SourceByte, SourceLine};
pub use image_row_reservation_provider::{
    compute_image_row_reservations, ImageRowReservation, ImageRowReservationInput,
};
pub use line::DisplayLineSpec;
pub use map::DisplayMap;
pub use markdown_toggles::MarkdownRenderToggles;
pub use row_index::dirty::RowDirty;
pub use row_index::splice::RowSplice;
pub use row_index::{DisplayRowIndex, IndexStamps, PartialRowIndexState};
pub use segment::{DisplaySegment, SegmentHit};
pub use segment_cache::{
    compute_line_projection_stamp, SegmentCache, SegmentCacheCounters, SegmentCacheKey,
    SEGMENT_CACHE_CAPACITY,
};
pub use style::{SpanRole, SpanStyle};
pub use wrap::{MeasureCacheStatus, MeasuredAdvance, WidthMeasure, WrapConfig};
pub use wrap_cache::{WrapCache, WrapCacheEntry, WrapCacheKey, WRAP_CACHE_CAPACITY};
pub use wrap_profile::row_count_from_profile;
