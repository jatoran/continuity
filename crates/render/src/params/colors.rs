//! Theme-derived color bags surfaced as plain-data params: editor body
//! colors, markdown decoration colors, and panel/pane-chrome colors.
//!
//! Each struct's field set tracks a contiguous group of theme keys in
//! spec §11 — the resolver fills them once per theme load and the
//! renderer paints them per frame.

use crate::params::Rgba;

/// Theme-derived editor colors used by [`crate::Renderer::draw_buffer`].
///
/// Field set tracks the `editor.*` keys in spec §11. New fields landed in
/// Phase 11 alongside the theme resolver.
#[derive(Copy, Clone, Debug, Default)]
pub struct EditorColors {
    /// `editor.background`.
    pub bg: Rgba,
    /// `editor.foreground`.
    pub fg: Rgba,
    /// `editor.cursor.primary`.
    pub caret: Rgba,
    /// `editor.cursor.secondary`.
    pub secondary_caret: Rgba,
    /// `editor.selection`.
    pub selection: Rgba,
    /// `editor.selection_inactive`.
    pub selection_inactive: Rgba,
    /// `editor.line_highlight` — drives the mouse-hover line band (the
    /// renderer scales its alpha down for the hover overlay).
    pub line_highlight: Rgba,
    /// `editor.caret_line_highlight` — distinct fill painted behind the
    /// caret's own line, independent of the hover band derived from
    /// [`Self::line_highlight`].
    pub caret_line_highlight: Rgba,
    /// `editor.line_number`.
    pub line_number: Rgba,
    /// `editor.line_number_active`.
    pub line_number_active: Rgba,
    /// `editor.indent_guide`.
    pub indent_guide: Rgba,
    /// `editor.indent_guide_active`.
    pub indent_guide_active: Rgba,
    /// `editor.search_match`.
    pub search_match: Rgba,
    /// `editor.search_match_active`.
    pub search_match_active: Rgba,
    /// `editor.find_bar.background`.
    pub find_bar_bg: Rgba,
    /// `editor.search_minimap.background` — Phase G4 strip fill.
    pub search_minimap_bg: Rgba,
    /// `editor.search_minimap.match` — per-match tick color.
    pub search_minimap_match: Rgba,
    /// `editor.search_minimap.match_active` — focused-match tick color.
    pub search_minimap_match_active: Rgba,
    /// `editor.minimap.background` — scaled-text minimap strip fill
    /// (right edge of pane while `[ui].show_minimap` is on).
    pub minimap_bg: Rgba,
    /// `editor.minimap.foreground` — color of the scaled-down glyphs
    /// inside the minimap strip; typically a low-alpha tint of the
    /// editor foreground so the strip reads as a thumbnail.
    pub minimap_fg: Rgba,
    /// `editor.minimap.viewport_indicator` — translucent box drawn over
    /// the section of the minimap currently visible in the editor.
    pub minimap_viewport_indicator: Rgba,
    /// `editor.loading_overlay.background` — translucent fill for the
    /// transient "building view" overlay drawn while paint waits on a
    /// slow projection-worker build (P0.8.3).
    pub loading_overlay_bg: Rgba,
    /// `editor.loading_overlay.foreground` — label text color for the
    /// transient "building view" overlay.
    pub loading_overlay_fg: Rgba,
    /// `editor.loading_overlay.border` — 1-DIP stroke around the
    /// overlay panel. Alpha `0` skips the stroke.
    pub loading_overlay_border: Rgba,
}

/// Theme-derived markdown decoration colors. Field set tracks the
/// `markdown.*` keys in spec §11.
#[derive(Copy, Clone, Debug, Default)]
pub struct MarkdownColors {
    /// Heading colors per level (`markdown.heading.{1..6}`); index 0 → `h1`.
    pub heading: [Rgba; 6],
    /// `markdown.bold`.
    pub bold: Rgba,
    /// `markdown.italic`.
    pub italic: Rgba,
    /// `markdown.strikethrough`.
    pub strikethrough: Rgba,
    /// `markdown.code.foreground`.
    pub code_fg: Rgba,
    /// `markdown.code.background`.
    pub code_bg: Rgba,
    /// `markdown.code_block.background`.
    pub code_block_bg: Rgba,
    /// `markdown.code_block.border`.
    pub code_block_border: Rgba,
    /// `markdown.blockquote.foreground`.
    pub blockquote_fg: Rgba,
    /// `markdown.blockquote.bar`.
    pub blockquote_bar: Rgba,
    /// `markdown.link`.
    pub link: Rgba,
    /// `markdown.footnote`.
    pub footnote: Rgba,
    /// `markdown.url`.
    pub url: Rgba,
    /// `markdown.image_alt`.
    pub image_alt: Rgba,
    /// `markdown.list_marker`.
    pub list_marker: Rgba,
    /// `markdown.checkbox.checked`.
    pub checkbox_checked: Rgba,
    /// `markdown.checkbox.unchecked`.
    pub checkbox_unchecked: Rgba,
    /// `markdown.hr`.
    pub hr: Rgba,
    /// `markdown.table.border`.
    pub table_border: Rgba,
    /// `markdown.table.header_bg` — subtle fill behind a pipe-table
    /// header row when the visual-table renderer is drawing the block
    /// (caret outside the table). Painted underneath the cell text by
    /// [`crate::table_paint`].
    pub table_header_bg: Rgba,
    /// `markdown.table.alignment_bg` — fill behind the pipe-table
    /// alignment-row slot (`|---|---|`). Visually distinct from
    /// `table_header_bg` so the strip reads as its own band between
    /// the header and the body.
    pub table_alignment_bg: Rgba,
    /// `markdown.table.active_cell_outline` — stroke color for the
    /// "you are editing here" outline drawn over the table cell whose
    /// `source_range` contains a caret head, and (at reduced opacity)
    /// the translucent fill of a fully-selected cell. Decoupled from the
    /// editor caret brush so the affordance can be themed independently;
    /// bundled themes default it to `editor.cursor.primary` to preserve
    /// the prior look. Painted fresh each frame by
    /// [`crate::table_paint::paint_active_cell_outline_line`] (NOT baked
    /// into the per-table chrome cache), so it never affects row counts
    /// or display-map layout.
    pub table_active_cell_outline: Rgba,
    /// Phase F3 — `editor.inline_highlight.foreground`. Painted as the
    /// run foreground when an `InlineColorSpan` of kind `Highlight` covers
    /// the byte range.
    pub inline_highlight_fg: Rgba,
    /// Phase F3 — `editor.inline_highlight.background`. Painted as a
    /// rect behind the `Highlight`-kind span inner text.
    pub inline_highlight_bg: Rgba,
    /// Phase F4 — `markdown.formula.value`. Foreground for the rendered
    /// computed-value text of a table-cell formula.
    pub formula_value: Rgba,
    /// Phase F4 — `markdown.formula.error`. Foreground for the
    /// `#DIV/0!` / `#ERR` sentinels.
    pub formula_error: Rgba,
}

/// Theme-derived panel + pane-border colors used for the Phase 13 tab
/// strip and pane chrome.
#[derive(Copy, Clone, Debug, Default)]
pub struct PanelColors {
    /// `panel.background` — base tab-strip fill.
    pub bg: Rgba,
    /// `panel.foreground` — tab-strip text default.
    pub fg: Rgba,
    /// `panel.active_tab.background`.
    pub active_tab_bg: Rgba,
    /// `panel.active_tab.foreground`.
    pub active_tab_fg: Rgba,
    /// `panel.inactive_tab.background`.
    pub inactive_tab_bg: Rgba,
    /// `panel.inactive_tab.foreground`.
    pub inactive_tab_fg: Rgba,
    /// `pane.border` — non-focused pane border.
    pub pane_border: Rgba,
    /// `pane.border_active` — focused pane border accent.
    pub pane_border_active: Rgba,
}
