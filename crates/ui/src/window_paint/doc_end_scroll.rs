//! Deferred document-end scroll correction for paint.

use continuity_render::FrameDisplay;

use crate::window::{Window, END_OF_BUFFER_BOTTOM_PADDING_DIP};

/// Upper bound on consecutive *moving* paints the document-end snap will
/// re-arm itself while the projection's whole-document row index is still
/// partial. Each re-arm is justified by a real jump (the count grew as
/// more of the bottom realized), so this only bounds a pathologically
/// oscillating count — normal convergence finalizes on the first
/// non-moving paint, well before the cap.
const DOC_END_SCROLL_MAX_ATTEMPTS: u8 = 8;

/// One step of the document-end snap state machine. Pure so the
/// convergence contract is unit-testable without a `Window` / DirectWrite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DocEndSnapStep {
    /// Absolute scroll target to jump to this paint, or `None` when the
    /// view is already at the best position the current count yields.
    pub jump_to: Option<f32>,
    /// Schedule a follow-up paint. Set only when we actually moved the
    /// view — a moved view needs to repaint at its new position, and a
    /// still-provisional count wants another paint to converge. **Never**
    /// set on a non-moving paint: a bare re-invalidate spins the
    /// whole-document cold walker for the focused build *and* every
    /// spectator pane.
    pub invalidate: bool,
    /// Clear `pending_doc_end_scroll` — the snap is done.
    pub finalize: bool,
    /// Next value for `pending_doc_end_scroll_attempts`.
    pub attempts: u8,
}

/// Paint-side effect of applying a pending document-end snap.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DocEndSnapPaintAction {
    /// Scroll value that the already-resolved frame was built for. When
    /// present, the current paint must draw with this value; the real
    /// `Window::view` has already moved to the snap target for the next
    /// paint.
    pub previous_scroll_y_dip: Option<f32>,
    /// Schedule the snap repaint after `EndPaint`, so Win32 cannot
    /// validate away an `InvalidateRect` issued during the active paint.
    pub post_paint_invalidate: bool,
}

/// Decide one document-end snap step from the current projection's scroll
/// extent and the live viewport geometry.
///
/// Convergence is driven by *downward movement* against the prefix-sum
/// total ([`FrameDisplay::display_line_count`]), never a guessed estimate.
/// On a large soft-wrapped buffer the offscreen rows are placeholders until
/// the off-thread jump realizes them, so each downward jump realizes more
/// of the bottom (growing the count) and the next paint can snap lower.
///
/// * Authoritative count (`!is_partial()`): jump to the exact bottom in
///   either direction and finalize (or stay put if already there).
/// * Provisional count, target below the current scroll: jump down and
///   re-arm so the next paint converges lower. Bounded by `max_attempts`.
/// * Provisional count, target at or above the current scroll: finalize
///   without moving. The partial count oscillates frame-to-frame in a small
///   (4-pane) viewport; chasing the up-swings is what made the viewport
///   flicker, so the snap only ever tracks the bottom's high-water-mark
///   downward, leaving any upward correction to the authoritative pass.
#[must_use]
pub(crate) fn compute_doc_end_snap_step(
    authoritative: bool,
    scroll_extent_height_dip: f32,
    viewport_height_dip: f32,
    scroll_y_dip: f32,
    attempts: u8,
    max_attempts: u8,
) -> DocEndSnapStep {
    let target = (scroll_extent_height_dip - viewport_height_dip).max(0.0);

    if authoritative {
        // Exact whole-document total: snap to the true bottom (in either
        // direction) and finalize, or stay put if already there.
        let moved = (target - scroll_y_dip).abs() > 0.5;
        return DocEndSnapStep {
            jump_to: if moved { Some(target) } else { None },
            invalidate: moved,
            finalize: true,
            attempts: 0,
        };
    }

    // Provisional count. In a small (4-pane) viewport it oscillates
    // paint-to-paint as different fragments realize
    // (`trace_20260530-181102`: realized_rows bouncing 10188<->10211 every
    // frame, the viewport flickering up and down by ~460 dip while typing
    // at the end). Move the view ONLY downward, toward a growing bottom —
    // never chase an up-swing. Typing appends, so the true bottom only
    // moves down; settling at the high-water-mark of the partial count is
    // correct and flicker-free. The authoritative pass above performs any
    // later upward correction once the full count lands.
    let moved_down = target > scroll_y_dip + 0.5;
    if !moved_down {
        // Already at (or below, after an oscillation shrink) the count's
        // bottom. Finalize without moving up; do NOT invalidate — a bare
        // re-invalidate only re-walks the document.
        return DocEndSnapStep {
            jump_to: None,
            invalidate: false,
            finalize: true,
            attempts: 0,
        };
    }
    // Moved toward a growing bottom: jump down and re-arm so the next paint
    // can converge lower. Bounded so an oscillating count can't loop.
    let next = attempts.saturating_add(1);
    let finalize = next >= max_attempts;
    DocEndSnapStep {
        jump_to: Some(target),
        invalidate: true,
        finalize,
        attempts: if finalize { 0 } else { next },
    }
}

impl Window {
    /// Apply the Ctrl+End / Shift+Ctrl+End exact bottom snap after the
    /// canonical paint projection is available.
    ///
    /// The snap target is `content_height + bottom_padding − viewport_height`,
    /// where `content_height` is the document's display-row count. The
    /// padding is a functional EOF inset: without it a fractional viewport
    /// can park the final row exactly on the bottom clip edge.
    ///
    /// On a large soft-wrapped buffer the row count is authoritative only
    /// once the whole-document row index is realized. A P18 viewport-
    /// priority build leaves offscreen rows as placeholders, so the
    /// prefix-sum [`FrameDisplay::display_line_count`] under-reports the
    /// true bottom. Convergence is driven by *movement*: each downward jump
    /// arms an off-thread build of the destination viewport
    /// ([`Window::arm_offthread_jump`]); when it lands the bottom rows are
    /// realized, the count grows, and the next paint snaps lower. The
    /// instant the view is already at the target the current count yields,
    /// the snap finalizes without scheduling another paint.
    pub(super) fn apply_pending_doc_end_scroll_after_projection(
        &mut self,
        frame_display: &FrameDisplay,
    ) -> DocEndSnapPaintAction {
        if !self.pending_doc_end_scroll {
            return DocEndSnapPaintAction::default();
        }

        let authoritative = !frame_display.row_index().is_partial();
        let realized_rows = frame_display.display_line_count();
        // `estimated_rows` is read only for the diagnostic trace below; the
        // scroll extent uses the prefix-sum total, never the estimate.
        let estimated_rows = frame_display.row_index().estimated_total_rows();
        let line_height = self.effective_line_height();
        // Row stride scales with zoom; the EOF breathing-room inset stays a
        // fixed `END_OF_BUFFER_BOTTOM_PADDING_DIP`.
        let content_h = realized_rows as f32 * line_height;
        let scroll_extent_h = content_h + END_OF_BUFFER_BOTTOM_PADDING_DIP;
        let previous_scroll_y_dip = self.view.scroll_y_dip;
        let step = compute_doc_end_snap_step(
            authoritative,
            scroll_extent_h,
            self.view.viewport_height_dip,
            self.view.scroll_y_dip,
            self.pending_doc_end_scroll_attempts,
            DOC_END_SCROLL_MAX_ATTEMPTS,
        );

        // Instrumentation: one `event:doc_end_snap` per snap evaluation.
        // `outcome`: `jump_authoritative` = exact snap; `settled_noop` =
        // already at the target this count yields; `jump_rearm` = moved
        // toward a growing bottom, re-armed; `cap_hit` = finalized at the
        // attempt cap on an oscillating count. `realized_rows` vs `est_rows`
        // shows the under-report. Gated so the format runs only when tracing
        // is on.
        if crate::paint_trace::is_trace_enabled() {
            let target = (scroll_extent_h - self.view.viewport_height_dip).max(0.0);
            let outcome = if step.jump_to.is_none() {
                "settled_noop"
            } else if authoritative {
                "jump_authoritative"
            } else if step.finalize {
                "cap_hit"
            } else {
                "jump_rearm"
            };
            crate::paint_trace::log_event(
                "event:doc_end_snap",
                &format!(
                    "outcome={outcome} authoritative={authoritative} \
                     current={current:.1} target={target:.1} \
                     realized_rows={realized} est_rows={est} \
                     viewport_h={vp:.1} attempts_in={ain} attempts_out={aout} \
                     finalize={fin} invalidate={inv}",
                    current = previous_scroll_y_dip,
                    realized = realized_rows,
                    est = estimated_rows,
                    vp = self.view.viewport_height_dip,
                    ain = self.pending_doc_end_scroll_attempts,
                    aout = step.attempts,
                    fin = step.finalize,
                    inv = step.invalidate,
                ),
            );
        }

        let mut action = DocEndSnapPaintAction::default();
        if let Some(target) = step.jump_to {
            self.view.jump_to(target, scroll_extent_h);
            action.previous_scroll_y_dip = Some(previous_scroll_y_dip);
            if !authoritative {
                // The bottom region isn't realized yet — its paint would
                // inline-walk the new rows on the UI thread (~20 ms on a
                // 10 k-line wrapped buffer). Build it on the worker and
                // reuse the prior frame + a placeholder strip until it
                // lands. Armed after the jump so the prewarm targets the
                // bottom viewport. This off-thread realization of the
                // destination is what converges the snap to the bottom — the
                // full background index is not required.
                self.arm_offthread_jump("doc_end_jump");
            }
        }
        if step.invalidate {
            action.post_paint_invalidate = true;
        }
        self.pending_doc_end_scroll_attempts = step.attempts;
        if step.finalize {
            self.pending_doc_end_scroll = false;
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::{compute_doc_end_snap_step, DocEndSnapStep};

    const CAP: u8 = 8;
    // 47-line wrapped buffer at LINE_HEIGHT=20 → 199 display rows, viewport
    // ~933 dip. EOF reveal adds one line of bottom inset, so target scroll
    // = 200*20 - 933 = 3067.
    const VIEWPORT: f32 = 933.0;

    #[test]
    fn authoritative_jumps_to_exact_bottom_and_finalizes() {
        let step = compute_doc_end_snap_step(true, 200.0 * 20.0, VIEWPORT, 0.0, 0, CAP);
        assert_eq!(
            step,
            DocEndSnapStep {
                jump_to: Some(3067.0),
                invalidate: true,
                finalize: true,
                attempts: 0,
            }
        );
    }

    #[test]
    fn authoritative_already_at_bottom_finalizes_without_repaint() {
        let step = compute_doc_end_snap_step(true, 200.0 * 20.0, VIEWPORT, 3067.0, 0, CAP);
        assert_eq!(step.jump_to, None);
        assert!(!step.invalidate);
        assert!(step.finalize);
    }

    #[test]
    fn provisional_converged_finalizes_without_spinning() {
        // A provisional (partial) count whose target equals the current
        // scroll must NOT invalidate — that is what re-ran the whole-
        // document cold walker every frame. content_h = 10127*20 = 202540;
        // target = 202540 - 933 = 201607.
        let step = compute_doc_end_snap_step(false, 10127.0 * 20.0, VIEWPORT, 201607.0, 1, CAP);
        assert_eq!(step.jump_to, None);
        assert!(!step.invalidate, "must not re-paint a converged view");
        assert!(step.finalize);
        assert_eq!(step.attempts, 0);
    }

    #[test]
    fn provisional_moving_rearms_and_counts_attempts() {
        let step = compute_doc_end_snap_step(false, 200.0 * 20.0, VIEWPORT, 0.0, 0, CAP);
        assert_eq!(step.jump_to, Some(3067.0));
        assert!(step.invalidate);
        assert!(!step.finalize);
        assert_eq!(step.attempts, 1);
    }

    #[test]
    fn provisional_moving_finalizes_at_attempt_cap() {
        let step = compute_doc_end_snap_step(false, 200.0 * 20.0, VIEWPORT, 0.0, CAP - 1, CAP);
        assert_eq!(step.jump_to, Some(3067.0));
        assert!(step.invalidate);
        assert!(step.finalize, "cap reached → stop re-arming");
        assert_eq!(step.attempts, 0);
    }

    #[test]
    fn provisional_up_swing_does_not_chase_and_finalizes() {
        // The anti-flicker contract: a partial count that shrank from an
        // oscillation (target now ABOVE the current scroll) must NOT jump
        // the view up. It settles at the high-water-mark instead.
        // current=203808, target = 203348 (scroll_extent 203348+VIEWPORT).
        let step =
            compute_doc_end_snap_step(false, 203348.0 + VIEWPORT, VIEWPORT, 203808.0, 2, CAP);
        assert_eq!(
            step.jump_to, None,
            "must not jump up to a shrunk partial count"
        );
        assert!(!step.invalidate, "must not re-paint an up-swing");
        assert!(step.finalize, "settle at the bottom high-water-mark");
        assert_eq!(step.attempts, 0);
    }

    #[test]
    fn bottom_padding_keeps_last_row_inside_viewport() {
        let step = compute_doc_end_snap_step(true, 1020.0, 1000.0, 0.0, 0, CAP);
        assert_eq!(step.jump_to, Some(20.0));
        assert!(step.invalidate);
        assert!(step.finalize);
    }

    #[test]
    fn content_shorter_than_viewport_targets_top() {
        let step = compute_doc_end_snap_step(true, 100.0, VIEWPORT, 0.0, 0, CAP);
        // max(100 - 933, 0) == 0, already there → finalize, no move.
        assert_eq!(step.jump_to, None);
        assert!(!step.invalidate);
        assert!(step.finalize);
    }
}
