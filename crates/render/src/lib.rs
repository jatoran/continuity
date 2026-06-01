#![warn(missing_docs)]
//! Direct2D draw commands, DXGI swap-chain management, and frame
//! presentation with frame-latency waitable.

pub mod breadcrumb;
pub mod buffer_history_panel;
pub mod chrome;
mod chrome_caret;
mod chrome_centered;
mod chrome_command_list;
// §H3 fold-triangle painter; called from `chrome_post::paint_post_text_chrome`.
pub mod chrome_fold;
mod chrome_line_numbers;
mod chrome_post;
pub mod code_copy_button_paint;
pub mod decoration_paint;
pub mod display_projection;
pub mod error;
pub mod inline_code_hit;
mod inline_code_paint;
mod inline_color_paint;
mod inline_image_types;
mod line_bands;
mod markdown_extension_paint;
mod table_chrome_cache;
mod table_formula_paint;
pub mod table_layout;
mod table_paint;
mod table_suppress;
// §H1 focus-mode dim painter; called from the body-glyph pass after
// the main paint.
mod focus_dim;
// §F5 Pass 2 — inline image rendering. Layout is pure; cache and
// paint pull in WIC / D2D wiring.
mod edit_pulse_paint;
pub mod file_tree;
pub mod file_tree_paint;
pub mod image_cache;
pub mod image_layout;
pub mod image_paint;
mod jump_glow_paint;
pub mod loading_overlay;
pub mod metrics_panel;
pub mod metrics_panel_paint;
pub mod minimap;
mod minimap_paint;
pub mod motion;
pub mod outline;
pub mod outline_paint;
pub mod overlay;
mod overlay_motion;
mod overlay_scrollbar;
pub mod pane_body;
pub mod pane_chrome;
mod pane_chrome_border;
mod pane_chrome_slide;
mod pane_chrome_tabs;
pub mod params;
pub mod render_stats;
pub mod renderer;
pub mod renderer_capture;
mod renderer_draw_main;
mod renderer_draw_stats;
mod renderer_focus_dim_pass;
mod renderer_gpu_memory;
mod renderer_image_cache;
mod renderer_line_text_pass;
mod renderer_misc;
mod renderer_post_body;
mod renderer_scroll_placeholder;
mod renderer_table_chrome;
pub mod scroll_fractional;
pub mod scroll_placeholder;
pub mod scrollbar;
mod search_highlight_paint;
mod search_minimap_paint;
mod spell;
mod status_bar;
mod tab_drag_paint;
mod text_role_effects;
pub mod time_machine_hud_paint;

pub use breadcrumb::{
    compute_breadcrumb_layout, estimate_text_width_dip as breadcrumb_estimate_text_width_dip,
    BreadcrumbColors, BreadcrumbData, BreadcrumbLayout, BreadcrumbSegment,
    SlotBounds as BreadcrumbSlotBounds, SlotKind as BreadcrumbSlotKind, BREADCRUMB_HEIGHT_DIP,
};
pub use buffer_history_panel::layout::{
    compute_buffer_history_panel_layout, hit_test_lane as buffer_history_hit_test_lane,
};
pub use buffer_history_panel::{
    paint_buffer_history_panel, paint_buffer_history_panel_no_present, BufferHistoryLaneLayout,
    BufferHistoryPanelColors, BufferHistoryPanelDraw, BufferHistoryPanelLayout,
    BufferHistoryRowDraw, PanelRect as BufferHistoryPanelRect, LANE_HEIGHT_DIP,
    PANEL_PAD_DIP as BUFFER_HISTORY_PANEL_PAD_DIP, RULER_HEIGHT_DIP, SNAPSHOT_DOT_RADIUS_DIP,
    TITLE_COLUMN_WIDTH_DIP,
};
pub use chrome_caret::caret_rect_for_shape;
pub use chrome_centered::{
    resolve_body_text_width_dip, resolve_body_text_width_for_line_count_dip,
};
pub use code_copy_button_paint::{
    paint_code_copy_button, CodeCopyButtonDraw, CodeCopyButtonFeedback,
};
pub use decoration_paint::{
    fenced_block_left_edge, fenced_block_right_edge, FENCED_BLOCK_LEFT_PADDING_DIP,
    FENCED_BLOCK_RIGHT_PADDING_DIP,
};
pub use file_tree::{
    FileTreeColors, FileTreeDraw, FileTreeEntryKind, FileTreeRowDraw, FILE_TREE_DEFAULT_WIDTH_DIP,
    FILE_TREE_HEADER_HEIGHT_DIP, FILE_TREE_ROW_HEIGHT_DIP,
};
pub use file_tree_paint::paint_file_tree_no_present;
pub use minimap::{
    compute_minimap_layout, hit_test as minimap_hit_test, MinimapColors, MinimapHit, MinimapLayout,
    MINIMAP_FONT_SIZE_DIP, MINIMAP_INNER_PADDING_DIP, MINIMAP_LINE_HEIGHT_DIP, MINIMAP_WIDTH_DIP,
};
pub use outline::{
    compute_outline_layout, compute_outline_scroll_offset,
    indent_for_level as outline_indent_for_level, OutlineColors, OutlineData, OutlineEntry,
    OutlineLayout, OutlineRowBounds, OUTLINE_DEFAULT_WIDTH_DIP, OUTLINE_LEVEL_INDENT_DIP,
    OUTLINE_ROW_HEIGHT_DIP, OUTLINE_ROW_INDENT_DIP,
};
pub use status_bar::{
    compute_layout as compute_status_bar_layout, estimate_segment_width_dip,
    min_slot_width_chars as status_bar_min_slot_width_chars, paint_status_bar, SegmentBounds,
    StatusBarColors, StatusBarData, StatusBarLayout, StatusBarSegmentDraw, StatusBarSegmentKind,
    STATUS_BAR_HEIGHT_DIP,
};
pub mod text_helpers;
pub mod text_metrics;
mod wrap_paint;

/// Default markdown heading scale. Spec §14 documents the *ideal*
/// hierarchy as `[2.0, 1.6, 1.35, 1.2, 1.1, 1.05]`, but the renderer
/// reserves one constant `LINE_HEIGHT_DIP` per logical row — so a glyph
/// rendered at 2.0× body height clips into the next row and looks like
/// overlapping text. Until per-row variable line heights land, the
/// renderer caps each level at `line_height / font_size ≈ 1.42` so the
/// hierarchy still reads "bigger at the top" without the visual clip.
pub const DEFAULT_HEADING_SCALE: [f32; 6] = [1.42, 1.32, 1.22, 1.14, 1.08, 1.04];
pub use display_projection::FrameDisplay;
pub use error::Error;
pub use image_cache::{ImageCache, ImageCacheError};
pub use image_layout::{compute_image_layout, ImageLayoutRect};
pub use inline_code_hit::InlineCodeHit;
pub use inline_code_paint::INLINE_CODE_BG_PAD_DIP;
pub use inline_image_types::{InlineImageHit, InlineImagePlacement};
pub use line_bands::LineHoverDraw;
pub use loading_overlay::{
    paint_loading_overlay, LoadingOverlayDraw, LOADING_OVERLAY_CORNER_RADIUS_DIP,
    LOADING_OVERLAY_HEIGHT_DIP, LOADING_OVERLAY_TOP_OFFSET_DIP, LOADING_OVERLAY_WIDTH_DIP,
};
pub use motion::{
    EditPulseDraw, JumpGlowDraw, StatusTransientDraw, StatusTransientGroup, SurfaceMotion,
};
pub use overlay::{
    paint_overlay, FocusField, FooterText, ListRow, OverlayDraw, OverlayScrollbar, PanelStyle, Rect,
};
pub use overlay_motion::paint_overlay_with_motion;
pub use pane_chrome::{
    paint_pane_chrome, tab_index_at, tab_slot_widths, tab_strip_layout, TabStripLayout,
    TabStripRow, BORDER_ACTIVE_DIP, BORDER_DIP, TAB_CLOSE_MIN_TAB_WIDTH_DIP, TAB_CLOSE_WIDTH_DIP,
    TAB_MIN_READABLE_WIDTH_DIP, TAB_MIN_WIDTH_DIP, TAB_PADDING_DIP,
};
pub use pane_chrome_tabs::close_button_rect;
pub use params::colors::{EditorColors, MarkdownColors, PanelColors};
pub use params::{
    CaretShape, DrawParams, PaneBodyDraw, PaneBodyDropHighlight, PaneChromeDraw, PaneStripDraw,
    Rgba, SearchMinimapDraw, SearchMinimapTickDraw, SpellSquiggleSpan, TabDragGhostDraw,
    TabDragOverlayDraw, TabDragSourceFade, TabLabel, TabStripInsertionBarDraw, ViewOptionsDraw,
};
pub use render_stats::chrome_overlay_breakdown::RendererChromeOverlayBreakdown;
pub use render_stats::draw_stages::RendererDrawStages;
pub use render_stats::{ChromePathMode, ChromePathStats, RenderStats, RendererPostBodyStages};
pub use renderer::Renderer;
pub use renderer_capture::CapturedBitmap;
pub use search_highlight_paint::SearchHighlightRangeDraw;
pub use search_minimap_paint::SEARCH_MINIMAP_WIDTH_DIP;
pub use tab_drag_paint::paint_tab_drag_overlay;
pub use table_chrome_cache::{TableChromePathMode, TableChromePathStats};
pub use table_layout::cell_wrap::CellLine;
pub use table_layout::directive::{
    format_table_directive, is_table_directive_line, parse_table_directive, TableDirective,
    TABLE_DIRECTIVE_PREFIX,
};
pub use table_layout::{
    compute_table_layouts, compute_table_layouts_with_overrides, table_row_reservations,
    TableCellLayout, TableColWidthOverride, TableLayout, DEFAULT_TABLE_COL_WIDTH_MAX_DIP,
    MAX_TABLE_COL_WIDTH_DIP, MIN_TABLE_COL_WIDTH_DIP, TABLE_CELL_PAD_DIP,
};
pub use table_suppress::compute_suppressed_table_blocks;
pub use text_helpers::{hit_test_x_to_byte, hit_test_x_to_byte_for_spec, utf16_index_to_utf8_byte};
pub use text_metrics::{DirectWriteCacheStats, DirectWriteWidthMeasure};
pub use time_machine_hud_paint::{paint_time_machine_hud, TimeMachineHudDraw, TimeMachineHudTick};
