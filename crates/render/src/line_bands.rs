//! Subtle whole-line bands for hover and selected-line orientation.
//!
//! The painter consumes the already-built [`crate::FrameDisplay`] row
//! index. It never builds layouts, measures text, or asks the display
//! map to realize additional rows.

use continuity_text::{Selection, SelectionKind};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};

use crate::chrome::ContentMargins;
use crate::params::Rgba;
use crate::FrameDisplay;

/// Focused-pane line currently under the mouse pointer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineHoverDraw {
    /// Source line under the pointer.
    pub source_line: u32,
    /// Absolute display row under the pointer.
    pub display_row: u32,
    /// `true` when the pointer is inside the gutter strip.
    pub in_gutter: bool,
}

#[must_use]
pub(crate) fn scaled_alpha(mut color: Rgba, scale: f32) -> Rgba {
    color.a = (color.a * scale).clamp(0.0, 1.0);
    color
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_line_bands(
    ctx: &ID2D1DeviceContext,
    selections: &[Selection],
    frame_display: &FrameDisplay,
    hover: Option<LineHoverDraw>,
    line_height: f32,
    scroll_y: f32,
    viewport_h: f32,
    viewport_w: f32,
    margins: ContentMargins,
    gutter_indicator_width: f32,
    selected_brush: &ID2D1SolidColorBrush,
    hover_brush: &ID2D1SolidColorBrush,
    gutter_hover_brush: &ID2D1SolidColorBrush,
) {
    let has_selected_lines = selections.iter().any(|selection| !selection.is_collapsed());
    if !has_selected_lines && hover.is_none() {
        return;
    }
    let total_rows = frame_display.display_line_count();
    if total_rows == 0 {
        return;
    }
    let first_row = ((scroll_y / line_height).floor() as i64).max(0) as u32;
    let last_row = ((((scroll_y + viewport_h) / line_height).ceil() as i64) + 1)
        .clamp(0, i64::from(total_rows)) as u32;
    for display_row in first_row..last_row {
        let Some((source_line, _)) = frame_display
            .row_index()
            .source_line_for_display_row(display_row)
        else {
            continue;
        };
        let source_line = source_line.raw();
        let is_hovered_line = hover.is_some_and(|hover| hover.source_line == source_line);
        let is_selected_line = has_selected_lines
            && selections
                .iter()
                .any(|selection| selection_intersects_line(*selection, source_line));
        if is_selected_line {
            paint_body_band(
                ctx,
                display_row,
                line_height,
                scroll_y,
                viewport_w,
                margins,
                selected_brush,
            );
        }
        if is_hovered_line {
            paint_body_band(
                ctx,
                display_row,
                line_height,
                scroll_y,
                viewport_w,
                margins,
                hover_brush,
            );
        }
    }
    if let Some(hover) = hover.filter(|_| gutter_indicator_width > 0.0) {
        paint_gutter_band(
            ctx,
            hover.display_row,
            line_height,
            scroll_y,
            gutter_indicator_width,
            gutter_hover_brush,
        );
    }
}

fn paint_body_band(
    ctx: &ID2D1DeviceContext,
    display_row: u32,
    line_height: f32,
    scroll_y: f32,
    viewport_w: f32,
    margins: ContentMargins,
    brush: &ID2D1SolidColorBrush,
) {
    let top = display_row as f32 * line_height - scroll_y;
    let rect = D2D_RECT_F {
        left: margins.left,
        top,
        right: (viewport_w - margins.right).max(margins.left),
        bottom: top + line_height,
    };
    unsafe { ctx.FillRectangle(&rect, brush) };
}

fn paint_gutter_band(
    ctx: &ID2D1DeviceContext,
    display_row: u32,
    line_height: f32,
    scroll_y: f32,
    gutter_right: f32,
    brush: &ID2D1SolidColorBrush,
) {
    let top = display_row as f32 * line_height - scroll_y;
    let rect = D2D_RECT_F {
        left: 0.0,
        top,
        right: gutter_right.max(0.0),
        bottom: top + line_height,
    };
    unsafe { ctx.FillRectangle(&rect, brush) };
}

fn selection_intersects_line(selection: Selection, source_line: u32) -> bool {
    if selection.is_collapsed() {
        return false;
    }
    if selection.kind == SelectionKind::BlockWise {
        let start = selection.anchor.line.min(selection.head.line);
        let end = selection.anchor.line.max(selection.head.line);
        return source_line >= start && source_line <= end;
    }
    let range = selection.ordered_range();
    if range.start.line == range.end.line {
        return source_line == range.start.line
            && range.start.byte_in_line != range.end.byte_in_line;
    }
    if source_line == range.start.line {
        return true;
    }
    if source_line == range.end.line {
        return range.end.byte_in_line > 0;
    }
    source_line > range.start.line && source_line < range.end.line
}
