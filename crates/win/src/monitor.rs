//! Monitor work-area helpers for top-level window placement.
//!
//! The functions here return plain screen-pixel tuples so higher layers
//! can keep Win32 handle ownership contained in this crate.

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromRect, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};

/// Return the work area for the currently focused monitor.
///
/// The foreground HWND wins when Windows reports one; otherwise the
/// cursor position chooses the nearest monitor. The tuple is
/// `(left, top, width, height)` in screen pixels.
#[must_use]
pub fn focused_monitor_work_area() -> Option<(i32, i32, i32, i32)> {
    let hwnd = unsafe { GetForegroundWindow() };
    if !hwnd.0.is_null() {
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if let Some(area) = monitor_work_area(monitor) {
            return Some(area);
        }
    }
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_ok() {
        let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
        return monitor_work_area(monitor);
    }
    None
}

/// Return a top-left origin that centers a window on the focused monitor.
#[must_use]
pub fn centered_origin_on_focused_monitor(window_size: (i32, i32)) -> Option<(i32, i32)> {
    let area = focused_monitor_work_area()?;
    let x = area.0 + (area.2 - window_size.0).max(0) / 2;
    let y = area.1 + (area.3 - window_size.1).max(0) / 2;
    Some((x, y))
}

/// Cascade a new window from `source_rect`, clamped to the source
/// window's monitor work area.
///
/// Rect and size tuples are in screen pixels: rect is
/// `(left, top, width, height)`, size is `(width, height)`.
#[must_use]
pub fn cascade_origin_on_source_monitor(
    source_rect: (i32, i32, i32, i32),
    window_size: (i32, i32),
    step: i32,
) -> Option<(i32, i32)> {
    let rect = RECT {
        left: source_rect.0,
        top: source_rect.1,
        right: source_rect.0.saturating_add(source_rect.2),
        bottom: source_rect.1.saturating_add(source_rect.3),
    };
    let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST) };
    let area = monitor_work_area(monitor)?;
    Some(clamp_origin_to_area(
        (
            source_rect.0.saturating_add(step),
            source_rect.1.saturating_add(step),
        ),
        window_size,
        area,
    ))
}

fn monitor_work_area(monitor: HMONITOR) -> Option<(i32, i32, i32, i32)> {
    if monitor.0.is_null() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    let rect = info.rcWork;
    Some((
        rect.left,
        rect.top,
        rect.right.saturating_sub(rect.left),
        rect.bottom.saturating_sub(rect.top),
    ))
}

fn clamp_origin_to_area(
    origin: (i32, i32),
    window_size: (i32, i32),
    area: (i32, i32, i32, i32),
) -> (i32, i32) {
    let max_x = area
        .0
        .saturating_add(area.2.saturating_sub(window_size.0).max(0));
    let max_y = area
        .1
        .saturating_add(area.3.saturating_sub(window_size.1).max(0));
    (origin.0.clamp(area.0, max_x), origin.1.clamp(area.1, max_y))
}
