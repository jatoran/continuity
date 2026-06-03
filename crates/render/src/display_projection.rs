//! Per-frame display projection — builds a single `Arc<DisplayMap>` once
//! at the top of [`crate::Renderer::draw_buffer`] and offers fast per-line
//! lookups for the inner paint loop.
//!
//! **Thread ownership**: built on the UI thread for the duration of one
//! frame. Owns no D2D / DirectWrite handles.
//!
//! The type's surface is split across topical siblings:
//! - [`build`](self) — constructors (full, viewport, dirty / spliced rebuilds).
//! - [`lookup`](self) — per-line, per-byte, and row-index queries.
//! - [`whitespace`](self) — leading-whitespace DIP advance helper for
//!   soft-wrap continuation indent.

use std::sync::Arc;

use continuity_display_map::DisplayMap;

mod build;
mod build_incremental;
mod build_partial;
mod lookup;
mod placeholder;
#[cfg(test)]
mod tests;
mod whitespace;

/// Per-frame projection wrapper. Holds an `Arc<DisplayMap>` plus a
/// fast `source_line → first DisplayLineSpec` lookup.
#[derive(Clone)]
pub struct FrameDisplay {
    map: Arc<DisplayMap>,
}
