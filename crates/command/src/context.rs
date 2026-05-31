//! The [`Context`] trait — the dispatch surface a command handler sees.
//!
//! Every method has a default `Err(Error::UnsupportedContext("…"))`
//! implementation so a stub context (tests, palette previews) can
//! implement only the methods it actually exercises. The real impl lives
//! on `ui::Window` (one trait, one production implementor).

use continuity_buffer::BufferId;
use continuity_core::SelectionEdit;

use crate::file_context::FileContext;
use crate::find_context::FindContext;
use crate::view_context::ViewContext;
use crate::Error;

/// A read-only view of the current editor state, queried by predicates,
/// plus the mutation surface invoked by command handlers.
///
/// Inherits [`ViewContext`] (view/theme/font) and [`FindContext`].
/// File commands opt in through [`Self::file_context`].
pub trait Context: ViewContext + FindContext {
    /// Look up the string value of an attribute (e.g., `"language"` →
    /// `Some("markdown")`). Return `None` when unset.
    fn lookup(&self, key: &str) -> Option<&str>;

    /// Return whether a boolean flag is set (e.g., `editor.focused`).
    fn flag(&self, key: &str) -> bool {
        self.lookup(key).is_some()
    }

    /// Wall-clock millis of this window's most-recently-closed tab,
    /// or `None` when its local `recently_closed` stack is empty.
    /// Used by smart `tab.reopen_closed`: the more recent close
    /// (local-tab vs persisted-window) wins.
    fn local_recently_closed_top_ms(&self) -> Option<i64> {
        None
    }

    /// Return the optional file-command surface for contexts that
    /// support native file/folder interactions.
    fn file_context(&mut self) -> Option<&mut dyn FileContext> {
        None
    }

    /// Insert text at the active caret.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when the active context has no
    /// editable buffer.
    fn insert_text(&mut self, _text: &str) -> Result<(), Error> {
        Err(Error::UnsupportedContext("insert_text"))
    }

    /// Delete one character before the active caret.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn delete_back(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("delete_back"))
    }

    /// Delete one character after the active caret.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn delete_forward(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("delete_forward"))
    }

    /// Apply a generic [`SelectionEdit`] — the dispatch surface for the
    /// Phase 6 text-mutation commands. Each call lands as one undo group.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported, or any core
    /// error from the underlying `EditorHandle::apply_selection_edit` call.
    fn apply_selection_edit(&mut self, _edit: SelectionEdit) -> Result<(), Error> {
        Err(Error::UnsupportedContext("apply_selection_edit"))
    }

    /// Move the active caret by a character delta.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn move_char(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("move_char"))
    }

    /// Move the active caret by a line delta.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn move_line(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("move_line"))
    }

    /// Move the active caret to the start of its line.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn move_line_start(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("move_line_start"))
    }

    /// Move the active caret to the end of its line.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn move_line_end(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("move_line_end"))
    }

    /// Move the active caret to the start of the document.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn move_doc_start(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("move_doc_start"))
    }

    /// Move the active caret to the end of the document.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn move_doc_end(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("move_doc_end"))
    }

    /// Extend the active selection by a character delta.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn extend_char(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("extend_char"))
    }

    /// Extend the active selection by a line delta.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn extend_line(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("extend_line"))
    }

    /// Extend the active selection to the start of its line.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn extend_line_start(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("extend_line_start"))
    }

    /// Extend the active selection to the end of its line.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn extend_line_end(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("extend_line_end"))
    }

    /// Extend the active selection to the start of the document.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn extend_doc_start(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("extend_doc_start"))
    }

    /// Extend the active selection to the end of the document.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn extend_doc_end(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("extend_doc_end"))
    }

    /// Move each caret by `delta` words.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn move_word(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("move_word"))
    }

    /// Extend each selection by `delta` words.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn extend_word(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("extend_word"))
    }

    /// Move each caret by `delta` paragraphs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn move_paragraph(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("move_paragraph"))
    }

    /// Extend each selection by `delta` paragraphs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn extend_paragraph(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("extend_paragraph"))
    }

    /// Add a secondary cursor on the line above the primary cursor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn add_cursor_above(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("add_cursor_above"))
    }

    /// Add a secondary cursor on the line below the primary cursor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn add_cursor_below(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("add_cursor_below"))
    }

    /// Add a cursor at the next match of the primary selected text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn add_cursor_at_next_match(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("add_cursor_at_next_match"))
    }

    /// Add cursors at all matches of the primary selected text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn add_cursor_at_all_matches(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("add_cursor_at_all_matches"))
    }

    /// Extend a block selection one line upward.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn column_select_up(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("column_select_up"))
    }

    /// Extend a block selection one line downward.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn column_select_down(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("column_select_down"))
    }

    /// Remove every secondary cursor, keeping the primary selection.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn clear_secondary_cursors(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("clear_secondary_cursors"))
    }

    /// Select the word at each active cursor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn select_word(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("select_word"))
    }

    /// Select the line at each active cursor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn select_line(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("select_line"))
    }

    /// Select the paragraph at each active cursor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn select_paragraph(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("select_paragraph"))
    }

    /// Expand each active selection to a larger syntactic or prose scope.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn expand_selection_smart(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("expand_selection_smart"))
    }

    /// Select the entire buffer. Defaults to `UnsupportedContext`.
    fn select_all(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("select_all"))
    }

    /// Shrink each active selection to a smaller scope.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn shrink_selection_smart(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("shrink_selection_smart"))
    }

    /// Log the current keymap conflicts.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn show_keymap_conflicts(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("show_keymap_conflicts"))
    }

    /// Reload the keymap from its configured source.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn reload_keymap(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("reload_keymap"))
    }

    /// Undo the most-recent edit group.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn undo(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("undo"))
    }

    /// Redo the most-recent child of the buffer's current undo head.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn redo(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("redo"))
    }

    /// Cycle to (and apply) an alternate sibling branch of the most-recent
    /// redo target.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn redo_alternate_branch(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("redo_alternate_branch"))
    }

    /// Surface the buffer's undo tree (current head + branches).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn undo_tree_pick(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("undo_tree_pick"))
    }

    /// Open the in-buffer find bar. If `with_replace`, the replace field is
    /// also visible.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn open_find(&mut self, _with_replace: bool) -> Result<(), Error> {
        Err(Error::UnsupportedContext("open_find"))
    }

    /// Open the find-in-all-buffers panel.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn open_find_in_all(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("open_find_in_all"))
    }

    /// Open the command palette.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn open_palette(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("open_palette"))
    }

    /// Open the quick-open buffer switcher.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn open_quick_open(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("open_quick_open"))
    }

    /// Open the goto-line dialog.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn open_goto_line(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("open_goto_line"))
    }

    /// Open the goto-heading picker.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn open_goto_heading(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("open_goto_heading"))
    }

    /// Dismiss any active overlay.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn dismiss_overlay(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("dismiss_overlay"))
    }

    // find_step / find_replace_one / find_replace_all / find_toggle live
    // on `FindContext` (a supertrait) — see `crates/command/src/find_context.rs`.

    /// G5 — selection arithmetic. `op` is `"keep"` (drop selections
    /// whose text doesn't match `regex`), `"discard"` (inverse), or
    /// `"split"` (break each selection at every regex match into
    /// sub-selections). Unknown ops and malformed regexes are no-ops.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn selection_arithmetic(&mut self, _op: &str, _regex: &str) -> Result<(), Error> {
        Err(Error::UnsupportedContext("selection_arithmetic"))
    }

    /// Adjust per-pane font zoom by `factor`. The Phase 9 view-state owner
    /// is responsible for invalidating any layout cache built against the
    /// previous font state.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn adjust_zoom(&mut self, _factor: f32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("adjust_zoom"))
    }

    /// Reset per-pane font zoom to 1.0.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn reset_zoom(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("reset_zoom"))
    }

    /// Toggle soft wrap for the active pane.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn toggle_soft_wrap(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_soft_wrap"))
    }

    /// Pixel-locked scroll by `lines` logical lines (negative = scroll up).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn scroll_lines(&mut self, _lines: f32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("scroll_lines"))
    }

    /// Animated scroll by one viewport-page worth (Page Up/Down). `direction`
    /// is `+1.0` for down, `-1.0` for up.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn scroll_page(&mut self, _direction: f32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("scroll_page"))
    }

    /// Animated scroll to the document start.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn scroll_doc_start(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("scroll_doc_start"))
    }

    /// Animated scroll to the document end.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn scroll_doc_end(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("scroll_doc_end"))
    }

    /// Open the link the active caret is currently inside, if any. Phase-10
    /// equivalent of Ctrl+click on a styled link.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported, or any
    /// shell-execute / clipboard error from the implementation.
    fn open_link_at_caret(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("open_link_at_caret"))
    }

    /// Copy the active selection's rendered (decoration-flattened) plain
    /// text to the clipboard. Phase-10 `Ctrl+Shift+C`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn copy_rendered_text(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("copy_rendered_text"))
    }

    /// Copy the active selection's source markdown to the clipboard.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn copy_source_text(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("copy_source_text"))
    }

    /// Render the current buffer to HTML via pulldown-cmark and place it on
    /// the clipboard. `markdown.copy_as_html`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn copy_as_html(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("copy_as_html"))
    }

    /// Tear the focused tab off the local pane tree and return its
    /// [`BufferId`]; the registry handler reopens it in a fresh window.
    /// Errors when the focused group is the last tab (would orphan the window).
    fn tear_off_focused_tab(&mut self) -> Result<BufferId, Error> {
        Err(Error::UnsupportedContext("tear_off_focused_tab"))
    }

    /// Phase-16.5 auto-pair lookup. `None` ⇒ plain insert.
    fn auto_pair_for(&self, _c: char) -> Option<(char, char)> {
        None
    }
    /// Phase-16.5 backspace-aware delete-pair. `Ok(true)` ⇒ pair
    /// deleted; `Ok(false)` ⇒ fall through to delete_back. Errors
    /// propagate from the underlying pair-delete plan.
    fn try_delete_back_pair(&mut self) -> Result<bool, Error> {
        Ok(false)
    }
}
