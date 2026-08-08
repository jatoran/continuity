//! Phase-16 IME (Input Method Editor) glue for [`crate::Window`].
//!
//! Thread ownership: UI thread (HIMC is a per-window resource).
//!
//! Wire: `WM_IME_STARTCOMPOSITION` clears the in-progress composition
//! string; `WM_IME_COMPOSITION` reads `GCS_COMPSTR` / `GCS_CURSORPOS`
//! into the composition state and the IME caret rect, and on
//! `GCS_RESULTSTR` commits the result through the normal text-input
//! path; `WM_IME_ENDCOMPOSITION` clears the composition state. While
//! `composing` is true, `WM_CHAR` is suppressed by the window proc to
//! avoid double-insertion of the result string.

use continuity_render::DEFAULT_HEADING_SCALE;
use continuity_win::ime::{self, CompositionState};
use windows::Win32::Foundation::HWND;

use crate::Window;

/// Window-level state tracking the active IME composition (if any).
#[derive(Debug, Default, Clone)]
pub struct ImeState {
    /// `true` between WM_IME_STARTCOMPOSITION and WM_IME_ENDCOMPOSITION.
    pub composing: bool,
    /// Most recent in-progress composition string (UTF-8).
    pub comp: String,
    /// Caret offset within `comp`, in UTF-8 bytes.
    pub caret_byte: usize,
}

impl ImeState {
    /// Reset to "not composing".
    pub fn clear(&mut self) {
        self.composing = false;
        self.comp.clear();
        self.caret_byte = 0;
    }
}

impl Window {
    /// Handle `WM_IME_STARTCOMPOSITION`. Mark the window as composing so
    /// `WM_CHAR` can be suppressed until the composition ends.
    pub(crate) fn on_ime_start_composition(&mut self) {
        self.surface.ime.composing = true;
        self.surface.ime.comp.clear();
        self.surface.ime.caret_byte = 0;
    }

    /// Handle `WM_IME_COMPOSITION`. Refreshes `ime_state.comp` /
    /// `caret_byte` from the IME and, when `GCS_RESULTSTR` fires, commits
    /// the result through the editor core.
    pub(crate) fn on_ime_composition(&mut self, hwnd: HWND, lparam: isize) -> bool {
        let Some(state) = ime::read_composition(hwnd, lparam) else {
            return false;
        };
        self.update_ime_visuals(hwnd);
        let CompositionState {
            comp,
            caret_byte,
            result,
        } = state;
        // In-progress composition: just snapshot for paint.
        self.surface.ime.comp = comp;
        self.surface.ime.caret_byte = caret_byte;
        if !result.is_empty() {
            // Committed text — route through the same path as a normal
            // text insert so undo/persistence/decoration all observe it
            // identically.
            self.note_input_now();
            let edit = continuity_core::SelectionEdit::InsertText(result);
            if let Err(e) = self.dispatch_selection_edit(edit) {
                eprintln!("continuity-ui: IME commit failed: {e}");
            }
            true
        } else {
            true
        }
    }

    /// Handle `WM_IME_ENDCOMPOSITION`. Clear in-progress state.
    pub(crate) fn on_ime_end_composition(&mut self) {
        self.surface.ime.clear();
    }

    /// Move the IME composition window to track the primary caret.
    fn update_ime_visuals(&mut self, hwnd: HWND) {
        if let Some((x, y)) = self.primary_caret_pixel() {
            ime::set_composition_position(hwnd, x, y);
        }
    }

    /// Physical-pixel client position of the primary caret bottom edge.
    /// `None` when the current projected caret row is unavailable or offscreen.
    fn primary_caret_pixel(&self) -> Option<(i32, i32)> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let sel = snap.selections().first().copied()?;
        let rope = snap.rope_snapshot().rope();
        let source_line = sel.head.line as usize;
        if source_line >= rope.len_lines() {
            return None;
        }
        let line_start = rope.line_to_byte(source_line);
        let source_byte = line_start + sel.head.byte_in_line as usize;
        let caret_bytes: Vec<usize> = snap
            .selections()
            .iter()
            .map(|selection| {
                let line = selection.head.line as usize;
                let start = if line < rope.len_lines() {
                    rope.line_to_byte(line)
                } else {
                    rope.len_bytes()
                };
                start + selection.head.byte_in_line as usize
            })
            .collect();
        let decorations = self
            .decoration_cache
            .get(self.buffer_id.as_uuid().as_u128());
        let metrics =
            self.display_projection_metrics(self.current_search_minimap_active(), rope.len_lines());
        let resolved = self.resolve_caret_display_line(sel.head)?;
        let (frame_display, _, _) = self.resolve_hit_test_frame_display(
            rope,
            snap.rope_snapshot().revision().0,
            decorations,
            &caret_bytes,
            metrics.wrap_width_dip,
            metrics.char_width_dip,
            resolved.display_row,
        );
        let display_row = frame_display
            .display_line_index_for_source_pos(source_line, sel.head.byte_in_line as usize)?;
        let spec = frame_display.display_line_by_index(display_row)?;
        let format = self.surface.render.text_format.as_ref()?;
        let caret_x = continuity_render::caret_x_for_spec(
            self.surface.render.dwrite.raw(),
            format,
            spec,
            source_byte,
            self.surface.view.viewport_width_dip.max(1.0),
            self.scaled_font_size(),
            DEFAULT_HEADING_SCALE,
        )?;
        let left_margin = if self.view_options.line_numbers {
            continuity_render::chrome::gutter_width_for_line_count(
                self.scaled_font_size(),
                rope.len_lines(),
            ) + continuity_render::chrome::GUTTER_BODY_GAP_DIP
        } else {
            continuity_render::chrome::BODY_LEFT_PADDING_DIP
        };
        let hanging_indent = if spec.is_wrap_continuation {
            let tab_advance = metrics.char_width_dip * self.view_options.tab_width.max(1) as f32;
            continuity_render::FrameDisplay::hanging_indent_advance_dip(
                rope,
                source_line,
                metrics.char_width_dip,
                tab_advance,
                metrics.wrap_width_dip.max(1) as f32,
            )
        } else {
            0.0
        };
        let body = self.focused_body_rect();
        let line_height = self.effective_line_height();
        let (x_dip, y_dip) = compute_ime_candidate_point_dip(
            (body.x, body.y),
            left_margin,
            hanging_indent,
            caret_x,
            display_row,
            line_height,
            self.surface.view.scroll_y_dip,
        );
        let caret_top_dip = y_dip - line_height;
        if caret_top_dip < body.y || caret_top_dip >= body.y + body.h {
            return None;
        }
        let scale = self.dpi_scale();
        Some((
            (x_dip * scale).round() as i32,
            (y_dip * scale).round() as i32,
        ))
    }
}

fn compute_ime_candidate_point_dip(
    body_origin: (f32, f32),
    left_margin: f32,
    hanging_indent: f32,
    caret_x: f32,
    display_row: u32,
    line_height: f32,
    scroll_y: f32,
) -> (f32, f32) {
    (
        body_origin.0 + left_margin + hanging_indent + caret_x,
        body_origin.1 + (display_row as f32 + 1.0) * line_height - scroll_y,
    )
}

#[cfg(test)]
mod tests {
    use super::compute_ime_candidate_point_dip;

    #[test]
    fn candidate_geometry_uses_wrapped_row_indent_and_scroll() {
        let point = compute_ime_candidate_point_dip((20.0, 40.0), 12.0, 24.0, 31.0, 7, 22.0, 110.0);

        assert_eq!(point, (87.0, 106.0));
    }
}
