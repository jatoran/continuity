//! δ.3 — caret-line screen-y anchor.
//!
//! Implements the principle from `.docs/design/principles.md` §"Layout
//! shifts preserve caret-line screen y": when font scale, font family,
//! soft-wrap width, viewport geometry, or any other reflow source
//! changes, the line the user is looking at must stay at the same screen
//! y. Content above and below reflows; the anchored line does not move.
//!
//! ## Contract
//!
//! Every reflow-causing call site routes through
//! [`Window::with_caret_line_anchored`]. The helper:
//!
//! 1. Picks the anchored line ([`AnchorTarget`]): the caret line when the
//!    caret row overlaps the viewport, otherwise the source line at the
//!    viewport's top edge (the caret is off screen, so its y is not what
//!    the user is tracking). Captures that line's screen y via the
//!    current [`continuity_render::FrameDisplay`] projection —
//!    wrap-aware, fold-aware.
//! 2. Runs the closure (which mutates `self.surface.view` or other state).
//! 3. Recomputes the anchored line's display-line index under the
//!    post-reflow projection.
//! 4. Adjusts `view.scroll_y_dip` so the line lands at the snapshotted
//!    screen y, clamped into `[0, max_scroll]`. Only a visible-caret
//!    anchor is additionally clamped into the viewport (staying visible
//!    wins over staying at the "right" y when the viewport shrank); an
//!    off-screen anchor is never pulled into view — a reflow the user did
//!    not ask for must not scroll them away from what they were reading.
//!
//! When the anchored line vanishes mid-reflow (a fold collapsed it), the
//! nearest surviving display line *above* it is anchored instead.
//!
//! ## Single helper, many funnels
//!
//! Today the helper wraps two funnels: [`Window::invalidate_font_state`]
//! (covers font-scale and font-family reflows) and
//! [`Window::refresh_focused_viewport`] (covers pane resize, window
//! resize, pane-focus switch, sidebar toggle, minimap appearance,
//! distraction-free). Direct callers exist for triggers that bypass both
//! funnels — currently the soft-wrap toggle in [`crate::window_view`] and
//! the silent external-change reload. All future reflow surfaces must
//! route through this helper; never write a parallel anchor.
//!
//! Sibling modules: `anchor_target.rs` (payload + pure scroll math),
//! `viewport_top.rs` (top-line capture + frame selection),
//! `resolve_build.rs` (projection builds for the resolve path).

use continuity_render::FrameDisplay;
use continuity_text::Position;

use crate::window::Window;

mod anchor_target;
mod resolve_build;
mod viewport_top;

pub(crate) use anchor_target::{
    anchored_scroll, anchored_scroll_without_reveal, is_row_overlapping_viewport, AnchorTarget,
    CaretAnchor,
};

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
    /// First absolute display row of the caret's *source line* (i.e. the
    /// prefix-sum of every display row above it). Unlike `display_row` this
    /// excludes the caret's own soft-wrap continuation offset, so it tracks
    /// only the geometry *above* the caret line — the quantity the
    /// viewport geometry-shift anchor compensates.
    pub source_line_first_display_row: u32,
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

    /// Snapshot the pre-reflow anchor. Returns `None` when no buffer is
    /// open or the caret's source line is fully folded with no surviving
    /// line above (in which case anchoring is a no-op and the closure
    /// runs without scroll adjustment).
    ///
    /// The anchored line is the caret line when the caret row overlaps
    /// the viewport. When the user has scrolled the caret off screen the
    /// source line at the viewport's top edge is anchored instead (see
    /// [`AnchorTarget`]) — a reflow must never yank the viewport back to
    /// an off-screen caret. This was the "view jumps when I click back
    /// into a pane" bug: a pane-focus switch refreshes the viewport
    /// geometry, and the old caret-only anchor clamped the off-screen
    /// caret line into view.
    pub(crate) fn capture_caret_anchor(&self) -> Option<CaretAnchor> {
        self.caret_anchor_capture_count
            .set(self.caret_anchor_capture_count.get().saturating_add(1));
        let (caret, caret_screen_y) = self.current_primary_caret_screen_y_dip()?;
        let line_height = self.effective_line_height();
        let viewport_h = self.surface.view.viewport_height_dip;
        if is_row_overlapping_viewport(caret_screen_y, line_height, viewport_h) {
            return Some(CaretAnchor {
                position: caret,
                screen_y: caret_screen_y,
                target: AnchorTarget::VisibleCaret,
            });
        }
        if let Some(anchor) = self.capture_viewport_top_line_anchor(caret, line_height) {
            return Some(anchor);
        }
        Some(CaretAnchor {
            position: caret,
            screen_y: caret_screen_y,
            target: AnchorTarget::OffScreenCaret,
        })
    }

    /// Current primary-caret line y in pane-body DIPs. Returns the caret
    /// position too so capture can restore against the same source point.
    pub(crate) fn current_primary_caret_screen_y_dip(&self) -> Option<(Position, f32)> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let sel = snap.selections().first()?;
        let caret = sel.head;
        let display_line = self.resolve_caret_display_line(caret)?;
        let screen_y = display_line.display_row as f32 * self.effective_line_height()
            - self.surface.view.scroll_y_dip;
        Some((caret, screen_y))
    }

    /// Recompute the anchored line's display-line index under the current
    /// projection and shift `view.scroll_y_dip` so the line sits at
    /// `anchor.screen_y`. Always clamps into `[0, max_scroll]`; only a
    /// [`AnchorTarget::VisibleCaret`] anchor additionally clamps the line
    /// into the visible viewport (on-screen wins over "right y" when the
    /// viewport shrank). An off-screen anchor is restored to its
    /// off-screen y so the reflow never scrolls the user away from what
    /// they were reading.
    pub(crate) fn restore_caret_anchor(&mut self, anchor: CaretAnchor) {
        let Some(display_line_after) = self.resolve_display_line_in_caret_frame(
            self.primary_caret_position().unwrap_or(anchor.position),
            anchor.position,
        ) else {
            return;
        };
        let line_height = self.effective_line_height();
        let row_after = match anchor.target {
            AnchorTarget::ViewportTopLine => display_line_after.source_line_first_display_row,
            AnchorTarget::VisibleCaret | AnchorTarget::OffScreenCaret => {
                display_line_after.display_row
            }
        };
        let new_line_top = row_after as f32 * line_height;
        let content_h = self
            .estimated_content_height()
            .max(display_line_after.total_display_rows.max(1) as f32 * line_height);
        let viewport_h = self.surface.view.viewport_height_dip;
        let new_scroll = match anchor.target {
            AnchorTarget::VisibleCaret => anchored_scroll(
                new_line_top,
                line_height,
                anchor.screen_y,
                viewport_h,
                content_h,
            ),
            AnchorTarget::ViewportTopLine | AnchorTarget::OffScreenCaret => {
                anchored_scroll_without_reveal(new_line_top, anchor.screen_y, viewport_h, content_h)
            }
        };
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "caret_anchor_restore",
                &format!(
                    "target={:?} line={} row_after={row_after} screen_y={:.1} scroll={:.1}->{new_scroll:.1}",
                    anchor.target,
                    anchor.position.line,
                    anchor.screen_y,
                    self.surface.view.scroll_y_dip,
                ),
            );
        }
        self.surface.view.scroll_y_dip = new_scroll;
        // The paint-time geometry anchor's baselines were captured under
        // the pre-reflow projection; comparing them against the next
        // painted frame would compensate the same row delta a second
        // time. Re-baseline on the next paint instead.
        self.surface.geometry_anchor.reset_baselines();
    }

    /// Primary caret head of the focused buffer, if any.
    pub(crate) fn primary_caret_position(&self) -> Option<Position> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        snap.selections().first().map(|sel| sel.head)
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
        self.resolve_display_line_in_caret_frame(caret, caret)
    }

    /// Resolve `lookup`'s display row in the projection that `caret`
    /// selects. The frame is chosen / built with `caret` as the revealed
    /// caret byte (so block-reveal geometry matches what is painted) and
    /// `lookup` is the line whose row is read out of it. The two coincide
    /// for the caret-line anchor; the viewport-top anchor passes the top
    /// source line as `lookup` while keeping the real caret in the query.
    pub(crate) fn resolve_display_line_in_caret_frame(
        &self,
        caret: Position,
        lookup: Position,
    ) -> Option<CaretDisplayLine> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let rope = snap.rope_snapshot().rope();
        let revision = snap.rope_snapshot().revision().0;
        let decorations = self
            .decoration_cache
            .get(self.buffer_id.as_uuid().as_u128());
        let line = lookup.line as usize;
        let caret_bytes = [caret_byte_offset(rope, caret)];

        let metrics =
            self.display_projection_metrics(self.current_search_minimap_active(), rope.len_lines());
        let query = crate::display_prewarm_cache::PrewarmQuery::new(
            self.buffer_id,
            revision,
            decorations.map(|decorations| decorations.revision),
            &caret_bytes,
            &[],
            metrics.wrap_width_dip,
            self.surface.render.font_state,
        );
        let (mut fd, mut frame_source) =
            if let Some(found) = self.select_anchor_frame_display(&query) {
                found
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
                        match self.surface.projection.last_painted_frame_display.as_ref() {
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
            compute_caret_display_line_from_frame(&fd, line, lookup.byte_in_line as usize)?;
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
                    lookup.byte_in_line as usize,
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
        let source_line_first_display_row =
            frame_display.first_display_line_index_for_source(source_line);
        if let Some(display_row) =
            frame_display.display_line_index_for_source_pos(source_line, byte_in_source_line)
        {
            return Some(CaretDisplayLine {
                display_row,
                source_line_first_display_row,
                total_display_rows,
                source_line_rows,
                resolution: CaretDisplayLineResolution::RealizedSpec,
                index_is_partial,
            });
        }
        return Some(CaretDisplayLine {
            display_row: source_line_first_display_row,
            source_line_first_display_row,
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
            let first = frame_display.first_display_line_index_for_source(probe as usize);
            return Some(CaretDisplayLine {
                display_row: first,
                source_line_first_display_row: first,
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

/// Absolute byte offset of `pos` in `rope`, clamped to the rope end.
pub(super) fn caret_byte_offset(rope: &ropey::Rope, pos: Position) -> usize {
    let line = pos.line as usize;
    let line_start = if line < rope.len_lines() {
        rope.line_to_byte(line)
    } else {
        rope.len_bytes()
    };
    (line_start + pos.byte_in_line as usize).min(rope.len_bytes())
}

fn caret_display_line_from_source_floor(
    rope: &ropey::Rope,
    source_line: usize,
) -> CaretDisplayLine {
    let total_source_lines = rope.len_lines().max(1) as u32;
    let display_row = (source_line as u32).min(total_source_lines.saturating_sub(1));
    CaretDisplayLine {
        display_row,
        source_line_first_display_row: display_row,
        total_display_rows: total_source_lines,
        source_line_rows: 1,
        resolution: CaretDisplayLineResolution::SourceFloor,
        // No usable row index at all — definitely can't trust the floor.
        index_is_partial: true,
    }
}

#[cfg(test)]
mod tests;
