//! δ.3 — caret-line screen-y anchor.
//!
//! Implements the principle from `.docs/design/principles.md` §"Layout
//! shifts preserve caret-line screen y": when font scale, font family,
//! soft-wrap width, viewport geometry, or any other reflow source
//! changes, the display line containing the primary caret must stay at
//! the same screen y. Content above and below reflows; the caret line
//! does not move.
//!
//! ## Contract
//!
//! Every reflow-causing call site routes through
//! [`Window::with_caret_line_anchored`]. The helper:
//!
//! 1. Captures the caret's display-line screen y (via the current
//!    [`continuity_render::FrameDisplay`] projection — wrap-aware,
//!    fold-aware).
//! 2. Runs the closure (which mutates `self.view` or other state).
//! 3. Recomputes the caret's display-line index under the post-reflow
//!    projection.
//! 4. Adjusts `view.scroll_y_dip` so the caret line lands at the
//!    snapshotted screen y, clamped into `[0, max_scroll]` and into the
//!    visible viewport.
//!
//! When the caret line vanishes mid-reflow (a fold collapsed it), the
//! nearest surviving display line *above* the caret position is
//! anchored instead. When the viewport shrank below the snapshotted y,
//! the caret line clamps into the viewport rather than drift off-screen
//! — staying visible wins over staying at the "right" y.
//!
//! ## Single helper, many funnels
//!
//! Today the helper wraps two funnels: [`Window::invalidate_font_state`]
//! (covers font-scale and font-family reflows) and
//! [`Window::refresh_focused_viewport`] (covers pane resize, window
//! resize, sidebar toggle, minimap appearance, distraction-free). Direct
//! callers exist for triggers that bypass both funnels — currently the
//! soft-wrap toggle in [`crate::window_view`]. All future reflow surfaces
//! must route through this helper; never write a parallel anchor.

use continuity_render::FrameDisplay;
use continuity_text::Position;

use crate::window::{Window, LINE_HEIGHT_DIP};

/// How the caret display row was resolved from a frame projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaretDisplayLineResolution {
    /// The source line's realized display specs covered the caret byte.
    RealizedSpec,
    /// The source line exists in the row index but its display specs
    /// were outside the viewport-realized window.
    RowIndexOnly,
    /// The source line was folded out; the row belongs to the nearest
    /// visible line above it.
    FoldedFallback,
    /// No usable projection was available, so the display row was
    /// estimated from the source-line index alone. This collapses every
    /// soft-wrap continuation above the caret, so it under-reports the
    /// true display row on a wrapped buffer — a low-confidence estimate.
    SourceFloor,
}

impl CaretDisplayLineResolution {
    fn as_str(self) -> &'static str {
        match self {
            Self::RealizedSpec => "realized_spec",
            Self::RowIndexOnly => "row_index_only",
            Self::FoldedFallback => "folded_fallback",
            Self::SourceFloor => "source_floor",
        }
    }
}

/// Display-row lookup result for a caret.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CaretDisplayLine {
    /// Absolute display row containing, or conservatively covering, the caret.
    pub display_row: u32,
    /// Total display rows in the projection's whole-document row index.
    pub total_display_rows: u32,
    /// Number of display rows occupied by the caret's source line.
    pub source_line_rows: u32,
    resolution: CaretDisplayLineResolution,
    /// `true` when the backing row index is a P18 viewport-priority
    /// partial walk (off-viewport source lines are placeholdered at one
    /// row each) or no index was available at all. A `RowIndexOnly` /
    /// `FoldedFallback` lookup against such an index under-counts the
    /// soft-wrap rows above the caret, so the reveal must fall back to the
    /// density-scaled estimate rather than trust the prefix sum.
    index_is_partial: bool,
}

impl CaretDisplayLine {
    /// `true` when the resolved `display_row` cannot be trusted as an
    /// absolute display position and the reveal should instead use the
    /// density-scaled estimate.
    ///
    /// A `RealizedSpec` resolution measured the caret's own row, so it is
    /// always exact. Every other resolution reads a prefix sum out of the
    /// row index; when that index is partial (or absent — the source-line
    /// floor), the off-viewport source lines above the caret are
    /// placeholdered at one row each, so the prefix sum under-counts the
    /// soft-wrap rows above the caret by an amount that grows the deeper
    /// the caret sits. That under-count is what makes a caret far below
    /// the viewport read as "already visible" (its floored row lands
    /// inside the display-row viewport), so the view never scrolls to it.
    pub(crate) fn needs_scaled_reveal_estimate(&self) -> bool {
        !matches!(self.resolution, CaretDisplayLineResolution::RealizedSpec)
            && self.index_is_partial
    }
}

/// Captured anchor state for a primary caret prior to a reflow.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CaretAnchor {
    /// Source-rope position of the primary caret head. The caret bytes
    /// do not move during a pure reflow, but the display line they map
    /// to may change (wrap, fold, scale).
    caret: Position,
    /// Screen y (pane-body-relative) of the caret's display line at the
    /// moment the snapshot was taken. This is the value we want to
    /// restore after the closure runs.
    screen_y: f32,
}

impl Window {
    /// Run `f`, preserving the screen y of the line containing the
    /// primary caret across whatever reflow `f` causes. δ.3.
    ///
    /// **Wrap every reflow-causing mutation in this helper.** Font
    /// scale, font family, soft-wrap toggle, viewport width/height,
    /// pane geometry, theme metrics — anything that can change the
    /// caret line's display-line index or the line height belongs here.
    /// Future reflow surfaces must route through this method; do not
    /// introduce parallel anchor logic.
    pub(crate) fn with_caret_line_anchored<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.cancel_scroll_inertia();
        let anchor = self.capture_caret_anchor();
        let out = f(self);
        if let Some(anchor) = anchor {
            self.restore_caret_anchor(anchor);
        }
        out
    }

    /// Snapshot the caret's pre-reflow anchor. Returns `None` when no
    /// buffer is open or the caret's source line is fully folded with
    /// no surviving line above (in which case anchoring is a no-op and
    /// the closure runs without scroll adjustment).
    pub(crate) fn capture_caret_anchor(&self) -> Option<CaretAnchor> {
        self.caret_anchor_capture_count
            .set(self.caret_anchor_capture_count.get().saturating_add(1));
        self.current_primary_caret_screen_y_dip()
            .map(|(caret, screen_y)| CaretAnchor { caret, screen_y })
    }

    /// Current primary-caret line y in pane-body DIPs. Returns the caret
    /// position too so capture can restore against the same source point.
    pub(crate) fn current_primary_caret_screen_y_dip(&self) -> Option<(Position, f32)> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let sel = snap.selections().first()?;
        let caret = sel.head;
        let display_line = self.resolve_caret_display_line(caret)?;
        let screen_y = display_line.display_row as f32 * LINE_HEIGHT_DIP - self.view.scroll_y_dip;
        Some((caret, screen_y))
    }

    /// Recompute the caret's display-line index under the current
    /// projection and shift `view.scroll_y_dip` so the line sits at
    /// `anchor.screen_y`. Clamps into `[0, max_scroll]` and into the
    /// visible viewport.
    pub(crate) fn restore_caret_anchor(&mut self, anchor: CaretAnchor) {
        let Some(display_line_after) = self.resolve_caret_display_line(anchor.caret) else {
            return;
        };
        let new_line_top = display_line_after.display_row as f32 * LINE_HEIGHT_DIP;
        let content_h = self
            .estimated_content_height()
            .max(display_line_after.total_display_rows.max(1) as f32 * LINE_HEIGHT_DIP);
        let viewport_h = self.view.viewport_height_dip;
        let new_scroll = anchored_scroll(
            new_line_top,
            LINE_HEIGHT_DIP,
            anchor.screen_y,
            viewport_h,
            content_h,
        );
        self.view.scroll_y_dip = new_scroll;
    }

    /// Display-line index of the primary caret under the *current*
    /// projection. Returns `None` when both the caret's source line
    /// and every prior source line are folded out.
    ///
    /// Path:
    /// 1. Try `last_painted_frame_display` via
    ///    [`crate::display_prewarm_cache::PrewarmQuery::is_compatible_for_motion`]
    ///    — the **capture** phase of `with_caret_line_anchored`
    ///    targets the pre-reflow projection that the painted frame
    ///    already realised, so the common cases (font-scale unchanged,
    ///    wrap unchanged) reuse the cached frame for free.
    /// 2. Fall back to a **viewport-only** build when a compatible
    ///    cached `DisplayRowIndex` exists. If the exact cache misses
    ///    but the previous painted frame has the same source-line
    ///    shape, refresh only the caret-affected source lines on top
    ///    of that row index. If neither direct path is available,
    ///    return a conservative source-line floor estimate.
    ///
    /// Reusing `last_painted_frame_display` was previously avoided
    /// because at a font-scale or wrap-width transition the cached
    /// frame represents the OLD layout while `restore_caret_anchor`
    /// needs row positions under the NEW layout. The
    /// [`PrewarmQuery::is_compatible_for_motion`] guard rejects
    /// exactly those cases (`font_state` and `wrap_width_dip` are
    /// part of motion-compat), so reuse is now safe — it triggers
    /// only when the inputs match the painted frame's. The viewport
    /// build covers the transition case correctly when a compatible
    /// row index is already cached.
    /// Resolve the caret's display row plus the total row-index height
    /// of the current projection.
    pub(crate) fn resolve_caret_display_line(&self, caret: Position) -> Option<CaretDisplayLine> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let rope = snap.rope_snapshot().rope();
        let revision = snap.rope_snapshot().revision().0;
        let decorations = self
            .decoration_cache
            .get(self.buffer_id.as_uuid().as_u128());
        let line = caret.line as usize;
        let line_start = if line < rope.len_lines() {
            rope.line_to_byte(line)
        } else {
            rope.len_bytes()
        };
        let caret_byte = line_start + caret.byte_in_line as usize;
        let caret_bytes = [caret_byte];

        let metrics =
            self.display_projection_metrics(self.current_search_minimap_active(), rope.len_lines());
        let query = crate::display_prewarm_cache::PrewarmQuery::new(
            self.buffer_id,
            revision,
            decorations.map(|decorations| decorations.revision),
            &caret_bytes,
            &[],
            metrics.wrap_width_dip,
            self.font_state,
        );
        let last_painted =
            self.last_painted_frame_display
                .as_ref()
                .and_then(|(cached_query, painted)| {
                    if cached_query.is_compatible_for_motion(&query) {
                        Some(painted.clone())
                    } else {
                        None
                    }
                });
        // Focus switches can promote the prior spectator projection.
        let spectator = if last_painted.is_none() {
            self.spectator_frame_cache
                .borrow()
                .lookup_for_focused_paint(self.tree.focused, &query)
                .map(|promoted| promoted.frame_display)
        } else {
            None
        };
        let (mut fd, mut frame_source) = if let Some(fd) = last_painted {
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event("caret_anchor_frame_source", "source=last_painted");
            }
            (fd, "last_painted")
        } else if let Some(fd) = spectator {
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event(
                    "caret_anchor_frame_source",
                    "source=spectator_cache",
                );
            }
            (fd, "spectator_cache")
        } else {
            let has_row_index_hit = self.has_cached_row_index_for_frame_display_viewport(
                Some(self.buffer_id),
                revision,
                decorations,
                &[],
                &[],
                metrics.wrap_width_dip,
            );
            if crate::paint_trace::is_trace_enabled() {
                let detail = if has_row_index_hit {
                    "source=row_index_cache_hit".to_string()
                } else {
                    match self.last_painted_frame_display.as_ref() {
                        Some((cached_query, _)) => {
                            let mismatch = cached_query
                                .motion_compat_mismatch(&query)
                                .unwrap_or("unknown");
                            format!("source=viewport_build stale_cache={mismatch}")
                        }
                        None => "source=viewport_build cache=empty".to_string(),
                    }
                };
                crate::paint_trace::log_event("caret_anchor_frame_source", &detail);
            }
            if has_row_index_hit {
                (
                    self.build_caret_anchor_viewport_frame_display(
                        rope,
                        revision,
                        decorations,
                        &caret_bytes,
                        metrics.wrap_width_dip,
                        metrics.char_width_dip,
                    ),
                    "viewport_build",
                )
            } else if let Some(fd) = self.build_caret_anchor_targeted_frame_display(
                &query,
                rope,
                revision,
                decorations,
                &caret_bytes,
                line,
                metrics.wrap_width_dip,
                metrics.char_width_dip,
            ) {
                (fd, "targeted_row_index")
            } else {
                return Some(caret_display_line_from_source_floor(rope, line));
            }
        };

        let mut resolved =
            compute_caret_display_line_from_frame(&fd, line, caret.byte_in_line as usize)?;
        if resolved.resolution == CaretDisplayLineResolution::RowIndexOnly
            && resolved.source_line_rows > 1
        {
            let target_rows = resolved.display_row
                ..resolved
                    .display_row
                    .saturating_add(resolved.source_line_rows.max(1));
            if self.has_cached_row_index_for_frame_display_viewport(
                Some(self.buffer_id),
                revision,
                decorations,
                &[],
                &[],
                metrics.wrap_width_dip,
            ) {
                let refined = self.build_frame_display_viewport_cached(
                    Some(self.buffer_id),
                    rope,
                    revision,
                    decorations,
                    &caret_bytes,
                    &[],
                    &[],
                    metrics.wrap_width_dip,
                    metrics.char_width_dip,
                    target_rows,
                    crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
                    continuity_display_map::WalkerCallReason::ViewportRealize,
                );
                if let Some(refined_line) = compute_caret_display_line_from_frame(
                    &refined,
                    line,
                    caret.byte_in_line as usize,
                ) {
                    if refined_line.resolution != CaretDisplayLineResolution::RowIndexOnly {
                        fd = refined;
                        frame_source = "caret_line_viewport_build";
                        resolved = refined_line;
                    }
                }
            }
        }

        if crate::paint_trace::is_trace_enabled() {
            let realized = fd.realized_row_range();
            crate::paint_trace::log_event(
                "caret_display_line_lookup",
                &format!(
                    "source={frame_source} resolution={} line={} byte={} display_row={} \
                     source_line_rows={} total_rows={} realized={}..{}",
                    resolved.resolution.as_str(),
                    line,
                    caret.byte_in_line,
                    resolved.display_row,
                    resolved.source_line_rows,
                    resolved.total_display_rows,
                    realized.start,
                    realized.end,
                ),
            );
        }
        Some(resolved)
    }

    /// Viewport-bounded frame-display build used by caret anchoring.
    /// Pulled out so the path with and without a cached frame share
    /// the same viewport-row math and overscan policy.
    fn build_caret_anchor_viewport_frame_display(
        &self,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&continuity_decorate::Decorations>,
        caret_bytes: &[usize],
        wrap_width_dip: u32,
        char_width_dip: f32,
    ) -> continuity_render::FrameDisplay {
        let visible_rows = crate::window_paint::visible_display_row_range(
            self.view.scroll_y_dip,
            self.view.viewport_height_dip,
            LINE_HEIGHT_DIP,
        );
        self.build_frame_display_viewport_cached(
            Some(self.buffer_id),
            rope,
            revision,
            decorations,
            caret_bytes,
            &[],
            &[],
            wrap_width_dip,
            char_width_dip,
            visible_rows,
            crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
            continuity_display_map::WalkerCallReason::ViewportRealize,
        )
    }
}

fn compute_caret_display_line_from_frame(
    frame_display: &FrameDisplay,
    source_line: usize,
    byte_in_source_line: usize,
) -> Option<CaretDisplayLine> {
    let total_display_rows = frame_display.display_line_count();
    let index_is_partial = frame_display.row_index().is_partial();
    let source_line_rows = frame_display.display_line_count_for_source(source_line);
    if source_line_rows > 0 {
        if let Some(display_row) =
            frame_display.display_line_index_for_source_pos(source_line, byte_in_source_line)
        {
            return Some(CaretDisplayLine {
                display_row,
                total_display_rows,
                source_line_rows,
                resolution: CaretDisplayLineResolution::RealizedSpec,
                index_is_partial,
            });
        }
        return Some(CaretDisplayLine {
            display_row: frame_display.first_display_line_index_for_source(source_line),
            total_display_rows,
            source_line_rows,
            resolution: CaretDisplayLineResolution::RowIndexOnly,
            index_is_partial,
        });
    }

    // Caret's source line is fully folded — walk upward to the nearest
    // surviving line and anchor against its first display row.
    let mut probe = source_line as i64 - 1;
    while probe >= 0 {
        let rows = frame_display.display_line_count_for_source(probe as usize);
        if rows > 0 {
            return Some(CaretDisplayLine {
                display_row: frame_display.first_display_line_index_for_source(probe as usize),
                total_display_rows,
                source_line_rows: rows,
                resolution: CaretDisplayLineResolution::FoldedFallback,
                index_is_partial,
            });
        }
        probe -= 1;
    }
    None
}

fn caret_display_line_from_source_floor(
    rope: &ropey::Rope,
    source_line: usize,
) -> CaretDisplayLine {
    let total_source_lines = rope.len_lines().max(1) as u32;
    let display_row = (source_line as u32).min(total_source_lines.saturating_sub(1));
    CaretDisplayLine {
        display_row,
        total_display_rows: total_source_lines,
        source_line_rows: 1,
        resolution: CaretDisplayLineResolution::SourceFloor,
        // No usable row index at all — definitely can't trust the floor.
        index_is_partial: true,
    }
}

/// Pure scroll-restoration math, factored out so it can be unit-tested
/// without a `Window`. Given the caret's new line top, the desired
/// pre-reflow screen y, and the post-reflow viewport/content heights,
/// returns the scroll position that places the caret line at
/// `screen_y_before` — clamped into `[0, max_scroll]` and into the
/// viewport.
#[must_use]
pub(crate) fn anchored_scroll(
    new_line_top: f32,
    line_height: f32,
    screen_y_before: f32,
    viewport_h: f32,
    content_h: f32,
) -> f32 {
    let max_scroll = (content_h - viewport_h).max(0.0);
    let target = (new_line_top - screen_y_before).clamp(0.0, max_scroll);
    let proposed_screen_y = new_line_top - target;
    // Caret-line would land below the viewport — pull scroll so the
    // caret bottom touches the viewport bottom instead. On-screen wins
    // over "right y" when the viewport shrunk past the target.
    if proposed_screen_y + line_height > viewport_h && viewport_h > 0.0 {
        return ((new_line_top + line_height - viewport_h).max(0.0)).min(max_scroll);
    }
    // Caret-line would land above the viewport — pin it to the top.
    if proposed_screen_y < 0.0 {
        return new_line_top.min(max_scroll);
    }
    target
}

#[cfg(test)]
mod tests;
