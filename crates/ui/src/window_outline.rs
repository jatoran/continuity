//! Phase F2 — outline-sidebar mouse hit-test and TOC mutation handlers.
//!
//! Four responsibilities live here so they stay together:
//!
//! 1. `try_outline_sidebar_mouse_wheel` — scroll the outline list
//!    independently when the pointer is over the sidebar.
//! 2. `try_outline_sidebar_left_down` — read the cached
//!    `view_options.outline_layout`, look up the clicked row, and
//!    place the caret on the matching heading line while scrolling
//!    that line to the viewport top.
//! 3. `markdown_insert_toc_impl` — generate a fresh TOC block at the
//!    caret line using [`continuity_decorate::toc::format_toc`].
//! 4. `markdown_refresh_toc_impl` — find an existing `<!-- toc -->`
//!    block via [`continuity_decorate::toc::find_toc_block_in_rope`] and
//!    replace it in place. No-op when no marker pair is present.
//!
//! All three run on the UI thread and route the mutating ones through
//! [`continuity_core::EditorHandle::apply_edit`] so each commit lands
//! as a single undo group (per spec §F2 acceptance criteria 1–2).

use continuity_text::{EditOp, Position, Range, Selection};

use crate::window::Window;
use crate::window_helpers::invalidate_hwnd;
use crate::window_outline_entries_cache::{build_outline_entries_snapshot, OutlineEntriesCacheKey};
use crate::window_timers::WHEEL_LINES_PER_NOTCH;
use crate::window_view_context::map_ui_to_command_error;

impl Window {
    /// `WM_MOUSEWHEEL` over the outline sidebar scrolls the outline
    /// list independently of the editor body.
    pub(crate) fn try_outline_sidebar_mouse_wheel(&mut self, x: i32, y: i32, notches: f32) -> bool {
        if !self.view_options.show_outline_sidebar {
            return false;
        }
        let Some(layout) = self.view_options.outline_layout.clone() else {
            let body = self.focused_body_rect();
            let width = self
                .view_options
                .outline_sidebar_width_dip
                .max(0.0)
                .min(body.w);
            let xf = x as f32;
            let yf = y as f32;
            return width > 0.0
                && xf >= body.x + body.w - width
                && xf <= body.x + body.w
                && yf >= body.y
                && yf <= body.y + body.h;
        };
        let (rx, ry, rw, rh) = layout.rect;
        let xf = x as f32;
        let yf = y as f32;
        if xf < rx || xf > rx + rw || yf < ry || yf > ry + rh {
            return false;
        }
        let dy = -notches * WHEEL_LINES_PER_NOTCH * continuity_render::OUTLINE_ROW_HEIGHT_DIP;
        let before = self.view_options.outline_scroll_offset_dip;
        let after = continuity_render::compute_outline_scroll_offset(
            before + dy,
            layout.content_height_dip,
            rh,
        );
        self.view_options.outline_scroll_offset_dip = after;
        if (after - before).abs() > f32::EPSILON {
            invalidate_hwnd(self.hwnd);
        }
        true
    }

    /// `WM_LBUTTONDOWN` hit-test against the cached outline layout. On
    /// hit, jumps the caret to the heading line and scrolls so that
    /// line sits at the viewport top.
    pub(crate) fn try_outline_sidebar_left_down(&mut self, x: i32, y: i32) -> bool {
        if !self.view_options.show_outline_sidebar {
            return false;
        }
        let Some(layout) = self.view_options.outline_layout.clone() else {
            return false;
        };
        let xf = x as f32;
        let yf = y as f32;
        let Some(entry_index) = layout.entry_at(xf, yf) else {
            return false;
        };
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return false;
        };
        let rope = snap.rope_snapshot().rope();
        let decoration_id = self.buffer_id.as_uuid().as_u128();
        let Some(decorations) = self.decoration_cache.get(decoration_id) else {
            return false;
        };
        let key = OutlineEntriesCacheKey {
            buffer_id: self.buffer_id,
            rope_revision: snap.rope_snapshot().revision().get(),
            decoration_revision: Some(decorations.revision),
        };
        let (snapshot, _status) = self
            .outline_entries_cache
            .borrow_mut()
            .get_or_build(key, || {
                build_outline_entries_snapshot(rope, Some(decorations))
            });
        let Some(target) = snapshot.headings.get(entry_index as usize) else {
            return false;
        };
        let line = target.line;
        let position = Position::new(line, 0);
        let _ = self
            .editor
            .set_selections(self.buffer_id, vec![Selection::caret_at(position)]);
        // Pin the heading to the viewport TOP. The source-line index
        // alone (`line * line_height`) is wrong under soft-wrap or folds:
        // every wrapped/folded line above the heading shifts its true
        // display row. Resolve the real display row through the
        // display-map projection (O(visible+overscan)) and pin THAT,
        // matching `center_primary_caret_in_viewport`'s row-aware math.
        let line_height = self.effective_line_height();
        let (target_y, content_h) = match self.resolve_caret_display_line(position) {
            Some(display_line) => {
                let display_row = display_line.display_row as f32;
                let content_h = self
                    .content_height_covering(display_row)
                    .max(display_line.total_display_rows.max(1) as f32 * line_height);
                (display_row * line_height, content_h)
            }
            None => (line as f32 * line_height, self.estimated_content_height()),
        };
        self.view.jump_to(target_y, content_h);
        invalidate_hwnd(self.hwnd);
        true
    }

    /// `markdown.insert_toc` impl: format the buffer's heading list as
    /// a marker-delimited bullet list and insert it at the caret line's
    /// start. One undo group.
    pub(crate) fn markdown_insert_toc_impl(&mut self) -> Result<(), continuity_command::Error> {
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return Ok(()),
        };
        let rope = snap.rope_snapshot().rope();
        let decoration_id = self.buffer_id.as_uuid().as_u128();
        let body = match self.decoration_cache.get(decoration_id) {
            Some(d) => continuity_decorate::toc::format_toc(&continuity_decorate::headings(
                &d.blocks, rope,
            )),
            None => continuity_decorate::toc::format_toc(&[]),
        };
        let caret_line = snap.selections().first().map(|s| s.head.line).unwrap_or(0);
        let insert_at = Position::new(caret_line, 0);
        self.editor
            .apply_edit(self.buffer_id, EditOp::insert(insert_at, body))
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// `markdown.refresh_toc` impl: locate the existing marker pair
    /// and replace the bytes between them with a freshly-formatted
    /// TOC. No-op when no marker pair exists.
    pub(crate) fn markdown_refresh_toc_impl(&mut self) -> Result<(), continuity_command::Error> {
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return Ok(()),
        };
        let rope = snap.rope_snapshot().rope();
        let Some((start_byte, end_byte)) = continuity_decorate::toc::find_toc_block_in_rope(rope)
        else {
            return Ok(());
        };
        let decoration_id = self.buffer_id.as_uuid().as_u128();
        let body = match self.decoration_cache.get(decoration_id) {
            Some(d) => continuity_decorate::toc::format_toc(&continuity_decorate::headings(
                &d.blocks, rope,
            )),
            None => continuity_decorate::toc::format_toc(&[]),
        };
        // The TOC formatter ends in `\n`; the TOC block finder
        // returns the half-open byte range covering the marker lines
        // up to and including the closing marker (without the
        // trailing newline).
        // Trim the formatter's trailing newline so we don't double it.
        let body = body.trim_end_matches('\n').to_string();
        let start_pos = byte_to_position(rope, start_byte);
        let end_pos = byte_to_position(rope, end_byte);
        self.editor
            .apply_edit(
                self.buffer_id,
                EditOp::replace(Range::new(start_pos, end_pos), body),
            )
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }
}

/// Convert an absolute UTF-8 byte offset into a [`Position`]
/// (line + byte_in_line). Clamps to EOF when the byte exceeds the
/// rope's length.
fn byte_to_position(rope: &ropey::Rope, byte: usize) -> Position {
    let total = rope.len_bytes();
    let b = byte.min(total);
    let line = rope.byte_to_line(b);
    let line_start = rope.line_to_byte(line);
    Position::new(line as u32, (b - line_start) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `byte_to_position` clamps over-large byte offsets to EOF so a
    /// refresh that lags the rope by one keystroke does not panic in
    /// the boundary case where the marker pair sits at the buffer end.
    #[test]
    fn byte_to_position_clamps_past_eof() {
        let rope = ropey::Rope::from_str("ab\ncd");
        let pos = byte_to_position(&rope, 99);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.byte_in_line, 2);
    }

    /// `byte_to_position` resolves the start of a non-zero line to
    /// column 0 so `find_toc_block` byte ranges round-trip through
    /// `EditOp::replace` without shifting the marker pair.
    #[test]
    fn byte_to_position_resolves_line_starts() {
        let rope = ropey::Rope::from_str("line0\nline1\nline2\n");
        let line1_start = rope.line_to_byte(1);
        let pos = byte_to_position(&rope, line1_start);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.byte_in_line, 0);
    }
}
