//! Phase-16 spell-check service for [`crate::Window`].
//!
//! Thread ownership: UI thread. The current implementation runs the
//! Windows `ISpellChecker` synchronously on the UI thread because notes
//! are small and a full-buffer pass on a few thousand words returns in
//! well under a frame. A worker-pool variant for large buffers is a
//! Phase-17 perf-budget consideration.

use std::ops::Range;

use continuity_command::Error as CommandError; // alias: collides with crate::Error
use continuity_core::SelectionEdit;
use windows::core::HSTRING;
use windows::Win32::Foundation::{POINT, S_OK};
use windows::Win32::Globalization::{
    ISpellChecker, ISpellCheckerFactory, ISpellingError, SpellCheckerFactory,
    CORRECTIVE_ACTION_DELETE, CORRECTIVE_ACTION_GET_SUGGESTIONS, CORRECTIVE_ACTION_REPLACE,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, TrackPopupMenu, MF_ENABLED, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN,
};

use crate::Window;

/// One misspelled word in a buffer.
#[derive(Debug, Clone)]
pub struct SpellSpan {
    /// Byte range in the rope (absolute) where the misspelled word lives.
    pub range: Range<usize>,
    /// The misspelled word itself.
    pub word: String,
    /// Suggested replacements; empty when the spell-checker offered none.
    pub suggestions: Vec<String>,
}

/// Per-window spell-check state.
#[derive(Default)]
pub struct SpellState {
    enabled: bool,
    last_revision: Option<u64>,
    errors: Vec<SpellSpan>,
    service: Option<SpellService>,
    ignored: Vec<String>,
    /// `true` when [`Window::ensure_spell_fresh`] deferred a recheck
    /// because the buffer exceeded the synchronous paint-path cap.
    /// Drained by [`Window::tick_spell_recheck`] on the idle prewarm
    /// timer. The deferral keeps focus return / first paint
    /// responsive on large natural-text buffers where the Windows
    /// `ISpellChecker` COM path can stall the UI thread for seconds.
    pending_recheck: bool,
}

impl SpellState {
    /// `true` when spell-check is on for the active buffer.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Cached misspellings (newest revision).
    #[must_use]
    pub fn errors(&self) -> &[SpellSpan] {
        &self.errors
    }

    /// Find the misspelling whose byte range contains `byte` (the
    /// caret), if any.
    #[must_use]
    pub(crate) fn at_byte(&self, byte: usize) -> Option<&SpellSpan> {
        self.errors
            .iter()
            .find(|s| byte >= s.range.start && byte <= s.range.end)
    }
}

struct SpellService {
    checker: ISpellChecker,
}

impl SpellService {
    fn new() -> Result<Self, windows::core::Error> {
        unsafe {
            let factory: ISpellCheckerFactory =
                CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER)?;
            let lang = HSTRING::from("en-US");
            let supported = factory.IsSupported(&lang)?;
            if !supported.as_bool() {
                return Err(windows::core::Error::from_hresult(
                    windows::Win32::Foundation::E_FAIL,
                ));
            }
            let checker = factory.CreateSpellChecker(&lang)?;
            Ok(Self { checker })
        }
    }

    fn check(&self, text: &str, ignored: &[String]) -> Vec<SpellSpan> {
        let mut errors = Vec::new();
        let wide: Vec<u16> = text.encode_utf16().collect();
        if wide.is_empty() {
            return errors;
        }
        unsafe {
            let pcwstr = windows::core::PCWSTR(wide.as_ptr());
            let Ok(enumer) = self.checker.Check(pcwstr) else {
                return errors;
            };
            loop {
                let mut err: Option<ISpellingError> = None;
                let hr = enumer.Next(&mut err as *mut _);
                if hr != S_OK {
                    break;
                }
                let Some(err) = err else { break };
                let utf16_start = err.StartIndex().unwrap_or(0) as usize;
                let utf16_len = err.Length().unwrap_or(0) as usize;
                let action = err.CorrectiveAction().unwrap_or_default();
                let utf16_end = utf16_start + utf16_len;
                if utf16_end > wide.len() || utf16_len == 0 {
                    continue;
                }
                let prefix = String::from_utf16_lossy(&wide[..utf16_start]);
                let span = String::from_utf16_lossy(&wide[utf16_start..utf16_end]);
                let byte_start = prefix.len();
                let byte_end = byte_start + span.len();
                if ignored.iter().any(|w| w.eq_ignore_ascii_case(&span)) {
                    continue;
                }
                let mut suggestions = Vec::new();
                if action == CORRECTIVE_ACTION_REPLACE {
                    if let Ok(replace_pcwstr) = err.Replacement() {
                        if !replace_pcwstr.is_null() {
                            suggestions.push(replace_pcwstr.to_string().unwrap_or_default());
                            CoTaskMemFree(Some(replace_pcwstr.0 as *const _));
                        }
                    }
                }
                let want_suggestions = action == CORRECTIVE_ACTION_GET_SUGGESTIONS
                    || action == CORRECTIVE_ACTION_DELETE
                    || (action == CORRECTIVE_ACTION_REPLACE && suggestions.is_empty());
                if want_suggestions {
                    self.collect_suggestions(&span, &mut suggestions);
                }
                errors.push(SpellSpan {
                    range: byte_start..byte_end,
                    word: span,
                    suggestions,
                });
            }
        }
        errors
    }

    unsafe fn collect_suggestions(&self, word: &str, out: &mut Vec<String>) {
        let span_wide: Vec<u16> = word.encode_utf16().collect();
        let Ok(sugs) = self
            .checker
            .Suggest(windows::core::PCWSTR(span_wide.as_ptr()))
        else {
            return;
        };
        // Pull at most 8 suggestions to keep menu sizes bounded.
        for _ in 0..8 {
            let mut buf: [windows::core::PWSTR; 1] = [windows::core::PWSTR::null()];
            let mut fetched = 0u32;
            let hr = sugs.Next(&mut buf[..], Some(&mut fetched));
            if hr != S_OK || fetched == 0 {
                break;
            }
            let p = buf[0];
            if p.is_null() {
                continue;
            }
            let s = p.to_string().unwrap_or_default();
            if !s.is_empty() {
                out.push(s);
            }
            CoTaskMemFree(Some(p.0 as *const _));
        }
    }
}

impl Window {
    /// Borrow the per-window spell state — used by the painter to
    /// render squiggles and to surface the misspelling list.
    pub(crate) fn spell(&self) -> &SpellState {
        &self.spell_state
    }

    /// Synchronous cap above which spell freshness is deferred to an
    /// idle prewarm tick. The Windows `ISpellChecker` COM path scales
    /// with the number of misspellings and pays a per-error
    /// suggestion fetch; on a multi-thousand-line natural-text buffer
    /// it can stall the UI thread for seconds. Letting that fire on
    /// focus return / first paint of a new buffer was the primary
    /// "seconds before interactable" complaint at 6000 lines.
    pub(crate) const MAX_SYNC_SPELL_BYTES: usize = 64 * 1024;

    /// Make sure the cached spell errors track the active buffer
    /// revision.
    ///
    /// On the paint path this is best-effort: above
    /// [`Window::MAX_SYNC_SPELL_BYTES`] the call defers via
    /// `spell_state.pending_recheck` and returns immediately so the
    /// frame is not blocked. [`Window::tick_spell_recheck`] (fired
    /// from the idle display-prewarm timer) picks the work up later.
    pub(crate) fn ensure_spell_fresh(&mut self) {
        if !self.spell_state.enabled {
            return;
        }
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let rev = snap.rope_snapshot().revision().0;
        if self.spell_state.last_revision == Some(rev) {
            return;
        }
        let rope = snap.rope_snapshot().rope();
        if rope.len_bytes() > Self::MAX_SYNC_SPELL_BYTES {
            self.spell_state.pending_recheck = true;
            return;
        }
        let text: String = rope.to_string();
        let Some(service) = self.spell_state.service.as_ref() else {
            return;
        };
        let errors = service.check(&text, &self.spell_state.ignored);
        self.spell_state.errors = errors;
        self.spell_state.last_revision = Some(rev);
        self.spell_state.pending_recheck = false;
    }

    /// Run a deferred spell recheck off the immediate paint path.
    /// Called from [`Window::on_display_prewarm_tick`] when the
    /// window is idle and not inside the activation grace.
    ///
    /// **Large-buffer hard cap.** The Windows `ISpellChecker` COM
    /// path is fundamentally synchronous; even deferred to idle a
    /// 6000-line natural-text buffer can block the UI thread for
    /// seconds. Above [`Window::MAX_SYNC_SPELL_BYTES`] this method
    /// refuses to run, leaving `pending_recheck` set so a real
    /// worker-thread move (the proper follow-up) can pick it up
    /// later. Squiggles for large buffers therefore stay at their
    /// previous state until the user disables spell or the worker
    /// lands — strictly better than the seconds-scale UI stall the
    /// previous behaviour produced.
    pub(crate) fn tick_spell_recheck(&mut self) {
        if !self.spell_state.enabled || !self.spell_state.pending_recheck {
            return;
        }
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let rev = snap.rope_snapshot().revision().0;
        if self.spell_state.last_revision == Some(rev) {
            self.spell_state.pending_recheck = false;
            return;
        }
        let rope = snap.rope_snapshot().rope();
        if rope.len_bytes() > Self::MAX_SYNC_SPELL_BYTES {
            crate::paint_trace::log_event(
                "tick_spell_recheck",
                "skipped=over_sync_cap_pending_worker",
            );
            // Leave pending_recheck = true so a future explicit
            // refresh / worker-thread path can still drain it; but
            // never run it sync on the UI thread for big buffers.
            return;
        }
        let text: String = rope.to_string();
        let Some(service) = self.spell_state.service.as_ref() else {
            self.spell_state.pending_recheck = false;
            return;
        };
        let errors = service.check(&text, &self.spell_state.ignored);
        self.spell_state.errors = errors;
        self.spell_state.last_revision = Some(rev);
        self.spell_state.pending_recheck = false;
    }

    /// `spell.toggle` — flip on/off. On enable, lazily construct the
    /// service and run a check; on disable, drop cached errors.
    pub(crate) fn spell_toggle_impl(&mut self) -> Result<(), CommandError> {
        if self.spell_state.enabled {
            self.spell_state.enabled = false;
            self.spell_state.errors.clear();
            self.spell_state.last_revision = None;
            return Ok(());
        }
        if self.spell_state.service.is_none() {
            match SpellService::new() {
                Ok(svc) => self.spell_state.service = Some(svc),
                Err(e) => {
                    eprintln!("continuity-ui: spell service unavailable: {e}");
                    return Err(CommandError::UnsupportedContext("spell unavailable"));
                }
            }
        }
        self.spell_state.enabled = true;
        self.ensure_spell_fresh();
        Ok(())
    }

    /// `spell.replace_at_caret` — replace the misspelled word at the
    /// caret with `with`. One undo group.
    pub(crate) fn spell_replace_at_caret_impl(&mut self, with: &str) -> Result<(), CommandError> {
        let byte = self
            .primary_caret_byte_for_spell()
            .ok_or(CommandError::UnsupportedContext("no caret"))?;
        let Some(span) = self.spell_state.at_byte(byte).cloned() else {
            return Err(CommandError::UnsupportedContext(
                "no misspelling under caret",
            ));
        };
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return Err(CommandError::UnsupportedContext("no buffer"));
        };
        let rope = snap.rope_snapshot().rope();
        let start_pos = match continuity_text::Position::from_byte_offset(rope, span.range.start) {
            Ok(p) => p,
            Err(_) => return Err(CommandError::UnsupportedContext("range not on rope")),
        };
        let end_pos = match continuity_text::Position::from_byte_offset(rope, span.range.end) {
            Ok(p) => p,
            Err(_) => return Err(CommandError::UnsupportedContext("range not on rope")),
        };
        let sel = continuity_text::Selection::new(
            start_pos,
            end_pos,
            continuity_text::SelectionKind::Caret,
        );
        if let Err(e) = self.editor.set_selections(self.buffer_id, vec![sel]) {
            eprintln!("continuity-ui: spell replace set_selections failed: {e}");
            return Err(CommandError::UnsupportedContext("set_selections failed"));
        }
        self.dispatch_selection_edit(SelectionEdit::InsertText(with.to_string()))?;
        self.ensure_spell_fresh();
        Ok(())
    }

    /// `spell.add_to_dictionary` — add the word under the caret to the
    /// session ignore list and re-check.
    pub(crate) fn spell_add_to_dictionary_impl(&mut self) -> Result<(), CommandError> {
        let byte = self
            .primary_caret_byte_for_spell()
            .ok_or(CommandError::UnsupportedContext("no caret"))?;
        let Some(span) = self.spell_state.at_byte(byte).cloned() else {
            return Err(CommandError::UnsupportedContext(
                "no misspelling under caret",
            ));
        };
        if !self
            .spell_state
            .ignored
            .iter()
            .any(|w| w.eq_ignore_ascii_case(&span.word))
        {
            self.spell_state.ignored.push(span.word.clone());
        }
        self.spell_state.last_revision = None;
        self.ensure_spell_fresh();
        Ok(())
    }

    /// `spell.show_suggestions` — entry point for the caret-driven
    /// menu. Pops the popup at the caret pixel position.
    pub(crate) fn spell_show_suggestions_impl(&mut self) -> Result<(), CommandError> {
        let byte = self
            .primary_caret_byte_for_spell()
            .ok_or(CommandError::UnsupportedContext("no caret"))?;
        let Some(span) = self.spell_state.at_byte(byte).cloned() else {
            return Err(CommandError::UnsupportedContext(
                "no misspelling under caret",
            ));
        };
        self.show_spell_popup_at_caret(&span)
    }

    fn primary_caret_byte_for_spell(&self) -> Option<usize> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let sel = snap.selections().first().copied()?;
        let rope = snap.rope_snapshot().rope();
        let line = sel.head.line as usize;
        if line >= rope.len_lines() {
            return Some(rope.len_bytes());
        }
        Some(rope.line_to_byte(line) + sel.head.byte_in_line as usize)
    }

    fn show_spell_popup_at_caret(&mut self, span: &SpellSpan) -> Result<(), CommandError> {
        unsafe {
            let menu = match CreatePopupMenu() {
                Ok(m) => m,
                Err(_) => return Err(CommandError::UnsupportedContext("popup menu failed")),
            };
            if span.suggestions.is_empty() {
                let label = HSTRING::from("(no suggestions)");
                let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, &label);
            } else {
                for (i, s) in span.suggestions.iter().take(8).enumerate() {
                    let label = HSTRING::from(s.as_str());
                    let _ = AppendMenuW(menu, MF_STRING | MF_ENABLED, i + 1, &label);
                }
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            let add_label = HSTRING::from("Add to dictionary");
            let _ = AppendMenuW(menu, MF_STRING | MF_ENABLED, 100, &add_label);
            let pt = self
                .spell_caret_screen_pixel()
                .map_or(POINT { x: 0, y: 0 }, |(x, y)| POINT { x, y });
            let chosen = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD,
                pt.x,
                pt.y,
                Some(0),
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            let id = chosen.0 as usize;
            if id == 100 {
                self.spell_add_to_dictionary_impl()?;
            } else if id >= 1 && id <= span.suggestions.len() {
                let with = span.suggestions[id - 1].clone();
                self.spell_replace_at_caret_impl(&with)?;
            }
        }
        Ok(())
    }

    fn spell_caret_screen_pixel(&self) -> Option<(i32, i32)> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let sel = snap.selections().first().copied()?;
        let line = sel.head.line as f32;
        let line_height = crate::window::LINE_HEIGHT_DIP;
        let view_top_lines = self.view.scroll_y_dip / line_height;
        let y = ((line - view_top_lines + 1.0) * line_height) as i32;
        let mut pt = POINT { x: 0, y };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
        }
        Some((pt.x, pt.y))
    }

    /// Spell-suggestion branch of `WM_CONTEXTMENU`. Called by the umbrella
    /// dispatcher in [`crate::window_context_menu`] after a tab-strip /
    /// pane chrome check rules out the click. `_client_pt` is the
    /// converted client-coords point (kept for parity with the tab-strip
    /// branch; the spell popup itself anchors on the caret pixel).
    /// Returns `true` when a suggestion popup was opened.
    pub(crate) fn spell_on_context_menu(&mut self, _client_pt: (i32, i32)) -> bool {
        self.spell_show_suggestions_impl().is_ok()
    }
}
