//! Phase F — live column-resize drag for visual pipe-tables.
//!
//! A mouse-down within the grab zone of a column boundary starts a
//! drag. Each `WM_MOUSEMOVE` updates the column's live width and repaints
//! a preview (the focused table layout rebuilds with a transient
//! [`TableColWidthOverride`], so wrapping and row reservations reflow
//! with the drag). `WM_LBUTTONUP` commits the final width to the table's
//! `<!--continuity:width=…-->` directive — the raw text stays the source
//! of truth.
//!
//! Thread ownership: UI thread of one window (HWND owner + mouse
//! capture).

use continuity_render::{TableColWidthOverride, MAX_TABLE_COL_WIDTH_DIP, MIN_TABLE_COL_WIDTH_DIP};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{LoadCursorW, SetCursor, IDC_SIZEWE};

use crate::mouse::TableColDrag;
use crate::window_helpers::invalidate_hwnd;
use crate::Window;

/// Half-width (DIP) of the grab zone centred on a column boundary.
const TABLE_COL_RESIZE_HANDLE_DIP: f32 = 4.0;

/// A column-boundary hit for the resize drag.
struct TableColBorderHit {
    block_start: usize,
    col: u32,
    start_width: f32,
}

impl Window {
    /// Map a client `(x, y)` to a focused-table column boundary within
    /// the resize handle zone. Returns the column to the LEFT of the
    /// boundary (the one a drag widens / narrows) and its current width.
    /// `None` when the point is not on a table boundary.
    fn table_col_border_at_pixel(&self, x: i32, y: i32) -> Option<TableColBorderHit> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let rope = snap.rope_snapshot().rope();
        let metrics =
            self.display_projection_metrics(self.current_search_minimap_active(), rope.len_lines());
        let column_advance = metrics.char_width_dip;
        let dl_idx = self.display_row_for_client_y(y);
        let revision = snap.rope_snapshot().revision().0;
        let decorations = self
            .decoration_cache
            .get(self.buffer_id.as_uuid().as_u128());
        let caret_bytes: Vec<usize> = snap
            .selections()
            .iter()
            .map(|s| {
                let line = s.head.line as usize;
                let line_start = if line < rope.len_lines() {
                    rope.line_to_byte(line)
                } else {
                    rope.len_bytes()
                };
                line_start + s.head.byte_in_line as usize
            })
            .collect();
        let (frame_display, _src, _folds) = self.resolve_hit_test_frame_display(
            rope,
            revision,
            decorations,
            &caret_bytes,
            metrics.wrap_width_dip,
            column_advance,
            dl_idx,
        );
        let total_dl = frame_display.display_line_count();
        if total_dl == 0 {
            return None;
        }
        let dl_idx = dl_idx.min(total_dl.saturating_sub(1));
        let spec = frame_display.display_line_by_index(dl_idx)?;
        let source_line = spec.source_line.raw();
        let body = self.focused_body_rect();
        let left_margin = self.focused_table_body_left_margin_dip(rope.len_lines());
        let layouts_cache = self.last_focused_table_layouts.borrow();
        let layouts = layouts_cache.get(&self.buffer_id)?.as_ref();
        for layout in layouts.iter() {
            if !layout.covers_source_line(source_line) {
                continue;
            }
            let col_count = layout.col_widths_dip.len();
            for col in 0..col_count {
                let boundary_client = body.x + left_margin + layout.cell_x_dip(col as u32 + 1);
                if (x as f32 - boundary_client).abs() <= TABLE_COL_RESIZE_HANDLE_DIP {
                    return Some(TableColBorderHit {
                        block_start: layout.block_range.start,
                        col: col as u32,
                        start_width: layout.col_widths_dip.get(col).copied().unwrap_or(0.0),
                    });
                }
            }
        }
        None
    }

    /// Body left margin (line-number gutter or padding) for the focused
    /// pane. Shared by the column-border hit-test and the cell hit-test
    /// so both agree on where column boundaries sit in client space.
    pub(crate) fn focused_table_body_left_margin_dip(&self, len_lines: usize) -> f32 {
        if self.view_options.line_numbers {
            continuity_render::chrome::gutter_width_for_line_count(
                self.scaled_font_size(),
                len_lines,
            ) + continuity_render::chrome::GUTTER_BODY_GAP_DIP
        } else {
            continuity_render::chrome::BODY_LEFT_PADDING_DIP
        }
    }

    /// `WM_LBUTTONDOWN`: start a column-resize drag when `(x, y)` is on a
    /// table column boundary. Returns `true` (caller swallows the click —
    /// no caret placement / selection).
    pub(crate) fn try_table_col_resize_left_down(&mut self, x: i32, y: i32) -> bool {
        let Some(hit) = self.table_col_border_at_pixel(x, y) else {
            return false;
        };
        self.mouse_state.table_col_drag = Some(TableColDrag {
            block_start: hit.block_start,
            col: hit.col,
            start_client_x: x as f32,
            start_width: hit.start_width,
            current_width: hit.start_width,
        });
        unsafe {
            let _ = SetCapture(self.hwnd);
        }
        true
    }

    /// `WM_MOUSEMOVE` while a column-resize drag is active: update the
    /// live width and repaint the preview. Returns `true` when a drag is
    /// in flight.
    pub(crate) fn drag_table_col_resize(&mut self, x: i32) -> bool {
        let Some(drag) = self.mouse_state.table_col_drag.as_mut() else {
            return false;
        };
        let delta = x as f32 - drag.start_client_x;
        drag.current_width =
            (drag.start_width + delta).clamp(MIN_TABLE_COL_WIDTH_DIP, MAX_TABLE_COL_WIDTH_DIP);
        if let Ok(cursor) = unsafe { LoadCursorW(None, IDC_SIZEWE) } {
            unsafe { SetCursor(Some(cursor)) };
        }
        invalidate_hwnd(self.hwnd);
        true
    }

    /// `WM_LBUTTONUP`: commit an in-flight column-resize drag to the
    /// table directive and release capture. Returns `true` when a drag
    /// was active.
    pub(crate) fn finish_table_col_resize(&mut self) -> bool {
        let Some(drag) = self.mouse_state.table_col_drag.take() else {
            return false;
        };
        unsafe {
            let _ = ReleaseCapture();
        }
        // Only rewrite the directive when the column actually moved — a
        // stray click on a boundary shouldn't freeze the column at its
        // current auto width by writing a directive.
        if (drag.current_width - drag.start_width).abs() >= 1.0 {
            let _ = self.commit_table_col_width(drag.block_start, drag.col, drag.current_width);
        } else {
            invalidate_hwnd(self.hwnd);
        }
        true
    }

    /// Transient per-column width override for the focused-pane table
    /// layout build, derived from an in-flight resize drag. `None` when
    /// no drag is active.
    pub(crate) fn active_table_col_override(&self) -> Option<TableColWidthOverride> {
        self.mouse_state
            .table_col_drag
            .as_ref()
            .map(|drag| TableColWidthOverride {
                block_start: drag.block_start,
                col: drag.col,
                width: drag.current_width,
            })
    }

    /// `true` when the cursor sits over a focused-table column boundary
    /// (or a resize drag is in flight) — drives the `IDC_SIZEWE` cursor.
    pub(crate) fn cursor_over_table_col_border(&self, x: i32, y: i32) -> bool {
        self.mouse_state.table_col_drag.is_some() || self.table_col_border_at_pixel(x, y).is_some()
    }
}
