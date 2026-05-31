//! Phase-16.5 spectator-pane body painter.
//!
//! The focused pane's body is painted by the legacy
//! [`crate::Renderer::draw_buffer`] path. This module covers the
//! *non-focused* leaves: every additional pane in the tree paints its
//! active tab's text inside its own clip rect so a two-pane layout
//! actually shows two distinct buffers' content.
//!
//! Phase 17.6 cleanup: each spectator body now builds its own
//! per-pane [`FrameDisplay`] so the layout cache keys on the *display*
//! content (markers hidden, bullets substituted, link-text-only, …)
//! and per-segment styles are baked into the cached layouts at build
//! time — identical pipeline to the focused pane. Spectators skip
//! caret painting; reveal logic is keyed on each pane's own selection
//! set.
//!
//! Thread ownership: caller is the UI thread.
//!
//! Soft-wrap inside a spectator body uses the same display-row model as
//! the focused pane: the UI builds a per-pane [`FrameDisplay`] at the
//! spectator's body width, and this painter walks display rows rather
//! than asking DirectWrite to wrap a source-line layout internally.

use continuity_layout::LayoutCache;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};
use windows::Win32::Graphics::DirectWrite::IDWriteFactory;

use crate::minimap::MinimapColors;
use crate::params::DrawParams;
use crate::text_metrics::{measure_space_advance_dip, measure_tab_advance_dip};
use crate::Error;

mod body;
mod geometry;
mod outline;
mod table_chrome;

/// Brushes shared by every spectator-pane body pass.
pub(crate) struct PaneBodyBrushes<'a> {
    /// Main text foreground.
    pub fg: &'a ID2D1SolidColorBrush,
    /// Footnote glyph foreground.
    pub footnote: &'a ID2D1SolidColorBrush,
    /// Body background fill.
    pub bg: &'a ID2D1SolidColorBrush,
    /// Placeholder fill for visible display rows not realized yet.
    pub placeholder: &'a ID2D1SolidColorBrush,
    /// Inactive gutter foreground.
    pub line_number: &'a ID2D1SolidColorBrush,
    /// Active gutter foreground.
    pub line_number_active: &'a ID2D1SolidColorBrush,
    /// Inline-highlight (`==text==`) background fill.
    pub inline_highlight_bg: &'a ID2D1SolidColorBrush,
    /// Fenced-code block background fill.
    pub code_panel: &'a ID2D1SolidColorBrush,
    /// Fenced-code block header-row background fill.
    pub code_panel_header: &'a ID2D1SolidColorBrush,
    /// Blockquote gutter bar fill.
    pub blockquote_bar: &'a ID2D1SolidColorBrush,
    /// Horizontal-rule fill.
    pub hr: &'a ID2D1SolidColorBrush,
    /// Inline `` `code` `` background fill.
    pub inline_code_bg: &'a ID2D1SolidColorBrush,
    /// Formula-evaluator computed-value text foreground.
    pub formula_value: &'a ID2D1SolidColorBrush,
    /// Formula-evaluator error sentinel foreground.
    pub formula_error: &'a ID2D1SolidColorBrush,
    /// Pipe-table cell border / grid line.
    pub table_border: &'a ID2D1SolidColorBrush,
    /// Pipe-table header-row background fill.
    pub table_header_bg: &'a ID2D1SolidColorBrush,
    pub table_alignment_bg: &'a ID2D1SolidColorBrush,
    /// Pipe-table active-cell outline / selected-fill (themeable;
    /// `markdown.table.active_cell_outline`).
    pub table_active_cell_outline: &'a ID2D1SolidColorBrush,
    /// Outline sidebar background.
    pub outline_bg: &'a ID2D1SolidColorBrush,
    /// Outline sidebar text.
    pub outline_fg: &'a ID2D1SolidColorBrush,
    /// Active outline sidebar text.
    pub outline_fg_active: &'a ID2D1SolidColorBrush,
    /// Outline sidebar separator.
    pub outline_separator: &'a ID2D1SolidColorBrush,
    /// Scaled-text minimap colors.
    pub minimap_colors: MinimapColors,
}

/// Paint every [`PaneBodyDraw`] in `params.pane_bodies`. No-op when
/// the slice is empty. Pulled into a helper so the renderer body
/// stays under the conventions line cap.
///
/// # Errors
///
/// Propagates [`Error::Graphics`] from the underlying per-body paint.
///
/// # Safety
///
/// Caller wraps in a `BeginDraw`/`EndDraw` block. The function leaves
/// the transform identity-reset on return.
pub(crate) unsafe fn paint_all_pane_bodies(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    cache: &mut LayoutCache,
    params: &DrawParams<'_>,
    line_height: f32,
    brushes: PaneBodyBrushes<'_>,
) -> Result<(), Error> {
    if params.pane_bodies.is_empty() {
        return Ok(());
    }
    let column_advance =
        measure_space_advance_dip(factory, params.format, params.base_font_size_dip);
    let tab_advance = measure_tab_advance_dip(factory, params.format, column_advance);
    for body in params.pane_bodies {
        body::paint_pane_body(
            ctx,
            factory,
            cache,
            body,
            &params.view_options,
            params.format,
            params.font_state,
            params.base_font_size_dip,
            column_advance,
            tab_advance,
            line_height,
            &brushes,
        )?;
    }
    Ok(())
}

/// Width of the text column inside a non-focused pane body without
/// right-edge chrome.
#[must_use]
pub fn spectator_body_text_width_dip(width: f32, font_size_dip: f32, line_numbers: bool) -> f32 {
    geometry::spectator_body_text_width_dip(width, font_size_dip, line_numbers)
}

/// Width of the text column inside a non-focused pane body for a
/// specific buffer line count.
#[must_use]
pub fn spectator_body_text_width_for_line_count_dip(
    width: f32,
    font_size_dip: f32,
    line_numbers: bool,
    source_line_count: usize,
) -> f32 {
    geometry::spectator_body_text_width_for_line_count_dip(
        width,
        font_size_dip,
        line_numbers,
        source_line_count,
    )
}

/// Width of the text column inside a non-focused pane body when
/// right-edge chrome is globally visible.
#[must_use]
pub fn spectator_body_text_width_with_right_edge_dip(
    width: f32,
    font_size_dip: f32,
    line_numbers: bool,
    minimap: bool,
    show_outline_sidebar: bool,
    outline_sidebar_width_dip: f32,
) -> f32 {
    geometry::spectator_body_text_width_with_right_edge_dip(
        width,
        font_size_dip,
        line_numbers,
        minimap,
        show_outline_sidebar,
        outline_sidebar_width_dip,
    )
}

/// Width of the text column inside a non-focused pane body when
/// right-edge chrome is globally visible for a specific buffer line count.
#[must_use]
pub fn spectator_body_text_width_with_right_edge_for_line_count_dip(
    width: f32,
    font_size_dip: f32,
    line_numbers: bool,
    source_line_count: usize,
    minimap: bool,
    show_outline_sidebar: bool,
    outline_sidebar_width_dip: f32,
) -> f32 {
    geometry::spectator_body_text_width_with_right_edge_for_line_count_dip(
        width,
        font_size_dip,
        line_numbers,
        source_line_count,
        minimap,
        show_outline_sidebar,
        outline_sidebar_width_dip,
    )
}
