//! Plain-data types passed through [`crate::renderer::Renderer::draw_buffer`]:
//! [`Rgba`], the [`EditorColors`] / [`MarkdownColors`] palettes, the Phase-11
//! [`ViewOptionsDraw`] toggle bag, [`CaretShape`], and the per-frame
//! [`DrawParams`] aggregate.
//!
//! No D2D / DirectWrite imports — these types live one layer above any
//! Win32 handle.

use continuity_decorate::{Decorations, EvaluatedTable, InlineColorSpan};
use continuity_layout::{FontStateId, ViewState};
use continuity_text::Selection;
use ropey::Rope;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;

use crate::breadcrumb::BreadcrumbData;
use crate::display_projection::FrameDisplay;
use crate::file_tree::FileTreeDraw;
use crate::motion::{EditPulseDraw, JumpGlowDraw, SurfaceMotion};
use crate::outline::OutlineData;
use crate::overlay::OverlayDraw;
use crate::status_bar::StatusBarData;
use crate::table_layout::TableLayout;

use crate::inline_image_types::InlineImagePlacement;
use crate::loading_overlay::LoadingOverlayDraw;

pub mod colors;
pub mod search_minimap;
use colors::{EditorColors, MarkdownColors, PanelColors};

/// 32-bit RGBA color in linear-ish space (matches D2D1_COLOR_F semantics).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Rgba {
    /// Red 0.0..=1.0.
    pub r: f32,
    /// Green 0.0..=1.0.
    pub g: f32,
    /// Blue 0.0..=1.0.
    pub b: f32,
    /// Alpha 0.0..=1.0.
    pub a: f32,
}

impl Rgba {
    /// Opaque black.
    pub const BLACK: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    /// Fully transparent.
    pub const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
}

impl From<Rgba> for D2D1_COLOR_F {
    fn from(c: Rgba) -> Self {
        D2D1_COLOR_F {
            r: c.r,
            g: c.g,
            b: c.b,
            a: c.a,
        }
    }
}

/// One tick painted on the search-active minimap strip.
pub type SearchMinimapTickDraw = search_minimap::SearchMinimapTickDraw;

/// Per-frame payload for the search-active minimap strip.
pub type SearchMinimapDraw = search_minimap::SearchMinimapDraw;

/// Caret shape mirrored from `ui::window_view_options::CaretStyle`.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum CaretShape {
    /// Thin vertical bar at the caret column (default).
    #[default]
    Bar,
    /// Block fill behind the grapheme under the caret.
    Block,
    /// Thin horizontal underline below the grapheme.
    Underline,
}

/// Per-frame view-toggle state — every spec §11 `view.toggle_*` flag
/// the renderer needs at draw time.
#[derive(Clone, Debug, Default)]
pub struct ViewOptionsDraw<'a> {
    /// Render the gutter line-number column on the left of the editor.
    pub line_numbers: bool,
    /// When the gutter is visible, render only the caret line's number.
    /// Phase A §A4 default; mirrors the ui-side flag.
    pub gutter_caret_line_only: bool,
    /// Render non-caret gutter labels as distance from the primary caret
    /// source line.
    pub relative_line_numbers: bool,
    /// Paint the current-line highlight band behind the caret line.
    pub current_line_highlight: bool,
    /// Paint vertical indent-guide rules at indent-column boundaries.
    pub indent_guides: bool,
    /// Render whitespace-marker glyphs over space + tab runs.
    pub whitespace_markers: bool,
    /// Paint a coloured fill on trailing whitespace runs.
    pub trailing_whitespace: bool,
    /// Render the minimap (subsampled glyph-density heatmap).
    pub minimap: bool,
    /// Indent step in spaces (indent-guide column spacing).
    pub indent_size: u32,
    /// On-screen width of a literal tab character, in columns. Drives
    /// the DirectWrite incremental tab stop applied to the body text
    /// format (so a `\t` renders at this width) and the indent-guide /
    /// whitespace-marker tab advance. `0` falls back to the font's
    /// default tab stop (the pre-settings behaviour) so test contexts
    /// constructing a `ViewOptionsDraw::default()` keep working.
    pub tab_width: u32,
    /// Ruler-column positions in characters.
    pub ruler_columns: &'a [u32],
    /// Active caret shape.
    pub caret_shape: CaretShape,
    /// `true` when the caret should be drawn this frame.
    pub caret_visible: bool,
    /// Phase B4: bar-mode caret width in DIPs. Used for
    /// [`CaretShape::Bar`]; ignored for block / underline. `0` falls
    /// back to a reasonable default in the painter.
    pub caret_bar_width_px: u32,
    /// Paint the bottom status bar (caret position + buffer dirty marker).
    pub show_status_bar: bool,
    /// Phase F1: paint the sticky heading breadcrumb at the top of the
    /// editor body. When `true` and a [`DrawParams::breadcrumb`] is
    /// supplied, the renderer reserves [`crate::BREADCRUMB_HEIGHT_DIP`]
    /// above the body for it.
    pub show_sticky_breadcrumb: bool,
    /// Phase F2: paint the right-docked outline sidebar. When `true`
    /// and a [`DrawParams::outline`] is supplied, the renderer reserves
    /// `data.width_dip` on the right edge of the body for it.
    pub show_outline_sidebar: bool,
    /// Phase F2: outline-sidebar width in DIPs. Only consulted when
    /// `show_outline_sidebar` is `true`. The renderer adds this to
    /// the right margin so the body reflows *additively* with any
    /// other right-edge consumer (e.g. the search-minimap strip).
    pub outline_sidebar_width_dip: f32,
    /// Phase G4: reserve right-edge space for the search-active minimap
    /// strip. Set by the UI only while the find bar is open and has at
    /// least one match. When `true` the renderer pulls
    /// `SEARCH_MINIMAP_WIDTH_DIP` off the body width even when the
    /// glyph-density minimap is off, so the editor body reflows away
    /// from the strip.
    pub search_minimap_active: bool,
    /// Phase H2: paint the tab strip at the top of every pane.
    /// Distraction-free mode flips this `false` while active.
    pub show_tab_strip: bool,
    /// Phase H2: paint pane borders around every pane leaf.
    /// Distraction-free mode flips this `false` while active.
    pub show_pane_borders: bool,
    /// Phase H2: when `true`, the renderer centers the editor body
    /// inside the focused pane and caps its width at
    /// `distraction_free_max_width_dip`. When `false`, the body fills
    /// the pane minus the usual gutter/minimap margins.
    pub distraction_free: bool,
    /// Phase H2: body-column cap in DIPs when `distraction_free` is
    /// `true`. Computed by the UI as
    /// `pane_modes.distraction_free_max_width * em_advance` so the
    /// renderer treats it as a pre-resolved pixel width.
    pub distraction_free_max_width_dip: f32,
    /// §H1 — focus-mode string. One of `"off" | "line" | "sentence" |
    /// "paragraph"`. The empty string is treated as `"off"`. The renderer
    /// uses this to dispatch to `decorate::focus_span::{line, sentence,
    /// paragraph}_span` and paint a dim overlay outside the focused
    /// source range.
    pub focus_mode: &'static str,
    /// §H1 — dim alpha (0.0..=1.0) for the focus-mode overlay. The
    /// caller has already resolved the precedence between
    /// `[focus].dim_alpha` (preferred when non-zero) and the theme key
    /// `editor.focus_dim_alpha`.
    pub focus_dim_alpha: f32,
    /// §H1 — RGB foreground-dim color from the theme key
    /// `editor.foreground_dim`. The renderer combines this with
    /// `focus_dim_alpha` to build the overlay brush.
    pub focus_dim_color: Rgba,
    /// §H3 — user-toggled fold source-line indices. `u32::MAX` is the
    /// "fold all top-level" sentinel; the display-map indent-fold
    /// provider expands it at frame-build time. The gutter triangle
    /// painter consumes this directly to choose ▾/▸ glyphs.
    pub folded_lines: &'a [u32],
    /// §H3 — markdown heading line list `(line, level)` sorted ascending
    /// by line. Sourced by the UI from
    /// `continuity_decorate::headings`. The gutter painter uses this to
    /// recognize heading lines as foldable (even without an indent
    /// subtree) and to compute the heading-fold extent for the `▸ N`
    /// indicator. Empty slice when no decorations are available or the
    /// buffer has no headings — the painter falls back to indent-only.
    pub markdown_headings: &'a [(u32, u8)],
    /// Paint the `==text==` highlight background fill. Mirrors
    /// `[markdown].render_highlight`. When `false` the highlight
    /// rectangles are skipped (and the display map keeps the `==`
    /// markers visible); independent of `{#hex:}` foreground color,
    /// which always paints. Default `false` via `derive(Default)` — the
    /// UI sets it explicitly each frame, so production rendering follows
    /// the live setting.
    pub render_highlight_bg: bool,
    /// Paint `---` / `***` / `___` thematic-break horizontal rules.
    /// Mirrors `[markdown].render_divider`. When `false`
    /// `paint_horizontal_rules` is skipped (and the display map keeps
    /// the literal characters visible). Default `false` via
    /// `derive(Default)`; the UI sets it explicitly each frame.
    pub render_divider: bool,
}

/// One pane's tab-strip layout for Phase 13 chrome painting.
#[derive(Clone, Debug)]
pub struct PaneStripDraw {
    /// Outer pane rect: `(x, y, w, h)` in client-area DIPs.
    pub outer: (f32, f32, f32, f32),
    /// `true` if this pane currently holds keyboard focus.
    pub focused: bool,
    /// Tab labels in positional order.
    pub tabs: Vec<TabLabel>,
    /// Index into `tabs` of the active tab in this pane.
    pub active_index: usize,
    /// Active-border alpha mix in `[0, 1]` while a pane-focus crossfade
    /// is in flight. When `Some`, the renderer blends `pane.border` and
    /// `pane.border_active` by `opacity` regardless of the `focused`
    /// flag — the focus-in pane carries a rising mix, the focus-out
    /// pane carries a falling mix. `None` paints the static border for
    /// the current `focused` flag.
    pub focus_motion: Option<SurfaceMotion>,
    /// Slide progress in `[0, 1]` for the active-tab underline, paired
    /// with [`Self::previous_active_tab_index`]. When `Some`, the
    /// renderer lerps a 3 DIP underline from the previous-active tab's
    /// rect to the current-active tab's rect at `opacity`.
    pub active_tab_motion: Option<SurfaceMotion>,
    /// Index of the previously-active tab while a tab-activation slide
    /// is in flight. Paired with [`Self::active_tab_motion`]; both must
    /// be `Some` for the renderer to paint the sliding underline.
    pub previous_active_tab_index: Option<usize>,
    /// Item 8 — per-pane horizontal tab-strip scroll offset in DIPs.
    pub tab_scroll_offset_dip: f32,
}

/// Single tab's display payload.
#[derive(Clone, Debug)]
pub struct TabLabel {
    /// Resolved label.
    pub text: String,
    /// `true` when the tab represents an unsaved file-associated buffer.
    /// Phase 13 leaves this `false` since file association lands in
    /// Phase 15.
    pub dirty: bool,
    /// Paint a clickable `×` close button at the right edge of the tab.
    pub show_close: bool,
}

/// One on-screen squiggle range — Phase-16 spell errors carried into
/// the renderer for the Phase-16.5 wavy-underline overlay.
///
/// Lines and byte offsets are zero-based and refer to the active
/// document's logical lines / line-relative byte indices, matching the
/// rest of the layout cache's keying.
#[derive(Copy, Clone, Debug)]
pub struct SpellSquiggleSpan {
    /// Logical line containing the misspelled word.
    pub line: u32,
    /// First UTF-8 byte of the misspelled run within the line.
    pub byte_in_line_start: u32,
    /// One-past-the-last UTF-8 byte of the misspelled run within the line.
    pub byte_in_line_end: u32,
}

/// Aggregate Phase 13 chrome data — all panes' tab strips + colors.
#[derive(Clone, Debug)]
pub struct PaneChromeDraw {
    /// Per-pane tab strips in document traversal order.
    pub panes: Vec<PaneStripDraw>,
    /// Theme colors.
    pub colors: PanelColors,
    /// Tab strip height in DIPs.
    pub strip_height: f32,
    /// In-flight tab-drag affordance — insertion bar, drop-pane border,
    /// or cursor-attached ghost. `None` when no drag is in flight.
    pub tab_drag: Option<TabDragOverlayDraw>,
}

/// In-flight tab-drag visual feedback. One of four mutually-exclusive
/// affordances paints per frame, mirroring the four
/// `TabDropResolution` outcomes the UI thread resolves.
#[derive(Clone, Debug)]
pub struct TabDragOverlayDraw {
    /// Live drop indicator on a tab strip (source pane or sibling pane
    /// inside this window). Carries the strip rect + slot widths so the
    /// painter draws a 2 DIP vertical accent bar at the slot's left
    /// edge without re-walking the pane tree.
    pub source_strip_indicator: Option<TabStripInsertionBarDraw>,
    /// Index of the *source* tab inside its pane so the painter knows
    /// which slot to fade to `source_tab_alpha` (the "lifted" tab).
    pub source_tab: Option<TabDragSourceFade>,
    /// Active drop target is a pane body in this window — paint a
    /// 2 DIP accent border + a tinted body fill so the user sees
    /// "release here ⇒ adopt into this pane."
    pub pane_body_highlight: Option<PaneBodyDropHighlight>,
    /// Cursor-attached tear-off ghost. Painted when the cursor is in
    /// the tear-off zone (desktop, another app, this window's chrome
    /// outside any pane). Tells the user "release here ⇒ new window."
    pub ghost: Option<TabDragGhostDraw>,
    /// Per-affordance fade opacity in `[0, 1]`. Driven by the
    /// 120 ms ease-out fade-in at drag start and fade-out at drag
    /// end / cancel. Reduced motion collapses this to `1.0`.
    pub fade_alpha: f32,
}

/// One insertion-bar paint instruction. The bar sits between two
/// existing tab slots (or at the start / end of the strip) on a tab
/// strip the cursor is currently over.
#[derive(Copy, Clone, Debug)]
pub struct TabStripInsertionBarDraw {
    /// Strip outer rect `(x, y, w, h)` matching the
    /// [`PaneStripDraw::outer`] of the strip the cursor is over.
    pub strip_outer: (f32, f32, f32, f32),
    /// Strip-relative x of the bar's left edge in DIPs.
    pub x_in_strip: f32,
    /// Bar width in DIPs (matches the design: 2 DIP).
    pub width: f32,
    /// Strip height in DIPs.
    pub height: f32,
}

/// One source-tab fade instruction. The painter blends the tab's
/// background + label down to `alpha` so the user can see the tab
/// is "lifted."
#[derive(Copy, Clone, Debug)]
pub struct TabDragSourceFade {
    /// Strip outer rect of the pane that owns the source tab.
    pub strip_outer: (f32, f32, f32, f32),
    /// Index of the source tab inside that pane's tab list.
    pub tab_index: usize,
    /// Final fade opacity (`0.6` per design when fully faded;
    /// scales with `fade_alpha` at drag start/end).
    pub alpha: f32,
}

/// Pane-body drop highlight. Painted when the cursor sits over a
/// pane body in this window and a release would adopt the tab into
/// that pane.
#[derive(Copy, Clone, Debug)]
pub struct PaneBodyDropHighlight {
    /// Body rect of the target pane in client DIPs.
    pub body_rect: (f32, f32, f32, f32),
}

/// Cursor-attached ghost-preview rectangle painted in the tear-off
/// zone. Anchored so the cursor sits near the ghost's top-left
/// corner — the typical OS drag-image anchor.
#[derive(Clone, Debug)]
pub struct TabDragGhostDraw {
    /// Top-left corner in client DIPs.
    pub origin: (f32, f32),
    /// Ghost rect width in DIPs.
    pub width: f32,
    /// Ghost rect height in DIPs.
    pub height: f32,
    /// Tab label rendered inside the ghost.
    pub label: String,
}

/// One non-focused pane body in the per-frame draw list. Phase 16.5
/// added this so every visible pane leaf paints its active tab's text
/// — not just the focused one.
///
/// The focused pane's body is still painted from the top-level
/// [`DrawParams`] fields (`document`, `view`, `decorations`, etc.) so
/// the legacy single-body path is unaffected. Non-focused bodies are
/// passed as a slice through [`DrawParams::pane_bodies`] and rendered
/// after the focused body, each in its own clip rect + transform.
///
/// Lifetimes match `DrawParams<'a>`: rope, selections, and view are
/// borrowed from the per-pane snapshots / state on the UI thread.
pub struct PaneBodyDraw<'a> {
    /// Document identifier (`BufferId.as_uuid().as_u128()`). The
    /// renderer uses this when keying into the shared layout cache so
    /// distinct buffers do not collide.
    pub document: u128,
    /// Body rect in client-area DIPs (the pane's outer rect with the
    /// tab strip already subtracted).
    pub rect: (f32, f32, f32, f32),
    /// Active rope for this pane.
    pub rope: &'a Rope,
    /// Active selection set.
    pub selections: &'a [Selection],
    /// Per-pane view (scroll, zoom, soft-wrap).
    pub view: &'a ViewState,
    /// Optional decoration snapshot. `None` ⇒ plain text.
    pub decorations: Option<&'a Decorations>,
    /// Inline-color spans for this pane's buffer (borrow into the
    /// spectator's decoration snapshot). Empty when the snapshot is
    /// absent or carries no inline-color markup. Spectators paint these
    /// the same way the focused pane does so a 2×2 grid shows real
    /// rendered previews rather than four panes of raw markdown source.
    pub inline_color_spans: &'a [InlineColorSpan],
    /// Per-table formula overrides for this pane's buffer (borrow into
    /// the spectator's `evaluated_tables`). Empty when no table block
    /// carries a formula cell.
    pub table_overrides: &'a [EvaluatedTable],
    /// Pipe-table visual layouts for this pane's caret-outside blocks.
    /// Computed per spectator (caret-inside vs -outside is decided by
    /// each pane's own selection set). Empty when no caret-outside
    /// table is visible.
    pub table_layouts: &'a [TableLayout],
    /// Per-pane display projection. Built upstream so the same instance
    /// drives both inline-image placement computation (in the UI layer)
    /// and the per-line text layout (in `pane_body`). When `None`, the
    /// painter falls back to building a projection from
    /// `decorations` + `selections` locally — kept for the unit tests
    /// that construct a `PaneBodyDraw` directly.
    pub frame_display: Option<&'a crate::display_projection::FrameDisplay>,
    /// Pre-computed inline-image placements for this pane's buffer.
    /// Empty when the buffer has no `![](url)` spans or
    /// `[markdown].inline_images` is off.
    pub images: &'a [InlineImagePlacement],
    /// Render this pane's scaled-text minimap.
    pub minimap: bool,
    /// Render this pane's right-docked outline sidebar.
    pub show_outline_sidebar: bool,
    /// `true` for the focused pane; `false` for spectators. Spectators
    /// suppress caret painting; the focused body uses the legacy paint
    /// path and is *not* listed here.
    pub is_focused: bool,
}

/// Per-frame draw parameters that don't fit naturally on the renderer or
/// view state.
pub struct DrawParams<'a> {
    /// Document identifier (`BufferId.as_uuid().as_u128()`).
    pub document: u128,
    /// Text format that the cached layouts were built against.
    pub format: &'a IDWriteTextFormat,
    /// Hash of `(font_family, size_dip, locale)` describing `format`.
    pub font_state: FontStateId,
    /// Fingerprint of the active theme content for retained chrome
    /// invalidation.
    pub theme_revision: u64,
    /// Current per-window DPI scale relative to 96 DPI.
    pub dpi_scale: f32,
    /// Current wheel-scroll velocity in DIPs per second.
    pub scroll_velocity_dip_per_s: f32,
    /// Pane id currently targeted by wheel inertia, formatted by trace
    /// consumers as a fixed-width hex value. Zero means no active target.
    pub scroll_target_pane_id: u128,
    /// Pane id that owns keyboard focus during this paint.
    pub scroll_focused_pane_id: u128,
    /// `true` when the active wheel impulse landed on a non-focused pane.
    pub scroll_hover_routed: bool,
    /// Logical line height in DIPs.
    pub line_height: f32,
    /// Base font size in DIPs (before zoom and heading scale).
    pub base_font_size_dip: f32,
    /// Heading scale multipliers per level (`[h1, h2, …, h6]`).
    pub heading_scale: [f32; 6],
    /// Per-pane view state (scroll, zoom, soft wrap).
    pub view: &'a ViewState,
    /// Theme-derived colors.
    pub colors: EditorColors,
    /// Markdown-specific colors.
    pub markdown_colors: MarkdownColors,
    /// Phase 11 view-toggle state.
    pub view_options: ViewOptionsDraw<'a>,
    /// Optional decoration snapshot. `None` ⇒ Phase-9 plain-text path.
    pub decorations: Option<&'a Decorations>,
    /// Phase F3 — inline-color spans to paint over the body text. Borrow
    /// of `decorations.inline_color_spans` when present; the explicit slot
    /// lets the painter avoid re-fetching the snapshot per line. Empty
    /// when no decoration snapshot is available or the buffer carries no
    /// inline-color markup.
    pub inline_color_spans: &'a [InlineColorSpan],
    /// Phase F4 — per-table formula overrides. The renderer iterates
    /// per-cell overrides to substitute the computed-value text for the
    /// source bytes (caret-out only — caret-in reveals the formula
    /// source so the user can edit). Empty when no decoration snapshot
    /// is available or no table block carries a formula cell.
    pub table_overrides: &'a [EvaluatedTable],
    /// Pipe-table visual layouts for blocks whose caret is outside.
    /// The visual renderer (`crate::table_paint`) draws cell borders,
    /// header background, and per-column-aligned text on top of the
    /// hidden-pipes-but-visible-cell-text projection the display map
    /// produces. Empty when no decoration snapshot is available, no
    /// pipe-table blocks exist, or every block has a caret inside (the
    /// raw-markdown reveal view).
    pub table_layouts: &'a [TableLayout],
    /// Optional overlay panel painted on top of the editor body.
    pub overlay: Option<&'a OverlayDraw>,
    /// Per-frame motion projection for [`Self::overlay`].
    pub overlay_motion: Option<SurfaceMotion>,
    /// Optional passive chord HUD. It paints above modal overlays and
    /// never owns input focus.
    pub chord_hud: Option<&'a OverlayDraw>,
    /// Per-frame motion projection for [`Self::chord_hud`].
    pub chord_hud_motion: Option<SurfaceMotion>,
    /// Phase-13 origin for body painting in client DIPs. Defaults to
    /// `(0.0, 0.0)`. Non-zero values shift the rendered editor body so
    /// the focused pane occupies its rect rather than the full window.
    pub body_origin: (f32, f32),
    /// Phase-13 chrome (tab strips + pane borders). `None` keeps the
    /// Phase-12 single-frame look.
    pub pane_chrome: Option<&'a PaneChromeDraw>,
    /// Phase-16.5 squiggle overlay — one entry per misspelled word in
    /// the active buffer. Empty when spell-check is disabled or the
    /// active buffer has no errors. Painted on top of the body text but
    /// underneath chrome / overlays.
    pub spell_spans: &'a [SpellSquiggleSpan],
    /// Phase-16.5 non-focused pane bodies. The renderer iterates this
    /// slice after the focused body has painted, drawing each entry
    /// inside its own clip rect with the per-pane view, decoration
    /// snapshot, and selection set. The focused body is *not* listed
    /// here — it is painted from the top-level [`DrawParams`] fields.
    pub pane_bodies: &'a [PaneBodyDraw<'a>],
    /// Phase-17.6 display projection.
    pub frame_display: &'a FrameDisplay,
    /// Hovered focused-pane source/display row, when the pointer is in body.
    pub line_hover: Option<crate::LineHoverDraw>,
    /// Full window client height in DIPs (post DPI scale). Used to
    /// position the global status bar at the window's bottom even
    /// when multiple panes stack vertically — `view.viewport_height_dip`
    /// is per-pane and would mis-place the bar in a split layout.
    pub client_height_dip: f32,
    /// Phase C1: status-bar segment data + theme colors. `None` means
    /// the UI didn't build a status bar this frame (e.g. early init
    /// before snapshots are ready) — the renderer paints a blank strip
    /// only if `view_options.show_status_bar` is also set.
    pub status_bar: Option<&'a StatusBarData<'a>>,
    /// Optional left file-tree pane. Painted as layout chrome below
    /// modal overlays and above the editor background.
    pub file_tree: Option<&'a FileTreeDraw>,
    /// Optional destination-row acknowledgement glow.
    pub jump_glow: Option<JumpGlowDraw>,
    /// α.1 edit-action echo tint. Painted after the body and any jump
    /// glow so it overlays text without competing with overlay/chrome.
    pub edit_pulse: Option<EditPulseDraw>,
    /// Phase F1: sticky heading breadcrumb data. `None` means the UI
    /// did not build a breadcrumb this frame (no decorations yet, or
    /// caret precedes every heading); the renderer paints nothing in
    /// the breadcrumb strip in that case. The strip itself only takes
    /// vertical space when `view_options.show_sticky_breadcrumb` is
    /// `true`.
    pub breadcrumb: Option<&'a BreadcrumbData<'a>>,
    /// Phase F2: outline-sidebar data. `None` means the UI did not
    /// build an outline this frame (no decorations yet, or buffer has
    /// no headings); the renderer paints only the empty strip when
    /// `view_options.show_outline_sidebar` is set.
    pub outline: Option<&'a OutlineData<'a>>,
    /// Phase G4: search-active minimap strip payload. `None` means the
    /// strip is not painted this frame (find bar closed or no matches).
    /// When `Some`, the renderer paints the strip on the right edge of
    /// the focused pane body; the UI also sets
    /// `view_options.search_minimap_active` so the body reflows away
    /// from the reserved column.
    pub search_minimap: Option<&'a SearchMinimapDraw>,
    /// Phase F5 Pass 2: per-frame inline-image placements. One entry
    /// per `![](url)` span the renderer should paint into the focused
    /// pane's body. `None` (or empty slice) means "no inline images
    /// this frame" — the renderer skips its image-paint pass. See
    /// [`InlineImagePlacement`].
    pub images: Option<&'a [InlineImagePlacement]>,
    /// Phase I1: time-machine slider HUD payload. `None` (the default)
    /// means the slider is not visible this frame and the renderer
    /// skips its HUD paint pass. When `Some`, the renderer paints the
    /// band/track/ticks/thumb between the pane-chrome and overlay
    /// passes — see [`crate::time_machine_hud_paint::paint_time_machine_hud`].
    pub time_machine_hud: Option<&'a crate::TimeMachineHudDraw>,
    /// P0.8.3: transient "building view" overlay drawn over a stale
    /// frame while paint waits on a slow projection-worker build. UI
    /// sets `Some` only when its loading-overlay state machine is
    /// armed; the renderer paints nothing when `None`.
    pub loading_overlay: Option<&'a LoadingOverlayDraw>,
    /// P0.8.3: per-frame motion projection for [`Self::loading_overlay`].
    /// Reduced motion collapses this to `SurfaceMotion::IDENTITY`.
    pub loading_overlay_motion: Option<SurfaceMotion>,
    /// Per-frame copy-button overlay for a fenced code block whose
    /// rendered rect contains the cursor and whose caret is outside.
    /// `None` when no fenced block is currently hovered or all carets
    /// sit inside one. The renderer paints the button on top of the
    /// body text but underneath chrome and overlays so the user-facing
    /// click target lines up exactly with what the UI hit-tests. See
    /// [`crate::code_copy_button_paint`] for the type definition and
    /// the painter that consumes it.
    pub code_copy_button: Option<crate::code_copy_button_paint::CodeCopyButtonDraw>,
}
