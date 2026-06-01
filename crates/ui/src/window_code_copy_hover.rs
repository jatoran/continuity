//! Fenced-code-block copy-button hover and click handling.
//!
//! When the cursor enters the painted rect of a fenced code block whose
//! caret is *outside* the block, a small "Copy" button appears at the
//! rendered block's top-right. A click on the button copies the block's inner
//! content (no fence ticks, no info string, no trailing close fence) to
//! the clipboard and flips the button to a brief "Copied" / "Failed"
//! confirmation state for [`CODE_COPY_FEEDBACK_TIMER_MS`] before
//! reverting.
//!
//! The button is a pure paint overlay — it never reflows the document,
//! never participates in the display map, never allocates vertical
//! space. Per-frame it is reborn from this module's hover state plus
//! the painted frame display.
//!
//! **Thread ownership**: UI thread (one Window). All mutation happens
//! on `Window::mouse_state.code_copy_hover`; never touched from worker
//! threads.

use continuity_decorate::{BlockKind, BlockSpan, Decorations};
use continuity_render::{
    chrome::resolve_body_left_margin_for_line_count_dip, fenced_block_left_edge,
    fenced_block_right_edge, CodeCopyButtonDraw, CodeCopyButtonFeedback,
};
use ropey::Rope;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::mouse::{CodeCopyFeedback, CodeCopyHover, CodeCopyKind};

/// Rectangle in client DIP space — `(x, y, width, height)`. Matches the
/// painted copy-button hit rect surfaced from
/// [`Window::fenced_code_block_at_pixel`].
type CodeCopyButtonRect = (f32, f32, f32, f32);

/// Hit-test result for the fenced-code-block copy affordance: the
/// containing block, its painted button rect in client DIPs, and the
/// block's inner content (no fence ticks, no info string).
type FencedCodeBlockHit = (BlockSpan, CodeCopyButtonRect, String);
use crate::window_timers::{CODE_COPY_FEEDBACK_TIMER_ID, CODE_COPY_FEEDBACK_TIMER_MS};
use crate::Window;

mod helpers;
use helpers::{
    block_client_span, button_rect_for_block, expand_rect, fence_info_string, fenced_inner_text,
    inline_button_rect, inline_code_inner_text, rect_contains,
};

/// Fenced-block button geometry — a small square chip just inside the
/// rendered block's top-right corner. Theme-agnostic.
pub(crate) const COPY_BUTTON_WIDTH_DIP: f32 = 22.0;
pub(crate) const COPY_BUTTON_HEIGHT_DIP: f32 = 18.0;
const COPY_BUTTON_INSET_RIGHT_DIP: f32 = 4.0;
const COPY_BUTTON_INSET_TOP_DIP: f32 = 2.0;
/// Heuristic ratio of monospace column-advance to body font size.
/// Mirrors the fallback the renderer's text-metrics layer would
/// produce when a measured advance is unavailable; used here so the
/// UI can hit-test the same rect the painter draws without needing
/// access to DirectWrite directly.
const COLUMN_ADVANCE_HEURISTIC: f32 = 0.6;
/// Inline-span button geometry — smaller chip so it can sit inside a
/// line of running prose while overlapping the rendered code chip.
pub(crate) const INLINE_COPY_BUTTON_WIDTH_DIP: f32 = 16.0;
pub(crate) const INLINE_COPY_BUTTON_HEIGHT_DIP: f32 = 14.0;
/// Slop margin (DIPs) added around the inline-code span's hover hit
/// area so the cursor can travel from the span to the button rect
/// without dropping out of the hover state.
const INLINE_HOVER_SLOP_DIP: f32 = 4.0;

impl Window {
    /// Update the copy-button hover state from a fresh client `(x, y)`.
    /// Returns `true` when the visible button state changed (appeared,
    /// moved to a different block, the cursor crossed into / out of
    /// the button's own rect) so the caller can invalidate.
    ///
    /// Inline-code spans win over fenced blocks at the same pixel —
    /// when a `` `code` `` span sits inside a fenced block (the user
    /// is hovering an inline `` `…` `` written *inside* a `` ``` `` …
    /// `` ``` `` block, e.g. a markdown sample in docs), the inline
    /// hover already has a tighter rect and a more specific copy
    /// target, so we prefer it.
    pub(crate) fn update_code_copy_hover_from_pixel(&mut self, x: i32, y: i32) -> bool {
        if let Some(target) = self.inline_code_span_at_pixel(x, y) {
            return self.apply_inline_hover(target);
        }
        if let Some(target) = self.fenced_code_block_at_pixel(x, y) {
            return self.apply_fenced_hover(target, x, y);
        }
        self.clear_code_copy_hover()
    }

    fn apply_fenced_hover(
        &mut self,
        target: (BlockSpan, (f32, f32, f32, f32), String),
        x: i32,
        y: i32,
    ) -> bool {
        let (block, button_rect, inner_text) = target;
        let button_hovered = rect_contains(button_rect, x as f32, y as f32);
        if let Some(prev) = self.mouse_state.code_copy_hover.as_mut() {
            if prev.kind == CodeCopyKind::Fenced
                && prev.block_start_byte == block.start_byte
                && prev.block_end_byte == block.end_byte
            {
                let prev_button_hovered = prev.button_hovered;
                prev.button_rect = button_rect;
                prev.button_hovered = button_hovered;
                prev.inner_text = inner_text;
                return prev_button_hovered != button_hovered;
            }
        }
        self.mouse_state.code_copy_hover = Some(CodeCopyHover {
            kind: CodeCopyKind::Fenced,
            block_start_byte: block.start_byte,
            block_end_byte: block.end_byte,
            inner_start_byte: block.start_byte,
            inner_end_byte: block.end_byte,
            button_rect,
            button_hovered,
            inner_text,
            feedback: CodeCopyFeedback::None,
        });
        true
    }

    fn apply_inline_hover(&mut self, target: InlineHoverTarget) -> bool {
        let InlineHoverTarget {
            outer_start_byte,
            outer_end_byte,
            inner_start_byte,
            inner_end_byte,
            button_rect,
            button_hovered,
            inner_text,
        } = target;
        if let Some(prev) = self.mouse_state.code_copy_hover.as_mut() {
            if prev.kind == CodeCopyKind::Inline
                && prev.inner_start_byte == inner_start_byte
                && prev.inner_end_byte == inner_end_byte
            {
                let prev_button_hovered = prev.button_hovered;
                prev.button_rect = button_rect;
                prev.button_hovered = button_hovered;
                prev.inner_text = inner_text;
                return prev_button_hovered != button_hovered;
            }
        }
        self.mouse_state.code_copy_hover = Some(CodeCopyHover {
            kind: CodeCopyKind::Inline,
            block_start_byte: outer_start_byte,
            block_end_byte: outer_end_byte,
            inner_start_byte,
            inner_end_byte,
            button_rect,
            button_hovered,
            inner_text,
            feedback: CodeCopyFeedback::None,
        });
        true
    }

    /// `WM_SETCURSOR` predicate — `true` when the client point
    /// `(x_dip, y_dip)` sits inside the live copy-button rect. The
    /// mouse-cursor router upgrades to `IDC_HAND` so the user gets a
    /// click affordance over the chip.
    pub(crate) fn cursor_over_code_copy_button(&self, x_dip: f32, y_dip: f32) -> bool {
        self.mouse_state
            .code_copy_hover
            .as_ref()
            .is_some_and(|hover| rect_contains(hover.button_rect, x_dip, y_dip))
    }

    /// Clear any visible copy-button hover. Returns `true` if state
    /// changed so the caller can invalidate.
    pub(crate) fn clear_code_copy_hover(&mut self) -> bool {
        let had_hover = self.mouse_state.code_copy_hover.take().is_some();
        if had_hover {
            self.stop_code_copy_feedback_timer(self.hwnd);
        }
        had_hover
    }

    /// Hit-test the copy button at `(x, y)`. When the click lands inside
    /// the button rect, copy the cached inner content (verified against
    /// the live rope) and flip the button to its feedback state.
    /// Returns `true` when the click was claimed.
    pub(crate) fn try_code_copy_button_left_down(&mut self, x: i32, y: i32) -> bool {
        let Some(hover) = self.mouse_state.code_copy_hover.as_ref() else {
            return false;
        };
        if !rect_contains(hover.button_rect, x as f32, y as f32) {
            return false;
        }
        let kind = hover.kind;
        let block_start_byte = hover.block_start_byte;
        let block_end_byte = hover.block_end_byte;
        let inner_start_byte = hover.inner_start_byte;
        let inner_end_byte = hover.inner_end_byte;
        let cached_inner_text = hover.inner_text.clone();
        // Recover the inner text from the current rope rather than
        // trusting the cached hint — the cache was built from the
        // last paint's hits and a stale revision could lie.
        let inner_text = self
            .current_snapshot()
            .map(|snap| {
                let rope = snap.rope_snapshot().rope();
                match kind {
                    CodeCopyKind::Fenced => {
                        fenced_inner_text(rope, block_start_byte, block_end_byte)
                    }
                    CodeCopyKind::Inline => {
                        inline_code_inner_text(rope, inner_start_byte, inner_end_byte)
                    }
                }
            })
            .unwrap_or(cached_inner_text);
        let copy_result = self.put_clipboard_text(&inner_text);
        let feedback = match copy_result {
            Ok(()) => CodeCopyFeedback::Copied,
            Err(_) => CodeCopyFeedback::Failed,
        };
        if let Some(hover) = self.mouse_state.code_copy_hover.as_mut() {
            hover.feedback = feedback;
        }
        if crate::paint_trace::is_trace_enabled() {
            let (kind_str, language) = match kind {
                CodeCopyKind::Fenced => {
                    let lang = self
                        .current_snapshot()
                        .and_then(|snap| {
                            fence_info_string(
                                snap.rope_snapshot().rope(),
                                block_start_byte,
                                block_end_byte,
                            )
                        })
                        .unwrap_or_default();
                    ("fenced", lang)
                }
                CodeCopyKind::Inline => ("inline", String::new()),
            };
            crate::paint_trace::log_event(
                "code_copy",
                &format!(
                    "kind={kind_str} chars={chars} language={language}",
                    chars = inner_text.chars().count(),
                ),
            );
        }
        unsafe {
            let _ = SetTimer(
                Some(self.hwnd),
                CODE_COPY_FEEDBACK_TIMER_ID,
                CODE_COPY_FEEDBACK_TIMER_MS,
                None,
            );
        }
        true
    }

    /// `WM_TIMER` handler for the feedback revert. The button drops
    /// back to its idle/hover state once the user has had time to see
    /// the confirmation.
    pub(crate) fn on_code_copy_feedback_timer(&mut self, hwnd: HWND) {
        self.stop_code_copy_feedback_timer(hwnd);
        if let Some(hover) = self.mouse_state.code_copy_hover.as_mut() {
            hover.feedback = CodeCopyFeedback::None;
        }
    }

    fn stop_code_copy_feedback_timer(&self, hwnd: HWND) {
        if hwnd.0.is_null() {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(hwnd), CODE_COPY_FEEDBACK_TIMER_ID);
        }
    }

    /// Locate a fenced code block whose painted rect contains `(x, y)`
    /// and whose caret is outside the block. Returns `(block, button
    /// rect in client DIPs, inner content)` or `None`.
    fn fenced_code_block_at_pixel(&self, x: i32, y: i32) -> Option<FencedCodeBlockHit> {
        let snap = self.current_snapshot()?;
        let document = self.buffer_id.as_uuid().as_u128();
        let (last_painted_query, frame_display) = self.last_painted_frame_display.as_ref()?;
        if last_painted_query.document() != document {
            return None;
        }
        let decorations: &Decorations = self.last_painted_decorations.as_deref()?;
        let body = self.focused_body_rect();
        let rope = snap.rope_snapshot().rope();
        let xf = x as f32;
        let yf = y as f32;
        // Caret bytes for the caret-outside check. A multi-cursor with
        // *any* caret inside the block reveals the fence; only when
        // every caret is outside do we paint the copy button.
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
                line_start
                    .saturating_add(s.head.byte_in_line as usize)
                    .min(rope.len_bytes())
            })
            .collect();
        let scaled_font = self.scaled_font_size();
        let column_advance = scaled_font * COLUMN_ADVANCE_HEURISTIC;
        for block in &decorations.blocks {
            if !matches!(block.kind, BlockKind::FencedCodeBlock) {
                continue;
            }
            if caret_inside_block(&caret_bytes, block) {
                continue;
            }
            let (top, bottom) = match block_client_span(
                frame_display,
                rope,
                block,
                self.effective_line_height(),
                self.view.scroll_y_dip,
            ) {
                Some(span) => span,
                None => continue,
            };
            // Content-width clip — mirrors the renderer's
            // `fenced_block_right_edge` so the hit rect aligns with
            // the painted highlight. The renderer paints starting at
            // `margins.left` (body-local) so the bg doesn't bleed
            // into the line-number gutter; UI mirrors that offset
            // here using the public line-count-aware margin helper
            // helper.
            let first = byte_to_line(rope, block.start_byte);
            let last = byte_to_line(rope, block.end_byte.saturating_sub(1));
            let body_text_left_dip = resolve_body_left_margin_for_line_count_dip(
                self.view_options.line_numbers,
                scaled_font,
                rope.len_lines(),
            );
            let inner_width = fenced_block_right_edge(
                rope,
                first,
                last,
                column_advance,
                (body.w - body_text_left_dip).max(0.0),
            );
            let block_left = body.x + fenced_block_left_edge(body_text_left_dip);
            let block_right = body.x + body_text_left_dip + inner_width;
            let block_top = body.y + top;
            let block_bottom = body.y + bottom;
            if xf < block_left || xf >= block_right || yf < block_top || yf >= block_bottom {
                continue;
            }
            let button_rect = button_rect_for_block(block_left, block_right, block_top);
            let inner_text = fenced_inner_text(rope, block.start_byte, block.end_byte);
            return Some((block.clone(), button_rect, inner_text));
        }
        None
    }

    /// Locate an inline `` `code` `` span whose painted hit rect (plus
    /// the standard slop margin) contains `(x, y)` and whose enclosing
    /// run has no caret inside. Reads the renderer's
    /// `last_inline_code_hits` ring filled during the last paint —
    /// the rects are already in client DIPs.
    fn inline_code_span_at_pixel(&self, x: i32, y: i32) -> Option<InlineHoverTarget> {
        let snap = self.current_snapshot()?;
        let rope = snap.rope_snapshot().rope();
        let xf = x as f32;
        let yf = y as f32;
        let hits = self.renderer.as_ref()?.inline_code_hits();
        if hits.is_empty() {
            return None;
        }
        // Walk in reverse so the most-recently-painted span (top of
        // the Z-order) wins on overlap, mirroring the image-hit
        // handler's policy.
        for hit in hits.iter().rev() {
            let span_rect = expand_rect(hit.rect_client, INLINE_HOVER_SLOP_DIP);
            let button_rect = inline_button_rect(hit.rect_client);
            let combined_left = span_rect.0.min(button_rect.0);
            let combined_top = span_rect.1.min(button_rect.1);
            let combined_right = (span_rect.0 + span_rect.2).max(button_rect.0 + button_rect.2);
            let combined_bottom = (span_rect.1 + span_rect.3).max(button_rect.1 + button_rect.3);
            if xf < combined_left
                || xf >= combined_right
                || yf < combined_top
                || yf >= combined_bottom
            {
                continue;
            }
            let inner_text = inline_code_inner_text(rope, hit.inner_start_byte, hit.inner_end_byte);
            let button_hovered = rect_contains(button_rect, xf, yf);
            return Some(InlineHoverTarget {
                outer_start_byte: hit.outer_start_byte,
                outer_end_byte: hit.outer_end_byte,
                inner_start_byte: hit.inner_start_byte,
                inner_end_byte: hit.inner_end_byte,
                button_rect,
                button_hovered,
                inner_text,
            });
        }
        None
    }
}

/// Plain-data payload built by `inline_code_span_at_pixel` and
/// consumed by `apply_inline_hover`. Kept private — no other module
/// constructs `CodeCopyHover` directly.
struct InlineHoverTarget {
    outer_start_byte: usize,
    outer_end_byte: usize,
    inner_start_byte: usize,
    inner_end_byte: usize,
    button_rect: (f32, f32, f32, f32),
    button_hovered: bool,
    inner_text: String,
}

fn caret_inside_block(caret_bytes: &[usize], block: &BlockSpan) -> bool {
    caret_bytes
        .iter()
        .any(|b| *b >= block.start_byte && *b <= block.end_byte)
}

fn byte_to_line(rope: &Rope, byte: usize) -> usize {
    let clamped = byte.min(rope.len_bytes());
    rope.byte_to_line(clamped)
}

/// Build the per-frame [`CodeCopyButtonDraw`] payload from the live
/// hover state. Pulled out of the [`crate::window_paint`] orchestrator
/// to keep the parent file under the 600-line conventions cap.
///
/// Re-verifies the caret-outside-block predicate against the current
/// snapshot so a keyboard motion (`Ctrl+End`, arrow keys) that lands
/// inside the hovered block hides the button on the very next paint
/// even though the mouse hasn't moved. The hover state itself is left
/// in place — the next `WM_MOUSEMOVE` will either clear it (cursor
/// has drifted out of the block) or re-arm it (caret leaves again).
pub(crate) fn build_code_copy_button_draw(window: &Window) -> Option<CodeCopyButtonDraw> {
    let hover = window.mouse_state.code_copy_hover.as_ref()?;
    let caret_inside = window
        .current_snapshot()
        .map(|snap| {
            let rope = snap.rope_snapshot().rope();
            snap.selections().iter().any(|sel| {
                let line = sel.head.line as usize;
                let line_start = if line < rope.len_lines() {
                    rope.line_to_byte(line)
                } else {
                    rope.len_bytes()
                };
                let byte = line_start
                    .saturating_add(sel.head.byte_in_line as usize)
                    .min(rope.len_bytes());
                byte >= hover.block_start_byte && byte <= hover.block_end_byte
            })
        })
        .unwrap_or(false);
    if caret_inside {
        return None;
    }
    let feedback = match hover.feedback {
        CodeCopyFeedback::None => CodeCopyButtonFeedback::None,
        CodeCopyFeedback::Copied => CodeCopyButtonFeedback::Copied,
        CodeCopyFeedback::Failed => CodeCopyButtonFeedback::Failed,
    };
    Some(CodeCopyButtonDraw {
        rect_client: hover.button_rect,
        hovered: hover.button_hovered,
        feedback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_inside_block_inclusive_of_end_byte() {
        let block = BlockSpan {
            kind: BlockKind::FencedCodeBlock,
            start_byte: 10,
            end_byte: 30,
        };
        assert!(caret_inside_block(&[15], &block));
        assert!(caret_inside_block(&[10], &block));
        assert!(caret_inside_block(&[30], &block));
        assert!(!caret_inside_block(&[31], &block));
        assert!(!caret_inside_block(&[9], &block));
    }
}
