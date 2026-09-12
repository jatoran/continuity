//! Viewport-top anchor capture + anchor frame selection for
//! [`crate::window_caret_anchor`].
//!
//! When the caret is off screen, [`Window::with_caret_line_anchored`]
//! holds the source line at the viewport's top edge instead of the caret
//! line. This module resolves which source line that is, using the same
//! projection the caret-line path would use (last painted frame, promoted
//! spectator frame, or a cheap viewport realize over a cached row index).
//!
//! Thread ownership: UI thread of one window; read-only over `Window`.

use continuity_render::FrameDisplay;
use continuity_text::Position;

use super::{AnchorTarget, CaretAnchor};
use crate::display_prewarm_cache::PrewarmQuery;
use crate::window::Window;

impl Window {
    /// Anchor the source line at the viewport's top edge. Returns `None`
    /// when no projection can name that line cheaply (no painted frame,
    /// no promoted spectator frame, no cached row index) — the caller
    /// then falls back to holding the off-screen caret line without a
    /// reveal clamp.
    ///
    /// `caret` is the primary caret head; it selects the projection (the
    /// block-reveal geometry of the painted frame depends on it) but is
    /// not the anchored line.
    pub(super) fn capture_viewport_top_line_anchor(
        &self,
        caret: Position,
        line_height: f32,
    ) -> Option<CaretAnchor> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let rope = snap.rope_snapshot().rope();
        let revision = snap.rope_snapshot().revision().0;
        let decorations = self
            .decoration_cache
            .get(self.buffer_id.as_uuid().as_u128());
        let caret_bytes = [super::caret_byte_offset(rope, caret)];
        let metrics =
            self.display_projection_metrics(self.current_search_minimap_active(), rope.len_lines());
        let query = PrewarmQuery::new(
            self.buffer_id,
            revision,
            decorations.map(|decorations| decorations.revision),
            &caret_bytes,
            &[],
            metrics.wrap_width_dip,
            self.surface.render.font_state,
        );
        let frame_display = match self.select_anchor_frame_display(&query) {
            Some((frame_display, _)) => frame_display,
            None => {
                let has_row_index_hit = self.has_cached_row_index_for_frame_display_viewport(
                    Some(self.buffer_id),
                    revision,
                    decorations,
                    &[],
                    &[],
                    metrics.wrap_width_dip,
                );
                if !has_row_index_hit {
                    if crate::paint_trace::is_trace_enabled() {
                        crate::paint_trace::log_event(
                            "caret_anchor_capture",
                            "target=off_screen_caret reason=no_cheap_projection",
                        );
                    }
                    return None;
                }
                self.build_caret_anchor_viewport_frame_display(
                    rope,
                    revision,
                    decorations,
                    &caret_bytes,
                    metrics.wrap_width_dip,
                    metrics.char_width_dip,
                )
            }
        };
        let scroll_y = self.surface.view.scroll_y_dip.max(0.0);
        let top_row = (scroll_y / line_height.max(1.0)).floor() as u32;
        let (source_line, _continuation) = frame_display
            .row_index()
            .source_line_for_display_row(top_row)?;
        let first_row = frame_display.first_display_line_index_for_source(source_line.as_usize());
        let screen_y = first_row as f32 * line_height - scroll_y;
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "caret_anchor_capture",
                &format!(
                    "target=viewport_top_line caret_line={} top_row={top_row} \
                     top_source_line={} first_row={first_row} screen_y={screen_y:.1}",
                    caret.line,
                    source_line.raw(),
                ),
            );
        }
        Some(CaretAnchor {
            position: Position::new(source_line.raw(), 0),
            screen_y,
            target: AnchorTarget::ViewportTopLine,
        })
    }

    /// Pick the cheapest projection that is motion-compatible with
    /// `query`: the last painted frame, else the promoted spectator frame
    /// for the focused pane (a focus switch has just made a spectator the
    /// focused buffer). Returns the frame plus a trace label.
    pub(super) fn select_anchor_frame_display(
        &self,
        query: &PrewarmQuery,
    ) -> Option<(FrameDisplay, &'static str)> {
        let last_painted = self
            .surface
            .projection
            .last_painted_frame_display
            .as_ref()
            .and_then(|(cached_query, painted)| {
                if cached_query.is_compatible_for_motion(query) {
                    Some(painted.clone())
                } else {
                    None
                }
            });
        if let Some(frame_display) = last_painted {
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event("caret_anchor_frame_source", "source=last_painted");
            }
            return Some((frame_display, "last_painted"));
        }
        let spectator = self
            .surface
            .projection
            .spectator_frame_cache
            .borrow()
            .lookup_for_focused_paint(self.tree.focused, query)
            .map(|promoted| promoted.frame_display);
        if let Some(frame_display) = spectator {
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event(
                    "caret_anchor_frame_source",
                    "source=spectator_cache",
                );
            }
            return Some((frame_display, "spectator_cache"));
        }
        None
    }
}
