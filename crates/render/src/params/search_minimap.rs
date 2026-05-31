//! Search-active minimap draw payloads.

use crate::params::Rgba;
use crate::search_highlight_paint::SearchHighlightRangeDraw;

/// One tick painted on the search-active minimap strip.
///
/// Mirrors the geometry produced by the UI-layer `MinimapLayout` builder
/// (`crates/ui/src/search_minimap.rs`) so the renderer doesn't need to
/// reach back into UI types. The UI converts its minimap layout into a
/// [`SearchMinimapDraw`] per frame before handing it to the renderer.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SearchMinimapTickDraw {
    /// Top edge of the tick in DIPs, relative to the strip's top.
    pub y_dip: f32,
    /// Tick height in DIPs.
    pub height_dip: f32,
    /// `true` for the currently-focused match; painter uses
    /// `match_active` and may draw it wider.
    pub is_active: bool,
}

/// Per-frame payload for the search-active minimap strip.
///
/// `None` on [`crate::params::DrawParams::search_minimap`] means the
/// strip is not painted this frame (find bar closed or no matches). The
/// UI layer is responsible for not building the payload outside that
/// window.
#[derive(Clone, Debug)]
pub struct SearchMinimapDraw {
    /// Strip left edge in pane-local DIPs (renderer adds
    /// `body_origin.x` before painting).
    pub x_dip: f32,
    /// Strip top edge in pane-local DIPs.
    pub y_dip: f32,
    /// Strip width in DIPs.
    pub width_dip: f32,
    /// Strip height in DIPs.
    pub height_dip: f32,
    /// One entry per find match in source order.
    pub ticks: Vec<SearchMinimapTickDraw>,
    /// Translucent background fill for the strip.
    pub bg: Rgba,
    /// Per-match tick color.
    pub match_color: Rgba,
    /// Focused-match tick color.
    pub match_active: Rgba,
    /// Source-byte ranges for the corresponding editor-body highlights.
    pub body_highlights: Vec<SearchHighlightRangeDraw>,
}
