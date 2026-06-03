//! View-options, theme, and font command surface, factored out as a
//! super-trait of [`crate::Context`] so the main `Context` trait stays
//! under the file cap.
//!
//! Every method has a default `Err(Error::UnsupportedContext("…"))`
//! implementation so test stubs can `impl ViewContext for X {}`.
//!
//! Layer note: this trait is consumed by the view-command handlers in
//! [`crate::view`]. Because [`crate::Context`] inherits from `ViewContext`,
//! handlers continue to take `&mut dyn Context` — the super-trait
//! relationship makes these methods callable on any `Context` reference.

#[macro_use]
mod timeline_metrics;
#[macro_use]
mod table_methods;

use crate::Error;

/// View-toggle, theme, and font command surface.
///
/// Implementors are expected to be UI-thread state holders (typically
/// `ui::Window`); each method mutates per-pane runtime state and triggers
/// an invalidate for the next paint.
pub trait ViewContext {
    /// Cycle theme mode (`dark` → `light` → `system` → `dark`). Returns
    /// `UnsupportedContext` when no theme system is wired.
    fn cycle_theme(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("cycle_theme"))
    }

    /// Re-read the active theme TOML and re-render. Returns
    /// `UnsupportedContext` when unsupported, or any theme-load error.
    fn reload_theme(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("reload_theme"))
    }

    /// Capture a layout/system diagnostic snapshot to disk.
    fn capture_layout_diagnostics(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("capture_layout_diagnostics"))
    }

    /// Open `ChooseFontW` and apply the chosen prose family.
    fn pick_font_family(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("pick_font_family"))
    }

    /// Open the palette in theme-picker mode.
    fn pick_theme(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("pick_theme"))
    }

    /// δ.5 — clone the currently active theme into a new editable custom
    /// theme. `name` is the user-supplied target name; `None` means the
    /// implementor should auto-name (typically `<active>-copy`) and
    /// open the name-prompt overlay for confirmation.
    fn theme_clone_active(&mut self, _name: Option<&str>) -> Result<(), Error> {
        Err(Error::UnsupportedContext("theme_clone_active"))
    }

    /// δ.5 — open a theme's TOML as an editable buffer. `None` targets
    /// the active theme; `Some(name)` targets a specific row. Bundled
    /// themes surface a clone-first banner instead.
    fn theme_edit(&mut self, _name: Option<&str>) -> Result<(), Error> {
        Err(Error::UnsupportedContext("theme_edit"))
    }

    /// δ.5 — clone any installed theme into a new editable custom theme.
    /// `source = None` falls back to the active theme; `new_name = None`
    /// opens the name-prompt overlay.
    fn theme_duplicate(
        &mut self,
        _source: Option<&str>,
        _new_name: Option<&str>,
    ) -> Result<(), Error> {
        Err(Error::UnsupportedContext("theme_duplicate"))
    }

    /// δ.5 — rename a custom theme on disk. `old = None` targets the
    /// active custom theme. Updates `[ui] theme_dark` / `theme_light` in
    /// `settings.toml` if the renamed theme is currently bound.
    fn theme_rename(&mut self, _old: Option<&str>, _new_name: Option<&str>) -> Result<(), Error> {
        Err(Error::UnsupportedContext("theme_rename"))
    }

    /// δ.5 — soft-delete a custom theme. `name = None` targets the
    /// active custom theme. Moves the file under `themes/.trash/`.
    fn theme_delete(&mut self, _name: Option<&str>) -> Result<(), Error> {
        Err(Error::UnsupportedContext("theme_delete"))
    }

    /// δ.5 — open the user's themes directory in the OS shell
    /// (Explorer on Windows). Escape hatch for copy-between-machines.
    fn theme_reveal_folder(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("theme_reveal_folder"))
    }

    /// δ.5 — write a minimal valid theme to disk (every required key
    /// populated from the neutral fallback) and open it for editing.
    /// `name = None` opens the name-prompt overlay.
    fn theme_create_blank(&mut self, _name: Option<&str>) -> Result<(), Error> {
        Err(Error::UnsupportedContext("theme_create_blank"))
    }

    /// Set the prose font size in DIPs.
    fn set_font_size(&mut self, _size_dip: f32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("set_font_size"))
    }

    /// Toggle gutter line numbers. Returns `UnsupportedContext` when
    /// unsupported.
    fn toggle_line_numbers(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_line_numbers"))
    }

    /// Toggle relative gutter line numbers. Returns `UnsupportedContext`
    /// when unsupported.
    fn toggle_relative_line_numbers(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_relative_line_numbers"))
    }

    /// Toggle all visible gutter line numbers. Returns `UnsupportedContext`
    /// when unsupported.
    fn toggle_all_line_numbers(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_all_line_numbers"))
    }

    /// Toggle current-line highlight. Returns `UnsupportedContext` when
    /// unsupported.
    fn toggle_current_line_highlight(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_current_line_highlight"))
    }

    /// Toggle indent-guide rules. Returns `UnsupportedContext` when
    /// unsupported.
    fn toggle_indent_guides(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_indent_guides"))
    }

    /// Toggle whitespace-marker glyphs. Returns `UnsupportedContext` when
    /// unsupported.
    fn toggle_whitespace_markers(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_whitespace_markers"))
    }

    /// Toggle trailing-whitespace fill. Returns `UnsupportedContext` when
    /// unsupported.
    fn toggle_trailing_whitespace(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_trailing_whitespace"))
    }

    /// Toggle minimap rendering. Returns `UnsupportedContext` when
    /// unsupported.
    fn toggle_minimap(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_minimap"))
    }

    /// Toggle the sticky heading breadcrumb at the top of every pane.
    fn toggle_sticky_breadcrumb(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_sticky_breadcrumb"))
    }

    /// Toggle the right-docked markdown outline sidebar.
    fn toggle_outline(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_outline"))
    }

    /// Insert a hyperlinked table of contents at the caret
    /// (bounded by `<!-- toc -->` / `<!-- /toc -->` markers so a later
    /// [`Self::markdown_refresh_toc`] call can find it). Returns
    /// `UnsupportedContext` when unsupported.
    fn markdown_insert_toc(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("markdown_insert_toc"))
    }

    /// Refresh the TOC block in the active buffer in place
    /// (one undo group). Looks for the marker pair from a prior
    /// [`Self::markdown_insert_toc`]; no-ops if no TOC is present.
    fn markdown_refresh_toc(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("markdown_refresh_toc"))
    }

    /// Wrap the primary selection in `==…==` (default highlight).
    fn markdown_highlight_selection(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("markdown_highlight_selection"))
    }

    /// Open the hex-input palette and wrap the primary selection in `{#rrggbb:…}` once a
    /// valid hex is committed. `prefill` carries an optional initial
    /// value (e.g. the last picked color) for the palette input field.
    fn markdown_color_selection(&mut self, _prefill: Option<&str>) -> Result<(), Error> {
        Err(Error::UnsupportedContext("markdown_color_selection"))
    }

    /// Unwrap the inline-color markup (highlight or hex)
    /// surrounding the primary caret. No-ops when the caret isn't
    /// inside such a markup span.
    fn markdown_clear_inline_color(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("markdown_clear_inline_color"))
    }

    /// Open the rows × cols prompt and insert a column-
    /// aligned markdown table skeleton at the caret. `rows` and
    /// `cols` arrive from the palette-mode prompt once the user
    /// commits.
    fn markdown_insert_table(&mut self, _rows: u32, _cols: u32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("markdown_insert_table"))
    }

    view_context_table_methods!();

    /// Scroll the focused pane so the heading line at the
    /// supplied source-byte offset is brought into view. Used by the
    /// breadcrumb click handler to jump to a clicked segment. Returns
    /// `UnsupportedContext` when unsupported.
    fn scroll_to_byte(&mut self, _byte: u32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("scroll_to_byte"))
    }

    /// Replace the per-pane ruler-column list. Returns
    /// `UnsupportedContext` when unsupported.
    fn set_ruler_columns(&mut self, _columns: Vec<u32>) -> Result<(), Error> {
        Err(Error::UnsupportedContext("set_ruler_columns"))
    }

    /// Cycle caret style (`bar` → `block` → `underline` → `bar`).
    /// Returns `UnsupportedContext` when unsupported.
    fn cycle_caret_style(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("cycle_caret_style"))
    }

    /// Set `[editor].indent_type` at runtime and persist it.
    /// `use_spaces == true` selects spaces, `false` selects tabs.
    /// Returns `UnsupportedContext` when unsupported.
    fn set_indent_type(&mut self, _use_spaces: bool) -> Result<(), Error> {
        Err(Error::UnsupportedContext("set_indent_type"))
    }

    /// Set `[editor].indent_width` to an explicit value (clamped to
    /// `1..=16`) and persist it. Returns `UnsupportedContext` when
    /// unsupported.
    fn set_indent_width(&mut self, _width: u32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("set_indent_width"))
    }

    /// Adjust `[editor].indent_width` by `delta` columns (result clamped
    /// to `1..=16`) and persist it. Returns `UnsupportedContext` when
    /// unsupported.
    fn adjust_indent_width(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("adjust_indent_width"))
    }

    /// Set `[editor].tab_width` to an explicit value (clamped to
    /// `1..=16`) and persist it. Returns `UnsupportedContext` when
    /// unsupported.
    fn set_tab_width(&mut self, _width: u32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("set_tab_width"))
    }

    /// Adjust `[editor].tab_width` by `delta` columns (result clamped to
    /// `1..=16`) and persist it. Returns `UnsupportedContext` when
    /// unsupported.
    fn adjust_tab_width(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("adjust_tab_width"))
    }

    /// Toggle DirectWrite typography ligatures. Returns
    /// `UnsupportedContext` when unsupported.
    fn toggle_ligatures(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_ligatures"))
    }

    /// Open the user's `settings.toml` in the OS-default text editor
    /// (Phase 12 `settings.open` command). Returns `UnsupportedContext`
    /// when no settings path is wired.
    fn open_settings(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("open_settings"))
    }

    // ---- Phase 13 — pane / tab manipulation ----

    /// Split the focused pane horizontally (side-by-side columns).
    fn pane_split_horizontal(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("pane_split_horizontal"))
    }

    /// Split the focused pane vertically (stacked rows).
    fn pane_split_vertical(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("pane_split_vertical"))
    }

    /// Close the focused pane (its tabs flow into recently-closed).
    fn pane_close(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("pane_close"))
    }

    /// Move focus to the leaf pane immediately left.
    fn pane_focus_left(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("pane_focus_left"))
    }

    /// Move focus to the leaf pane immediately right.
    fn pane_focus_right(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("pane_focus_right"))
    }

    /// Move focus to the leaf pane immediately above.
    fn pane_focus_up(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("pane_focus_up"))
    }

    /// Move focus to the leaf pane immediately below.
    fn pane_focus_down(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("pane_focus_down"))
    }

    /// Toggle maximize-within-window for the focused pane.
    fn pane_maximize_toggle(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("pane_maximize_toggle"))
    }

    /// Resize the focused pane along `axis` (`"horizontal"`/`"vertical"`)
    /// by `delta` DIPs.
    fn pane_resize(&mut self, _axis: &str, _delta_dip: f32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("pane_resize"))
    }

    /// Apply one of the spec §6 layout shortcuts (`1..=5,8`).
    fn apply_layout_shortcut(&mut self, _shortcut: u32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("apply_layout_shortcut"))
    }

    /// Apply the "two rows" layout shortcut (Ctrl+Alt+Shift+2).
    fn apply_layout_two_rows(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("apply_layout_two_rows"))
    }

    /// Open a fresh empty buffer as a new tab in the focused pane.
    fn tab_new(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("tab_new"))
    }

    /// Close the active tab in the focused pane.
    fn tab_close(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("tab_close"))
    }

    /// Step to the next positional tab (wraps).
    fn tab_next(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("tab_next"))
    }

    /// Step to the previous positional tab (wraps).
    fn tab_prev(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("tab_prev"))
    }

    /// MRU step (Ctrl+Tab semantics).
    fn tab_step_mru(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("tab_step_mru"))
    }

    /// Activate the 1-indexed positional tab.
    fn tab_go_to(&mut self, _one_indexed: u32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("tab_go_to"))
    }

    /// Reopen the most-recently-closed tab in the focused pane.
    fn tab_reopen_closed(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("tab_reopen_closed"))
    }

    /// δ.1 — flip the active tab's pinned flag. Pinned tabs render
    /// leftmost (across the tab strip), prefix with a pin glyph, and
    /// are exempt from any future close-others / mass-close action.
    fn tab_pin_toggle(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("tab_pin_toggle"))
    }

    // ---- Phase 16 — clipboard, paste history, spell-check ----

    /// Copy the primary selection's source text to the OS clipboard and
    /// then delete it (Ctrl+X).
    fn cut_selection(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("cut_selection"))
    }

    /// Copy the primary selection's source text to the OS clipboard
    /// (Ctrl+C). Records the entry into the paste-history ring.
    fn copy_selection(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("copy_selection"))
    }

    /// Paste the OS clipboard's `CF_UNICODETEXT` payload at the active
    /// caret(s) (Ctrl+V).
    fn paste_clipboard(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("paste_clipboard"))
    }

    /// Paste the OS clipboard as plain text (Ctrl+Shift+V): explicitly
    /// skips the smart-paste URL transform and the clipboard-image
    /// import that `paste_clipboard` (Ctrl+V) performs, inserting the
    /// raw `CF_UNICODETEXT` payload verbatim.
    fn paste_as_plain_text(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("paste_as_plain_text"))
    }

    /// Open the paste-history overlay; or, when an `index` is provided,
    /// paste that history entry directly without UI.
    fn paste_from_history(&mut self, _index: Option<usize>) -> Result<(), Error> {
        Err(Error::UnsupportedContext("paste_from_history"))
    }

    /// δ.1 — copy the caret's current line (including its trailing
    /// newline) to the OS clipboard. Independent of selection state:
    /// if the user has nothing selected, this is the "yank the line I'm
    /// on" command. With a selection, the same line under the caret is
    /// still what gets copied — symmetric with vim's `yy`.
    fn copy_caret_line(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("copy_caret_line"))
    }

    /// δ.1 — jump back to the most-recently-edited position in the
    /// active buffer. Per-buffer history; repeated invocations walk
    /// further back through the stack. No-op when the stack is empty.
    fn goto_last_edit(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("goto_last_edit"))
    }

    /// Toggle spell-check on the active buffer.
    fn spell_toggle(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("spell_toggle"))
    }

    /// Replace the misspelled word under the caret with `with`. Lands as
    /// one undo group.
    fn spell_replace_at_caret(&mut self, _with: &str) -> Result<(), Error> {
        Err(Error::UnsupportedContext("spell_replace_at_caret"))
    }

    /// Add the word under the caret to the user's session ignore list.
    fn spell_add_to_dictionary(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("spell_add_to_dictionary"))
    }

    /// Pop the spell-check suggestion menu at the caret position.
    fn spell_show_suggestions(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("spell_show_suggestions"))
    }

    /// Outer screen rect of the current window in pixels — `(x, y, w, h)`.
    /// Used by `window.new_window` / tear-off to cascade the new window
    /// from the focused one. `None` for headless / test contexts.
    fn current_window_rect(&self) -> Option<(i32, i32, i32, i32)> {
        None
    }

    /// Smart-home (spec §12): toggle the caret between column 0 and the
    /// line's first non-whitespace byte. `Home` lands on first-non-ws;
    /// pressing it again jumps to column 0.
    fn smart_home(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("smart_home"))
    }

    /// Smart-home extending: same as [`smart_home`](Self::smart_home) but
    /// the selection's `anchor` stays put and the `head` moves.
    fn extend_smart_home(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("extend_smart_home"))
    }

    // ---- Phase H1 — granular focus mode ----

    /// §H1: set the focus-mode dim pass on the active pane.
    ///
    /// `mode` is one of `"off" | "line" | "sentence" | "paragraph"`. The
    /// default impl errors so headless contexts don't need to track it.
    fn set_focus_mode(&mut self, _mode: &str) -> Result<(), Error> {
        Err(Error::UnsupportedContext("set_focus_mode"))
    }

    /// §H1: cycle `off → line → sentence → paragraph → off`.
    fn cycle_focus_mode(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("cycle_focus_mode"))
    }

    // ---- Phase H2 — distraction-free mode ----

    /// §H2: toggle distraction-free mode for the focused window. Hides tab
    /// strip, status bar, pane borders, scroll bar, gutter; centers body
    /// at `focus.max_column_width`; dims non-current paragraphs. Hot-
    /// toggling returns the chrome state to its pre-toggle snapshot.
    fn toggle_distraction_free_mode(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_distraction_free_mode"))
    }

    // ---- Phase H3 — indent-folding ----

    /// §H3: fold the indent subtree at the caret. The foldable region
    /// runs from the caret's line N through every following line whose
    /// indent is deeper than N's (blanks absorbed).
    fn fold_at_caret(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("fold_at_caret"))
    }

    /// §H3: unfold whatever the caret currently sits inside (or on top
    /// of). No-op when no fold spans the caret.
    fn unfold_at_caret(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("unfold_at_caret"))
    }

    /// §H3: fold every top-level indent block in the buffer.
    fn fold_all(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("fold_all"))
    }

    /// §H3: unfold every fold in the buffer.
    fn unfold_all(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("unfold_all"))
    }

    /// §H3: toggle the indent-fold at the caret. Folds if currently
    /// unfolded; unfolds if currently folded.
    fn toggle_fold_at_caret(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_fold_at_caret"))
    }

    // ---- Phase H5 — slash-command palette ----

    /// §H5: open the slash-command palette docked at the caret. The
    /// palette filters by characters typed after the leading `/`; only
    /// commands flagged `palette_safe = true` (registry A7) appear.
    fn show_slash_palette(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("show_slash_palette"))
    }

    // ---- Phase H6 — Ctrl+Tab transient overlay ----

    /// §H6: open the Ctrl+Tab positional-order tab overlay. Released
    /// on Ctrl-up. Fast swaps (release < 600 ms) do not show this
    /// overlay; this command is the discoverable equivalent for users
    /// who want it on demand.
    fn show_tab_overlay(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("show_tab_overlay"))
    }

    view_context_timeline_metrics_methods!();
}
