//! `ViewContext` (Phase 11 view + Phase 13 pane/tab) impl for `Window`.
//!
//! Pulled out of `window_commanding.rs` to keep that file under the
//! 600-line cap once Phase 13's pane/tab dispatch landed.
//!
//! Thread ownership: UI thread of one window. Every method mutates
//! UI-thread-owned state and requests a repaint via `request_repaint`.

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

use crate::pane_layout::FocusDirection;
use crate::pane_shortcuts::LayoutShortcut;
use crate::pane_tree::SplitAxis;
use crate::window::Window;

impl continuity_command::ViewContext for Window {
    fn current_window_rect(&self) -> Option<(i32, i32, i32, i32)> {
        if self.hwnd.0.is_null() {
            return None;
        }
        let mut r = RECT::default();
        if unsafe { GetWindowRect(self.hwnd, &mut r) }.is_err() {
            return None;
        }
        Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
    }

    fn smart_home(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.smart_home_selection(false);
        Ok(())
    }

    fn show_buffer_history_tab(&mut self) -> Result<(), continuity_command::Error> {
        self.show_buffer_history_tab_impl()
            .map_err(|_| continuity_command::Error::UnsupportedContext("show_buffer_history_tab"))
    }

    fn extend_smart_home(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.smart_home_selection(true);
        Ok(())
    }

    fn cycle_theme(&mut self) -> Result<(), continuity_command::Error> {
        self.cycle_theme_impl().map_err(map_ui_to_command_error)
    }
    fn reload_theme(&mut self) -> Result<(), continuity_command::Error> {
        self.reload_theme_impl().map_err(map_ui_to_command_error)
    }
    fn capture_layout_diagnostics(&mut self) -> Result<(), continuity_command::Error> {
        self.capture_layout_diagnostics_impl()
    }
    fn pick_font_family(&mut self) -> Result<(), continuity_command::Error> {
        self.pick_font_family_impl()
            .map_err(map_ui_to_command_error)
    }
    fn pick_theme(&mut self) -> Result<(), continuity_command::Error> {
        self.pick_theme_impl().map_err(map_ui_to_command_error)
    }
    fn set_font_size(&mut self, size_dip: f32) -> Result<(), continuity_command::Error> {
        self.set_font_size_impl(size_dip)
            .map_err(map_ui_to_command_error)
    }
    fn toggle_line_numbers(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_line_numbers_impl()
            .map_err(map_ui_to_command_error)
    }
    fn toggle_relative_line_numbers(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_relative_line_numbers_impl()
            .map_err(map_ui_to_command_error)
    }
    fn toggle_all_line_numbers(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_all_line_numbers_impl()
            .map_err(map_ui_to_command_error)
    }
    fn toggle_current_line_highlight(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_current_line_highlight_impl()
            .map_err(map_ui_to_command_error)
    }
    fn toggle_indent_guides(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_indent_guides_impl()
            .map_err(map_ui_to_command_error)
    }
    fn toggle_whitespace_markers(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_whitespace_markers_impl()
            .map_err(map_ui_to_command_error)
    }
    fn toggle_trailing_whitespace(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_trailing_whitespace_impl()
            .map_err(map_ui_to_command_error)
    }
    fn toggle_minimap(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_minimap_impl().map_err(map_ui_to_command_error)
    }
    fn toggle_sticky_breadcrumb(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_sticky_breadcrumb_impl()
            .map_err(map_ui_to_command_error)
    }
    fn toggle_outline(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_outline_impl().map_err(map_ui_to_command_error)
    }
    fn markdown_insert_toc(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_insert_toc_impl()
    }
    fn markdown_refresh_toc(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_refresh_toc_impl()
    }
    fn markdown_highlight_selection(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_highlight_selection_impl()
    }
    fn markdown_color_selection(
        &mut self,
        prefill: Option<&str>,
    ) -> Result<(), continuity_command::Error> {
        self.markdown_color_selection_impl(prefill)
    }
    fn markdown_clear_inline_color(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_clear_inline_color_impl()
    }
    fn markdown_insert_table(
        &mut self,
        rows: u32,
        cols: u32,
    ) -> Result<(), continuity_command::Error> {
        self.markdown_insert_table_impl(rows, cols)
    }
    fn markdown_table_insert_row(&mut self, above: bool) -> Result<(), continuity_command::Error> {
        self.markdown_table_insert_row_impl(above)
    }
    fn markdown_table_insert_column(
        &mut self,
        before: bool,
    ) -> Result<(), continuity_command::Error> {
        self.markdown_table_insert_column_impl(before)
    }
    fn markdown_table_delete_row(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_table_delete_row_impl()
    }
    fn markdown_table_delete_column(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_table_delete_column_impl()
    }
    fn markdown_table_delete_table(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_table_delete_table_impl()
    }
    fn markdown_table_select_cell(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_table_select_cell_impl()
    }
    fn markdown_table_caret_cell_edge(
        &mut self,
        to_start: bool,
        extend: bool,
    ) -> Result<(), continuity_command::Error> {
        self.markdown_table_caret_cell_edge_impl(to_start, extend)
    }
    fn markdown_table_tab_next(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_table_tab_step_impl(true)
    }
    fn markdown_table_tab_prev(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_table_tab_step_impl(false)
    }
    fn markdown_table_enter(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_table_enter_impl()
    }
    fn markdown_table_insert_break(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_table_insert_break_impl()
    }
    fn markdown_table_move_vertical(
        &mut self,
        down: bool,
    ) -> Result<(), continuity_command::Error> {
        self.markdown_table_move_vertical_impl(down)
    }
    fn markdown_table_cell_up(&mut self) -> Result<(), continuity_command::Error> {
        self.markdown_table_cell_up_impl()
    }
    fn set_ruler_columns(&mut self, columns: Vec<u32>) -> Result<(), continuity_command::Error> {
        self.set_ruler_columns_impl(columns)
            .map_err(map_ui_to_command_error)
    }
    fn cycle_caret_style(&mut self) -> Result<(), continuity_command::Error> {
        self.cycle_caret_style_impl()
            .map_err(map_ui_to_command_error)
    }
    fn set_indent_type(&mut self, use_spaces: bool) -> Result<(), continuity_command::Error> {
        self.set_indent_type_impl(use_spaces);
        Ok(())
    }
    fn set_indent_width(&mut self, width: u32) -> Result<(), continuity_command::Error> {
        self.set_indent_width_impl(width);
        Ok(())
    }
    fn adjust_indent_width(&mut self, delta: i32) -> Result<(), continuity_command::Error> {
        self.adjust_indent_width_impl(delta);
        Ok(())
    }
    fn set_tab_width(&mut self, width: u32) -> Result<(), continuity_command::Error> {
        self.set_tab_width_impl(width);
        Ok(())
    }
    fn adjust_tab_width(&mut self, delta: i32) -> Result<(), continuity_command::Error> {
        self.adjust_tab_width_impl(delta);
        Ok(())
    }
    fn toggle_ligatures(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_ligatures_impl()
            .map_err(map_ui_to_command_error)
    }
    fn open_settings(&mut self) -> Result<(), continuity_command::Error> {
        self.open_settings_impl().map_err(map_ui_to_command_error)
    }

    // ---- Phase 13 — pane / tab manipulation ----

    fn pane_split_horizontal(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.split(SplitAxis::Horizontal);
        self.request_repaint();
        Ok(())
    }
    fn pane_split_vertical(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.split(SplitAxis::Vertical);
        self.request_repaint();
        Ok(())
    }
    fn pane_close(&mut self) -> Result<(), continuity_command::Error> {
        self.close_focused_pane().map_err(map_ui_to_command_error)?;
        self.request_repaint();
        Ok(())
    }
    fn pane_focus_left(&mut self) -> Result<(), continuity_command::Error> {
        self.focus_direction(FocusDirection::Left);
        self.request_repaint();
        Ok(())
    }
    fn pane_focus_right(&mut self) -> Result<(), continuity_command::Error> {
        self.focus_direction(FocusDirection::Right);
        self.request_repaint();
        Ok(())
    }
    fn pane_focus_up(&mut self) -> Result<(), continuity_command::Error> {
        self.focus_direction(FocusDirection::Up);
        self.request_repaint();
        Ok(())
    }
    fn pane_focus_down(&mut self) -> Result<(), continuity_command::Error> {
        self.focus_direction(FocusDirection::Down);
        self.request_repaint();
        Ok(())
    }
    fn pane_maximize_toggle(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_maximize_focused_pane();
        self.request_repaint();
        Ok(())
    }
    fn pane_resize(&mut self, axis: &str, delta_dip: f32) -> Result<(), continuity_command::Error> {
        let a = match axis {
            "horizontal" | "h" => SplitAxis::Horizontal,
            "vertical" | "v" => SplitAxis::Vertical,
            other => {
                return Err(continuity_command::Error::Other(format!(
                    "pane_resize: unknown axis '{}'",
                    other
                )))
            }
        };
        self.resize_focused_pane(a, delta_dip);
        self.request_repaint();
        Ok(())
    }
    fn apply_layout_shortcut(&mut self, shortcut: u32) -> Result<(), continuity_command::Error> {
        let s = match shortcut {
            1 => LayoutShortcut::Single,
            2 => LayoutShortcut::TwoCols,
            3 => LayoutShortcut::ThreeCols,
            4 => LayoutShortcut::FourCols,
            5 => LayoutShortcut::Grid2x2,
            8 => LayoutShortcut::Grid2x4,
            other => {
                return Err(continuity_command::Error::Other(format!(
                    "apply_layout_shortcut: unsupported shortcut {}",
                    other
                )))
            }
        };
        Window::apply_layout_shortcut(self, s);
        self.request_repaint();
        Ok(())
    }
    fn apply_layout_two_rows(&mut self) -> Result<(), continuity_command::Error> {
        Window::apply_layout_shortcut(self, LayoutShortcut::TwoRows);
        self.request_repaint();
        Ok(())
    }
    fn tab_new(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.open_new_tab();
        self.request_repaint();
        Ok(())
    }
    fn tab_close(&mut self) -> Result<(), continuity_command::Error> {
        // Confirm before closing if the active tab has unsaved typing
        // (untitled buffer with content). Right-click menu has already
        // prompted on its own path and short-circuits before dispatching
        // here when the user cancels — see
        // [`Window::confirm_close_tab`] for the shared semantics.
        if !self.confirm_close_active_tab() {
            return Ok(());
        }
        self.close_active_tab().map_err(map_ui_to_command_error)?;
        self.request_repaint();
        Ok(())
    }
    fn tab_next(&mut self) -> Result<(), continuity_command::Error> {
        // §H6 — route through the chord state machine so a sustained
        // Ctrl-hold trips the 600 ms timer and opens the overlay; an
        // already-open overlay steps the cursor instead of swapping.
        self.tab_chord_step(1);
        Ok(())
    }
    fn tab_prev(&mut self) -> Result<(), continuity_command::Error> {
        self.tab_chord_step(-1);
        Ok(())
    }
    fn tab_step_mru(&mut self, delta: i32) -> Result<(), continuity_command::Error> {
        self.step_tab_mru(delta);
        self.request_repaint();
        Ok(())
    }
    fn tab_go_to(&mut self, one_indexed: u32) -> Result<(), continuity_command::Error> {
        self.activate_positional_tab(one_indexed as usize);
        self.request_repaint();
        Ok(())
    }
    fn tab_reopen_closed(&mut self) -> Result<(), continuity_command::Error> {
        let _ = self.reopen_closed_tab();
        self.request_repaint();
        Ok(())
    }
    fn tab_pin_toggle(&mut self) -> Result<(), continuity_command::Error> {
        self.tab_pin_toggle_impl();
        self.request_repaint();
        Ok(())
    }

    // ---- Phase 16 — clipboard, paste history, spell-check ----

    fn cut_selection(&mut self) -> Result<(), continuity_command::Error> {
        let r = self.cut_selection_impl();
        self.request_repaint();
        r
    }
    fn copy_selection(&mut self) -> Result<(), continuity_command::Error> {
        // E2: when the command palette is open with a math preview,
        // Ctrl+C copies the formatted result instead of the editor's
        // current selection.
        if self.palette_math_copy() {
            return Ok(());
        }
        self.copy_selection_impl()
    }
    fn paste_clipboard(&mut self) -> Result<(), continuity_command::Error> {
        let r = self.paste_clipboard_impl();
        self.request_repaint();
        r
    }
    fn paste_as_plain_text(&mut self) -> Result<(), continuity_command::Error> {
        let r = self.paste_as_plain_text_impl();
        self.request_repaint();
        r
    }
    fn paste_from_history(
        &mut self,
        index: Option<usize>,
    ) -> Result<(), continuity_command::Error> {
        let r = self.paste_from_history_impl(index);
        self.request_repaint();
        r
    }
    fn copy_caret_line(&mut self) -> Result<(), continuity_command::Error> {
        self.copy_caret_line_impl()
    }
    fn goto_last_edit(&mut self) -> Result<(), continuity_command::Error> {
        let moved = self.goto_last_edit_impl();
        if moved {
            self.request_repaint();
        }
        Ok(())
    }
    fn spell_toggle(&mut self) -> Result<(), continuity_command::Error> {
        let r = self.spell_toggle_impl();
        self.request_repaint();
        r
    }
    fn spell_replace_at_caret(&mut self, with: &str) -> Result<(), continuity_command::Error> {
        let r = self.spell_replace_at_caret_impl(with);
        self.request_repaint();
        r
    }
    fn spell_add_to_dictionary(&mut self) -> Result<(), continuity_command::Error> {
        let r = self.spell_add_to_dictionary_impl();
        self.request_repaint();
        r
    }
    fn spell_show_suggestions(&mut self) -> Result<(), continuity_command::Error> {
        self.spell_show_suggestions_impl()
    }

    // ---- Phase H ----

    fn set_focus_mode(&mut self, mode: &str) -> Result<(), continuity_command::Error> {
        self.set_focus_mode_impl(mode)
            .map_err(map_ui_to_command_error)
    }
    fn cycle_focus_mode(&mut self) -> Result<(), continuity_command::Error> {
        self.cycle_focus_mode_impl()
            .map_err(map_ui_to_command_error)
    }
    fn toggle_distraction_free_mode(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_distraction_free_mode_impl()
            .map_err(map_ui_to_command_error)
    }
    fn fold_at_caret(&mut self) -> Result<(), continuity_command::Error> {
        self.fold_at_caret_impl().map_err(map_ui_to_command_error)
    }
    fn unfold_at_caret(&mut self) -> Result<(), continuity_command::Error> {
        self.unfold_at_caret_impl().map_err(map_ui_to_command_error)
    }
    fn fold_all(&mut self) -> Result<(), continuity_command::Error> {
        self.fold_all_impl().map_err(map_ui_to_command_error)
    }
    fn unfold_all(&mut self) -> Result<(), continuity_command::Error> {
        self.unfold_all_impl().map_err(map_ui_to_command_error)
    }
    fn toggle_fold_at_caret(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_fold_at_caret_impl()
            .map_err(map_ui_to_command_error)
    }
    fn show_slash_palette(&mut self) -> Result<(), continuity_command::Error> {
        // The chord entry point always passes the `ExplicitChord`
        // trigger so the Esc cleanup path knows there is no trailing
        // `/` to remove. The typed-`/` line-start hook bypasses this
        // method and calls `show_slash_palette_impl` directly with
        // `SlashTrigger::TypedSlash`.
        self.show_slash_palette_impl(crate::slash_palette::SlashTrigger::ExplicitChord)
            .map_err(map_ui_to_command_error)
    }
    fn show_tab_overlay(&mut self) -> Result<(), continuity_command::Error> {
        self.show_tab_overlay_impl()
            .map_err(map_ui_to_command_error)
    }
    fn show_previous_buffer_browser(&mut self) -> Result<(), continuity_command::Error> {
        self.show_previous_buffer_browser_impl()
            .map_err(map_ui_to_command_error)
    }
    fn open_timeline_for_closed_buffer(
        &mut self,
        buffer_id: continuity_buffer::BufferId,
    ) -> Result<(), continuity_command::Error> {
        self.open_timeline_for_closed_buffer_impl(buffer_id)
            .map_err(map_ui_to_command_error)
    }
    fn open_buffer_timeline(&mut self) -> Result<(), continuity_command::Error> {
        self.open_buffer_timeline_impl()
            .map_err(map_ui_to_command_error)
    }
    fn mark_next_snapshot(&mut self, label: &str) -> Result<(), continuity_command::Error> {
        self.mark_next_snapshot_impl(label)
            .map_err(map_ui_to_command_error)
    }
    fn show_metrics_buffer(&mut self) -> Result<(), continuity_command::Error> {
        self.show_metrics_buffer_impl()
            .map_err(map_ui_to_command_error)
    }
    fn purge_metrics(&mut self) -> Result<(), continuity_command::Error> {
        self.purge_metrics_impl().map_err(map_ui_to_command_error)
    }

    fn show_tutorial_buffer(&mut self) -> Result<(), continuity_command::Error> {
        self.show_tutorial_buffer_impl()
            .map_err(map_ui_to_command_error)
    }

    // δ.5 — theme-management workflow. Bodies live in
    // `window_theme_manage.rs` so this file stays under the 600-line cap.
    fn theme_clone_active(&mut self, name: Option<&str>) -> Result<(), continuity_command::Error> {
        self.theme_clone_active_impl(name)
            .map_err(map_ui_to_command_error)
    }
    fn theme_edit(&mut self, name: Option<&str>) -> Result<(), continuity_command::Error> {
        self.theme_edit_impl(name).map_err(map_ui_to_command_error)
    }
    fn theme_duplicate(
        &mut self,
        source: Option<&str>,
        new_name: Option<&str>,
    ) -> Result<(), continuity_command::Error> {
        self.theme_duplicate_impl(source, new_name)
            .map_err(map_ui_to_command_error)
    }
    fn theme_rename(
        &mut self,
        old: Option<&str>,
        new_name: Option<&str>,
    ) -> Result<(), continuity_command::Error> {
        self.theme_rename_impl(old, new_name)
            .map_err(map_ui_to_command_error)
    }
    fn theme_delete(&mut self, name: Option<&str>) -> Result<(), continuity_command::Error> {
        self.theme_delete_impl(name)
            .map_err(map_ui_to_command_error)
    }
    fn theme_reveal_folder(&mut self) -> Result<(), continuity_command::Error> {
        self.theme_reveal_folder_impl()
            .map_err(map_ui_to_command_error)
    }
    fn theme_create_blank(&mut self, name: Option<&str>) -> Result<(), continuity_command::Error> {
        self.theme_create_blank_impl(name)
            .map_err(map_ui_to_command_error)
    }
}

pub(crate) fn map_ui_to_command_error(err: crate::Error) -> continuity_command::Error {
    match err {
        crate::Error::Command(e) => e,
        other => continuity_command::Error::Other(other.to_string()),
    }
}
