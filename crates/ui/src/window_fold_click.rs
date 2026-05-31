//! §H3 — fold-triangle click hit-testing.
//!
//! Routed from `window_mouse::on_left_button_down` ahead of caret
//! placement so a click on a triangle toggles its line's fold instead
//! of moving the caret. The triangle column lives on the right edge of
//! the gutter at width `chrome_fold::FOLD_TRIANGLE_WIDTH_DIP`.
//!
//! **Thread ownership**: UI thread of one window.

use continuity_render::chrome::gutter_width_for_line_count;
use continuity_render::chrome_fold::FOLD_TRIANGLE_WIDTH_DIP;

use crate::window::{Window, LINE_HEIGHT_DIP};

impl Window {
    /// Try to consume a `WM_LBUTTONDOWN` at `(x, y)` (client coords) as a
    /// fold-triangle toggle. Returns `true` when the click landed inside
    /// the triangle column for some visible foldable source line and the
    /// fold was toggled.
    ///
    /// No-op when the gutter is hidden (`view_options.line_numbers ==
    /// false`) or when the click misses the triangle column.
    pub(crate) fn try_fold_triangle_left_down(&mut self, x: i32, y: i32) -> bool {
        if !self.view_options.line_numbers {
            return false;
        }
        let body = self.focused_body_rect();
        let xf = x as f32;
        let yf = y as f32;
        if yf < body.y || yf >= body.y + body.h {
            return false;
        }
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return false;
        };
        let gutter_width = gutter_width_for_line_count(
            self.scaled_font_size(),
            snap.rope_snapshot().rope().len_lines(),
        );
        let triangle_left = body.x + gutter_width - FOLD_TRIANGLE_WIDTH_DIP;
        let triangle_right = body.x + gutter_width;
        if xf < triangle_left || xf >= triangle_right {
            return false;
        }
        // Map y → source line. Soft-wrap rows are not yet considered —
        // this hits the source line corresponding to the row at this
        // y, which matches what the painter draws for unwrapped lines
        // and is a close approximation when wrap is on (the gutter
        // triangle today paints once per source line, not per wrap row).
        let y_in_body = yf - body.y;
        let virtual_y = y_in_body + self.view.scroll_y_dip;
        if virtual_y < 0.0 {
            return false;
        }
        let line_idx = (virtual_y / LINE_HEIGHT_DIP).floor() as i64;
        if line_idx < 0 {
            return false;
        }
        let total_lines = snap.rope_snapshot().rope().len_lines() as i64;
        if line_idx >= total_lines {
            return false;
        }
        let line = match u32::try_from(line_idx) {
            Ok(n) => n,
            Err(_) => return false,
        };
        // Toggle the fold on this source line — adds when absent, drops
        // when present. Keep the slice sorted so coalescing in the
        // provider stays deterministic.
        let folded = &mut self.view_options.pane_modes.folded_lines;
        if let Some(pos) = folded.iter().position(|&l| l == line) {
            folded.remove(pos);
        } else {
            folded.push(line);
            folded.sort_unstable();
        }
        self.invalidate_with_reason(self.hwnd, "invalidate_rect");
        true
    }
}
