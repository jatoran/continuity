//! Window-side footnote hover-peek wiring.
//!
//! Thread ownership: the hover state is stored in `Window::mouse_state`
//! and is mutated only on the owning window's UI thread.

use continuity_decorate::ByteRange;
use continuity_render::{FooterText, OverlayDraw, Rect};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::footnote_hover::{format_footnote_body, FootnoteHover};
use crate::overlay_render::{make_panel, PRIMARY_FG, ROW_HEIGHT};
use crate::window_timers::{FOOTNOTE_HOVER_TIMER_ID, FOOTNOTE_HOVER_TIMER_MS};
use crate::Window;

impl Window {
    /// Build a passive peek overlay for the currently-ready footnote.
    pub(crate) fn footnote_hover_overlay(
        &self,
        client_width: f32,
        client_height: f32,
    ) -> Option<OverlayDraw> {
        let hover = self.mouse_state.footnote_hover.as_ref()?;
        if !hover.ready {
            return None;
        }
        let panel_width = client_width.mul_add(0.45, 0.0).clamp(260.0, 520.0);
        let lines = hover.body_text.lines().count().clamp(1, 5) as f32;
        let panel_height = 58.0 + lines * ROW_HEIGHT;
        let x = ((hover.anchor_x as f32) + 18.0)
            .min(client_width - panel_width - 12.0)
            .max(12.0);
        let y = ((hover.anchor_y as f32) + 22.0)
            .min(client_height - panel_height - 12.0)
            .max(12.0);

        let mut panel = make_panel(Rect::new(x, y, panel_width, panel_height));
        panel.corner_radius = 6.0;
        Some(OverlayDraw {
            panel,
            input_focused: false,
            focus_field: None,
            secondary_field: None,
            list_rows: Vec::new(),
            scrollbar: None,
            footer: Some(FooterText {
                rect: Rect::new(x + 14.0, y + 14.0, panel_width - 28.0, panel_height - 28.0),
                text: format!("[^{}] {}", hover.label, hover.body_text),
                fg: PRIMARY_FG,
            }),
        })
    }

    /// Update passive footnote hover state from the current mouse pixel.
    /// Returns `true` when the visible peek state changed and needs paint.
    pub(crate) fn update_footnote_hover_from_pixel(&mut self, x: i32, y: i32) -> bool {
        let Some((label, reference_range, body_text)) = self.footnote_hover_target_at_pixel(x, y)
        else {
            return self.clear_footnote_hover();
        };
        let now_ms = self.now_ms();
        if let Some(hover) = self.mouse_state.footnote_hover.as_mut() {
            if hover.is_same_reference(&label, reference_range) {
                let was_ready = hover.ready;
                hover.anchor_x = x;
                hover.anchor_y = y;
                if hover.dwell_elapsed(now_ms) {
                    hover.ready = true;
                }
                return was_ready != hover.ready;
            }
        }
        let was_ready = self
            .mouse_state
            .footnote_hover
            .as_ref()
            .is_some_and(|hover| hover.ready);
        self.mouse_state.footnote_hover = Some(FootnoteHover {
            label,
            reference_range,
            body_text,
            anchor_x: x,
            anchor_y: y,
            started_ms: now_ms,
            ready: false,
        });
        unsafe {
            let _ = SetTimer(
                Some(self.hwnd),
                FOOTNOTE_HOVER_TIMER_ID,
                FOOTNOTE_HOVER_TIMER_MS,
                None,
            );
        }
        was_ready
    }

    /// Dwell timer callback for the passive footnote hover-peek.
    pub(crate) fn on_footnote_hover_timer(&mut self, hwnd: HWND) {
        let now_ms = self.now_ms();
        let Some(hover) = self.mouse_state.footnote_hover.as_mut() else {
            self.stop_footnote_hover_timer(hwnd);
            return;
        };
        if hover.dwell_elapsed(now_ms) {
            hover.ready = true;
            self.stop_footnote_hover_timer(hwnd);
        }
    }

    /// Clear any pending or visible footnote hover-peek.
    pub(crate) fn clear_footnote_hover(&mut self) -> bool {
        let had_hover = self.mouse_state.footnote_hover.take().is_some();
        if had_hover {
            self.stop_footnote_hover_timer(self.hwnd);
        }
        had_hover
    }

    fn footnote_hover_target_at_pixel(
        &self,
        x: i32,
        y: i32,
    ) -> Option<(String, ByteRange, String)> {
        // Cheap-check first: footnote hover is only meaningful when
        // the buffer **has** at least one footnote reference. Without
        // this gate every `WM_MOUSEMOVE` paid a `client_to_buffer_position`
        // — and that in turn paid a viewport-bounded `FrameDisplay`
        // build on a 9 k-line markdown buffer when
        // `last_painted_frame_display`'s wrap_width drifted from the
        // current pane geometry (~460 ms per mouse move, surfaced as
        // `WM_MOUSEMOVE 466 000 us` in
        // `perf-snapshots/manual-lag_after-coalesce_20260517-235814.tsv`).
        let snap = self.current_snapshot()?;
        let revision = snap.rope_snapshot().revision().get();
        let decorations = self
            .decoration_cache
            .get(self.buffer_id.as_uuid().as_u128())
            .filter(|decorations| decorations.revision == revision)?;
        if !decorations.inlines.iter().any(|s| {
            matches!(
                s.kind,
                continuity_decorate::InlineKind::FootnoteReference { .. }
            )
        }) {
            return None;
        }
        let position = self.client_to_buffer_position(x, y)?;
        let rope = snap.rope_snapshot().rope();
        let source_byte = source_byte_for_position(rope, position);
        let (label, reference_range) = decorations.footnote_reference_at(source_byte)?;
        let (_, body_range) = decorations.footnote_definition_for(&label)?;
        // Defensive char-boundary check matches the table-layout and
        // image-placement guards: skip the hover when the decoration
        // span's bytes don't align to the current rope's char
        // boundaries (the decoration revision check above should
        // already prevent this, but a transient mid-paint worker
        // delivery can theoretically race the check).
        if rope.try_byte_to_char(body_range.start).is_err()
            || rope.try_byte_to_char(body_range.end).is_err()
        {
            return None;
        }
        let raw_body = rope
            .byte_slice(body_range.start..body_range.end)
            .to_string();
        let body_text = format_footnote_body(&raw_body);
        Some((label, reference_range, body_text))
    }

    fn stop_footnote_hover_timer(&self, hwnd: HWND) {
        if hwnd.0.is_null() {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(hwnd), FOOTNOTE_HOVER_TIMER_ID);
        }
    }
}

fn source_byte_for_position(rope: &ropey::Rope, position: continuity_text::Position) -> usize {
    let line = position.line as usize;
    let line_start = if line < rope.len_lines() {
        rope.line_to_byte(line)
    } else {
        rope.len_bytes()
    };
    line_start
        .saturating_add(position.byte_in_line as usize)
        .min(rope.len_bytes())
}
