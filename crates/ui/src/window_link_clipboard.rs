//! Phase-10 link / clipboard / source↔display motion-skip implementations
//! for [`crate::Window`].
//!
//! **Thread ownership**: UI thread; calls into Win32 shell + clipboard APIs
//! synchronously. Per spec §13 ("never on UI thread") this is a Phase-15
//! follow-up — for Phase 10 the operations are short (open one URL, copy
//! one selection) and the shell call returns immediately.

use continuity_command::Error as CommandError; // alias: collides with crate::Error
use continuity_decorate::{Decorations, InlineKind, MarkerKind};
use windows::core::HSTRING;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::Window;

impl Window {
    /// Compute the absolute byte position of the primary caret.
    fn primary_caret_byte(&self) -> Option<usize> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let sel = snap.selections().first().copied()?;
        let rope = snap.rope_snapshot().rope();
        let line = sel.head.line as usize;
        if line >= rope.len_lines() {
            return Some(rope.len_bytes());
        }
        Some(rope.line_to_byte(line) + sel.head.byte_in_line as usize)
    }

    fn current_decoration_url_at_caret(&self) -> Option<(usize, usize)> {
        let id = self.buffer_id.as_uuid().as_u128();
        let dec: &Decorations = self.decoration_cache.get(id)?;
        let byte = self.primary_caret_byte()?;
        let r = dec.url_at(byte)?;
        Some((r.start, r.end))
    }

    /// Open the link at the primary caret, if any. Uses `ShellExecuteW`
    /// with the `"open"` verb so the OS picks the user's default browser.
    pub(crate) fn open_link_at_caret_impl(&mut self) -> Result<(), CommandError> {
        let Some((start, end)) = self.current_decoration_url_at_caret() else {
            return Err(CommandError::UnsupportedContext("no link under caret"));
        };
        let snap = self
            .editor
            .snapshot(self.buffer_id)
            .ok_or(CommandError::UnsupportedContext("no buffer"))?;
        let url: String = snap
            .rope_snapshot()
            .rope()
            .byte_slice(start..end)
            .to_string();
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err(CommandError::UnsupportedContext("empty url"));
        }
        let verb = HSTRING::from("open");
        let target = HSTRING::from(trimmed);
        unsafe {
            ShellExecuteW(None, &verb, &target, None, None, SW_SHOWNORMAL);
        }
        Ok(())
    }

    /// Copy a UTF-16 string to the Win32 clipboard as `CF_UNICODETEXT`.
    pub(crate) fn put_clipboard_text(&self, text: &str) -> Result<(), CommandError> {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0); // NUL terminator required for CF_UNICODETEXT
        let bytes = wide.len() * std::mem::size_of::<u16>();
        unsafe {
            if OpenClipboard(Some(self.hwnd())).is_err() {
                return Err(CommandError::UnsupportedContext("OpenClipboard failed"));
            }
            let _ = EmptyClipboard();
            let h = match GlobalAlloc(GMEM_MOVEABLE, bytes) {
                Ok(h) => h,
                Err(_) => {
                    let _ = CloseClipboard();
                    return Err(CommandError::UnsupportedContext("GlobalAlloc failed"));
                }
            };
            let dst = GlobalLock(h) as *mut u16;
            if dst.is_null() {
                let _ = CloseClipboard();
                return Err(CommandError::UnsupportedContext("GlobalLock failed"));
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            let _ = GlobalUnlock(h);
            // Per Win32 docs the system takes ownership of `h` after
            // SetClipboardData succeeds; on failure we'd need to free it,
            // but GlobalAlloc(MOVEABLE) handles cleanup on app exit.
            let _ = SetClipboardData(
                CF_UNICODETEXT.0.into(),
                Some(windows::Win32::Foundation::HANDLE(h.0)),
            );
            let _ = CloseClipboard();
        }
        Ok(())
    }

    /// Compute rendered (decoration-flattened) text for a byte range.
    /// Skips structural-marker bytes (heading hashes, list markers, fence
    /// ticks, blockquote carets, table pipes, thematic breaks) and emphasis
    /// delimiters; preserves alphanumeric text and inline content.
    fn flatten_decorated_range(
        decorations: &Decorations,
        source: &str,
        start: usize,
        end: usize,
    ) -> String {
        let mut out = String::with_capacity(end - start);
        let mut byte = start;
        while byte < end {
            // Is `byte` inside any marker span?
            let in_marker = decorations.inlines.iter().any(|s| {
                s.range.contains(byte)
                    && matches!(
                        s.kind,
                        InlineKind::Marker(
                            MarkerKind::HeadingHash
                                | MarkerKind::ListMarker
                                | MarkerKind::FenceTick
                                | MarkerKind::BlockquoteCaret
                                | MarkerKind::TablePipe
                                | MarkerKind::ThematicBreak
                                | MarkerKind::EmphasisDelim
                                | MarkerKind::StrikeDelim
                                | MarkerKind::CodeDelim,
                        )
                    )
            });
            if let Some(ch) = source[byte..].chars().next() {
                if !in_marker {
                    out.push(ch);
                }
                byte += ch.len_utf8();
            } else {
                break;
            }
        }
        out
    }

    /// Copy the primary selection's decoration-flattened text to the
    /// clipboard.
    pub(crate) fn copy_rendered_text_impl(&mut self) -> Result<(), CommandError> {
        let snap = self
            .editor
            .snapshot(self.buffer_id)
            .ok_or(CommandError::UnsupportedContext("no buffer"))?;
        let sel = snap
            .selections()
            .first()
            .copied()
            .ok_or(CommandError::UnsupportedContext("no selection"))?;
        let rope = snap.rope_snapshot().rope();
        let id = self.buffer_id.as_uuid().as_u128();
        let dec = self.decoration_cache.get(id);
        let source: String = rope.to_string();
        let range = sel.ordered_range();
        let start = range.start.to_byte_offset(rope).unwrap_or(0);
        let end = range.end.to_byte_offset(rope).unwrap_or(source.len());
        let text = if start == end {
            // No selection — copy the whole buffer flattened.
            if let Some(dec) = dec {
                Self::flatten_decorated_range(dec, &source, 0, source.len())
            } else {
                source.clone()
            }
        } else if let Some(dec) = dec {
            Self::flatten_decorated_range(dec, &source, start, end)
        } else {
            source[start..end].to_string()
        };
        self.put_clipboard_text(&text)
    }

    /// Copy the primary selection's source markdown to the clipboard.
    pub(crate) fn copy_source_text_impl(&mut self) -> Result<(), CommandError> {
        let snap = self
            .editor
            .snapshot(self.buffer_id)
            .ok_or(CommandError::UnsupportedContext("no buffer"))?;
        let sel = snap
            .selections()
            .first()
            .copied()
            .ok_or(CommandError::UnsupportedContext("no selection"))?;
        let rope = snap.rope_snapshot().rope();
        let range = sel.ordered_range();
        let start = range.start.to_byte_offset(rope).unwrap_or(0);
        let end = range.end.to_byte_offset(rope).unwrap_or(rope.len_bytes());
        let text: String = if start == end {
            rope.to_string()
        } else {
            rope.byte_slice(start..end).to_string()
        };
        self.put_clipboard_text(&text)
    }

    /// Source↔display motion-skip: after a basic `move_char` the head may
    /// have landed inside a hidden structural-marker range (heading
    /// hashes, list markers, fence ticks, blockquote carets, table pipes,
    /// thematic breaks). Advance the head one character at a time in the
    /// direction of travel until it's no longer inside a structural
    /// marker. Per spec §9, emphasis/strike/code delimiters are *not*
    /// auto-skipped — those take an extra arrow press to cross.
    ///
    /// `delta_sign` is `+1` for forward motion, `-1` for backward.
    pub(crate) fn apply_structural_skip(&mut self, delta_sign: i32) {
        if delta_sign == 0 {
            return;
        }
        let id = self.buffer_id.as_uuid().as_u128();
        let dec = match self.decoration_cache.get(id) {
            Some(d) => d.clone(),
            None => return,
        };
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return,
        };
        let rope = snap.rope_snapshot().rope();
        let mut sels: Vec<continuity_text::Selection> = snap.selections().to_vec();
        let mut changed = false;
        for sel in &mut sels {
            let line = sel.head.line as usize;
            if line >= rope.len_lines() {
                continue;
            }
            let line_start = rope.line_to_byte(line);
            // Skip while structural up to the end of the current source
            // line. The line-length bound covers the worst case where a
            // single arrow press lands the caret on a long hidden run
            // (e.g. a `=SUM(A1:A20)` formula payload inside a table
            // cell that the display map hides). The previous fixed
            // `max_iters = 8` cap was sized for short pipe-only runs
            // and trapped motion inside long structural payloads once
            // tables started rendering unconditionally.
            let line_end = if line + 1 < rope.len_lines() {
                rope.line_to_byte(line + 1)
            } else {
                rope.len_bytes()
            };
            let line_len_bytes = line_end.saturating_sub(line_start);
            let mut byte = line_start + sel.head.byte_in_line as usize;
            let mut iters = 0usize;
            let total_chars = rope.len_chars();
            while iters < line_len_bytes && dec.is_structural_marker_byte(byte) {
                let char_idx = match rope.try_byte_to_char(byte) {
                    Ok(c) => c,
                    Err(_) => break,
                };
                let new_char_idx = if delta_sign > 0 {
                    if char_idx >= total_chars {
                        break;
                    }
                    char_idx + 1
                } else {
                    if char_idx == 0 {
                        break;
                    }
                    char_idx - 1
                };
                byte = rope.char_to_byte(new_char_idx);
                if byte < line_start || byte >= line_end {
                    // Crossed off the original source line — the
                    // structural run ended at the line edge. Step back
                    // so the caret stays on this line at its boundary.
                    let snap_to = if delta_sign > 0 { line_end } else { line_start };
                    byte = snap_to;
                    break;
                }
                iters += 1;
            }
            // Convert back to (line, byte_in_line).
            if let Ok(new_pos) = continuity_text::Position::from_byte_offset(rope, byte) {
                if new_pos != sel.head {
                    sel.head = new_pos;
                    if sel.is_collapsed() {
                        sel.anchor = new_pos;
                    }
                    changed = true;
                }
            }
        }
        if changed {
            let _ = self.editor.set_selections(self.buffer_id, sels);
        }
    }

    /// Render the buffer to HTML via pulldown-cmark and copy it to clipboard.
    pub(crate) fn copy_as_html_impl(&mut self) -> Result<(), CommandError> {
        let snap = self
            .editor
            .snapshot(self.buffer_id)
            .ok_or(CommandError::UnsupportedContext("no buffer"))?;
        let source: String = snap.rope_snapshot().rope().to_string();
        let parser = pulldown_cmark::Parser::new(&source);
        let mut html = String::with_capacity(source.len());
        pulldown_cmark::html::push_html(&mut html, parser);
        self.put_clipboard_text(&html)
    }
}
