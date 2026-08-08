//! Fixed vector icons for vault status-bar actions.
//!
//! Geometry is independent of the prose font, so icon shape and hit width
//! stay stable across themes, fallback fonts, and body zoom.

use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};

use super::{SegmentBounds, StatusBarSegmentKind, STATUS_BAR_HEIGHT_DIP};

const FILES_SLOT_WIDTH_DIP: f32 = 22.0;
const SETTINGS_SLOT_WIDTH_DIP: f32 = 17.0;
const RIGHT_ACTION_SLOT_WIDTH_DIP: f32 = 22.0;
const ICON_STROKE_DIP: f32 = 1.25;

/// Fixed hit/paint width for vector-backed action kinds.
#[must_use]
pub(super) const fn slot_width_dip(kind: StatusBarSegmentKind) -> Option<f32> {
    match kind {
        StatusBarSegmentKind::VaultLauncher => Some(FILES_SLOT_WIDTH_DIP),
        StatusBarSegmentKind::VaultFiles => Some(FILES_SLOT_WIDTH_DIP),
        StatusBarSegmentKind::VaultSettings => Some(SETTINGS_SLOT_WIDTH_DIP),
        StatusBarSegmentKind::VaultOutline | StatusBarSegmentKind::VaultMinimap => {
            Some(RIGHT_ACTION_SLOT_WIDTH_DIP)
        }
        _ => None,
    }
}

/// Paint one vector-backed status icon. Returns whether `kind` was handled.
pub(super) fn paint_status_bar_icon(
    context: &ID2D1DeviceContext,
    kind: StatusBarSegmentKind,
    bounds: SegmentBounds,
    top: f32,
    brush: &ID2D1SolidColorBrush,
    alpha: f32,
) -> bool {
    if slot_width_dip(kind).is_none() {
        return false;
    }
    unsafe { brush.SetOpacity(alpha.clamp(0.0, 1.0)) };
    match kind {
        StatusBarSegmentKind::VaultLauncher => paint_launcher(context, bounds, top, brush),
        StatusBarSegmentKind::VaultFiles => paint_sidebar(context, bounds, top, brush),
        StatusBarSegmentKind::VaultSettings => paint_settings(context, bounds, top, brush),
        StatusBarSegmentKind::VaultOutline => paint_outline(context, bounds, top, brush),
        StatusBarSegmentKind::VaultMinimap => paint_minimap(context, bounds, top, brush),
        _ => {}
    }
    unsafe { brush.SetOpacity(1.0) };
    true
}

fn paint_launcher(
    context: &ID2D1DeviceContext,
    bounds: SegmentBounds,
    top: f32,
    brush: &ID2D1SolidColorBrush,
) {
    let rect = centered_rect(bounds, top, 14.0, 12.0);
    let middle_x = (rect.left + rect.right) * 0.5;
    let middle_y = (rect.top + rect.bottom) * 0.5;
    for cell in [
        D2D_RECT_F {
            left: rect.left,
            top: rect.top,
            right: middle_x - 1.2,
            bottom: middle_y - 1.2,
        },
        D2D_RECT_F {
            left: middle_x + 1.2,
            top: rect.top,
            right: rect.right,
            bottom: middle_y - 1.2,
        },
        D2D_RECT_F {
            left: rect.left,
            top: middle_y + 1.2,
            right: middle_x - 1.2,
            bottom: rect.bottom,
        },
        D2D_RECT_F {
            left: middle_x + 1.2,
            top: middle_y + 1.2,
            right: rect.right,
            bottom: rect.bottom,
        },
    ] {
        draw_rect(context, cell, brush);
    }
}

fn paint_sidebar(
    context: &ID2D1DeviceContext,
    bounds: SegmentBounds,
    top: f32,
    brush: &ID2D1SolidColorBrush,
) {
    let rect = centered_rect(bounds, top, 15.0, 12.0);
    draw_rect(context, rect, brush);
    draw_line(
        context,
        rect.left + 5.0,
        rect.top,
        rect.left + 5.0,
        rect.bottom,
        brush,
    );
}

fn paint_settings(
    context: &ID2D1DeviceContext,
    bounds: SegmentBounds,
    top: f32,
    brush: &ID2D1SolidColorBrush,
) {
    let rect = centered_rect(bounds, top, 10.0, 8.0);
    for (row, knob_fraction) in [(0.0, 0.68), (4.0, 0.32), (8.0, 0.58)] {
        let y = rect.top + row;
        draw_line(context, rect.left, y, rect.right, y, brush);
        let knob_x = rect.left + (rect.right - rect.left) * knob_fraction;
        draw_line(context, knob_x, y - 1.4, knob_x, y + 1.4, brush);
    }
}

fn paint_outline(
    context: &ID2D1DeviceContext,
    bounds: SegmentBounds,
    top: f32,
    brush: &ID2D1SolidColorBrush,
) {
    let rect = centered_rect(bounds, top, 14.0, 11.0);
    for (row, line_fraction) in [(0.0, 1.0), (5.0, 0.72), (10.0, 0.88)] {
        let y = rect.top + row;
        draw_line(context, rect.left, y, rect.left + 1.0, y, brush);
        draw_line(
            context,
            rect.left + 4.0,
            y,
            rect.left + 4.0 + 10.0 * line_fraction,
            y,
            brush,
        );
    }
}

fn paint_minimap(
    context: &ID2D1DeviceContext,
    bounds: SegmentBounds,
    top: f32,
    brush: &ID2D1SolidColorBrush,
) {
    let rect = centered_rect(bounds, top, 11.0, 14.0);
    draw_rect(context, rect, brush);
    for (row, start, end) in [
        (2.0, 2.0, 8.0),
        (4.0, 2.0, 6.5),
        (6.0, 3.5, 9.0),
        (8.0, 2.0, 7.5),
        (10.0, 4.0, 9.0),
        (12.0, 2.0, 6.0),
    ] {
        draw_line(
            context,
            rect.left + start,
            rect.top + row,
            rect.left + end,
            rect.top + row,
            brush,
        );
    }
}

fn centered_rect(bounds: SegmentBounds, top: f32, width: f32, height: f32) -> D2D_RECT_F {
    let center_x = (bounds.left + bounds.right) * 0.5;
    let center_y = top + STATUS_BAR_HEIGHT_DIP * 0.5;
    D2D_RECT_F {
        left: center_x - width * 0.5,
        top: center_y - height * 0.5,
        right: center_x + width * 0.5,
        bottom: center_y + height * 0.5,
    }
}

fn draw_rect(context: &ID2D1DeviceContext, rect: D2D_RECT_F, brush: &ID2D1SolidColorBrush) {
    unsafe { context.DrawRectangle(&rect, brush, ICON_STROKE_DIP, None) };
}

fn draw_line(
    context: &ID2D1DeviceContext,
    start_x: f32,
    start_y: f32,
    end_x: f32,
    end_y: f32,
    brush: &ID2D1SolidColorBrush,
) {
    unsafe {
        context.DrawLine(
            D2D_POINT_2F {
                x: start_x,
                y: start_y,
            },
            D2D_POINT_2F { x: end_x, y: end_y },
            brush,
            ICON_STROKE_DIP,
            None,
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_button_is_narrower_than_content_actions() {
        assert!(
            slot_width_dip(StatusBarSegmentKind::VaultSettings)
                < slot_width_dip(StatusBarSegmentKind::VaultFiles)
        );
        assert_eq!(
            slot_width_dip(StatusBarSegmentKind::VaultOutline),
            slot_width_dip(StatusBarSegmentKind::VaultMinimap)
        );
        assert_eq!(slot_width_dip(StatusBarSegmentKind::Position), None);
    }

    #[test]
    fn icon_rect_is_centered_in_its_hit_slot() {
        let bounds = SegmentBounds {
            left: 10.0,
            right: 32.0,
            kind: StatusBarSegmentKind::VaultFiles,
        };
        let rect = centered_rect(bounds, 100.0, 14.0, 10.0);
        assert_eq!((rect.left + rect.right) * 0.5, 21.0);
        assert_eq!((rect.top + rect.bottom) * 0.5, 111.0);
    }
}
