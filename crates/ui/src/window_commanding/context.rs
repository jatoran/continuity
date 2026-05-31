//! `impl Context for Window` — the command-handler-facing context
//! surface. Split out of `window_commanding.rs` for the 600-line cap.

use continuity_command::Context;
use continuity_core::SelectionEdit;

use crate::window_view_context::map_ui_to_command_error;
use crate::Window;

impl Context for Window {
    fn file_context(&mut self) -> Option<&mut dyn continuity_command::FileContext> {
        Some(self)
    }

    fn lookup(&self, key: &str) -> Option<&str> {
        match key {
            "editor.focused" => Some("true"),
            "selection.is_caret" => self
                .current_snapshot()
                .and_then(|s| s.selections().first().map(|selection| selection.is_caret()))
                .and_then(|is_caret| is_caret.then_some("true")),
            "shift.held" => self.shift_held.then_some("true"),
            "language" => Some(self.language_atom()),
            // G1: predicate atom that gates the Alt+C/W/R toggles to when
            // the find bar is the active overlay. Visibility is whatever
            // `Overlays::find_bar` reports — a `Some` value means the find
            // bar is open and accepting key input.
            "find_bar.visible" => self.overlays.find_bar().map(|_| "true"),
            // §H4: `editor.line_is_heading` is `"true"` when the caret
            // sits on a markdown heading line (first non-whitespace
            // char on the line is `#`). Used by the H4 chord
            // re-bindings so `Tab` / `Shift+Tab` / `Alt+Up/Down` flow
            // to `markdown.{de,pro}mote_section` / `move_section_*`
            // only when on a heading, falling through to the
            // unconditional bindings otherwise.
            // Active iff the primary caret falls inside any pipe-table
            // block's `block_range`. Drives the Ctrl+A / Home / End
            // bindings that scope to the current cell instead of the
            // whole buffer / line, and any future cell-aware keymap
            // overrides.
            "editor.in_table" => {
                let in_table = self
                    .current_snapshot()
                    .map(|snap| {
                        let Some(primary) = snap.selections().first() else {
                            return false;
                        };
                        let rope = snap.rope_snapshot().rope();
                        let line = primary.head.line as usize;
                        let line_start = if line < rope.len_lines() {
                            rope.line_to_byte(line)
                        } else {
                            rope.len_bytes()
                        };
                        let caret_byte = line_start + primary.head.byte_in_line as usize;
                        let id = self.buffer_id.as_uuid().as_u128();
                        self.decoration_cache
                            .get(id)
                            .map(|dec| {
                                dec.evaluated_tables.iter().any(|t| {
                                    caret_byte >= t.block_range.start
                                        && caret_byte < t.block_range.end
                                })
                            })
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                in_table.then_some("true")
            }
            "editor.line_is_heading" => {
                let on_heading = self
                    .current_snapshot()
                    .map(|snap| {
                        let line = snap
                            .selections()
                            .first()
                            .map(|s| s.head.line as usize)
                            .unwrap_or(0);
                        let rope = snap.rope_snapshot().rope();
                        if line >= rope.len_lines() {
                            return false;
                        }
                        let start = rope.line_to_byte(line);
                        let end = if line + 1 < rope.len_lines() {
                            rope.line_to_byte(line + 1)
                        } else {
                            rope.len_bytes()
                        };
                        for ch in rope.byte_slice(start..end).chars() {
                            match ch {
                                ' ' | '\t' => continue,
                                '#' => return true,
                                _ => return false,
                            }
                        }
                        false
                    })
                    .unwrap_or(false);
                on_heading.then_some("true")
            }
            _ => None,
        }
    }

    fn insert_text(&mut self, text: &str) -> Result<(), continuity_command::Error> {
        self.insert_text_at_selections(text)
    }

    fn delete_back(&mut self) -> Result<(), continuity_command::Error> {
        let result = self.delete_back_at_selections();
        if result.is_ok() {
            self.note_metrics_keystroke(crate::window_time_machine::MetricsKeystroke::Deleted {
                chars: 1,
            });
        }
        result
    }

    fn delete_forward(&mut self) -> Result<(), continuity_command::Error> {
        let result = self.delete_forward_at_selections();
        if result.is_ok() {
            self.note_metrics_keystroke(crate::window_time_machine::MetricsKeystroke::Deleted {
                chars: 1,
            });
        }
        result
    }

    fn apply_selection_edit(
        &mut self,
        edit: SelectionEdit,
    ) -> Result<(), continuity_command::Error> {
        // Phase B5: every edit, whether keystroke- or command-driven,
        // counts as recent input — keep the caret solid until the typing
        // pause elapses.
        self.note_input_now();
        self.dispatch_selection_edit(edit)
    }

    fn auto_pair_for(&self, c: char) -> Option<(char, char)> {
        self.auto_pair.pair_for(c)
    }

    fn try_delete_back_pair(&mut self) -> Result<bool, continuity_command::Error> {
        self.try_delete_auto_pair()
    }

    fn move_word(&mut self, delta: i32) -> Result<(), continuity_command::Error> {
        let _ = self.move_word_selection(delta, false);
        Ok(())
    }

    fn extend_word(&mut self, delta: i32) -> Result<(), continuity_command::Error> {
        let _ = self.move_word_selection(delta, true);
        Ok(())
    }

    fn move_paragraph(&mut self, delta: i32) -> Result<(), continuity_command::Error> {
        let _ = self.move_paragraph_selection(delta, false);
        Ok(())
    }

    fn extend_paragraph(&mut self, delta: i32) -> Result<(), continuity_command::Error> {
        let _ = self.move_paragraph_selection(delta, true);
        Ok(())
    }

    fn shrink_selection_smart(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.shrink_selection_smart_at();
        Ok(())
    }

    fn move_char(&mut self, delta: i32) -> Result<(), continuity_command::Error> {
        let _ = self.move_char_selection(delta, false);
        Ok(())
    }

    fn move_line(&mut self, delta: i32) -> Result<(), continuity_command::Error> {
        let _ = self.move_line_selection(delta, false);
        Ok(())
    }

    fn move_line_start(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.move_line_start_selection(false);
        Ok(())
    }

    fn move_line_end(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.move_line_end_selection(false);
        Ok(())
    }

    fn move_doc_start(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.move_doc_start_selection(false);
        Ok(())
    }

    fn move_doc_end(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.move_doc_end_selection(false);
        self.pending_doc_end_scroll = true;
        self.pending_doc_end_scroll_attempts = 0;
        Ok(())
    }

    fn extend_char(&mut self, delta: i32) -> Result<(), continuity_command::Error> {
        let _ = self.move_char_selection(delta, true);
        Ok(())
    }

    fn extend_line(&mut self, delta: i32) -> Result<(), continuity_command::Error> {
        let _ = self.move_line_selection(delta, true);
        Ok(())
    }

    fn extend_line_start(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.move_line_start_selection(true);
        Ok(())
    }

    fn extend_line_end(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.move_line_end_selection(true);
        Ok(())
    }

    fn extend_doc_start(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.move_doc_start_selection(true);
        Ok(())
    }

    fn extend_doc_end(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.move_doc_end_selection(true);
        self.pending_doc_end_scroll = true;
        self.pending_doc_end_scroll_attempts = 0;
        Ok(())
    }

    fn add_cursor_above(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.add_cursor_line(-1);
        Ok(())
    }

    fn add_cursor_below(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.add_cursor_line(1);
        Ok(())
    }

    fn add_cursor_at_next_match(&mut self) -> Result<(), continuity_command::Error> {
        let _ = Window::add_cursor_at_next_match(self);
        Ok(())
    }

    fn add_cursor_at_all_matches(&mut self) -> Result<(), continuity_command::Error> {
        let _ = Window::add_cursor_at_all_matches(self);
        Ok(())
    }

    fn column_select_up(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.column_select(-1);
        Ok(())
    }

    fn column_select_down(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.column_select(1);
        Ok(())
    }

    fn clear_secondary_cursors(&mut self) -> Result<(), continuity_command::Error> {
        let _ = Window::clear_secondary_cursors(self);
        Ok(())
    }

    fn select_word(&mut self) -> Result<(), continuity_command::Error> {
        let _ = Window::select_word(self);
        Ok(())
    }

    fn select_line(&mut self) -> Result<(), continuity_command::Error> {
        let _ = Window::select_line(self);
        Ok(())
    }

    fn select_paragraph(&mut self) -> Result<(), continuity_command::Error> {
        let _ = Window::select_paragraph(self);
        Ok(())
    }

    fn expand_selection_smart(&mut self) -> Result<(), continuity_command::Error> {
        let _ = Window::expand_selection_smart(self);
        Ok(())
    }

    fn select_all(&mut self) -> Result<(), continuity_command::Error> {
        let _ = Window::select_all(self);
        Ok(())
    }

    fn show_keymap_conflicts(&mut self) -> Result<(), continuity_command::Error> {
        self.log_keymap_conflicts();
        Ok(())
    }

    fn reload_keymap(&mut self) -> Result<(), continuity_command::Error> {
        self.reload_keymap_from_sources()
    }

    fn undo(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.editor.undo(self.buffer_id);
        // α.1 undo/redo target echo — flash the line(s) the caret
        // lands on so the writer doesn't have to hunt for *where*
        // the buffer reverted to.
        self.pulse_undo_target();
        Ok(())
    }

    fn redo(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.editor.redo(self.buffer_id);
        self.pulse_undo_target();
        Ok(())
    }

    fn redo_alternate_branch(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.editor.redo_alternate_branch(self.buffer_id);
        self.pulse_undo_target();
        Ok(())
    }

    fn undo_tree_pick(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.editor.undo_tree_pick(self.buffer_id);
        self.pulse_undo_target();
        Ok(())
    }

    fn open_find(&mut self, with_replace: bool) -> Result<(), continuity_command::Error> {
        self.open_find_impl(with_replace)
    }

    fn open_palette(&mut self) -> Result<(), continuity_command::Error> {
        self.overlays.open(crate::overlays::OverlayKind::Palette);
        self.focus_overlay_input();
        self.populate_palette_candidates();
        Ok(())
    }

    fn open_quick_open(&mut self) -> Result<(), continuity_command::Error> {
        self.overlays.open(crate::overlays::OverlayKind::QuickOpen);
        self.focus_overlay_input();
        self.populate_quick_open_candidates();
        Ok(())
    }

    fn open_goto_line(&mut self) -> Result<(), continuity_command::Error> {
        self.overlays.open(crate::overlays::OverlayKind::GotoLine);
        self.focus_overlay_input();
        Ok(())
    }

    fn open_goto_heading(&mut self) -> Result<(), continuity_command::Error> {
        self.overlays
            .open(crate::overlays::OverlayKind::GotoHeading);
        self.focus_overlay_input();
        self.populate_goto_heading();
        Ok(())
    }

    fn open_find_in_all(&mut self) -> Result<(), continuity_command::Error> {
        self.overlays.open(crate::overlays::OverlayKind::FindInAll);
        self.focus_overlay_input();
        Ok(())
    }

    fn dismiss_overlay(&mut self) -> Result<(), continuity_command::Error> {
        self.overlays.dismiss();
        self.blur_overlay_input();
        Ok(())
    }

    // G2 split: find-bar methods live on `impl FindContext for Window`
    // below — see the `continuity_command::FindContext` supertrait.

    fn selection_arithmetic(
        &mut self,
        op: &str,
        regex: &str,
    ) -> Result<(), continuity_command::Error> {
        let _ = self.selection_arithmetic_impl(op, regex);
        Ok(())
    }

    fn adjust_zoom(&mut self, factor: f32) -> Result<(), continuity_command::Error> {
        self.view_adjust_zoom_impl(factor)
            .map_err(map_ui_to_command_error)
    }

    fn reset_zoom(&mut self) -> Result<(), continuity_command::Error> {
        self.view_reset_zoom_impl().map_err(map_ui_to_command_error)
    }

    fn toggle_soft_wrap(&mut self) -> Result<(), continuity_command::Error> {
        self.view_toggle_soft_wrap_impl()
            .map_err(map_ui_to_command_error)
    }

    fn scroll_lines(&mut self, lines: f32) -> Result<(), continuity_command::Error> {
        self.view_scroll_lines_impl(lines)
            .map_err(map_ui_to_command_error)
    }

    fn scroll_page(&mut self, direction: f32) -> Result<(), continuity_command::Error> {
        self.view_scroll_page_impl(direction)
            .map_err(map_ui_to_command_error)
    }

    fn scroll_doc_start(&mut self) -> Result<(), continuity_command::Error> {
        self.view_scroll_doc_start_impl()
            .map_err(map_ui_to_command_error)
    }

    fn scroll_doc_end(&mut self) -> Result<(), continuity_command::Error> {
        self.view_scroll_doc_end_impl()
            .map_err(map_ui_to_command_error)
    }

    fn open_link_at_caret(&mut self) -> Result<(), continuity_command::Error> {
        self.open_link_at_caret_impl()
    }

    fn copy_rendered_text(&mut self) -> Result<(), continuity_command::Error> {
        self.copy_rendered_text_impl()
    }

    fn copy_source_text(&mut self) -> Result<(), continuity_command::Error> {
        self.copy_source_text_impl()
    }

    fn copy_as_html(&mut self) -> Result<(), continuity_command::Error> {
        self.copy_as_html_impl()
    }

    fn tear_off_focused_tab(
        &mut self,
    ) -> Result<continuity_buffer::BufferId, continuity_command::Error> {
        self.tear_off_focused_tab_impl()
    }
    fn local_recently_closed_top_ms(&self) -> Option<i64> {
        self.tree
            .recently_closed
            .first()
            .map(|c| c.closed_at_ms as i64)
    }
}
