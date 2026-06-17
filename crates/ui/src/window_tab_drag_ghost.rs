//! Screen-space tab-drag ghost window.
//!
//! The normal D2D tab-drag overlay is clipped to the source HWND, so it
//! disappears as soon as the cursor leaves that window. This helper owns
//! a tiny no-activate popup that follows the cursor in screen pixels
//! while a tab drag is in flight.
//!
//! Thread ownership: created, moved, and destroyed only on the source
//! window's UI thread.

use std::ffi::c_void;

use continuity_render::{pane_chrome, tab_slot_widths, Rgba, TAB_MIN_WIDTH_DIP};
use continuity_win::WindowClass;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, InvalidateRect,
    SetBkMode, SetTextColor, DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, HGDIOBJ, PAINTSTRUCT,
    TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, CREATESTRUCTW, GWLP_USERDATA, HMENU, HWND_TOPMOST,
    LWA_ALPHA, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNA,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_NCCREATE, WM_PAINT, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::pane_layout::metrics;
use crate::Window;

const GHOST_CURSOR_OFFSET_X_PX: i32 = 12;
const GHOST_CURSOR_OFFSET_Y_PX: i32 = 8;
const GHOST_ALPHA: u8 = 235;

/// Small no-activate popup that follows an in-flight tab drag.
pub(crate) struct TabDragGhostWindow {
    hwnd: HWND,
    _class: WindowClass,
    paint: Box<GhostPaint>,
}

#[derive(Clone, Debug)]
struct GhostPaint {
    label: String,
    width_px: i32,
    height_px: i32,
    show_close: bool,
    background: COLORREF,
    foreground: COLORREF,
    border: COLORREF,
}

#[derive(Clone, Debug)]
struct GhostStyle {
    width_px: i32,
    height_px: i32,
    show_close: bool,
    background: COLORREF,
    foreground: COLORREF,
    border: COLORREF,
}

impl TabDragGhostWindow {
    fn create(owner: HWND, label: &str, style: GhostStyle) -> Option<Self> {
        if owner.0.is_null() {
            return None;
        }
        let class = WindowClass::register_unique_with_proc(
            "ContinuityTabDragGhost",
            Some(tab_drag_ghost_wndproc),
        )
        .ok()?;
        let mut paint = Box::new(GhostPaint {
            label: label.to_string(),
            width_px: style.width_px,
            height_px: style.height_px,
            show_close: style.show_close,
            background: style.background,
            foreground: style.foreground,
            border: style.border,
        });
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(
                    WS_EX_TOOLWINDOW.0
                        | WS_EX_NOACTIVATE.0
                        | WS_EX_LAYERED.0
                        | WS_EX_TOPMOST.0
                        | WS_EX_TRANSPARENT.0,
                ),
                PCWSTR(class.name().as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(WS_POPUP.0),
                0,
                0,
                style.width_px,
                style.height_px,
                Some(owner),
                Option::<HMENU>::None,
                Some(class.hinstance().into()),
                Some(paint.as_mut() as *mut GhostPaint as *mut c_void),
            )
        }
        .ok()?;
        let _ = unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), GHOST_ALPHA, LWA_ALPHA) };
        Some(Self {
            hwnd,
            _class: class,
            paint,
        })
    }

    fn update(&mut self, label: &str, style: GhostStyle, screen_x: i32, screen_y: i32) {
        let changed = self.paint.label != label
            || self.paint.width_px != style.width_px
            || self.paint.height_px != style.height_px
            || self.paint.show_close != style.show_close
            || self.paint.background != style.background
            || self.paint.foreground != style.foreground
            || self.paint.border != style.border;
        if changed {
            self.paint.label.clear();
            self.paint.label.push_str(label);
            self.paint.width_px = style.width_px;
            self.paint.height_px = style.height_px;
            self.paint.show_close = style.show_close;
            self.paint.background = style.background;
            self.paint.foreground = style.foreground;
            self.paint.border = style.border;
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, true);
            }
        }
        let left = screen_x - GHOST_CURSOR_OFFSET_X_PX;
        let top = screen_y - GHOST_CURSOR_OFFSET_Y_PX;
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                left,
                top,
                style.width_px,
                style.height_px,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
            )
        };
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNA);
        }
    }
}

impl Drop for TabDragGhostWindow {
    fn drop(&mut self) {
        if !self.hwnd.0.is_null() {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                let _ = SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

impl Window {
    /// Show or move the screen-space tab-drag ghost to the current
    /// cursor location.
    pub(crate) fn update_tab_drag_ghost_at_client_point(&mut self, x: i32, y: i32) {
        let Some(label) = self
            .mouse_state
            .tab_drag
            .as_ref()
            .map(|drag| drag.label.clone())
        else {
            return;
        };
        let Some((screen_x, screen_y)) = self.client_dip_point_to_screen(x, y) else {
            return;
        };
        let style = self.compute_tab_drag_ghost_style(&label);
        if self.tab_drag_ghost_window.is_none() {
            self.tab_drag_ghost_window =
                TabDragGhostWindow::create(self.hwnd, &label, style.clone());
        }
        if let Some(ghost) = self.tab_drag_ghost_window.as_mut() {
            ghost.update(&label, style, screen_x, screen_y);
        }
    }

    /// Destroy the tab-drag ghost, if the current window owns one.
    pub(crate) fn clear_tab_drag_ghost(&mut self) {
        self.tab_drag_ghost_window = None;
    }

    fn compute_tab_drag_ghost_style(&self, label: &str) -> GhostStyle {
        let labels = [label];
        let width_dip = tab_slot_widths(&labels, 4096.0)
            .first()
            .copied()
            .unwrap_or(TAB_MIN_WIDTH_DIP)
            .max(TAB_MIN_WIDTH_DIP);
        let scale = self.dpi_scale().max(0.01);
        let width_px = (width_dip * scale).round().max(1.0) as i32;
        let height_px = (metrics::TAB_STRIP_HEIGHT_DIP * scale).round().max(1.0) as i32;
        let theme = &self.active_theme.current;
        let show_close = match self.view_options.tab_close_button {
            continuity_config::TabCloseButton::Always
            | continuity_config::TabCloseButton::Hover => {
                width_dip >= pane_chrome::TAB_CLOSE_MIN_TAB_WIDTH_DIP
            }
            continuity_config::TabCloseButton::Never => false,
        };
        GhostStyle {
            width_px,
            height_px,
            show_close,
            background: colorref_from_theme(theme.panel_active_tab_background()),
            foreground: colorref_from_theme(theme.panel_active_tab_foreground()),
            border: colorref_from_theme(theme.pane_border_active()),
        }
    }
}

unsafe extern "system" fn tab_drag_ghost_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = lparam.0 as *const CREATESTRUCTW;
        if !create.is_null() {
            let paint = unsafe { (*create).lpCreateParams as *mut GhostPaint };
            unsafe {
                let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, paint as isize);
            }
        }
        return LRESULT(1);
    }
    if msg == WM_PAINT {
        let paint = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const GhostPaint };
        if !paint.is_null() {
            paint_ghost(hwnd, unsafe { &*paint });
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn paint_ghost(hwnd: HWND, paint: &GhostPaint) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let outer = RECT {
            left: 0,
            top: 0,
            right: paint.width_px,
            bottom: paint.height_px,
        };
        let bg = CreateSolidBrush(paint.background);
        let _ = FillRect(hdc, &outer, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        let border = CreateSolidBrush(paint.border);
        let top = RECT {
            left: 0,
            top: 0,
            right: paint.width_px,
            bottom: 1,
        };
        let bottom = RECT {
            left: 0,
            top: paint.height_px.saturating_sub(1),
            right: paint.width_px,
            bottom: paint.height_px,
        };
        let left = RECT {
            left: 0,
            top: 0,
            right: 1,
            bottom: paint.height_px,
        };
        let right = RECT {
            left: paint.width_px.saturating_sub(1),
            top: 0,
            right: paint.width_px,
            bottom: paint.height_px,
        };
        let _ = FillRect(hdc, &top, border);
        let _ = FillRect(hdc, &bottom, border);
        let _ = FillRect(hdc, &left, border);
        let _ = FillRect(hdc, &right, border);
        let _ = DeleteObject(HGDIOBJ(border.0));

        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, paint.foreground);
        let padding_px = (pane_chrome::TAB_PADDING_DIP as i32).min(paint.width_px / 3);
        let close_cell_px = if paint.show_close {
            (pane_chrome::TAB_CLOSE_WIDTH_DIP as i32 + 4).min(paint.width_px / 3)
        } else {
            0
        };
        let mut text_rect = RECT {
            left: padding_px,
            top: 0,
            right: paint.width_px.saturating_sub(padding_px + close_cell_px),
            bottom: paint.height_px,
        };
        let mut wide: Vec<u16> = paint.label.encode_utf16().collect();
        let _ = DrawTextW(
            hdc,
            &mut wide,
            &mut text_rect,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        if paint.show_close {
            let inset_px = 4;
            let mut close_rect = RECT {
                left: paint
                    .width_px
                    .saturating_sub(inset_px + pane_chrome::TAB_CLOSE_WIDTH_DIP as i32),
                top: 0,
                right: paint.width_px.saturating_sub(inset_px),
                bottom: paint.height_px,
            };
            let mut close = vec![0x00D7];
            let _ = DrawTextW(
                hdc,
                &mut close,
                &mut close_rect,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

fn colorref_from_theme(color: continuity_theme::Color) -> COLORREF {
    colorref_from_rgba(crate::window_theme::rgba_from_color(color))
}

fn colorref_from_rgba(color: Rgba) -> COLORREF {
    let r = (color.r.clamp(0.0, 1.0) * 255.0).round() as u32;
    let g = (color.g.clamp(0.0, 1.0) * 255.0).round() as u32;
    let b = (color.b.clamp(0.0, 1.0) * 255.0).round() as u32;
    COLORREF(r | (g << 8) | (b << 16))
}
