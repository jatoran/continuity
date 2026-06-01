//! Visual pipe-table cell hit-testing for mouse input.

use continuity_render::TABLE_CELL_PAD_DIP;
use continuity_text::{Position, Selection, SelectionKind};

use crate::Window;

impl Window {
    /// Cell-rect-aware hit-test for visual table cells.
    pub(super) fn try_cell_rect_hit_test(
        &self,
        rope: &ropey::Rope,
        source_line: usize,
        dl_idx: u32,
        client_x: f32,
        client_y: f32,
        column_advance: f32,
    ) -> Option<Position> {
        let hit = self.cell_hit_at_pixel_for_row(
            rope,
            source_line,
            dl_idx,
            client_x,
            client_y,
            column_advance,
        )?;
        Some(Position::new(hit.source_line, hit.click_byte_in_line))
    }

    /// Single-click in a visual table cell: set the primary selection
    /// to span the cell's content.
    pub(crate) fn try_select_cell_at_pixel(&mut self, x: i32, y: i32) -> bool {
        let Some(hit) = self.try_cell_hit_at_pixel(x, y) else {
            return false;
        };
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return false,
        };
        let rope = snap.rope_snapshot().rope();
        let line_start = if (hit.source_line as usize) < rope.len_lines() {
            rope.line_to_byte(hit.source_line as usize)
        } else {
            rope.len_bytes()
        };
        let start_pos = byte_to_position(rope, hit.cell_source_range.start);
        let end_pos = byte_to_position(rope, hit.cell_source_range.end);
        let selection = if hit.cell_source_range.start == hit.cell_source_range.end {
            let mid_byte = line_start + hit.click_byte_in_line as usize;
            let mid_pos = byte_to_position(rope, mid_byte);
            Selection::caret_at(mid_pos)
        } else {
            Selection::new(start_pos, end_pos, SelectionKind::Caret)
        };
        let selections = vec![selection];
        let _ = self.editor.set_selections(self.buffer_id, selections);
        true
    }

    /// Maps a client `(x, y)` to a [`CellHit`] for a focused visual table cell.
    pub(crate) fn try_cell_hit_at_pixel(&self, x: i32, y: i32) -> Option<CellHit> {
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
        let source_line = spec.source_line.raw() as usize;
        self.cell_hit_at_pixel_for_row(
            rope,
            source_line,
            dl_idx,
            x as f32,
            y as f32,
            column_advance,
        )
    }

    fn cell_hit_at_pixel_for_row(
        &self,
        rope: &ropey::Rope,
        source_line: usize,
        dl_idx: u32,
        client_x: f32,
        client_y: f32,
        column_advance: f32,
    ) -> Option<CellHit> {
        let body = self.focused_body_rect();
        let left_margin = if self.view_options.line_numbers {
            continuity_render::chrome::gutter_width_for_line_count(
                self.scaled_font_size(),
                rope.len_lines(),
            ) + continuity_render::chrome::GUTTER_BODY_GAP_DIP
        } else {
            continuity_render::chrome::BODY_LEFT_PADDING_DIP
        };
        let line_height = self.effective_line_height();
        let row_top_client = body.y + (dl_idx as f32) * line_height - self.view.scroll_y_dip;
        let row_bottom_client = row_top_client + line_height;
        if client_y < row_top_client || client_y > row_bottom_client {
            return None;
        }
        let layouts_cache = self.last_focused_table_layouts.borrow();
        let layouts: &Vec<continuity_render::TableLayout> =
            layouts_cache.get(&self.buffer_id)?.as_ref();
        let source_line_u32 = source_line as u32;
        let line_start = if source_line < rope.len_lines() {
            rope.line_to_byte(source_line)
        } else {
            rope.len_bytes()
        };
        for layout in layouts.iter() {
            if !layout.covers_source_line(source_line_u32) {
                continue;
            }
            if layout.is_alignment_row(source_line_u32) {
                continue;
            }
            let col_count = layout.col_widths_dip.len();
            for col_index in 0..col_count {
                let cell_left_body = left_margin + layout.cell_x_dip(col_index as u32);
                let cell_width = layout.col_widths_dip.get(col_index).copied().unwrap_or(0.0);
                let cell_left_client = body.x + cell_left_body;
                let cell_right_client = cell_left_client + cell_width;
                if client_x < cell_left_client || client_x > cell_right_client {
                    continue;
                }
                let cell = layout
                    .cells
                    .iter()
                    .find(|c| c.source_line == source_line_u32 && c.col == col_index as u32)?;
                let source_byte =
                    cell_byte_at_pixel(cell, cell_left_client, client_x, column_advance);
                let byte_in_line = (source_byte as u64).saturating_sub(line_start as u64) as u32;
                return Some(CellHit {
                    source_line: source_line_u32,
                    cell_source_range: cell.source_range.clone(),
                    click_byte_in_line: byte_in_line,
                });
            }
        }
        None
    }
}

fn byte_to_position(rope: &ropey::Rope, byte: usize) -> Position {
    let clamped = byte.min(rope.len_bytes());
    let line = rope.byte_to_line(clamped);
    let line_start = rope.line_to_byte(line);
    Position::new(line as u32, (clamped - line_start) as u32)
}

/// Result of a successful cell hit-test.
pub(crate) struct CellHit {
    pub source_line: u32,
    pub cell_source_range: std::ops::Range<usize>,
    pub click_byte_in_line: u32,
}

fn cell_byte_at_pixel(
    cell: &continuity_render::TableCellLayout,
    cell_left_client: f32,
    client_x: f32,
    column_advance: f32,
) -> usize {
    let advance = column_advance.max(1.0);
    let x_in_content = (client_x - cell_left_client - TABLE_CELL_PAD_DIP).max(0.0);
    let char_offset = (x_in_content / advance).round() as usize;
    let total_chars = cell.display_text.chars().count();
    let clamped = char_offset.min(total_chars);
    let byte_offset_in_cell = cell
        .display_text
        .char_indices()
        .nth(clamped)
        .map(|(byte, _)| byte)
        .unwrap_or(cell.display_text.len());
    cell.source_range.start + byte_offset_in_cell
}
