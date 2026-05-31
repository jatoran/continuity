//! Buffer-history swimlane panel — custom Direct2D paint surface
//! for the buffer-history visualization tab.
//!
//! Layout + paint in one file (compact swimlane); modeled on
//! [`crate::metrics_panel_paint`] (same shape: pure-data
//! [`BufferHistoryPanelLayout`] consumed by
//! [`paint_buffer_history_panel`]). Paints inside the focused
//! pane's body rect after the regular pipeline has drawn the
//! tab strip / status bar / pane border — chrome stays untouched.
//!
//! Pure-projection layout:
//!  * lanes are laid out top-to-bottom inside the body rect; each
//!    carries a sortable title slot, an age subtitle, and a
//!    horizontal timeline strip,
//!  * snapshot dots project from `[viewport_start_ms, viewport_end_ms)`
//!    to pixel x in the lane's timeline strip,
//!  * the time-axis ruler at the top labels four buckets (today /
//!    this week / this month / older) so the user can find
//!    "yesterday" without parsing absolute dates.
//!
//! Thread ownership: a [`Renderer`] is bound to one HWND, so
//! [`paint_buffer_history_panel`] is implicitly UI-thread-only.

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ELLIPSE};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteTextFormat, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
};
use windows::Win32::Graphics::Dxgi::DXGI_PRESENT;

use crate::params::Rgba;
use crate::renderer::Renderer;
use crate::Error;

/// Pure-data rect used by the layout — `(x, y, w, h)` in DIPs. Local
/// to avoid a render-crate dependency cycle on `windows-sys` types in
/// public layout APIs.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PanelRect {
    /// X origin in DIPs.
    pub x: f32,
    /// Y origin in DIPs.
    pub y: f32,
    /// Width in DIPs.
    pub w: f32,
    /// Height in DIPs.
    pub h: f32,
}

/// Color palette consumed by the buffer-history-panel paint.
#[derive(Copy, Clone, Debug)]
pub struct BufferHistoryPanelColors {
    /// Body background fill.
    pub background: Rgba,
    /// Primary text (lane titles, ruler labels).
    pub foreground: Rgba,
    /// Muted text (subtitle / "no buffers yet" placeholder).
    pub muted_foreground: Rgba,
    /// Snapshot dot fill.
    pub snapshot_dot: Rgba,
    /// Color of the selection chrome around the highlighted lane.
    pub selected_outline: Rgba,
    /// Hovered lane background fill.
    pub hovered_background: Rgba,
    /// Lane row separator / time-axis ruler color.
    pub rule: Rgba,
    /// Strong separator between the lane list and the preview band.
    pub preview_divider: Rgba,
}

/// One swimlane row to render. The UI crate projects its
/// [`crate::params::Rgba`]-naïve persisted record into this shape so
/// the render crate never depends on `continuity-persist`.
#[derive(Clone, Debug)]
pub struct BufferHistoryRowDraw {
    /// Tab title (already truncated by the caller — used verbatim).
    pub title: String,
    /// Age subtitle (e.g. `"3h ago · 21 edits"` — projection of the
    /// caller's humanized timestamps).
    pub subtitle: String,
    /// Snapshot times in unix milliseconds, ascending.
    pub snapshot_times_ms: Vec<i64>,
    /// `true` to gray out the row (the underlying buffer is trashed).
    pub is_trashed: bool,
    /// First ~6 lines of the latest snapshot content. Rendered in the
    /// preview band at the bottom of the panel when this lane is
    /// hovered (or selected, when no hover). `None` falls back to a
    /// muted placeholder.
    pub preview: Option<String>,
}

/// Full per-frame draw payload.
#[derive(Clone, Debug)]
pub struct BufferHistoryPanelDraw {
    /// Outer panel rect (pane body rect, tab-strip-subtracted).
    pub rect: PanelRect,
    /// One row per visible lane, top-to-bottom.
    pub rows: Vec<BufferHistoryRowDraw>,
    /// Time-axis viewport lower bound (unix ms).
    pub viewport_start_ms: i64,
    /// Time-axis viewport upper bound (unix ms).
    pub viewport_end_ms: i64,
    /// Wall-clock now (unix ms) — drives the ruler bucket labels.
    pub now_ms: i64,
    /// Active filter discriminant as a human-readable string
    /// (e.g. `"active"`, `"all"`, `"trash"`). Painted in the ruler.
    pub filter_label: String,
    /// Index of the keyboard-selected lane, when in range.
    pub selected_lane: Option<usize>,
    /// Index of the hovered lane, when in range.
    pub hovered_lane: Option<usize>,
    /// First visible lane index — applied by the layout to scroll
    /// the swimlane list vertically when there are more buffers
    /// than the panel can show in one screen.
    pub scroll_lane_offset: usize,
}

/// Height of the bottom preview band when populated.
pub const PREVIEW_BAND_HEIGHT_DIP: f32 = 110.0;

/// Per-row computed rects + dot positions. Returned by
/// [`compute_buffer_history_panel_layout`] so callers can hit-test
/// pointer events without re-running the math.
#[derive(Clone, Debug)]
pub struct BufferHistoryLaneLayout {
    /// Index into the parent [`BufferHistoryPanelDraw::rows`] vector.
    /// May differ from the lane's position in
    /// [`BufferHistoryPanelLayout::lanes`] when `scroll_lane_offset`
    /// is non-zero.
    pub lane_index: usize,
    /// Full row rect (background-fill target).
    pub row_rect: PanelRect,
    /// Subrect inside `row_rect` reserved for the timeline strip
    /// (right of the title column).
    pub strip_rect: PanelRect,
    /// Snapshot-dot centers projected into `strip_rect`. One entry
    /// per snapshot timestamp that falls inside the viewport.
    pub dot_centers_x: Vec<f32>,
}

/// Computed layout for the whole panel: the ruler band on top,
/// one [`BufferHistoryLaneLayout`] per row, and the preview band at
/// the bottom (when populated).
#[derive(Clone, Debug)]
pub struct BufferHistoryPanelLayout {
    /// Background rect of the entire panel.
    pub background_rect: PanelRect,
    /// Ruler band rect (top of the panel).
    pub ruler_rect: PanelRect,
    /// Per-row geometry (filtered by `scroll_lane_offset` — the
    /// `lane_index` field carries the index into the original
    /// `draw.rows` so hit-tests stay consistent).
    pub lanes: Vec<BufferHistoryLaneLayout>,
    /// Preview band rect (bottom of the panel). `None` when no lanes
    /// are present or when the panel is too short to spare the
    /// vertical space.
    pub preview_rect: Option<PanelRect>,
    /// Lane-list scrollbar when the history has more rows than fit.
    pub scrollbar: Option<BufferHistoryScrollbarLayout>,
    /// Number of lane rows that fit in the current lane-list viewport.
    pub visible_lane_capacity: usize,
}

/// Computed geometry for the buffer-history lane-list scrollbar.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BufferHistoryScrollbarLayout {
    /// Full track rect.
    pub track_rect: PanelRect,
    /// Thumb rect, proportional to visible lanes vs. total lanes.
    pub thumb_rect: PanelRect,
}

/// Height of the time-axis ruler band in DIPs. Splits into a
/// top header row (`HEADER_ROW_HEIGHT_DIP`) and a tick row below
/// it carrying the date labels.
pub const RULER_HEIGHT_DIP: f32 = 52.0;
/// Sub-band inside the ruler reserved for the header chip line.
pub const HEADER_ROW_HEIGHT_DIP: f32 = 24.0;
/// Height of a single swimlane row in DIPs. Generous enough that
/// the title row, subtitle row, and timeline strip get their own
/// vertical band without visual overlap; gap below each row is
/// painted with the rule color for a clean separator.
pub const LANE_HEIGHT_DIP: f32 = 56.0;
/// Width of the lane-title column (left of the timeline strip).
pub const TITLE_COLUMN_WIDTH_DIP: f32 = 220.0;
/// Inner horizontal padding inside `background_rect`.
pub const PANEL_PAD_DIP: f32 = 12.0;
/// Radius (DIPs) of the snapshot dot.
pub const SNAPSHOT_DOT_RADIUS_DIP: f32 = 3.5;

/// Paint the buffer-history panel and present.
///
/// # Errors
///
/// Returns [`Error`] if any Direct2D / DXGI call fails.
pub fn paint_buffer_history_panel(
    renderer: &Renderer,
    draw: &BufferHistoryPanelDraw,
    colors: BufferHistoryPanelColors,
    text_format: &IDWriteTextFormat,
) -> Result<(), Error> {
    paint_buffer_history_panel_no_present(renderer, draw, colors, text_format)?;
    unsafe {
        renderer.swap_chain.Present(0, DXGI_PRESENT(0)).ok()?;
    }
    Ok(())
}

/// Paint pass without the final `Present`. Symmetric to
/// [`crate::metrics_panel_paint::paint_metrics_panel_no_present`] so a
/// capture canary can sample the back buffer before flip-discard
/// makes its contents undefined.
///
/// # Errors
///
/// Returns [`Error`] if any Direct2D call fails.
pub fn paint_buffer_history_panel_no_present(
    renderer: &Renderer,
    draw: &BufferHistoryPanelDraw,
    colors: BufferHistoryPanelColors,
    text_format: &IDWriteTextFormat,
) -> Result<(), Error> {
    let layout = compute_buffer_history_panel_layout(draw);
    let bg = rgba_to_d2d(colors.background);
    let fg = rgba_to_d2d(colors.foreground);
    let muted = rgba_to_d2d(colors.muted_foreground);
    let dot = rgba_to_d2d(colors.snapshot_dot);
    let outline = rgba_to_d2d(colors.selected_outline);
    let hover = rgba_to_d2d(colors.hovered_background);
    let rule = rgba_to_d2d(colors.rule);
    let divider = rgba_to_d2d(colors.preview_divider);
    let render_target: ID2D1RenderTarget = renderer.d2d_context.cast()?;
    unsafe {
        renderer.d2d_context.BeginDraw();
        let bg_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&bg, None)?;
        let fg_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&fg, None)?;
        let muted_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&muted, None)?;
        let dot_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&dot, None)?;
        let outline_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&outline, None)?;
        let hover_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&hover, None)?;
        let rule_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&rule, None)?;
        let divider_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&divider, None)?;

        // Body fill — overdraw only the panel rect; chrome stays.
        let body = panel_rect_to_d2d(layout.background_rect);
        renderer.d2d_context.FillRectangle(&body, &bg_brush);

        // Ruler band: two stacked rows — header chip line on top
        // ("Buffer history · active buffers" left, bucket-count hint right)
        // and a tick row below it carrying date labels aligned with
        // vertical guides that extend down through the lanes area.
        let ruler = panel_rect_to_d2d(layout.ruler_rect);
        let ruler_border = D2D_RECT_F {
            top: ruler.bottom - 1.0,
            bottom: ruler.bottom,
            ..ruler
        };
        renderer
            .d2d_context
            .FillRectangle(&ruler_border, &rule_brush);
        let header_rect = PanelRect {
            x: layout.ruler_rect.x,
            y: layout.ruler_rect.y,
            w: layout.ruler_rect.w,
            h: HEADER_ROW_HEIGHT_DIP.min(layout.ruler_rect.h),
        };
        draw_label(
            renderer,
            text_format,
            &header_rect,
            &format!("Buffer history · {}", draw.filter_label),
            &fg_brush,
            DWRITE_TEXT_ALIGNMENT_LEADING,
        )?;
        if let Some(hint) = ruler_bucket_hint(draw) {
            draw_label(
                renderer,
                text_format,
                &header_rect,
                &hint,
                &muted_brush,
                DWRITE_TEXT_ALIGNMENT_TRAILING,
            )?;
        }
        // Tick row + vertical guides. Ticks are placed at "nice"
        // calendar intervals derived from the viewport span (hourly
        // / daily / weekly / monthly), so the user can read absolute
        // times off the chart without doing mental arithmetic.
        let strip_x_for_ticks = layout
            .lanes
            .first()
            .map(|l| l.strip_rect.x)
            .unwrap_or(layout.ruler_rect.x + PANEL_PAD_DIP);
        let strip_w_for_ticks = layout
            .lanes
            .first()
            .map(|l| l.strip_rect.w)
            .unwrap_or((layout.ruler_rect.w - 2.0 * PANEL_PAD_DIP).max(0.0));
        let tick_band_rect = PanelRect {
            x: strip_x_for_ticks,
            y: layout.ruler_rect.y + HEADER_ROW_HEIGHT_DIP,
            w: strip_w_for_ticks,
            h: (layout.ruler_rect.h - HEADER_ROW_HEIGHT_DIP).max(0.0),
        };
        let guides_bottom = match layout.preview_rect {
            Some(p) => p.y,
            None => layout.background_rect.y + layout.background_rect.h,
        };
        for tick in compute_time_axis_ticks(
            draw.viewport_start_ms,
            draw.viewport_end_ms,
            strip_w_for_ticks,
        ) {
            let frac = ((tick.ts_ms - draw.viewport_start_ms) as f64
                / (draw.viewport_end_ms - draw.viewport_start_ms).max(1) as f64)
                as f32;
            let x = tick_band_rect.x + frac * tick_band_rect.w;
            // Faint vertical guide running from the bottom of the
            // ruler down to the top of the preview band. Pre-multipy
            // alpha already lives in the brush; we re-use `rule_brush`
            // which is the lane-separator color.
            let guide = D2D_RECT_F {
                left: x - 0.5,
                right: x + 0.5,
                top: layout.ruler_rect.y + layout.ruler_rect.h,
                bottom: guides_bottom,
            };
            renderer.d2d_context.FillRectangle(&guide, &rule_brush);
            // Tick label centered above the guide. We render with a
            // ~80 DIP label box so adjacent labels don't crowd.
            let label_rect = PanelRect {
                x: x - 50.0,
                y: tick_band_rect.y,
                w: 100.0,
                h: tick_band_rect.h,
            };
            draw_label(
                renderer,
                text_format,
                &label_rect,
                &tick.label,
                &muted_brush,
                windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_CENTER,
            )?;
        }

        // Empty-state placeholder.
        if draw.rows.is_empty() {
            let mut placeholder = layout.background_rect;
            placeholder.y = layout.ruler_rect.y + layout.ruler_rect.h + 24.0;
            placeholder.h = 28.0;
            draw_label(
                renderer,
                text_format,
                &placeholder,
                "No buffers in history yet. Notes appear here after they have content or a file path.",
                &muted_brush,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;
        }

        // Lanes. `i` is the *visible* index in `layout.lanes`, but
        // every per-row data lookup (title, subtitle, preview,
        // trashed flag) has to go through `lane.lane_index` — the
        // ORIGINAL index into `draw.rows`. With a non-zero
        // `scroll_lane_offset` those two differ, and using `i`
        // would paint row 0/1/2… titles next to the actual
        // (scrolled) buffer's dots, which is the bug that made
        // scrolled-rows show "wrong title for the dot column" and
        // click-vs-preview mismatch.
        for lane in layout.lanes.iter() {
            let Some(row) = draw.rows.get(lane.lane_index) else {
                continue;
            };
            if draw.hovered_lane == Some(lane.lane_index) {
                let hover_rect = panel_rect_to_d2d(lane.row_rect);
                renderer
                    .d2d_context
                    .FillRectangle(&hover_rect, &hover_brush);
            }
            // Bottom separator on every row.
            let sep = D2D_RECT_F {
                left: lane.row_rect.x,
                right: lane.row_rect.x + lane.row_rect.w,
                top: lane.row_rect.y + lane.row_rect.h - 1.0,
                bottom: lane.row_rect.y + lane.row_rect.h,
            };
            renderer.d2d_context.FillRectangle(&sep, &rule_brush);

            // Selection outline.
            if draw.selected_lane == Some(lane.lane_index) {
                let outline_rect = panel_rect_to_d2d(PanelRect {
                    x: lane.row_rect.x + 2.0,
                    y: lane.row_rect.y + 2.0,
                    w: (lane.row_rect.w - 4.0).max(0.0),
                    h: (lane.row_rect.h - 4.0).max(0.0),
                });
                renderer
                    .d2d_context
                    .DrawRectangle(&outline_rect, &outline_brush, 1.5, None);
            }

            // Title (top half of the title column). Generous top
            // padding now that `LANE_HEIGHT_DIP` is taller; the
            // subtitle sits below with its own row instead of
            // butting up against the title baseline.
            let title_color = if row.is_trashed {
                &muted_brush
            } else {
                &fg_brush
            };
            let title_rect = PanelRect {
                x: lane.row_rect.x + PANEL_PAD_DIP,
                y: lane.row_rect.y + 8.0,
                w: (TITLE_COLUMN_WIDTH_DIP - PANEL_PAD_DIP).max(0.0),
                h: 22.0,
            };
            let displayed_title = if row.is_trashed {
                format!("[trash] {}", row.title)
            } else {
                row.title.clone()
            };
            draw_label(
                renderer,
                text_format,
                &title_rect,
                &displayed_title,
                title_color,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;

            // Subtitle row, anchored below the title with a 2-DIP gap.
            let sub_rect = PanelRect {
                x: title_rect.x,
                y: title_rect.y + title_rect.h + 2.0,
                w: title_rect.w,
                h: 18.0,
            };
            draw_label(
                renderer,
                text_format,
                &sub_rect,
                &row.subtitle,
                &muted_brush,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;

            // Timeline strip background (faint rule along the
            // vertical midline so empty lanes still read as a
            // timeline).
            let mid_y = lane.strip_rect.y + lane.strip_rect.h * 0.5;
            let midline = D2D_RECT_F {
                left: lane.strip_rect.x,
                right: lane.strip_rect.x + lane.strip_rect.w,
                top: mid_y - 0.5,
                bottom: mid_y + 0.5,
            };
            renderer.d2d_context.FillRectangle(&midline, &rule_brush);

            // Snapshot dots.
            for &cx in &lane.dot_centers_x {
                let ellipse = D2D1_ELLIPSE {
                    point: windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F {
                        x: cx,
                        y: mid_y,
                    },
                    radiusX: SNAPSHOT_DOT_RADIUS_DIP,
                    radiusY: SNAPSHOT_DOT_RADIUS_DIP,
                };
                renderer.d2d_context.FillEllipse(&ellipse, &dot_brush);
            }
        }

        scrollbar::paint_scrollbar(&renderer.d2d_context, &layout, &rule_brush, &muted_brush);

        // Preview band: hovered lane wins over selected. Renders the
        // lane's title (foreground) and its preview text (muted)
        // pulled from `BufferHistoryRowDraw::preview`. When neither
        // hover nor selection is set, a hint paragraph fills the
        // band.
        if let Some(band) = layout.preview_rect {
            let band_rect = panel_rect_to_d2d(band);
            renderer.d2d_context.FillRectangle(&band_rect, &bg_brush);
            let top_border = D2D_RECT_F {
                left: band.x,
                right: band.x + band.w,
                top: band.y,
                bottom: band.y + 2.0,
            };
            renderer
                .d2d_context
                .FillRectangle(&top_border, &divider_brush);
            let target = draw.hovered_lane.or(draw.selected_lane);
            if let Some(idx) = target.and_then(|i| draw.rows.get(i)) {
                let title_rect = PanelRect {
                    x: band.x + PANEL_PAD_DIP,
                    y: band.y + 8.0,
                    w: (band.w - 2.0 * PANEL_PAD_DIP).max(0.0),
                    h: 20.0,
                };
                draw_label(
                    renderer,
                    text_format,
                    &title_rect,
                    &idx.title,
                    &fg_brush,
                    DWRITE_TEXT_ALIGNMENT_LEADING,
                )?;
                let preview_text = idx
                    .preview
                    .as_deref()
                    .unwrap_or("(no persisted content preview)");
                let preview_rect = PanelRect {
                    x: title_rect.x,
                    y: title_rect.y + title_rect.h + 4.0,
                    w: title_rect.w,
                    h: (band.h - (title_rect.h + 16.0)).max(0.0),
                };
                draw_multiline_label(
                    renderer,
                    text_format,
                    &preview_rect,
                    preview_text,
                    &muted_brush,
                )?;
            } else {
                let hint_rect = PanelRect {
                    x: band.x + PANEL_PAD_DIP,
                    y: band.y + 12.0,
                    w: (band.w - 2.0 * PANEL_PAD_DIP).max(0.0),
                    h: band.h - 24.0,
                };
                draw_label(
                    renderer,
                    text_format,
                    &hint_rect,
                    "Latest persisted content preview appears here.",
                    &muted_brush,
                    DWRITE_TEXT_ALIGNMENT_LEADING,
                )?;
            }
        }

        renderer.d2d_context.EndDraw(None, None)?;
    }
    Ok(())
}

pub mod layout;
mod paint_helpers;
mod scrollbar;
mod time_axis;

#[cfg(test)]
mod tests;

use layout::compute_buffer_history_panel_layout;
use paint_helpers::{
    draw_label, draw_multiline_label, panel_rect_to_d2d, rgba_to_d2d, ruler_bucket_hint,
};
use time_axis::compute_time_axis_ticks;
