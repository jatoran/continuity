//! Phase-16 IME (Input Method Editor) glue for [`crate::Window`].
//!
//! Thread ownership: UI thread (HIMC is a per-window resource).
//!
//! Wire: `WM_IME_STARTCOMPOSITION` clears the in-progress composition
//! string; `WM_IME_COMPOSITION` reads `GCS_COMPSTR` / `GCS_CURSORPOS`
//! into the composition state and the IME caret rect, and on
//! `GCS_RESULTSTR` commits the result through the normal text-input
//! path; `WM_IME_ENDCOMPOSITION` clears the composition state. While
//! `composing` is true, `WM_CHAR` is suppressed by the window proc to
//! avoid double-insertion of the result string.

use continuity_win::ime::{self, CompositionState};
use windows::Win32::Foundation::HWND;

use crate::Window;

/// Window-level state tracking the active IME composition (if any).
#[derive(Debug, Default, Clone)]
pub struct ImeState {
    /// `true` between WM_IME_STARTCOMPOSITION and WM_IME_ENDCOMPOSITION.
    pub composing: bool,
    /// Most recent in-progress composition string (UTF-8).
    pub comp: String,
    /// Caret offset within `comp`, in UTF-8 bytes.
    pub caret_byte: usize,
}

impl ImeState {
    /// Reset to "not composing".
    pub fn clear(&mut self) {
        self.composing = false;
        self.comp.clear();
        self.caret_byte = 0;
    }
}

impl Window {
    /// Handle `WM_IME_STARTCOMPOSITION`. Mark the window as composing so
    /// `WM_CHAR` can be suppressed until the composition ends.
    pub(crate) fn on_ime_start_composition(&mut self) {
        self.ime_state.composing = true;
        self.ime_state.comp.clear();
        self.ime_state.caret_byte = 0;
    }

    /// Handle `WM_IME_COMPOSITION`. Refreshes `ime_state.comp` /
    /// `caret_byte` from the IME and, when `GCS_RESULTSTR` fires, commits
    /// the result through the editor core.
    pub(crate) fn on_ime_composition(&mut self, hwnd: HWND, lparam: isize) -> bool {
        let Some(state) = ime::read_composition(hwnd, lparam) else {
            return false;
        };
        self.update_ime_visuals(hwnd);
        let CompositionState {
            comp,
            caret_byte,
            result,
        } = state;
        // In-progress composition: just snapshot for paint.
        self.ime_state.comp = comp;
        self.ime_state.caret_byte = caret_byte;
        if !result.is_empty() {
            // Committed text — route through the same path as a normal
            // text insert so undo/persistence/decoration all observe it
            // identically.
            self.note_input_now();
            let edit = continuity_core::SelectionEdit::InsertText(result);
            if let Err(e) = self.dispatch_selection_edit(edit) {
                eprintln!("continuity-ui: IME commit failed: {e}");
            }
            true
        } else {
            true
        }
    }

    /// Handle `WM_IME_ENDCOMPOSITION`. Clear in-progress state.
    pub(crate) fn on_ime_end_composition(&mut self) {
        self.ime_state.clear();
    }

    /// Move the IME composition window to track the primary caret.
    fn update_ime_visuals(&mut self, hwnd: HWND) {
        if let Some((x, y)) = self.primary_caret_pixel() {
            ime::set_composition_position(hwnd, x, y);
        }
    }

    /// Pixel-space client position of the primary caret bottom edge.
    /// `None` when the caret is offscreen or no buffer is mapped.
    fn primary_caret_pixel(&self) -> Option<(i32, i32)> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let sel = snap.selections().first().copied()?;
        let line = sel.head.line as f32;
        let line_height = self.effective_line_height();
        let view_top_lines = self.view.scroll_y_dip / line_height;
        let y = ((line - view_top_lines) * line_height) as i32;
        Some((0, y.max(0)))
    }
}
