//! α.1 Edit-action echo family.
//!
//! Renders a short, low-alpha tint over a contiguous source-line range
//! after a *structural* edit (paste, duplicate-line, move-line, autocorrect
//! substitution), an undo/redo step, or a smart-selection-expand boundary
//! step. Tells the writer *what just happened* without consulting the undo
//! tree, and *where* the caret landed without re-targeting their eyes.
//!
//! Three kinds share one mechanism:
//!
//! - [`EditPulseKind::EditRegion`] — 120 ms tint over the bytes a command
//!   added or rewrote.
//! - [`EditPulseKind::UndoTarget`] — 120 ms tint over the line the caret
//!   lands on after `editor.undo` / `editor.redo`.
//! - [`EditPulseKind::SelectionExpand`] — 80 ms tint over the head/anchor
//!   rows after a `select.expand_smart` step, so the smart-expansion
//!   ladder feels tactile.
//!
//! Thread ownership: all state lives on [`crate::Window`] and is touched
//! only from that window's UI thread. Reduced motion suppresses every
//! kind unconditionally (see `evict_expired_edit_pulse` and the
//! `apply_reduced_motion` clear in [`crate::window_motion`]).

use windows::Win32::System::SystemInformation::GetTickCount64;

use continuity_core::SelectionEdit;
use continuity_render::EditPulseDraw;
use continuity_text::Selection;

use crate::motion::ease_out_cubic;
use crate::window_theme::rgba_from_color;
use crate::Window;

/// Duration of an edit-region or undo-target tint (ms).
pub(crate) const EDIT_PULSE_DURATION_MS: u32 = 120;
/// Duration of a selection-expand boundary tint (ms).
pub(crate) const SELECTION_EXPAND_PULSE_DURATION_MS: u32 = 80;

/// Classification of an edit-action echo.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum EditPulseKind {
    /// Structural edit (paste, duplicate, move-line, format-on-save).
    EditRegion,
    /// Undo or redo target row.
    UndoTarget,
    /// Smart-selection-expand boundary.
    SelectionExpand,
}

/// One active edit-action echo.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct EditPulse {
    /// First affected source-line index (inclusive).
    pub first_line: u32,
    /// Last affected source-line index (inclusive). Equal to `first_line`
    /// for a single-line pulse.
    pub last_line: u32,
    /// `GetTickCount64` value captured when the pulse was triggered.
    pub started_ms: u64,
    /// Total fade duration in milliseconds.
    pub duration_ms: u32,
    /// Classification — drives the duration the caller chose plus future
    /// theming hooks. Painted identically today.
    pub kind: EditPulseKind,
}

impl Window {
    /// Trigger an edit-region pulse over `[first_line, last_line]`.
    /// Reduced-motion windows clear any active pulse and return without
    /// arming the motion timer, per the motion contract.
    pub(crate) fn trigger_edit_pulse(
        &mut self,
        first_line: u32,
        last_line: u32,
        duration_ms: u32,
        kind: EditPulseKind,
    ) {
        if self.motion_policy().is_reduced_motion() {
            self.edit_pulse = None;
            return;
        }
        let (lo, hi) = if first_line <= last_line {
            (first_line, last_line)
        } else {
            (last_line, first_line)
        };
        self.edit_pulse = Some(EditPulse {
            first_line: lo,
            last_line: hi,
            started_ms: unsafe { GetTickCount64() },
            duration_ms,
            kind,
        });
        self.start_motion_timer();
    }

    /// Drop the pulse once its fade window has elapsed. Called from the
    /// shared motion tick.
    pub(crate) fn evict_expired_edit_pulse(&mut self) {
        if let Some(p) = self.edit_pulse {
            let now = unsafe { GetTickCount64() };
            if fade_alpha(p, now).is_none() {
                self.edit_pulse = None;
            }
        }
    }

    /// Pulse the source-line range that changed between pre- and
    /// post-edit snapshots. Selects the right kind from
    /// [`EditPulseKind::EditRegion`]. The caller captures the pre-edit
    /// primary-caret line and the pre-edit `len_lines()` before the
    /// dispatch, then hands them here once the edit lands.
    pub(crate) fn pulse_edit_region_after_dispatch(
        &mut self,
        pre_caret_line: u32,
        pre_line_count: usize,
    ) {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let post_line_count = snap.rope_snapshot().rope().len_lines();
        let Some(sel) = snap.selections().first() else {
            return;
        };
        let (first, last) = edit_pulse_range(pre_caret_line, pre_line_count, *sel, post_line_count);
        self.trigger_edit_pulse(
            first,
            last,
            EDIT_PULSE_DURATION_MS,
            EditPulseKind::EditRegion,
        );
    }

    /// Pulse the row the caret landed on after undo / redo. Always fires
    /// unless reduced motion is set — the user explicitly asked for
    /// historical state and benefits from a visible landing marker.
    pub(crate) fn pulse_undo_target(&mut self) {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let Some(sel) = snap.selections().first() else {
            return;
        };
        let lo = sel.head.line.min(sel.anchor.line);
        let hi = sel.head.line.max(sel.anchor.line);
        self.trigger_edit_pulse(lo, hi, EDIT_PULSE_DURATION_MS, EditPulseKind::UndoTarget);
    }

    /// Pulse the head/anchor rows after a `select.expand_smart` step.
    /// Uses the shorter 80 ms tactile-feedback window.
    pub(crate) fn pulse_selection_expand_boundary(&mut self) {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let Some(sel) = snap.selections().first() else {
            return;
        };
        let lo = sel.head.line.min(sel.anchor.line);
        let hi = sel.head.line.max(sel.anchor.line);
        self.trigger_edit_pulse(
            lo,
            hi,
            SELECTION_EXPAND_PULSE_DURATION_MS,
            EditPulseKind::SelectionExpand,
        );
    }

    /// Build the per-frame draw payload for the active pulse, or `None`
    /// when nothing is active.
    pub(crate) fn edit_pulse_draw(&self, now_ms: u64) -> Option<EditPulseDraw> {
        let pulse = self.edit_pulse?;
        let alpha = fade_alpha(pulse, now_ms)?;
        Some(EditPulseDraw {
            first_line: pulse.first_line,
            last_line: pulse.last_line,
            alpha,
            color: rgba_from_color(self.active_theme.current.editor_edit_pulse()),
        })
    }
}

/// Compute the inclusive source-line range covered by the edit, given the
/// pre-edit caret line + line count and the post-edit selection + line
/// count. Bridges the three common shapes:
///
/// - **caret moves with inserted content** (paste, MoveLineDown): post-head
///   already covers the new bytes.
/// - **caret stays, content inserted below** (DuplicateLine): widen by the
///   `len_lines` delta.
/// - **caret stays, content removed** (DeleteWord): narrow to the caret
///   line — nothing else changed visually.
#[must_use]
pub(crate) fn edit_pulse_range(
    pre_caret_line: u32,
    pre_line_count: usize,
    post_selection: Selection,
    post_line_count: usize,
) -> (u32, u32) {
    let post_head = post_selection.head.line;
    let post_anchor = post_selection.anchor.line;
    let first = pre_caret_line.min(post_head).min(post_anchor);
    let post_max = post_head.max(post_anchor);
    let delta_lines = post_line_count.saturating_sub(pre_line_count) as u32;
    let last = if post_max == pre_caret_line && delta_lines > 0 {
        // Caret didn't move; inserted content sits below it (DuplicateLine).
        pre_caret_line.saturating_add(delta_lines)
    } else {
        // When the post-edit caret lands ABOVE the pre-edit caret (an
        // upward jump), the pulse must still cover both endpoints —
        // otherwise the range collapses to just `post_max` and the
        // line the writer was on (pre_caret_line) loses its echo.
        post_max.max(pre_caret_line)
    };
    if first <= last {
        (first, last)
    } else {
        (first, first)
    }
}

/// Classify a [`SelectionEdit`] as *structural* (worth an α.1 echo when
/// dispatched through `Window::dispatch_selection_edit`) or not.
///
/// Returning `false` does **not** mean "this edit never pulses" — paste,
/// autocorrect substitution, and undo/redo flow through their own
/// command handlers that arm the pulse explicitly with the correct
/// pre-state. The rule here only governs the inline path: continuous
/// typing (InsertText, DeleteBack/Forward, single-key newline inserts),
/// indent/outdent, and whole-buffer normalisations are silent; everything
/// that visibly rearranges lines or adds markdown structure pulses.
#[must_use]
pub(crate) fn is_structural_edit(edit: &SelectionEdit) -> bool {
    match edit {
        // Continuous typing / single-key edits — silent.
        SelectionEdit::InsertText(_)
        | SelectionEdit::DeleteBack
        | SelectionEdit::DeleteForward
        | SelectionEdit::DeleteWordBackward
        | SelectionEdit::DeleteWordForward
        | SelectionEdit::DeleteToLineStart
        | SelectionEdit::DeleteToLineEnd
        | SelectionEdit::DeleteToBracket
        | SelectionEdit::InsertNewlineAbove
        | SelectionEdit::InsertNewlineBelow
        | SelectionEdit::InsertNewlineSmart
        | SelectionEdit::ToggleBulletAtLineStart
        | SelectionEdit::ToggleBulletWithContinuationIndent { .. }
        | SelectionEdit::Indent { .. }
        | SelectionEdit::Outdent { .. }
        | SelectionEdit::InsertPair { .. }
        | SelectionEdit::DeletePair { .. } => false,
        // Whole-buffer normalisations — would pulse every line, which
        // defeats the "where did it land" purpose.
        SelectionEdit::TrimTrailingWhitespaceAll
        | SelectionEdit::TrimWhitespaceAll
        | SelectionEdit::ConvertLineEndingsAll(_)
        | SelectionEdit::TabsToSpacesAll { .. } => false,
        // Selection-scope reflows, transpositions, and markdown edits
        // — pulse so the writer sees the changed footprint.
        SelectionEdit::DuplicateLine
        | SelectionEdit::DuplicateSelection
        | SelectionEdit::MoveLineUp
        | SelectionEdit::MoveLineDown
        | SelectionEdit::JoinLines
        | SelectionEdit::JoinSelectedLines
        | SelectionEdit::SortLines(_)
        | SelectionEdit::ReverseLines
        | SelectionEdit::UniqueLines
        | SelectionEdit::ShuffleLines(_)
        | SelectionEdit::TrimTrailingWhitespace
        | SelectionEdit::WrapAtColumn(_)
        | SelectionEdit::ReflowParagraph(_)
        | SelectionEdit::TransposeChars
        | SelectionEdit::TransposeWords
        | SelectionEdit::ChangeCase(_)
        | SelectionEdit::SpacesToTabs { .. }
        | SelectionEdit::TabsToSpaces { .. }
        | SelectionEdit::ConvertLineEndings(_)
        | SelectionEdit::SurroundSelection { .. }
        | SelectionEdit::MarkdownToggleEmphasis(_)
        | SelectionEdit::MarkdownSetHeading(_)
        | SelectionEdit::MarkdownCycleHeading(_)
        | SelectionEdit::MarkdownPromoteSection
        | SelectionEdit::MarkdownDemoteSection
        | SelectionEdit::MarkdownMoveSectionUp
        | SelectionEdit::MarkdownMoveSectionDown
        | SelectionEdit::MarkdownToggleBullet
        | SelectionEdit::MarkdownToggleNumbered
        | SelectionEdit::MarkdownToggleCheckbox
        | SelectionEdit::MarkdownToggleTask
        | SelectionEdit::MarkdownCycleListMarker
        | SelectionEdit::MarkdownRenumberList
        | SelectionEdit::MarkdownInsertLink
        | SelectionEdit::MarkdownInsertImageRef
        | SelectionEdit::MarkdownInsertCodeFence
        | SelectionEdit::MarkdownWrapInBlockquote
        | SelectionEdit::MarkdownStripFormatting => true,
    }
}

/// Eased fade alpha for an active pulse, or `None` once the duration
/// elapsed.
#[must_use]
pub(crate) fn fade_alpha(pulse: EditPulse, now_ms: u64) -> Option<f32> {
    if pulse.duration_ms == 0 {
        return None;
    }
    let elapsed = now_ms.saturating_sub(pulse.started_ms);
    if elapsed >= u64::from(pulse.duration_ms) {
        return None;
    }
    let t = elapsed as f32 / pulse.duration_ms as f32;
    Some(1.0 - ease_out_cubic(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_text::{Position, SelectionKind};

    fn sel(anchor_line: u32, head_line: u32) -> Selection {
        Selection::new(
            Position::new(anchor_line, 0),
            Position::new(head_line, 0),
            SelectionKind::Caret,
        )
    }

    #[test]
    fn caret_moved_with_inserted_text_bounds_pre_to_post_head() {
        // Paste multi-line: pre caret at line 3, caret lands on line 8,
        // line count grew by 5 → range [3, 8].
        let (first, last) = edit_pulse_range(3, 10, sel(8, 8), 15);
        assert_eq!((first, last), (3, 8));
    }

    #[test]
    fn caret_stays_with_lines_inserted_below_extends_by_delta() {
        // Duplicate-line at line 5: post-caret stays at 5, line count
        // grows by 1 → range [5, 6].
        let (first, last) = edit_pulse_range(5, 20, sel(5, 5), 21);
        assert_eq!((first, last), (5, 6));
    }

    #[test]
    fn pure_delete_collapses_to_caret_line() {
        // DeleteWord-backwards on line 7: caret stays, line count drops.
        let (first, last) = edit_pulse_range(7, 30, sel(7, 7), 29);
        assert_eq!((first, last), (7, 7));
    }

    #[test]
    fn fade_starts_at_one() {
        let pulse = EditPulse {
            first_line: 0,
            last_line: 0,
            started_ms: 1000,
            duration_ms: EDIT_PULSE_DURATION_MS,
            kind: EditPulseKind::EditRegion,
        };
        let alpha = fade_alpha(pulse, 1000).expect("active");
        assert!((alpha - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fade_completes_at_duration() {
        let pulse = EditPulse {
            first_line: 0,
            last_line: 0,
            started_ms: 1000,
            duration_ms: EDIT_PULSE_DURATION_MS,
            kind: EditPulseKind::EditRegion,
        };
        assert!(fade_alpha(pulse, 1000 + u64::from(EDIT_PULSE_DURATION_MS)).is_none());
    }

    #[test]
    fn fade_midpoint_uses_ease_out() {
        let pulse = EditPulse {
            first_line: 0,
            last_line: 0,
            started_ms: 1000,
            duration_ms: 200,
            kind: EditPulseKind::EditRegion,
        };
        // t=0.5 → ease_out=0.875 → alpha = 1 - 0.875 = 0.125
        let alpha = fade_alpha(pulse, 1100).expect("active");
        assert!((alpha - 0.125).abs() < 1e-3);
    }

    #[test]
    fn duration_zero_evicts_immediately() {
        let pulse = EditPulse {
            first_line: 0,
            last_line: 0,
            started_ms: 1000,
            duration_ms: 0,
            kind: EditPulseKind::EditRegion,
        };
        assert!(fade_alpha(pulse, 1000).is_none());
    }

    #[test]
    fn selection_expand_uses_shorter_duration() {
        const _: () = assert!(SELECTION_EXPAND_PULSE_DURATION_MS < EDIT_PULSE_DURATION_MS);
    }

    #[test]
    fn reduced_motion_policy_schedules_no_pulse() {
        // Mirrors the assertion pattern used by jump_glow / status_motion
        // reduced-motion tests: the shared scheduler refuses to arm a
        // span when reduced motion is set.
        assert!(crate::motion::MotionPolicy::new(true)
            .schedule(0, EDIT_PULSE_DURATION_MS)
            .is_none());
    }

    #[test]
    fn pulse_range_clamps_inverted_inputs() {
        // post_head < pre_caret_line: should still produce a valid
        // [first, last] with first <= last.
        let (first, last) = edit_pulse_range(10, 20, sel(2, 2), 20);
        assert!(first <= last);
        assert_eq!((first, last), (2, 10));
    }

    #[test]
    fn structural_classifier_silences_continuous_typing() {
        assert!(!is_structural_edit(&SelectionEdit::InsertText("a".into())));
        assert!(!is_structural_edit(&SelectionEdit::DeleteBack));
        assert!(!is_structural_edit(&SelectionEdit::DeleteForward));
        assert!(!is_structural_edit(&SelectionEdit::DeleteWordBackward));
        assert!(!is_structural_edit(&SelectionEdit::InsertNewlineSmart));
        assert!(!is_structural_edit(&SelectionEdit::InsertPair {
            open: "(".into(),
            close: ")".into(),
        }));
    }

    #[test]
    fn structural_classifier_flags_rearrangements() {
        assert!(is_structural_edit(&SelectionEdit::DuplicateLine));
        assert!(is_structural_edit(&SelectionEdit::MoveLineUp));
        assert!(is_structural_edit(&SelectionEdit::MoveLineDown));
        assert!(is_structural_edit(&SelectionEdit::JoinLines));
        assert!(is_structural_edit(&SelectionEdit::ReverseLines));
        assert!(is_structural_edit(&SelectionEdit::SurroundSelection {
            open: "*".into(),
            close: "*".into(),
        }));
        assert!(is_structural_edit(&SelectionEdit::MarkdownPromoteSection));
        assert!(is_structural_edit(&SelectionEdit::MarkdownMoveSectionDown));
    }

    #[test]
    fn whole_buffer_normalisations_are_silent() {
        // Pulsing every line would defeat the "where did it land"
        // contract; these stay silent.
        assert!(!is_structural_edit(
            &SelectionEdit::TrimTrailingWhitespaceAll
        ));
        assert!(!is_structural_edit(&SelectionEdit::ConvertLineEndingsAll(
            continuity_core::LineEnding::Lf,
        )));
        assert!(!is_structural_edit(&SelectionEdit::TabsToSpacesAll {
            tab_width: 4,
        }));
    }
}
