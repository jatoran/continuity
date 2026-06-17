//! Phase-16 clipboard, paste-history, and IME implementations on
//! [`crate::Window`].
//!
//! Thread ownership: UI thread (HWND owner). All clipboard / IME calls
//! are short and synchronous; per spec §12 the editor blocks on these
//! while accepting the I/O cost (clipboard reads and IME composition
//! events both originate on the UI thread already).

use std::collections::VecDeque;

use continuity_command::Error as CommandError; // alias: collides with crate::Error
use continuity_core::SelectionEdit;
use continuity_win::clipboard;

use crate::Window;

/// Default depth of the in-memory paste history ring buffer.
///
/// Spec §12: "Paste-from-history (last N clipboard entries; ring buffer
/// in memory only, not persisted)". 16 covers a comfortable session
/// without runaway memory.
pub(crate) const PASTE_HISTORY_CAPACITY: usize = 16;

/// In-memory ring buffer of recently copied/cut snippets. Newest entry
/// at index 0. Not persisted (spec §12).
#[derive(Debug, Default, Clone)]
pub struct PasteHistory {
    entries: VecDeque<String>,
}

impl PasteHistory {
    /// Build an empty history.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(PASTE_HISTORY_CAPACITY),
        }
    }

    /// Push `text` to the front. Drops the oldest entry once the ring is
    /// full. Skips empty strings and immediate duplicates.
    pub fn push(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if self.entries.front().map(String::as_str) == Some(text.as_str()) {
            return;
        }
        self.entries.push_front(text);
        while self.entries.len() > PASTE_HISTORY_CAPACITY {
            self.entries.pop_back();
        }
    }

    /// Number of entries currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` iff the ring is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Borrow the entry at `index` (0 = newest).
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    /// Iterator over entries newest-first.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }
}

impl Window {
    /// Read the clipboard payload for the current selections: every
    /// non-collapsed selection's source text in document order, joined
    /// with newlines. With a single selection this is exactly that
    /// selection's text; with Ctrl-drag multi-region highlights the
    /// clipboard carries all of them — matching the cut/delete path,
    /// which already removes every highlighted range. `None` when no
    /// selection covers any text.
    fn selections_clipboard_source(&self) -> Option<String> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let rope = snap.rope_snapshot().rope();
        let mut ranges: Vec<(usize, usize)> = snap
            .selections()
            .iter()
            .filter(|sel| !sel.is_collapsed())
            .filter_map(|sel| {
                let range = sel.ordered_range();
                let start = range.start.to_byte_offset(rope).ok()?;
                let end = range.end.to_byte_offset(rope).ok()?;
                (start < end).then_some((start, end))
            })
            .collect();
        if ranges.is_empty() {
            return None;
        }
        ranges.sort_unstable();
        let pieces: Vec<String> = ranges
            .iter()
            .map(|(start, end)| rope.byte_slice(*start..*end).to_string())
            .collect();
        Some(pieces.join("\n"))
    }

    /// `editor.copy` — copy every selection's source to the OS clipboard
    /// (newline-joined, document order) and record it in the
    /// paste-history ring.
    pub(crate) fn copy_selection_impl(&mut self) -> Result<(), CommandError> {
        let Some(text) = self.selections_clipboard_source() else {
            return Err(CommandError::UnsupportedContext("no selection"));
        };
        if let Err(e) = clipboard::write_text(self.hwnd, &text) {
            eprintln!("continuity-ui: clipboard write failed: {e}");
            return Err(CommandError::UnsupportedContext("clipboard write failed"));
        }
        self.paste_history.push(text);
        Ok(())
    }

    /// `editor.cut` — copy every selection's source, then delete all of
    /// them.
    pub(crate) fn cut_selection_impl(&mut self) -> Result<(), CommandError> {
        let Some(text) = self.selections_clipboard_source() else {
            return Err(CommandError::UnsupportedContext("no selection"));
        };
        if let Err(e) = clipboard::write_text(self.hwnd, &text) {
            eprintln!("continuity-ui: clipboard write failed: {e}");
            return Err(CommandError::UnsupportedContext("clipboard write failed"));
        }
        self.paste_history.push(text);
        self.dispatch_selection_edit(SelectionEdit::InsertText(String::new()))
    }

    /// `editor.paste` — insert the clipboard at every caret, preferring
    /// richer formats in order.
    ///
    /// Phase F5: probe `CF_DIB` / `CF_DIBV5` / `CF_HDROP` *first* — a
    /// clipboard image lands as `![](images/<hash>.<ext>)` at the
    /// caret (single undo group, hash-deduped in the shared store).
    ///
    /// Item 16: when the clipboard advertises `"HTML Format"`, convert the
    /// fragment to markdown and insert that (one `SelectionEdit::InsertText`)
    /// ahead of the plain-text fallthrough. Falls back to plain text when
    /// no HTML is present or the conversion yields nothing.
    ///
    /// Phase B13: when the clipboard is a single bare URL, transform the
    /// paste into a markdown link / image ref / autolink. Plain text falls
    /// through.
    ///
    /// Item 30: when the resolved text (plain or HTML-converted) is a GFM
    /// pipe table and the caret is not at column 0 of a blank line, a
    /// leading newline (and a synthesized delimiter row when missing) is
    /// prefixed so the table begins its own block and reparses as a
    /// `PipeTable`.
    pub(crate) fn paste_clipboard_impl(&mut self) -> Result<(), CommandError> {
        // F5 — image branches take precedence over the text path.
        // `try_paste_clipboard_image` returns Ok(true) when it
        // consumed an image; we then bypass the text fallthrough so
        // a screenshot doesn't ALSO paste the legacy "[Image]"-style
        // text alternate format some apps populate alongside CF_DIB.
        if let Ok(true) = self.try_paste_clipboard_image() {
            return Ok(());
        }

        // Item 16 — rich HTML paste takes precedence over plain text.
        // Ctrl+Shift+V (plain paste) does NOT reach here; it routes
        // through `insert_plain_clipboard_text`.
        if clipboard::has_html() {
            if let Ok(Some(fragment)) = clipboard::read_html(self.hwnd) {
                if let Some(markdown) = crate::html_to_markdown::html_to_markdown(&fragment) {
                    let normalized = normalize_line_endings(&markdown);
                    return self.insert_paste_text(normalized);
                }
            }
            // No usable HTML fragment — fall through to plain text.
        }

        let text_opt = clipboard::read_text(self.hwnd).map_err(|e| {
            eprintln!("continuity-ui: clipboard read failed: {e}");
            CommandError::UnsupportedContext("clipboard read failed")
        })?;
        let Some(text) = text_opt else {
            return Err(CommandError::UnsupportedContext("clipboard has no text"));
        };
        let normalized = normalize_line_endings(&text);
        let has_selection = self
            .current_snapshot()
            .map(|s| s.selections.iter().any(|sel| !sel.is_collapsed()))
            .unwrap_or(false);
        if let Some(op) = crate::smart_paste::smart_paste_transform(&normalized, has_selection) {
            // α.1 — capture pre-state for the edit-region pulse.
            let pre = self.editor.snapshot(self.buffer_id);
            let pre_caret_line = pre
                .as_ref()
                .and_then(|s| s.selections().first().map(|sel| sel.head.line));
            let pre_line_count = pre.as_ref().map(|s| s.rope_snapshot().rope().len_lines());
            let result = match op {
                crate::smart_paste::SmartPasteOp::WrapAsLink { open, close } => {
                    self.dispatch_selection_edit(SelectionEdit::SurroundSelection { open, close })
                }
                crate::smart_paste::SmartPasteOp::InsertImageRef(s)
                | crate::smart_paste::SmartPasteOp::InsertBareUrl(s) => {
                    self.dispatch_selection_edit(SelectionEdit::InsertText(s))
                }
            };
            if result.is_ok() {
                if let (Some(line), Some(lines)) = (pre_caret_line, pre_line_count) {
                    self.pulse_edit_region_after_dispatch(line, lines);
                }
            }
            return result;
        }
        self.insert_paste_text(normalized)
    }

    /// Insert `text` at every caret as a single `SelectionEdit::InsertText`
    /// undo group, applying item-30 GFM-table block normalization and
    /// arming the edit-region pulse.
    ///
    /// `text` must already be line-ending-normalized. Shared by the HTML
    /// and plain-text branches of [`Self::paste_clipboard_impl`].
    fn insert_paste_text(&mut self, text: String) -> Result<(), CommandError> {
        let text = self.normalize_table_paste(text);
        // α.1 — paste flows through `SelectionEdit::InsertText` which is
        // intentionally NOT in the structural-edit allowlist (so single-
        // char typing doesn't pulse). Capture pre-state and arm the pulse
        // after the apply lands.
        let pre = self.editor.snapshot(self.buffer_id);
        let pre_caret_line = pre
            .as_ref()
            .and_then(|s| s.selections().first().map(|sel| sel.head.line));
        let pre_line_count = pre.as_ref().map(|s| s.rope_snapshot().rope().len_lines());
        let result = self.dispatch_selection_edit(SelectionEdit::InsertText(text));
        if result.is_ok() {
            if let (Some(line), Some(lines)) = (pre_caret_line, pre_line_count) {
                self.pulse_edit_region_after_dispatch(line, lines);
            }
        }
        result
    }

    /// Item 30 — apply GFM-table block normalization to a paste payload.
    ///
    /// When `text` is (or could become) a pipe table, prefix a newline so
    /// the table starts its own block unless the primary caret is already
    /// at column 0 of a blank line, and synthesize a missing delimiter
    /// row. Non-table pastes are returned unchanged.
    fn normalize_table_paste(&self, text: String) -> String {
        use crate::window_markdown_table_ops::paste_normalize::{
            is_gfm_table_text, is_pipe_table_missing_delimiter, normalize_pasted_table,
        };
        if !is_gfm_table_text(&text) && !is_pipe_table_missing_delimiter(&text) {
            return text;
        }
        let at_blank_line_start = self.primary_caret_at_blank_line_start();
        normalize_pasted_table(&text, at_blank_line_start)
    }

    /// `true` when the primary caret sits at column 0 of a blank line (so a
    /// pasted block already begins its own block and needs no leading
    /// newline). A caret at the very start of an empty buffer also counts.
    fn primary_caret_at_blank_line_start(&self) -> bool {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return true;
        };
        let Some(sel) = snap.selections().first() else {
            return true;
        };
        if sel.head.byte_in_line != 0 {
            return false;
        }
        let rope = snap.rope_snapshot().rope();
        let line_idx = sel.head.line as usize;
        if line_idx >= rope.len_lines() {
            return true;
        }
        rope.line(line_idx)
            .to_string()
            .trim_end_matches(['\n', '\r'])
            .is_empty()
    }

    /// Insert the clipboard's `CF_UNICODETEXT` payload verbatim at every
    /// caret, skipping both the clipboard-image branch and the
    /// smart-paste URL transform that [`Self::paste_clipboard_impl`] runs.
    ///
    /// This is the literal "plain text" path: the only transformation
    /// applied is [`normalize_line_endings`] (so stray `\r` glyphs never
    /// reach the rope). When the clipboard holds an image but no text the
    /// call is a no-op — plain paste never imports images.
    ///
    /// Thread ownership: UI thread (HWND owner). The mutation lands as a
    /// single [`SelectionEdit::InsertText`] via
    /// [`Self::dispatch_selection_edit`] (one undo group), then arms the
    /// edit-region pulse exactly as the paste path does.
    fn insert_plain_clipboard_text(&mut self) -> Result<(), CommandError> {
        let text_opt = clipboard::read_text(self.hwnd).map_err(|e| {
            eprintln!("continuity-ui: clipboard read failed: {e}");
            CommandError::UnsupportedContext("clipboard read failed")
        })?;
        let Some(text) = text_opt else {
            return Err(CommandError::UnsupportedContext("clipboard has no text"));
        };
        let normalized = normalize_line_endings(&text);
        // Same pre-state capture as `paste_clipboard_impl`: `InsertText`
        // is not in the structural-edit allowlist, so we arm the pulse
        // manually after the apply lands.
        let pre = self.editor.snapshot(self.buffer_id);
        let pre_caret_line = pre
            .as_ref()
            .and_then(|s| s.selections().first().map(|sel| sel.head.line));
        let pre_line_count = pre.as_ref().map(|s| s.rope_snapshot().rope().len_lines());
        let result = self.dispatch_selection_edit(SelectionEdit::InsertText(normalized));
        if result.is_ok() {
            if let (Some(line), Some(lines)) = (pre_caret_line, pre_line_count) {
                self.pulse_edit_region_after_dispatch(line, lines);
            }
        }
        result
    }

    /// `editor.paste_as_plain_text` — paste the clipboard's
    /// `CF_UNICODETEXT` payload raw (Ctrl+Shift+V): skips the image and
    /// smart-paste branches that `editor.paste` (Ctrl+V) runs, so a
    /// clipboard image or single-URL payload is inserted as literal text
    /// (or, for image-only clipboards, nothing). Surfaced as a
    /// discoverable command + Ctrl+Shift+V binding per spec §12.
    pub(crate) fn paste_as_plain_text_impl(&mut self) -> Result<(), CommandError> {
        self.insert_plain_clipboard_text()
    }

    /// `editor.paste_from_history` — paste history entry at `index`
    /// (default = 0, newest).
    pub(crate) fn paste_from_history_impl(
        &mut self,
        index: Option<usize>,
    ) -> Result<(), CommandError> {
        let idx = index.unwrap_or(0);
        let Some(text) = self.paste_history.get(idx).map(str::to_owned) else {
            return Err(CommandError::UnsupportedContext("paste history empty"));
        };
        let pre = self.editor.snapshot(self.buffer_id);
        let pre_caret_line = pre
            .as_ref()
            .and_then(|s| s.selections().first().map(|sel| sel.head.line));
        let pre_line_count = pre.as_ref().map(|s| s.rope_snapshot().rope().len_lines());
        let result =
            self.dispatch_selection_edit(SelectionEdit::InsertText(normalize_line_endings(&text)));
        if result.is_ok() {
            if let (Some(line), Some(lines)) = (pre_caret_line, pre_line_count) {
                self.pulse_edit_region_after_dispatch(line, lines);
            }
        }
        result
    }

    /// δ.1 — `editor.copy_line`: copy the caret's current line to the
    /// OS clipboard and record it in the paste-history ring. The copy
    /// includes the trailing `\n` (or "" for the last line of a file
    /// with no trailing newline) so a subsequent paste reinserts a
    /// whole-line snippet rather than a column run.
    pub(crate) fn copy_caret_line_impl(&mut self) -> Result<(), CommandError> {
        let Some(text) = self.primary_caret_line_source() else {
            return Err(CommandError::UnsupportedContext("no buffer for copy_line"));
        };
        if text.is_empty() {
            return Err(CommandError::UnsupportedContext("line is empty"));
        }
        if let Err(e) = clipboard::write_text(self.hwnd, &text) {
            eprintln!("continuity-ui: clipboard write failed: {e}");
            return Err(CommandError::UnsupportedContext("clipboard write failed"));
        }
        self.paste_history.push(text);
        Ok(())
    }

    /// Read the source text of the caret's current line, including its
    /// trailing newline if present.
    fn primary_caret_line_source(&self) -> Option<String> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let sel = snap.selections().first().copied()?;
        let rope = snap.rope_snapshot().rope();
        let line_idx = sel.head.line as usize;
        if line_idx >= rope.len_lines() {
            return None;
        }
        Some(rope.line(line_idx).to_string())
    }
}

/// Replace every line-ending variant with `\n`.
///
/// The rope and the renderer agree that only `\n` ends a line. Windows
/// clipboards typically deliver `\r\n`; pasting that verbatim leaves a
/// stray `\r` in the middle of the rope's logical line, which DirectWrite
/// then renders as a carriage return — every following glyph overdraws
/// the first character. Mac-style `\r`-only line endings have the same
/// failure mode without the trailing `\n`. Normalizing both at the
/// boundary keeps the rest of the editor a single-line-break world.
fn normalize_line_endings(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                // Collapse `\r\n` into a single `\n`; standalone `\r`
                // also becomes `\n` so legacy Mac files render correctly.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod normalize_tests {
    use super::normalize_line_endings;

    #[test]
    fn crlf_becomes_lf() {
        assert_eq!(normalize_line_endings("a\r\nb"), "a\nb");
        assert_eq!(normalize_line_endings("a\r\nb\r\nc"), "a\nb\nc");
    }

    #[test]
    fn lone_cr_becomes_lf() {
        assert_eq!(normalize_line_endings("a\rb"), "a\nb");
    }

    #[test]
    fn unaffected_when_no_cr() {
        assert_eq!(normalize_line_endings("a\nb"), "a\nb");
        assert_eq!(normalize_line_endings("plain"), "plain");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_skips_empty_and_dedups_front() {
        let mut h = PasteHistory::new();
        h.push("foo".into());
        h.push("foo".into());
        h.push("bar".into());
        h.push(String::new());
        assert_eq!(h.len(), 2);
        assert_eq!(h.get(0), Some("bar"));
        assert_eq!(h.get(1), Some("foo"));
    }

    #[test]
    fn history_caps_at_capacity() {
        let mut h = PasteHistory::new();
        for i in 0..(PASTE_HISTORY_CAPACITY + 4) {
            h.push(format!("{i}"));
        }
        assert_eq!(h.len(), PASTE_HISTORY_CAPACITY);
        assert_eq!(
            h.get(0),
            Some(format!("{}", PASTE_HISTORY_CAPACITY + 3).as_str())
        );
    }
}
