//! Caret-visibility reveal: keep the primary caret inside the viewport
//! after edits and motion, and center it for find-bar navigation.
//!
//! Split out of `window_view.rs` to keep that file under the 600-line
//! cap; the scroll/zoom/soft-wrap command implementations stay there.
//!
//! Thread ownership: UI-thread-only. These methods read the editor
//! snapshot, the projection caches, and `self.view`, and mutate
//! `self.view.scroll_y_dip` plus the geometry-anchor hysteresis
//! hysteresis flag (UI-thread-owned).

use continuity_decorate::Decorations;
use continuity_text::Position;
use ropey::Rope;

use super::caret_visibility::{
    approximate_caret_continuation_row, estimate_from_frame, is_source_floor_visibility_safe,
    CaretVisibilityEstimate,
};
use crate::window::END_OF_BUFFER_BOTTOM_PADDING_DIP;
use crate::Window;

fn compute_eof_append_minimum_reveal(
    previous_display_rows: u32,
    viewport_height_dip: f32,
    scroll_y_dip: f32,
    line_height: f32,
) -> Option<(f32, f32)> {
    // Row stride scales with zoom; the EOF breathing-room inset stays a
    // fixed `END_OF_BUFFER_BOTTOM_PADDING_DIP`.
    let scroll_extent_h = (previous_display_rows.max(1) as f32 + 1.0) * line_height
        + END_OF_BUFFER_BOTTOM_PADDING_DIP;
    let target = (scroll_extent_h - viewport_height_dip).max(0.0);
    (target > scroll_y_dip + 0.5).then_some((target, scroll_extent_h))
}

fn is_beyond_eof_reveal_inset(
    scroll_y_dip: f32,
    content_height_dip: f32,
    viewport_height_dip: f32,
) -> bool {
    let eof_reveal_max_scroll =
        (content_height_dip + END_OF_BUFFER_BOTTOM_PADDING_DIP - viewport_height_dip).max(0.0);
    scroll_y_dip > eof_reveal_max_scroll + 0.5
}

impl Window {
    /// Display-row lookup for the primary caret under the *current* rope
    /// snapshot, not the cached one. Delegates to
    /// [`Window::resolve_caret_display_line`] which transparently
    /// rebuilds a viewport-bounded `FrameDisplay` when
    /// `last_painted_frame_display` is stale (different rope revision,
    /// font state, wrap width, fold signature). That fresh build is the
    /// only way to get a correct row index across an edit that changed
    /// line count or wrap geometry — the cached frame predates the new
    /// line and answers with the *prior* line's row, which is the root
    /// cause of "Enter at bottom leaves caret 1 row off-screen" and
    /// "Enter mid-wrapped-line scrolls past the caret".
    fn primary_caret_display_line(&self) -> Option<crate::window_caret_anchor::CaretDisplayLine> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let sel = snap.selections().first().copied()?;
        self.resolve_caret_display_line(sel.head)
    }

    /// Content-height floor that **always** covers the caret's display
    /// row, even when `last_painted_frame_display.display_line_count()`
    /// is stale (the cache was built before the edit grew the rope).
    /// Without this floor, `view.jump_to` would clamp the target to
    /// `stale_content_h - viewport_h`, which sits above the new caret's
    /// row — typing past the bottom would scroll *almost* but not
    /// quite enough, leaving the caret invisible. Computed from
    /// `(display_row + 1) * LINE_HEIGHT_DIP` plus the EOF inset so the clamp
    /// can reach the caret without placing the final row exactly on the clip
    /// edge regardless of cache freshness.
    pub(crate) fn content_height_covering(&self, display_row: f32) -> f32 {
        let line_height = self.effective_line_height();
        self.estimated_content_height()
            .max((display_row + 1.0) * line_height + END_OF_BUFFER_BOTTOM_PADDING_DIP)
    }

    fn bottom_padding_for_display_row(display_row: f32, total_display_rows: u32) -> f32 {
        if display_row + 1.0 >= total_display_rows.max(1) as f32 {
            END_OF_BUFFER_BOTTOM_PADDING_DIP
        } else {
            0.0
        }
    }

    /// True when this whole pre-paint reveal should be deferred to the
    /// paint-time geometry anchor + visibility clamp. The caret is still on
    /// the same source line it occupied at the previous paint, and that line
    /// was on screen — i.e. the user is typing on a visible line.
    ///
    /// In that case any pre-paint scroll is *harmful*: the reveal estimates
    /// the caret's display row against `last_painted_frame_display`, whose
    /// whole-document geometry swings between paints while the decoration
    /// parse catches up (`window_view::geometry_anchor` documents the
    /// swing). When that estimate disagrees with the frame the paint will
    /// actually resolve, the reveal scrolls toward the *wrong* geometry and
    /// the anchor then has to undo it — a double-count that snaps the caret
    /// to the viewport edge. Deferring is safe in both directions because
    /// the anchor holds the caret line's screen y across the swing and the
    /// visibility clamp (measured against the resolved frame) guarantees the
    /// caret is on screen — so neither an upward nor a downward pre-paint
    /// scroll is needed. A genuine reveal (the caret moved to a *different*
    /// line, or its line was scrolled off screen) fails this predicate and
    /// keeps the proven pre-paint reveal path.
    fn should_defer_reveal_to_anchor(&self, caret_source_line: usize) -> bool {
        self.geometry_anchor.caret_was_on_screen_prior_frame
            && self
                .geometry_anchor
                .previous_paint_caret_line_anchor
                .is_some_and(|(line, _)| line as usize == caret_source_line)
    }

    fn estimate_caret_visibility_row(
        &self,
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret: Position,
        total_source_lines: usize,
    ) -> Option<CaretVisibilityEstimate> {
        let metrics = self
            .display_projection_metrics(self.current_search_minimap_active(), total_source_lines);
        let line = caret.line as usize;
        let line_start = if line < rope.len_lines() {
            rope.line_to_byte(line)
        } else {
            rope.len_bytes()
        };
        let caret_byte = line_start + caret.byte_in_line as usize;
        let caret_bytes = [caret_byte];
        let (query, _) = self.hit_test_projection_query_and_folds(
            rope,
            revision,
            decorations,
            &caret_bytes,
            metrics.wrap_width_dip,
        );
        let continuation = approximate_caret_continuation_row(
            rope,
            line,
            caret.byte_in_line as usize,
            metrics.wrap_width_dip,
            metrics.char_width_dip,
        );

        if let Some((cached_query, frame)) = self.last_painted_frame_display.as_ref() {
            if cached_query.is_compatible_for_hit_test(&query) {
                if let Some(estimate) = estimate_from_frame(
                    frame,
                    line,
                    total_source_lines,
                    continuation,
                    "last_painted_hit_test",
                ) {
                    return Some(estimate);
                }
            }
        }

        if let Some(entry) = self.mouse_hit_test_frame_cache.borrow().as_ref() {
            if entry.query().is_compatible_for_hit_test(&query) {
                if let Some(estimate) = estimate_from_frame(
                    entry.frame_display(),
                    line,
                    total_source_lines,
                    continuation,
                    "mouse_hit_test_cache",
                ) {
                    return Some(estimate);
                }
            }
        }

        if let Some(promoted) = self
            .spectator_frame_cache
            .borrow()
            .lookup_for_focused_paint(self.tree.focused, &query)
        {
            if let Some(estimate) = estimate_from_frame(
                &promoted.frame_display,
                line,
                total_source_lines,
                continuation,
                "spectator_cache",
            ) {
                return Some(estimate);
            }
        }

        Some(CaretVisibilityEstimate {
            display_row: (line as u32).saturating_add(continuation),
            total_display_rows: total_source_lines as u32,
            source: "source_line_floor",
            is_projection_backed: false,
        })
    }

    /// Adjust scroll so the primary caret stays inside the viewport.
    /// Called after edits and motion commands so typing past the
    /// bottom auto-scrolls.
    pub(crate) fn ensure_primary_caret_visible(&mut self) {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let Some(sel) = snap.selections().first().copied() else {
            return;
        };
        let total_source_lines = snap.rope_snapshot().rope().len_lines().max(1);
        let caret_source_line = sel.head.line as usize;
        let line_height = self.effective_line_height();
        // When the caret sits at the very end of the document — Enter or
        // typing that appends a new final line, or a click on the last
        // byte — the new caret line is the document's last display row.
        // Reveal it through the hardened document-end snap rather than the
        // direct estimate below: on a soft-wrapped buffer the cold
        // display-row lookup under-counts the tail (it falls back to the
        // source-line floor), so the direct downward check concludes
        // "already visible" and strands the caret below the viewport. The
        // snap converges to the true bottom via the row index, exactly as
        // Ctrl+End does. (`move_doc_end` is skipped from this reveal hook
        // and arms the snap itself; this covers the edit/append paths.)
        {
            let rope = snap.rope_snapshot().rope();
            let caret_line_start = if caret_source_line < rope.len_lines() {
                rope.line_to_byte(caret_source_line)
            } else {
                rope.len_bytes()
            };
            let caret_byte = caret_line_start + sel.head.byte_in_line as usize;
            let caret_at_doc_end = caret_byte >= rope.len_bytes();
            // Scroll-past-end guard: when the viewport is parked in the
            // overscroll zone (scrolled past the normal bottom so blank space
            // shows below the last line), the caret at EOF is already on
            // screen. Typing there must not yank the viewport back up to pin
            // the last line to the bottom — the caret is visible, so leave the
            // scroll untouched. The reveal/snap below still fires when the
            // caret is genuinely below the viewport (i.e. not in overscroll).
            if caret_at_doc_end {
                let content_h = self.estimated_content_height();
                if is_beyond_eof_reveal_inset(
                    self.view.scroll_y_dip,
                    content_h,
                    self.view.viewport_height_dip,
                ) {
                    // EOF caret parked in the overscroll zone is on screen.
                    self.geometry_anchor.caret_was_on_screen_prior_frame = true;
                    return;
                }
            }
            // Only defer to the snap when the cold display-row lookup can
            // actually under-count — i.e. the document soft-wraps (more
            // display rows than source lines). On a 1:1 buffer the direct
            // reveal below is exact and resolves in a single pre-paint
            // jump, so keep it there rather than paying the snap's extra
            // repaint. `estimated_content_height` counts wrap continuations
            // (it reads the last painted frame's whole-document row count).
            let estimated_display_rows =
                (self.estimated_content_height() / line_height).round() as usize;
            let doc_appears_wrapped = estimated_display_rows > total_source_lines;
            if caret_at_doc_end && doc_appears_wrapped {
                if let Some((_, frame)) = self.last_painted_frame_display.as_ref() {
                    let frame_source_lines = frame.row_index().source_line_count() as usize;
                    let appended_final_source_line = frame_source_lines.saturating_add(1)
                        == total_source_lines
                        && caret_source_line == frame_source_lines;
                    if appended_final_source_line {
                        if let Some((target, extent)) = compute_eof_append_minimum_reveal(
                            frame.display_line_count(),
                            self.view.viewport_height_dip,
                            self.view.scroll_y_dip,
                            line_height,
                        ) {
                            self.view.jump_to(target, extent);
                        }
                    }
                }
                self.pending_doc_end_scroll = true;
                self.pending_doc_end_scroll_attempts = 0;
                // The doc-end snap converges the caret to the bottom row.
                self.geometry_anchor.caret_was_on_screen_prior_frame = true;
                return;
            }
        }
        // Request a paint-time visibility floor against the resolved frame.
        // The authoritative "is the caret actually on screen" decision is
        // made in `apply_geometry_anchor` using the frame that will be drawn.
        self.geometry_anchor.pending_caret_reveal = true;
        // Typing on a visible line: skip the pre-paint scroll entirely and
        // let the anchor hold the line + clamp it visible. A pre-paint
        // estimate scroll here races the resolved-frame geometry and the
        // anchor has to undo it, snapping the caret to the viewport edge —
        // the "it warps me to the top while typing" bug. The clamp covers
        // both scroll directions, so deferring never strands the caret.
        if self.should_defer_reveal_to_anchor(caret_source_line) {
            self.geometry_anchor.caret_was_on_screen_prior_frame = true;
            return;
        }
        let revision = snap.rope_snapshot().revision().0;
        let decorations = self
            .decoration_cache
            .get(self.buffer_id.as_uuid().as_u128());
        if let Some(estimate) = self.estimate_caret_visibility_row(
            snap.rope_snapshot().rope(),
            revision,
            decorations,
            sel.head,
            total_source_lines,
        ) {
            let source_floor_is_safe =
                is_source_floor_visibility_safe(&estimate, &self.view, line_height);
            if estimate.is_projection_backed || source_floor_is_safe {
                self.apply_caret_visibility_estimate(
                    estimate,
                    total_source_lines,
                    caret_source_line,
                );
                return;
            }
        }

        // No fast estimate could resolve the caret confidently. Get the
        // TRUE display row by measurement, never by the document-average
        // wrap factor: for a line whose wrap depth differs from the
        // document mean, the scaled estimate over/undershoots, so the
        // view jumps this frame and snaps back the next — the visible
        // "viewport jumps on typing" bug on large soft-wrapped buffers.
        // `try_resolve_caret_display_row_exact` returns `Some` only for a
        // measured row (caret's own row realized, or a fully-built row
        // index); it forces one viewport-bounded build of the caret
        // region (O(visible+overscan)) before giving up.
        let exact = self.try_resolve_caret_display_row_exact(sel.head);
        let was_on_screen_prior_frame = self.geometry_anchor.caret_was_on_screen_prior_frame;
        let Some(caret_display_line) = exact else {
            // We have only an estimate. Do NOT scroll on it. If the caret
            // was already on screen last frame, hold the viewport
            // (hysteresis: never chase an estimate when nothing forced the
            // caret off-screen). Either way arm an off-thread build so the
            // next frame materializes the caret region and re-runs this
            // path with a measured row before deciding to scroll.
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event(
                    "ensure_primary_caret_visible",
                    &format!(
                        "caret_line={caret_source_line} total_lines={total_source_lines} \
                         resolution=estimate_only action=defer_no_scroll \
                         was_on_screen_prior={was_on_screen_prior_frame}",
                    ),
                );
            }
            if !was_on_screen_prior_frame {
                self.arm_offthread_jump("reveal_caret_region");
                self.invalidate(self.hwnd);
            }
            // Conservatively assume on-screen so a transient estimate miss
            // does not later license an estimate-driven jump.
            self.geometry_anchor.caret_was_on_screen_prior_frame = true;
            return;
        };

        let display_row = caret_display_line.display_row as f32;
        let line_top = display_row * line_height;
        let line_bottom = line_top + line_height;
        let viewport_top = self.view.scroll_y_dip;
        let viewport_bot = viewport_top + self.view.viewport_height_dip;
        let reveal_bottom = line_bottom
            + Self::bottom_padding_for_display_row(
                display_row,
                caret_display_line.total_display_rows,
            );
        let content_h = self
            .content_height_covering(display_row)
            .max(total_source_lines as f32 * line_height)
            .max(caret_display_line.total_display_rows.max(1) as f32 * line_height);
        // The row is measured, so the reveal is exact in both directions:
        // scroll up when the caret row is above the viewport, down when it
        // is below, and leave the viewport alone when it is already inside.
        // (Same-line-on-screen typing already returned early above, deferring
        // to the paint anchor; this path is a genuine reveal of a moved or
        // off-screen caret line.)
        let action = if line_top < viewport_top {
            self.view.jump_to(line_top, content_h);
            "scroll_up"
        } else if reveal_bottom > viewport_bot {
            let target = reveal_bottom - self.view.viewport_height_dip;
            self.view.jump_to(target, content_h);
            "scroll_down"
        } else {
            "visible"
        };
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "ensure_primary_caret_visible",
                &format!(
                    "caret_line={caret_source_line} total_lines={total_source_lines} \
                     display_row={display_row:.0} action={action} \
                     viewport={viewport_top:.0}..{viewport_bot:.0} content_h={content_h:.0}",
                ),
            );
        }
        // The caret now sits inside the viewport (either it already did or
        // we just scrolled it there), so the next frame may rely on the
        // hysteresis hold.
        self.geometry_anchor.caret_was_on_screen_prior_frame = true;
    }

    fn apply_caret_visibility_estimate(
        &mut self,
        estimate: CaretVisibilityEstimate,
        total_source_lines: usize,
        caret_source_line: usize,
    ) {
        let line_height = self.effective_line_height();
        let display_row = estimate.display_row as f32;
        let line_top = display_row * line_height;
        let line_bottom = line_top + line_height;
        let viewport_top = self.view.scroll_y_dip;
        let viewport_bot = viewport_top + self.view.viewport_height_dip;
        let reveal_bottom = line_bottom
            + Self::bottom_padding_for_display_row(display_row, estimate.total_display_rows);
        let content_h = self
            .content_height_covering(display_row)
            .max(total_source_lines as f32 * line_height)
            .max(estimate.total_display_rows.max(1) as f32 * line_height);
        let action = if line_top < viewport_top {
            // Monotonic-safe: a non-projection-backed estimate (source-line
            // floor) under-reports the caret's display row on a wrapped
            // buffer; don't scroll up toward an unmeasured row.
            if estimate.is_projection_backed {
                self.view.jump_to(line_top, content_h);
                "scroll_up"
            } else {
                "skip_low_confidence_up"
            }
        } else if reveal_bottom > viewport_bot {
            let target = reveal_bottom - self.view.viewport_height_dip;
            self.view.jump_to(target, content_h);
            "scroll_down"
        } else {
            "visible"
        };
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "ensure_primary_caret_visible_fast",
                &format!(
                    "source={} action={action} caret_line={caret_source_line} \
                     total_lines={total_source_lines} display_row={} \
                     total_display_rows={} projection_backed={} \
                     viewport={viewport_top:.0}..{viewport_bot:.0} content_h={content_h:.0}",
                    estimate.source,
                    estimate.display_row,
                    estimate.total_display_rows,
                    estimate.is_projection_backed,
                ),
            );
        }
        // The fast estimate path either revealed the caret or left it
        // inside the viewport; record on-screen so the hysteresis hold in
        // the measured path can trust the prior frame.
        self.geometry_anchor.caret_was_on_screen_prior_frame = true;
    }

    /// Scroll so the primary caret's display row is roughly vertically
    /// centered in the viewport. Used by find-bar match navigation
    /// (`jump_to_current_find_match`) where the user expects the match
    /// surrounded by context, not pinned to the viewport edge.
    pub(crate) fn center_primary_caret_in_viewport(&mut self) {
        let Some(caret_display_line) = self.primary_caret_display_line() else {
            return;
        };
        let line_height = self.effective_line_height();
        let display_row = caret_display_line.display_row as f32;
        let line_top = display_row * line_height;
        let viewport_h = self.view.viewport_height_dip;
        let target = (line_top - (viewport_h - line_height) * 0.5).max(0.0);
        let content_h = self
            .content_height_covering(display_row)
            .max(caret_display_line.total_display_rows.max(1) as f32 * line_height);
        self.view.jump_to(target, content_h);
    }
}

#[cfg(test)]
mod tests {
    use super::{compute_eof_append_minimum_reveal, is_beyond_eof_reveal_inset};
    use crate::window::END_OF_BUFFER_BOTTOM_PADDING_DIP;
    use crate::window_constants::LINE_HEIGHT_DIP;

    #[test]
    fn eof_append_minimum_reveal_scrolls_one_new_display_row() {
        let previous_rows = 160;
        let viewport_h = 700.0;
        let current =
            previous_rows as f32 * LINE_HEIGHT_DIP + END_OF_BUFFER_BOTTOM_PADDING_DIP - viewport_h;

        let (target, extent) =
            compute_eof_append_minimum_reveal(previous_rows, viewport_h, current, LINE_HEIGHT_DIP)
                .expect("appended EOF row should move the viewport down");

        assert_eq!(target, current + LINE_HEIGHT_DIP);
        assert_eq!(
            extent,
            (previous_rows as f32 + 1.0) * LINE_HEIGHT_DIP + END_OF_BUFFER_BOTTOM_PADDING_DIP
        );
    }

    #[test]
    fn eof_append_minimum_reveal_does_not_chase_high_water_scroll() {
        let previous_rows = 160;
        let viewport_h = 700.0;
        let current = previous_rows as f32 * LINE_HEIGHT_DIP + END_OF_BUFFER_BOTTOM_PADDING_DIP
            - viewport_h
            + LINE_HEIGHT_DIP;

        assert_eq!(
            compute_eof_append_minimum_reveal(previous_rows, viewport_h, current, LINE_HEIGHT_DIP),
            None
        );
    }

    #[test]
    fn eof_reveal_inset_is_not_user_overscroll() {
        let content_h = 160.0 * LINE_HEIGHT_DIP;
        let viewport_h = 700.0;
        let eof_bottom = content_h + END_OF_BUFFER_BOTTOM_PADDING_DIP - viewport_h;

        assert!(!is_beyond_eof_reveal_inset(
            eof_bottom, content_h, viewport_h
        ));
        assert!(is_beyond_eof_reveal_inset(
            eof_bottom + LINE_HEIGHT_DIP,
            content_h,
            viewport_h
        ));
    }
}
