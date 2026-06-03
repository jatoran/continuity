//! Pixel → buffer-position hit-test helpers for [`crate::Window`] mouse
//! handlers. Lives next to `window_mouse.rs` (sibling per the codebase's
//! flat `crates/ui/src/window_*` layout) to keep that file under the
//! 600-line cap.
//!
//! Surface: [`Window::client_to_buffer_position`] (`pub(crate)`,
//! consumed by sibling hover / segment-hit modules) plus the
//! caret-placement entry points used from the mouse dispatcher.

use continuity_display_map::{DisplayByte, DisplayLineSpec};
use continuity_render::{hit_test_x_to_byte_for_spec, DEFAULT_HEADING_SCALE};
use continuity_text::{Position, Selection, SelectionKind};

use crate::Window;

mod frame_display;
mod table_cells;

impl Window {
    /// `WM_LBUTTONDOWN` / shift-extend path: place the primary caret at
    /// the pixel location. `extend = true` keeps the existing anchor.
    pub(crate) fn place_caret_at_pixel(&mut self, x: i32, y: i32, extend: bool) -> bool {
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return false,
        };
        let target = self
            .client_to_buffer_position(x, y)
            .unwrap_or(Position::ZERO);
        let selections = if extend {
            let prev = snap
                .selections()
                .first()
                .copied()
                .unwrap_or_else(|| Selection::caret_at(Position::ZERO));
            vec![Selection::new(prev.anchor, target, SelectionKind::Caret)]
        } else {
            vec![Selection::caret_at(target)]
        };
        let changed = selections.as_slice() != snap.selections();
        let count = selections.len();
        let _scope = crate::paint_trace::is_trace_enabled().then(|| {
            crate::paint_trace::EventScope::with_detail(
                "selection_set",
                format!(
                    "entry=place_caret_at_pixel extend={extend} changed={changed} selections={count}"
                ),
            )
        });
        let result = self.editor.set_selections(self.buffer_id, selections);
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "selection_update",
                &format!(
                    "entry=place_caret_at_pixel extend={extend} changed={changed} selections={count} ok={}",
                    result.is_ok()
                ),
            );
        }
        // §H1 caret-move repaint: not needed here — the WM_LBUTTONDOWN
        // dispatcher in `window.rs` invalidates the client area when
        // `on_left_button_down` returns `true`, which it does whenever
        // this path runs. Keyboard caret moves invalidate via the
        // matching `WM_KEYDOWN` arm. The focus-mode dim layer therefore
        // follows the caret on every move without extra plumbing.
        true
    }

    /// Alt+Click: append a new caret at the click position without
    /// disturbing existing selections. If a caret already sits at the
    /// click target it is removed instead — same toggling behaviour the
    /// keyboard `add_cursor_*` commands rely on (the core thread's
    /// `coalesce_selections` will dedup identical entries either way).
    pub(crate) fn add_cursor_at_pixel(&mut self, x: i32, y: i32) -> bool {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return false;
        };
        let target = self
            .client_to_buffer_position(x, y)
            .unwrap_or(Position::ZERO);
        let mut selections: Vec<Selection> = snap.selections().to_vec();
        let new_caret = Selection::caret_at(target);
        // Toggle: if a caret already exists exactly at the click target,
        // remove it. Otherwise append a new secondary caret. Refuse to
        // remove the last cursor — at least one selection must remain.
        if let Some(idx) = selections.iter().position(|s| *s == new_caret) {
            if selections.len() > 1 {
                selections.remove(idx);
            }
        } else {
            selections.push(new_caret);
        }
        let count = selections.len();
        let _scope = crate::paint_trace::is_trace_enabled().then(|| {
            crate::paint_trace::EventScope::with_detail(
                "selection_set",
                format!("entry=add_cursor_at_pixel changed=true selections={count}"),
            )
        });
        let result = self.editor.set_selections(self.buffer_id, selections);
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "selection_update",
                &format!(
                    "entry=add_cursor_at_pixel changed=true selections={count} ok={}",
                    result.is_ok()
                ),
            );
        }
        // Force the blink-visible flag on so the new caret renders on
        // the very next paint instead of waiting for the next blink
        // tick — fixes the "added cursor doesn't show until I type"
        // case for the click path. Keyboard paths get this via
        // `note_input_now` at the top of `on_keydown`; the mouse path
        // calls it here for the same effect.
        self.note_input_now();
        true
    }

    /// Map a client-area `(x, y)` (in DIPs) to a buffer `Position` for
    /// the focused pane. Accounts for:
    /// - the pane's body origin (tab strip height + multi-pane layout),
    /// - the renderer's left margin (gutter when line numbers on, the
    ///   small `BODY_LEFT_PADDING_DIP` when off),
    /// - the active vertical scroll offset, and
    /// - DirectWrite hit-testing for the actual rendered glyph metrics.
    pub(crate) fn client_to_buffer_position(&self, x: i32, y: i32) -> Option<Position> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let rope = snap.rope_snapshot().rope();
        let body = self.focused_body_rect();

        // Body-relative y (clamped to body), then add scroll offset so a
        // click in the visible viewport resolves to the right display row.
        let target_display_row = self.display_row_for_client_y(y);

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
        // Build the projection with the *actual* wrap width so a click on
        // a wrap-continuation row resolves to that row's source line
        // instead of the source line N rows below (where N = continuation
        // row index). Falls back to `0` when soft-wrap is off — one
        // display line per source line, same as before.
        let metrics =
            self.display_projection_metrics(self.current_search_minimap_active(), rope.len_lines());
        let column_advance = metrics.char_width_dip;
        let (frame_display, _source, _folds) = self.resolve_hit_test_frame_display(
            rope,
            revision,
            decorations,
            &caret_bytes,
            metrics.wrap_width_dip,
            column_advance,
            target_display_row,
        );
        let total_dl = frame_display.display_line_count();
        if total_dl == 0 {
            return Some(Position::new(0, 0));
        }

        // Below-last-row guard: a click whose floored row is past the last
        // existing display row resolves to the END of the last source line,
        // regardless of click x (matching common editors). Only fires when
        // the document tail is realized in the resolved frame — otherwise
        // (a partially-realized large buffer scrolled away from its tail)
        // fall through to the existing clamp + x-hit-test path so there is
        // no regression. A click *within* the last row's band yields
        // `target_display_row == total_dl - 1` and is unaffected. Placed
        // after the spec count is known but before the table-cell branch:
        // continuation/empty-band rows are never table-cell rows, so the
        // cell hit-test cannot apply to a below-content click anyway.
        let tail_realized = frame_display.realized_row_range().end == total_dl;
        if target_display_row >= total_dl && tail_realized {
            return Some(compute_end_of_last_source_line_position(rope));
        }

        let dl_idx = target_display_row.min(total_dl.saturating_sub(1));
        let spec = frame_display.display_line_by_index(dl_idx)?;
        let source_line = spec.source_line.raw() as usize;
        let line_u32 = source_line as u32;

        // Hanging indent: wrap-continuation rows are painted shifted
        // right by the source line's leading-whitespace advance (see
        // wrap_paint.rs). Subtract the same offset so the hit-test
        // happens in the row's display-text coordinates.
        let leading_dip = if spec.is_wrap_continuation {
            // A literal tab advances by `tab_width` columns (the same
            // width the renderer paints it at). Spaces advance by one
            // column each. Keep this in lock-step with `tab_width` so
            // the hanging-indent hit-test lands where the glyph is.
            let tab_advance = column_advance * self.view_options.tab_width.max(1) as f32;
            continuity_render::FrameDisplay::leading_whitespace_advance_dip(
                rope,
                source_line,
                column_advance,
                tab_advance,
            )
        } else {
            0.0
        };

        // Cell-rect hit-test: when the click falls inside a visual
        // table cell rect, snap to a source byte INSIDE that cell's
        // `source_range`. The default hit-test below uses the projected
        // display layout (pipes already hidden, glyphs left-justified),
        // which sits at completely different x positions than the
        // visual cell chrome — so without this branch the click on
        // cell C2 typically lands the caret somewhere in C3, or on a
        // hidden pipe byte between cells. Returns `None` when the
        // click doesn't fall inside any cell rect; the default
        // hit-test then runs.
        if let Some(pos) = self.try_cell_rect_hit_test(
            rope,
            source_line,
            dl_idx,
            x as f32,
            y as f32,
            column_advance,
        ) {
            return Some(pos);
        }

        let byte = self
            .pixel_to_source_byte_for_row(rope, spec, x, body.x, leading_dip)
            .unwrap_or(0);
        Some(Position::new(line_u32, byte))
    }

    /// Convert a client y coordinate into the focused pane's absolute
    /// display row, clamping the coordinate to the current body rect.
    pub(crate) fn display_row_for_client_y(&self, y: i32) -> u32 {
        let body = self.focused_body_rect();
        let y_in_body_max = (body.h - 1.0).max(0.0);
        let y_in_body = ((y as f32) - body.y).clamp(0.0, y_in_body_max);
        let virtual_y = y_in_body + self.view.scroll_y_dip;
        (virtual_y / self.effective_line_height()).floor().max(0.0) as u32
    }

    /// Map a client-area x (in DIPs) on a specific display row (`spec`)
    /// to a *source* byte-in-line. Hit-testing happens against the row's
    /// own display text (already wrap-segmented and reveal/hide
    /// resolved), then translated back to a source byte relative to the
    /// source line's start. `leading_dip` is the hanging-indent shift the
    /// renderer applies to wrap-continuation rows — subtract it so the
    /// hit-test runs in the row's display-text coordinates.
    fn pixel_to_source_byte_for_row(
        &self,
        rope: &ropey::Rope,
        spec: &DisplayLineSpec,
        x_client: i32,
        body_origin_x: f32,
        leading_dip: f32,
    ) -> Option<u32> {
        let format = self.text_format.as_ref()?;
        let left_margin = if self.view_options.line_numbers {
            continuity_render::chrome::gutter_width_for_line_count(
                self.scaled_font_size(),
                rope.len_lines(),
            ) + continuity_render::chrome::GUTTER_BODY_GAP_DIP
        } else {
            continuity_render::chrome::BODY_LEFT_PADDING_DIP
        };
        let source_line = spec.source_line.raw() as usize;
        let line_start = if source_line < rope.len_lines() {
            rope.line_to_byte(source_line)
        } else {
            rope.len_bytes()
        };
        let row_start_in_line = (spec.source_byte_start.raw() as usize).saturating_sub(line_start);

        let x_in_text = (x_client as f32) - body_origin_x - left_margin - leading_dip;
        if x_in_text <= 0.0 {
            return Some(u32::try_from(row_start_in_line).unwrap_or(0));
        }

        let max_width = self.view.viewport_width_dip.max(1.0);
        let display_byte = hit_test_x_to_byte_for_spec(
            self.dwrite.raw(),
            format,
            spec,
            x_in_text,
            max_width,
            self.scaled_font_size(),
            DEFAULT_HEADING_SCALE,
        )?;
        let abs_src = spec
            .display_to_source(DisplayByte::from_usize(display_byte))
            .map(|sb| sb.raw() as usize)
            .unwrap_or_else(|| spec.source_byte_start.raw() as usize);
        let snapped_abs_src =
            snap_source_byte_to_line_char_boundary(rope, source_line, line_start, abs_src);
        Some(u32::try_from(snapped_abs_src.saturating_sub(line_start)).unwrap_or(0))
    }
}

/// Position at the end of the last source line of `rope`, mirroring the
/// trailing-newline handling of the display-map builder's
/// `source_line_range`. Used by the below-last-row click guard so a click
/// in the empty band beneath the document lands the caret at the document
/// tail regardless of click x.
///
/// An empty buffer (`len_bytes() == 0`) yields [`Position::ZERO`]. A buffer
/// ending in `\n` has a synthetic final empty source line, so the end of
/// that line is its start byte. Built from the rope-end byte via
/// [`Position::from_byte_offset`] to avoid duplicating boundary math; falls
/// back to [`Position::ZERO`] on error rather than panicking.
fn compute_end_of_last_source_line_position(rope: &ropey::Rope) -> Position {
    // The last source line spans `line_to_byte(last_line)..len_bytes()`. A
    // buffer ending in `\n`/`\r\n` carries the trailing newline on the
    // *previous* line, so the final source line is the synthetic empty line
    // whose start == end == len_bytes(); mapping the rope-end byte back to a
    // Position therefore lands on that empty line's start (byte_in_line 0),
    // matching source_line_range's trailing-newline handling. For an
    // unwrapped trailing line it lands at end-of-line, and for the empty
    // buffer (len_bytes() == 0) it yields Position::ZERO.
    let end_byte = rope.len_bytes();
    Position::from_byte_offset(rope, end_byte).unwrap_or(Position::ZERO)
}

fn snap_source_byte_to_line_char_boundary(
    rope: &ropey::Rope,
    source_line: usize,
    line_start: usize,
    abs_src: usize,
) -> usize {
    let next = if source_line + 1 < rope.len_lines() {
        rope.line_to_byte(source_line + 1)
    } else {
        rope.len_bytes()
    };
    let Some(line_text) = rope.get_byte_slice(line_start..next) else {
        return line_start;
    };
    let line_text = line_text.to_string();
    let mut local = abs_src.saturating_sub(line_start).min(line_text.len());
    while local > 0 && !line_text.is_char_boundary(local) {
        local = local.saturating_sub(1);
    }
    line_start + local
}
