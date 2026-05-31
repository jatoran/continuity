//! UI-thread diagnostic capture for layout and DPI issues.
//!
//! Thread ownership: every `Window` field read here is owned by this
//! window's UI thread. The only mutation is opening a new tab in the
//! focused pane with the captured JSON.

use serde_json::{json, Value};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN,
    SM_CYVIRTUALSCREEN,
};

use crate::pane_layout::Rect;
use crate::window::Window;

impl Window {
    pub(crate) fn capture_layout_diagnostics_impl(
        &mut self,
    ) -> Result<(), continuity_command::Error> {
        let doc = self.layout_diagnostics_document();
        let content = layout_diagnostics_buffer_text(&doc)?;
        let buffer_id = self.editor.open_buffer(content);
        let tab_id = self.tree.open_tab_in_focused(buffer_id, self.now_ms());
        if let Some(tab) = self.tree.tabs.get_mut(&tab_id) {
            tab.label_override = Some("Layout diagnostic".to_string());
        }

        self.apply_new_pane_state(buffer_id);
        self.refresh_focused_viewport();
        self.refresh_language();
        self.maybe_submit_decoration();
        let _ =
            self.try_dispatch_projection_worker_early("capture_layout_diagnostics", "focus_change");
        self.retarget_find_bar_to_focused_pane();
        self.request_state_save();
        self.request_repaint();
        Ok(())
    }

    fn layout_diagnostics_document(&self) -> Value {
        let snapshot = self.editor.snapshot(self.buffer_id);
        let line_count = snapshot
            .as_ref()
            .map(|snap| snap.rope_snapshot().rope().len_lines())
            .unwrap_or(0);
        let byte_count = snapshot
            .as_ref()
            .map(|snap| snap.rope_snapshot().rope().len_bytes())
            .unwrap_or(0);
        let revision = snapshot
            .as_ref()
            .map(|snap| snap.rope_snapshot().revision().get())
            .unwrap_or(0);
        let projection = self.current_display_projection_metrics();
        let body = self.focused_body_rect();
        let chrome = self.layout_diagnostics_chrome(body.w, line_count);

        json!({
            "schema": "continuity.layout_diagnostic.v1",
            "generated_at_ms": self.now_ms(),
            "process": self.layout_diagnostics_process(),
            "system": self.layout_diagnostics_system(),
            "window": self.layout_diagnostics_window(),
            "focused_buffer": {
                "buffer_id": self.buffer_id.as_uuid().to_string(),
                "revision": revision,
                "line_count": line_count,
                "byte_count": byte_count,
                "file": snapshot.as_ref().and_then(|snap| {
                    snap.file.as_ref().map(|file| file.path.display().to_string())
                }),
                "selections": snapshot.as_ref().map(|snap| selections_json(snap.selections())),
            },
            "focused_pane": {
                "pane_id": self.tree.focused.0,
                "pane_root_rect_dip": rect_json(self.pane_root_rect()),
                "body_rect_dip": rect_json(body),
                "viewport_dip": {
                    "width": self.view.viewport_width_dip,
                    "height": self.view.viewport_height_dip,
                    "wrap_width_key": self.view.wrap_width_key(),
                    "projection_wrap_width": projection.wrap_width_dip,
                    "projection_char_width": projection.char_width_dip,
                },
                "view": {
                    "scroll_y_dip": self.view.scroll_y_dip,
                    "font_size_scale": self.view.font_size_scale,
                    "soft_wrap": self.view.soft_wrap,
                    "font_size_dip": self.scaled_font_size(),
                    "font_state": format!("{:?}", self.font_state),
                    "font_family": self.prose_font_family,
                },
                "chrome": chrome,
            },
            "panes": self.layout_diagnostics_panes(),
            "last_painted_frame": self.layout_diagnostics_last_frame(),
        })
    }

    fn layout_diagnostics_process(&self) -> Value {
        json!({
            "exe": std::env::current_exe().ok().map(|p| p.display().to_string()),
            "cwd": std::env::current_dir().ok().map(|p| p.display().to_string()),
            "args": std::env::args().collect::<Vec<_>>(),
            "env": {
                "OS": std::env::var("OS").ok(),
                "APPDATA": std::env::var("APPDATA").ok(),
                "CONTINUITY_DATA_DIR": std::env::var("CONTINUITY_DATA_DIR").ok(),
                "NUMBER_OF_PROCESSORS": std::env::var("NUMBER_OF_PROCESSORS").ok(),
                "PROCESSOR_ARCHITECTURE": std::env::var("PROCESSOR_ARCHITECTURE").ok(),
                "PROCESSOR_IDENTIFIER": std::env::var("PROCESSOR_IDENTIFIER").ok(),
            },
            "live_reload": self.live_reload.as_ref().map(|reload| json!({
                "settings_path": reload.settings_path.display().to_string(),
                "themes_dir": reload.themes_dir.display().to_string(),
            })),
        })
    }

    fn layout_diagnostics_system(&self) -> Value {
        let monitor = monitor_json(self.hwnd);
        json!({
            "primary_screen_px": {
                "width": unsafe { GetSystemMetrics(SM_CXSCREEN) },
                "height": unsafe { GetSystemMetrics(SM_CYSCREEN) },
            },
            "virtual_screen_px": {
                "width": unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) },
                "height": unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) },
            },
            "monitor": monitor,
        })
    }

    fn layout_diagnostics_window(&self) -> Value {
        let renderer_back_buffer = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.back_buffer_size());
        json!({
            "hwnd": format!("{:#x}", self.hwnd.0 as usize),
            "client_px_state": {
                "width": self.client_width,
                "height": self.client_height,
            },
            "client_dip_state": {
                "width": self.client_width_dip(),
                "height": self.client_height_dip(),
            },
            "win32_client_rect_px": win32_client_rect_json(self.hwnd),
            "win32_window_rect_px": win32_window_rect_json(self.hwnd),
            "window_dpi_state": self.window_dpi,
            "dpi_for_window": continuity_win::dpi_for_window(self.hwnd),
            "dpi_scale": self.dpi_scale(),
            "renderer_back_buffer_px": renderer_back_buffer.map(|(w, h)| json!({
                "width": w,
                "height": h,
            })),
            "renderer_matches_client": renderer_back_buffer
                .map(|(w, h)| w == self.client_width && h == self.client_height),
            "deferred_renderer_resize": self.deferred_renderer_resize.map(|(w, h)| json!({
                "width": w,
                "height": h,
            })),
            "inited": self.inited,
            "is_live_resizing": self.is_live_resizing,
            "is_applying_dpi_change": self.is_applying_dpi_change,
            "file_tree_width_dip": self.file_tree.visible_width_dip(),
        })
    }

    fn layout_diagnostics_chrome(&self, body_width_dip: f32, source_line_count: usize) -> Value {
        let font = self.scaled_font_size();
        let search_minimap_active = self.current_search_minimap_active();
        let left = continuity_render::chrome::resolve_body_left_margin_for_line_count_dip(
            self.view_options.line_numbers,
            font,
            source_line_count,
        );
        let right = continuity_render::chrome::resolve_body_right_margin_dip(
            self.view_options.minimap,
            search_minimap_active,
            self.view_options.show_outline_sidebar,
            self.view_options.outline_sidebar_width_dip,
        );
        let max_width =
            self.view_options.pane_modes.distraction_free_max_width as f32 * font * 0.55;
        let base_text = (body_width_dip.max(1.0) - left - right).max(0.0);
        let text_width = if self.view_options.pane_modes.distraction_free {
            base_text.min(max_width.max(1.0))
        } else {
            base_text
        };
        let centered_pad = ((base_text - text_width) * 0.5).max(0.0);
        json!({
            "line_numbers": self.view_options.line_numbers,
            "gutter_caret_line_only": self.view_options.gutter_caret_line_only,
            "relative_line_numbers": self.view_options.relative_line_numbers,
            "minimap": self.view_options.minimap,
            "search_minimap_active": search_minimap_active,
            "outline_sidebar": self.view_options.show_outline_sidebar,
            "outline_sidebar_width_dip": self.view_options.outline_sidebar_width_dip,
            "status_bar": self.view_options.show_status_bar,
            "tab_strip": self.view_options.show_tab_strip,
            "pane_borders": self.view_options.show_pane_borders,
            "sticky_breadcrumb": self.view_options.show_sticky_breadcrumb,
            "distraction_free": self.view_options.pane_modes.distraction_free,
            "distraction_free_max_width_chars": self.view_options.pane_modes.distraction_free_max_width,
            "computed_margins_dip": {
                "base_left": left,
                "base_right": right,
                "centered_extra_left": centered_pad,
                "centered_extra_right": centered_pad,
                "painted_left": left + centered_pad,
                "painted_right": right + centered_pad,
            },
            "computed_text_width_dip": text_width,
            "computed_right_blank_dip": right + centered_pad,
        })
    }

    fn layout_diagnostics_panes(&self) -> Vec<Value> {
        self.pane_outer_rects()
            .into_iter()
            .map(|(pane, outer)| {
                let group = self.tree.groups.get(&pane);
                let active_tab = group.and_then(|g| self.tree.tabs.get(&g.active));
                json!({
                    "pane_id": pane.0,
                    "focused": pane == self.tree.focused,
                    "outer_rect_dip": rect_json(outer),
                    "body_rect_dip": self.pane_body_rect(pane).map(rect_json),
                    "tab_count": group.map(|g| g.tabs.len()).unwrap_or(0),
                    "active_tab": group.map(|g| g.active.0),
                    "active_buffer_id": active_tab.map(|tab| tab.buffer_id.as_uuid().to_string()),
                    "saved_view": self.panes.get(&pane).map(|state| json!({
                        "buffer_id": state.buffer_id.as_uuid().to_string(),
                        "scroll_y_dip": state.view.scroll_y_dip,
                        "viewport_width_dip": state.view.viewport_width_dip,
                        "viewport_height_dip": state.view.viewport_height_dip,
                        "wrap_width_key": state.view.wrap_width_key(),
                        "soft_wrap": state.view.soft_wrap,
                        "font_size_scale": state.view.font_size_scale,
                    })),
                })
            })
            .collect()
    }

    fn layout_diagnostics_last_frame(&self) -> Value {
        let Some((query, frame)) = self.last_painted_frame_display.as_ref() else {
            return json!({ "present": false });
        };
        let row_index = frame.row_index();
        json!({
            "present": true,
            "query": format!("{:?}", query),
            "realized_row_range": {
                "start": frame.realized_row_range().start,
                "end": frame.realized_row_range().end,
            },
            "display_line_count": frame.display_line_count(),
            "row_index": {
                "source_line_count": row_index.source_line_count(),
                "display_row_count": row_index.display_row_count(),
                "estimated_total_rows": row_index.estimated_total_rows(),
                "is_partial": row_index.is_partial(),
                "stamps": format!("{:?}", row_index.stamps()),
                "partial_state": row_index.partial_state().map(|state| json!({
                    "walked_source_range": {
                        "start": state.walked_source_range.start,
                        "end": state.walked_source_range.end,
                    },
                    "scrollbar_estimate": state.scrollbar_estimate,
                    "full_revision_target": state.full_revision_target,
                })),
            },
        })
    }
}

fn layout_diagnostics_buffer_text(value: &Value) -> Result<String, continuity_command::Error> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|err| continuity_command::Error::Other(err.to_string()))?;
    Ok(format!(
        "# Continuity layout diagnostic\n\n```json\n{json}\n```\n"
    ))
}

fn selections_json(selections: &[continuity_text::Selection]) -> Value {
    json!(selections
        .iter()
        .map(|selection| json!({
            "anchor": {
                "line": selection.anchor.line,
                "byte_in_line": selection.anchor.byte_in_line,
            },
            "head": {
                "line": selection.head.line,
                "byte_in_line": selection.head.byte_in_line,
            },
            "kind": format!("{:?}", selection.kind),
        }))
        .collect::<Vec<_>>())
}

fn rect_json(rect: Rect) -> Value {
    json!({
        "x": rect.x,
        "y": rect.y,
        "w": rect.w,
        "h": rect.h,
        "right": rect.right(),
        "bottom": rect.bottom(),
    })
}

fn win32_client_rect_json(hwnd: windows::Win32::Foundation::HWND) -> Value {
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
        return Value::Null;
    }
    win32_rect_json(rect)
}

fn win32_window_rect_json(hwnd: windows::Win32::Foundation::HWND) -> Value {
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return Value::Null;
    }
    win32_rect_json(rect)
}

fn win32_rect_json(rect: RECT) -> Value {
    json!({
        "left": rect.left,
        "top": rect.top,
        "right": rect.right,
        "bottom": rect.bottom,
        "width": rect.right - rect.left,
        "height": rect.bottom - rect.top,
    })
}

fn monitor_json(hwnd: windows::Win32::Foundation::HWND) -> Value {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Value::Null;
    }
    json!({
        "rect_px": win32_rect_json(info.rcMonitor),
        "work_rect_px": win32_rect_json(info.rcWork),
        "flags": info.dwFlags,
    })
}
