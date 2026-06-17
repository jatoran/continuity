//! Status-bar left-click routing: pixel → segment kind →
//! per-kind action (Go-to-line picker, count-mode cycle, line-ending
//! toggle, chip-normalize). Chip clicks inspect the rope to decide
//! which anomalies need normalising and run each as its own undo
//! group, surfacing a non-blocking banner so the user knows the
//! action ran (Ctrl+Z reverts).
//!
//! Thread ownership: UI thread of one window.

use continuity_core::SelectionEdit;
use continuity_render::StatusBarSegmentKind;

use crate::window_file::FileBanner;
use crate::window_status_bar::hit_test::hit_test;
use crate::window_status_bar_line_ending::{detect_line_endings, LineEnding};
use crate::Window;

impl Window {
    /// Route a left click that landed inside the status-bar strip to
    /// the right action. Returns `true` when the click was consumed;
    /// `WM_LBUTTONDOWN` should skip the pane mouse handler in that
    /// case.
    pub(crate) fn try_status_bar_left_down(&mut self, x: i32, y: i32) -> bool {
        let Some(layout) = self.view_options.status_bar_layout.clone() else {
            return false;
        };
        let kind = match hit_test(&layout.bounds, layout.top, x as f32, y as f32) {
            Some(k) => k,
            None => return false,
        };
        self.dispatch_status_bar_click(kind);
        true
    }

    /// Apply the click action for one segment kind. Exposed as a
    /// distinct entry point so tests can drive the dispatch without
    /// faking pixel coords.
    pub(crate) fn dispatch_status_bar_click(&mut self, kind: StatusBarSegmentKind) {
        match kind {
            StatusBarSegmentKind::Position => {
                let _ = self.open_goto_line_via_context();
            }
            StatusBarSegmentKind::Chars
            | StatusBarSegmentKind::Words
            | StatusBarSegmentKind::Lines => {
                self.view_options.status_count_mode = self.view_options.status_count_mode.next();
            }
            StatusBarSegmentKind::LineEndings => {
                self.toggle_line_endings();
            }
            StatusBarSegmentKind::Chip => {
                // Click any chip → run the matching normalize(s). Each
                // is its own undo group; the user gets a feedback
                // banner with the undo affordance.
                self.handle_status_chip_click();
            }
            StatusBarSegmentKind::Encoding
            | StatusBarSegmentKind::NumericSum
            | StatusBarSegmentKind::Selection
            | StatusBarSegmentKind::Language
            | StatusBarSegmentKind::IdleStale
            | StatusBarSegmentKind::NoticeChip
            | StatusBarSegmentKind::PersistQueueChip => {
                // No-op for now — encoding picker, language picker and
                // selection/numeric-sum click handlers are queued for a
                // follow-up. The hover hint already telegraphs intent.
            }
        }
    }

    /// Dispatch the `editor.open_goto_line` command via the window's
    /// context. Used as the action for the position segment.
    fn open_goto_line_via_context(&mut self) -> bool {
        use continuity_command::Context;
        Context::open_goto_line(self).is_ok()
    }

    /// Line-ending toggle. Reads the current rope's dominant line
    /// ending and flips it. Persists transparently through the
    /// existing `apply_selection_edit` path so the change rides one
    /// undo group.
    fn toggle_line_endings(&mut self) {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let detected = detect_line_endings(snap.rope_snapshot().rope());
        let target = match detected {
            LineEnding::Crlf => continuity_core::LineEnding::Lf,
            _ => continuity_core::LineEnding::Crlf,
        };
        let _ = self
            .editor
            .apply_selection_edit(self.buffer_id, SelectionEdit::ConvertLineEndingsAll(target));
    }

    /// Normalise every line ending to LF. Called from the mixed-LE
    /// chip and exposed to tests.
    pub(crate) fn normalize_line_endings_to_lf(&mut self) {
        let _ = self.editor.apply_selection_edit(
            self.buffer_id,
            SelectionEdit::ConvertLineEndingsAll(continuity_core::LineEnding::Lf),
        );
    }

    /// Normalise every leading tab to four spaces. One undo group via
    /// the standard `apply_selection_edit` path.
    pub(crate) fn normalize_indent_tabs_to_spaces(&mut self) {
        let _ = self.editor.apply_selection_edit(
            self.buffer_id,
            SelectionEdit::TabsToSpacesAll { tab_width: 4 },
        );
    }

    /// Chip click. Inspects the rope to decide which anomalies need
    /// normalizing, runs them (each as its own undo group), and
    /// surfaces a non-blocking banner with the result so the user
    /// knows the action ran (Ctrl+Z reverts).
    fn handle_status_chip_click(&mut self) {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let rope = snap.rope_snapshot().rope().clone();
        let mut applied: Vec<&'static str> = Vec::new();
        if detect_line_endings(&rope).is_mixed() {
            self.normalize_line_endings_to_lf();
            applied.push("line endings → LF");
        }
        if crate::window_status_chips::detect_chips(&rope)
            .iter()
            .any(|c| c.text.contains("indent"))
        {
            self.normalize_indent_tabs_to_spaces();
            applied.push("indent → 4 spaces");
        }
        if !applied.is_empty() {
            let summary = format!("Normalized {} (Ctrl+Z to undo)", applied.join(" + "));
            // Confirmation-only notice: the action already ran and Ctrl+Z
            // reverts it. Auto-dismiss so the chrome doesn't linger past
            // the change it announces.
            let now = self.now_ms();
            self.file_banner = Some(FileBanner::transient(summary, now));
            self.start_file_io_poll(self.hwnd);
        }
    }
}
