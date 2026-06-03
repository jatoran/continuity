//! View command implementations: scroll (instant + animated), zoom,
//! soft-wrap toggle, caret-follows-viewport.
//!
//! These are wired through `command::Context` so the registry-driven
//! command path stays uniform with everything in Phases 4–8.

use continuity_decorate::Decorations;
use continuity_text::Position;
use ropey::Rope;
use windows::Win32::System::SystemInformation::GetTickCount64;

use self::caret_visibility::{
    approximate_caret_continuation_row, estimate_from_frame, is_source_floor_visibility_safe,
    reveal_caret_display_row, CaretVisibilityEstimate,
};
use crate::motion::STRUCTURAL_MOTION_MS;
use crate::window::END_OF_BUFFER_BOTTOM_PADDING_DIP;
use crate::window_helpers::invalidate_hwnd;
use crate::{Error, Window};

mod caret_visibility;

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
    /// Toggle soft wrap. Invalidates layouts for the now-stale wrap width.
    ///
    /// δ.3 — anchored so the caret stays at the same screen y across
    /// the wrap-mode flip. Without this, turning wrap on against a long
    /// line could push the caret off-screen by many rows.
    pub(crate) fn view_toggle_soft_wrap_impl(&mut self) -> Result<(), Error> {
        self.with_caret_line_anchored(|w| {
            w.view.toggle_soft_wrap();
            let new_key = w.view.wrap_width_key();
            w.cache.invalidate_other_wrap_widths(new_key);
        });
        // δ.6 Tier 3 — contract (C) writeback to settings.toml.
        self.persist_toggle_or_log("editor", "word_wrap", self.view.soft_wrap);
        self.request_state_save();
        // The toggle reflows; without an explicit invalidate the only
        // thing scheduling the repaint is the caret-blink timer, so the
        // visual change lags ~500 ms behind the keypress.
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// Pixel-locked scroll by `lines` logical lines (negative = up).
    pub(crate) fn view_scroll_lines_impl(&mut self, lines: f32) -> Result<(), Error> {
        let hwnd = self.hwnd();
        self.stop_scroll_anim(hwnd);
        let line_height = self.effective_line_height();
        let dy = lines * line_height;
        let content_h = self.estimated_content_height();
        self.view.line_height_dip = line_height;
        self.view.overscroll_bottom_dip = self.overscroll_bottom_dip();
        self.view.scroll_instant(dy, content_h);
        self.request_state_save();
        Ok(())
    }

    /// Animated scroll by one viewport-page worth (PageDown / PageUp).
    pub(crate) fn view_scroll_page_impl(&mut self, direction: f32) -> Result<(), Error> {
        self.cancel_scroll_inertia();
        let viewport_h = self.view.viewport_height_dip;
        let line_height = self.effective_line_height();
        // Leave one line of overlap so the user doesn't lose context per
        // page (Sublime / VS Code convention).
        let delta = direction * (viewport_h - line_height).max(line_height);
        let target = self.view.scroll_y_dip + delta;
        let content_h = self.estimated_content_height();
        self.view.line_height_dip = line_height;
        self.view.overscroll_bottom_dip = self.overscroll_bottom_dip();
        if self.motion_policy().is_reduced_motion() || !self.view_options.smooth_scroll {
            self.view.jump_to(target, content_h);
            let hwnd = self.hwnd();
            self.stop_scroll_anim(hwnd);
            self.request_state_save();
            return Ok(());
        }
        let now_ms = unsafe { GetTickCount64() };
        self.view
            .scroll_animated(target, content_h, now_ms, u64::from(STRUCTURAL_MOTION_MS));
        let hwnd = self.hwnd();
        self.start_scroll_anim(hwnd);
        self.request_state_save();
        Ok(())
    }

    /// Animated scroll to the document start.
    pub(crate) fn view_scroll_doc_start_impl(&mut self) -> Result<(), Error> {
        self.cancel_scroll_inertia();
        let content_h = self.estimated_content_height();
        self.view.line_height_dip = self.effective_line_height();
        self.view.overscroll_bottom_dip = self.overscroll_bottom_dip();
        if self.motion_policy().is_reduced_motion() || !self.view_options.smooth_scroll {
            self.view.jump_to(0.0, content_h);
            let hwnd = self.hwnd();
            self.stop_scroll_anim(hwnd);
            self.request_state_save();
            return Ok(());
        }
        let now_ms = unsafe { GetTickCount64() };
        self.view
            .scroll_animated(0.0, content_h, now_ms, u64::from(STRUCTURAL_MOTION_MS));
        let hwnd = self.hwnd();
        self.start_scroll_anim(hwnd);
        self.request_state_save();
        Ok(())
    }

    /// Animated scroll to the document end.
    pub(crate) fn view_scroll_doc_end_impl(&mut self) -> Result<(), Error> {
        self.cancel_scroll_inertia();
        let content_h = self.estimated_content_height() + END_OF_BUFFER_BOTTOM_PADDING_DIP;
        // MUST land the last line at the viewport BOTTOM (one EOF inset),
        // never the top: this path passes `content_h` itself as the scroll
        // target, so a non-zero overscroll allowance in the clamp would
        // overshoot upward by that allowance. Zero it for the doc-end snap
        // so Ctrl+End is unaffected by scroll-past-end.
        self.view.overscroll_bottom_dip = 0.0;
        if self.motion_policy().is_reduced_motion() || !self.view_options.smooth_scroll {
            self.view.jump_to(content_h, content_h);
            let hwnd = self.hwnd();
            self.stop_scroll_anim(hwnd);
            self.request_state_save();
            return Ok(());
        }
        let now_ms = unsafe { GetTickCount64() };
        self.view.scroll_animated(
            content_h,
            content_h,
            now_ms,
            u64::from(STRUCTURAL_MOTION_MS),
        );
        let hwnd = self.hwnd();
        self.start_scroll_anim(hwnd);
        self.request_state_save();
        Ok(())
    }

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
    fn content_height_covering(&self, display_row: f32) -> f32 {
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
                return;
            }
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

        let caret_display_line = self.resolve_caret_display_line(sel.head);
        let display_row_lookup = caret_display_line.map(|line| line.display_row);
        // The resolved row is an exact display position only when the
        // caret's own row was realized. A source-line floor, or a
        // `RowIndexOnly` / `FoldedFallback` lookup against a still-partial
        // row index, reads a prefix sum whose off-viewport lines are
        // placeholdered at one row each — so it under-counts the soft-wrap
        // rows *above* the caret by an amount that grows the deeper the
        // caret sits. That under-count is exactly why a caret far below the
        // viewport reads as "already visible" (its floored row lands inside
        // the display-row viewport) and the view never scrolls to it. In
        // that case estimate the true display row by scaling the source
        // line by the document's average wrap factor.
        let estimated_total_display_rows =
            (self.estimated_content_height() / line_height).round() as u32;
        let needs_scaled =
            caret_display_line.is_none_or(|line| line.needs_scaled_reveal_estimate());
        let display_row = reveal_caret_display_row(
            caret_source_line as u32,
            display_row_lookup,
            total_source_lines as u32,
            estimated_total_display_rows,
            needs_scaled,
        );
        let line_top = display_row * line_height;
        let line_bottom = line_top + line_height;
        let viewport_top = self.view.scroll_y_dip;
        let viewport_bot = viewport_top + self.view.viewport_height_dip;
        let padding_total_display_rows = caret_display_line
            .map(|line| line.total_display_rows)
            .unwrap_or(estimated_total_display_rows)
            .max(estimated_total_display_rows)
            .max(display_row as u32 + 1);
        let reveal_bottom = line_bottom
            + Self::bottom_padding_for_display_row(display_row, padding_total_display_rows);
        // Content-height floor keeps `view.jump_to`'s clamp from cutting
        // the scroll short of the (possibly scaled) caret row.
        let content_h = self
            .content_height_covering(display_row)
            .max(total_source_lines as f32 * line_height)
            .max(estimated_total_display_rows as f32 * line_height)
            .max(
                caret_display_line
                    .map(|line| line.total_display_rows.max(1) as f32 * line_height)
                    .unwrap_or(0.0),
            );
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "ensure_primary_caret_visible",
                &format!(
                    "caret_line={caret_source_line} total_lines={total_source_lines} \
                     lookup={display_row_lookup:?} display_row={display_row:.0} \
                     needs_scaled={needs_scaled} est_total_rows={estimated_total_display_rows} \
                     viewport={viewport_top:.0}..{viewport_bot:.0} content_h={content_h:.0}",
                ),
            );
        }
        // The scaled estimate is an accurate position (not a lower bound),
        // so the reveal is unconditional in both directions: scroll up when
        // the caret row is above the viewport, down when it is below.
        let did_jump = if line_top < viewport_top {
            self.view.jump_to(line_top, content_h);
            true
        } else if reveal_bottom > viewport_bot {
            let target = reveal_bottom - self.view.viewport_height_dip;
            self.view.jump_to(target, content_h);
            true
        } else {
            false
        };
        if did_jump && needs_scaled {
            // The caret's region is unrealized (the row lookup was a scaled
            // estimate over a partial index), so the jump lands on rows the
            // paint would otherwise inline-walk on the UI thread. Off-thread
            // it (fix A): build on the worker, reuse the prior frame + a
            // placeholder strip until it lands.
            self.arm_offthread_jump("reveal_jump");
            self.invalidate(self.hwnd);
        }
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
    use crate::window::{END_OF_BUFFER_BOTTOM_PADDING_DIP, LINE_HEIGHT_DIP};

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
