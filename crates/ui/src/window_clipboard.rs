//! Clipboard mutation orchestration on [`crate::Window`].
//!
//! Thread ownership: UI thread (HWND owner). Native format I/O is isolated in
//! `continuity_win::clipboard` and `continuity_win::clipboard_image`; this
//! module maps returned payloads into surface and engine operations.

use continuity_command::Error as CommandError; // alias: collides with crate::Error
use continuity_core::SelectionEdit;
use continuity_win::clipboard;

use crate::editor_surface::clipboard::normalize_line_endings;
use crate::Window;

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
        self.request_host_clipboard_write(&text)?;
        self.surface.clipboard.remember(text);
        Ok(())
    }

    /// `editor.cut` — copy every selection's source, then delete all of
    /// them.
    pub(crate) fn cut_selection_impl(&mut self) -> Result<(), CommandError> {
        let Some(text) = self.selections_clipboard_source() else {
            return Err(CommandError::UnsupportedContext("no selection"));
        };
        self.request_host_clipboard_write(&text)?;
        self.surface.clipboard.remember(text);
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

        let text_opt = self.request_host_clipboard_read()?;
        let Some(text) = text_opt else {
            return Err(CommandError::UnsupportedContext("clipboard has no text"));
        };
        let normalized = normalize_line_endings(&text);
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
    /// caret, skipping the clipboard-image and rich-HTML branches that
    /// [`Self::paste_clipboard_impl`] runs.
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
        let text_opt = self.request_host_clipboard_read()?;
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
    /// rich-HTML branches that `editor.paste` (Ctrl+V) runs, so a clipboard
    /// image's text alternate is inserted literally (or, for image-only
    /// clipboards, nothing). Surfaced as a
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
        let Some(text) = self.surface.clipboard.history_entry(idx).map(str::to_owned) else {
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
        self.request_host_clipboard_write(&text)?;
        self.surface.clipboard.remember(text);
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
