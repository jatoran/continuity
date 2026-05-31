//! Confirm-key (Enter) dispatch for every overlay variant.
//!
//! Split from [`crate::window_overlays`] so that file stays under the
//! conventions cap. Each `confirm_*` method commits one overlay's
//! selection: the palette dispatches the chosen command, the goto
//! overlays jump the caret, the pickers settle the preview into the
//! live state, and the tab switcher promotes the highlighted tab. The
//! parent module's `overlay_on_keydown` calls [`Window::overlay_confirm`]
//! which fans out to these methods.
//!
//! Thread ownership: UI thread of one window.

use continuity_text::Position;

use crate::find_bar::FindFocus;
use crate::overlays::Overlays;
use crate::Window;

impl Window {
    pub(crate) fn overlay_confirm(&mut self) {
        match &mut self.overlays {
            Overlays::Find(_) => {
                let should_replace = self
                    .overlays
                    .find_bar()
                    .is_some_and(|fb| fb.replace_visible && fb.focus == FindFocus::Replace);
                if should_replace {
                    let _ = self.find_replace_one_impl();
                } else {
                    self.step_find_bar(1);
                }
            }
            Overlays::FindInAll(_) => self.confirm_find_in_all(),
            Overlays::Palette(_) => self.confirm_palette(),
            Overlays::QuickOpen(_) => self.confirm_quick_open(),
            Overlays::GotoLine(_) => self.confirm_goto_line(),
            Overlays::GotoHeading(_) => self.confirm_goto_heading(),
            Overlays::FontPicker(_) => self.confirm_font_picker(),
            Overlays::ThemePicker(_) => self.confirm_theme_picker(),
            Overlays::TabSwitcher(_) => self.confirm_tab_switcher(),
            Overlays::SlashPalette(_) => self.confirm_slash_palette(),
            Overlays::HexPicker(_) => self.confirm_hex_picker(),
            Overlays::PreviousBufferBrowser(_) => self.confirm_previous_buffer_browser(),
            Overlays::Idle => {}
        }
    }

    /// Phase F3 — commit the hex picker. Validates the digit count, then
    /// re-dispatches `markdown.color_selection` with the entered hex as
    /// the JSON arg so the wrap path is identical to a pre-supplied-arg
    /// invocation. Dismisses the overlay regardless of commit outcome —
    /// an invalid commit leaves the overlay state stable but signals the
    /// user via the existing banner pattern.
    fn confirm_hex_picker(&mut self) {
        let digits = match self.overlays.hex_picker() {
            Some(hp) if hp.can_commit() => hp.digits().to_string(),
            _ => {
                // Invalid digit count — keep the overlay open so the
                // user can fix the input. Falls through without
                // dismissing.
                return;
            }
        };
        self.dismiss_overlay_and_blur();
        // Wrap directly via the Window impl so we bypass the registry
        // (the registry hook on `markdown.color_selection` would
        // re-route us back here on `None` prefill). Calling the impl
        // with `Some(hex)` is the supported "skip the picker" path.
        let _ = self.markdown_color_selection_impl(Some(&digits));
        self.request_repaint();
    }

    /// §H6 — commit the highlighted tab. The preview path already set
    /// the group's `active` to this tab without touching MRU; here we
    /// re-promote via `Group::activate` to refresh the MRU stack, then
    /// dismiss the overlay.
    fn confirm_tab_switcher(&mut self) {
        let Some(tab) = self
            .overlays
            .tab_switcher()
            .and_then(|t| t.selected_row().map(|r| r.tab_id))
        else {
            self.dismiss_overlay_and_blur();
            return;
        };
        let focused = self.tree.focused;
        if let Some(group) = self.tree.groups.get_mut(&focused) {
            group.activate(tab);
        }
        self.adopt_focused_tab();
        self.dismiss_overlay_and_blur();
        self.request_repaint();
    }

    /// §H6 — preview path: switch the focused group's active to `tab`
    /// without mutating its MRU stack, then re-sync the window's
    /// scalar buffer state.
    pub(crate) fn preview_tab_via_switcher(&mut self, tab: crate::pane_tree::TabId) {
        let focused = self.tree.focused;
        let applied = self
            .tree
            .groups
            .get_mut(&focused)
            .map(|g| g.set_active_for_preview(tab))
            .unwrap_or(false);
        if applied {
            self.adopt_focused_tab();
            self.request_repaint();
        }
    }

    /// §H6 — Esc cancel path: restore the focused group's active to
    /// `original` (the tab that was active when the overlay opened).
    pub(crate) fn restore_tab_after_tab_switcher(&mut self, original: crate::pane_tree::TabId) {
        let focused = self.tree.focused;
        let applied = self
            .tree
            .groups
            .get_mut(&focused)
            .map(|g| g.set_active_for_preview(original))
            .unwrap_or(false);
        if applied {
            self.adopt_focused_tab();
            self.request_repaint();
        }
    }

    /// §E4 — commit the currently-previewed theme: persist the selected
    /// entry to `settings.toml` so every open window (and the next
    /// launch) picks it up through the settings file-watcher's
    /// `ConfigEvent::Settings` echo. Dismiss without reverting; the
    /// preview already left this window showing the selection, so the
    /// reload that follows is visually idempotent in the source
    /// window. Returns immediately on empty filter / no highlighted
    /// row — that case has nothing to commit.
    fn confirm_theme_picker(&mut self) {
        let entry = self
            .overlays
            .theme_picker()
            .and_then(|tp| tp.selected_entry().cloned());
        self.dismiss_overlay_and_blur();
        if let Some(entry) = entry {
            self.commit_theme_entry(&entry);
        }
    }

    /// §E3: commit the highlighted family. Two things have to happen
    /// here: (1) the runtime font has to actually change to the
    /// highlighted row (the picker's preview path only fires on
    /// arrow-step, so a typed-and-Enter flow never updated the runtime),
    /// and (2) the choice has to be written back to `settings.toml` so
    /// it survives relaunch.
    ///
    /// The runtime change is routed through [`Window::request_font_change`]
    /// rather than the instant-swap [`Window::set_font_family`] used by
    /// preview. The request defers the swap until the projection worker
    /// has a display map built for the new font, so the body never paints
    /// new glyphs against old wrap break points. See `window_font_swap`.
    /// Persistence runs immediately (the user's *intent* is durable
    /// regardless of when the swap visually lands).
    fn confirm_font_picker(&mut self) {
        let highlighted = self
            .overlays
            .font_picker()
            .and_then(|fp| fp.selected_family().map(str::to_owned));
        if let Some(family) = highlighted {
            self.request_font_change(Some(family), None);
        }
        let persist_family = self
            .pending_font_change
            .as_ref()
            .map(|p| p.target_family.clone())
            .unwrap_or_else(|| self.prose_font_family.clone());
        self.persist_string_or_log("editor", "font_family_prose", &persist_family);
        self.dismiss_overlay_and_blur();
    }

    fn confirm_find_in_all(&mut self) {
        let Some(row) = self
            .overlays
            .find_in_all_mut()
            .and_then(|f| f.selected_row().cloned())
        else {
            return;
        };
        self.save_current_right_edge_chrome_state();
        self.buffer_id = row.buffer_id;
        self.sync_focused_tab_buffer();
        self.apply_right_edge_chrome_for_current_view();
        self.clear_right_edge_layout_caches();
        let Some(snap) = self.editor.snapshot(row.buffer_id) else {
            return;
        };
        let rope = snap.rope_snapshot().rope().clone();
        let Ok(start) = Position::from_byte_offset(&rope, row.start_byte) else {
            return;
        };
        let Ok(end) = Position::from_byte_offset(&rope, row.end_byte) else {
            return;
        };
        let _ = self.editor.set_selections(
            row.buffer_id,
            vec![continuity_text::Selection::new(
                start,
                end,
                continuity_text::SelectionKind::Caret,
            )],
        );
        self.dismiss_overlay_and_blur();
    }

    fn confirm_palette(&mut self) {
        // E2: when the math preview row is selected, Enter inserts the
        // formatted result at the caret (using the same `insert_text`
        // Context path normal typing uses) and dismisses the palette.
        if let Some(value) = self.overlays.palette_mut().and_then(|p| {
            p.math_row_selected()
                .then(|| p.math_preview.as_ref().map(|m| m.value))
                .flatten()
        }) {
            let text = crate::palette_math::format_value(value);
            self.dismiss_overlay_and_blur();
            let _ = continuity_command::Context::insert_text(self, &text);
            return;
        }
        let Some(entry) = self
            .overlays
            .palette_mut()
            .and_then(|p| p.selected_entry().cloned())
        else {
            return;
        };
        if !entry.applicable {
            return;
        }
        // δ.2 — stamp recency on the window-level map (the palette is
        // about to be dismissed and dropped, so writing only into the
        // palette's own state would lose the use). The palette helper
        // is also called so the in-overlay state stays consistent if
        // any code path inspects it before the dismiss completes.
        self.palette_recency_tick = self.palette_recency_tick.saturating_add(1);
        self.palette_command_recency
            .insert(entry.command.clone(), self.palette_recency_tick);
        if let Some(p) = self.overlays.palette_mut() {
            p.note_command_used(&entry.command);
        }
        self.dismiss_overlay_and_blur();
        let _ = Window::dispatch_command(self, &entry.command, &serde_json::Value::Null);
    }

    /// E2: copy the math preview result to the clipboard when the
    /// palette is open and a math preview is active. Returns `true`
    /// when handled (the input controller suppresses default Ctrl+C).
    pub(crate) fn palette_math_copy(&mut self) -> bool {
        let Some(value) = self
            .overlays
            .palette_mut()
            .and_then(|p| p.math_preview.as_ref().map(|m| m.value))
        else {
            return false;
        };
        let text = crate::palette_math::format_value(value);
        let _ = self.put_clipboard_text(&text);
        true
    }

    fn confirm_quick_open(&mut self) {
        let Some(entry) = self
            .overlays
            .quick_open_mut()
            .and_then(|q| q.selected_entry().cloned())
        else {
            return;
        };
        self.save_current_right_edge_chrome_state();
        self.buffer_id = entry.id;
        self.sync_focused_tab_buffer();
        self.apply_right_edge_chrome_for_current_view();
        self.clear_right_edge_layout_caches();
        self.dismiss_overlay_and_blur();
        // Phase B6: cross-buffer jump → unconditional glow.
        self.maybe_trigger_jump_glow(None);
    }

    fn confirm_goto_line(&mut self) {
        let Some(target) = self.overlays.goto_line_mut().and_then(|g| g.target()) else {
            return;
        };
        let from_line = self.capture_caret_line_for_jump();
        let _ = self.editor.set_selections(
            self.buffer_id,
            vec![continuity_text::Selection::caret_at(Position::new(
                target.0, target.1,
            ))],
        );
        self.dismiss_overlay_and_blur();
        self.maybe_trigger_jump_glow(from_line);
    }

    fn confirm_goto_heading(&mut self) {
        let Some(entry) = self
            .overlays
            .goto_heading_mut()
            .and_then(|g| g.selected_entry().cloned())
        else {
            return;
        };
        let from_line = self.capture_caret_line_for_jump();
        let _ = self.editor.set_selections(
            self.buffer_id,
            vec![continuity_text::Selection::caret_at(Position::new(
                entry.line, 0,
            ))],
        );
        self.dismiss_overlay_and_blur();
        self.maybe_trigger_jump_glow(from_line);
    }
}
