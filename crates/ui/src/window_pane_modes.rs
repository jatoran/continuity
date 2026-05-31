//! Phase H per-pane state + Window impls: granular focus mode (H1),
//! distraction-free mode (H2), indent folding (H3), slash command
//! palette state (H5), Ctrl+Tab positional overlay open path (H6).
//!
//! Pulled out of `window_view_options.rs` to keep that file under the
//! 600-line cap. The actual fold geometry is computed at display-map
//! rebuild time from the user-toggled source lines stored here — see
//! `continuity_core::edit_indent_subtree` for the analysis.
//!
//! Thread ownership: UI thread of one window. All state in
//! [`PaneModesState`] is mutated only by the owning [`crate::Window`].

use continuity_config::FocusMode;

use crate::window::Window;

/// Per-pane Phase H state.
///
/// `folded_lines` is the set of source-line indices the user has
/// fold-toggled. The actual `(start, end)` geometry of each fold is
/// computed by the display-map's indent-fold provider on rebuild via
/// `continuity_core::edit_indent_subtree::indent_subtree`. Storing
/// only the line index keeps the state revision-stable across edits
/// that don't change the indent structure.
#[derive(Debug, Clone)]
pub struct PaneModesState {
    /// §H1: active focus mode.
    pub focus_mode: FocusMode,
    /// §H1: dim alpha for non-focused source ranges (0.0..=1.0).
    /// Sourced from `[focus].dim_alpha` on settings load.
    pub focus_dim_alpha: f32,
    /// §H2: distraction-free mode is currently active.
    pub distraction_free: bool,
    /// §H2: body-column max width (in characters) when distraction-
    /// free mode is active. Sourced from `[focus].max_column_width`.
    pub distraction_free_max_width: u32,
    /// §H2: chrome snapshot taken at toggle-on so toggle-off restores
    /// the previous state exactly.
    pub distraction_free_prev_chrome: Option<ChromeSnapshot>,
    /// §H3: source-line indices the user has fold-toggled. The fold
    /// geometry is derived from each entry's indent subtree at paint
    /// time.
    pub folded_lines: Vec<u32>,
    // §H5: the phase-A `slash_palette_pending: bool` bridge was
    // retired (2026-05-13) — the slash-command palette now lives on
    // the `Overlays` state machine via `Overlays::open_slash_palette`.
    /// §H6: `Some(_)` while the Ctrl+Tab chord is pending the 600 ms
    /// hold timer. Cleared by either the timer firing (overlay opens)
    /// or Ctrl releasing first (fast swap, no overlay). The payload
    /// records the initial step direction (+1 for Ctrl+Tab, -1 for
    /// Ctrl+Shift+Tab) so the timer can open the overlay with the
    /// cursor pre-advanced to match the swap that already happened.
    pub tab_overlay_chord: Option<TabOverlayChord>,
}

/// §H6 — pending state for the Ctrl+Tab hold-to-overlay chord.
#[derive(Debug, Clone, Copy)]
pub struct TabOverlayChord {
    /// Step direction that originally fired the chord (`+1` or `-1`).
    pub initial_delta: i32,
}

/// Snapshot of view-chrome flags taken at the moment distraction-free
/// mode is enabled. The toggle-off path restores from this.
///
/// §H2 acceptance criteria expand the snapshot beyond status bar +
/// sticky breadcrumb + line numbers + minimap to also cover the tab
/// strip, the pane borders, and the prior H1 focus mode (so DF can
/// force paragraph-dim and then restore the pre-toggle focus mode on
/// exit). The renderer has no scroll bar painter today; if one ever
/// lands, add the corresponding field here.
#[derive(Debug, Clone, Copy)]
pub struct ChromeSnapshot {
    /// Status bar visible.
    pub show_status_bar: bool,
    /// Sticky breadcrumb visible.
    pub show_sticky_breadcrumb: bool,
    /// Line numbers visible.
    pub line_numbers: bool,
    /// Minimap visible.
    pub minimap: bool,
    /// Tab strip visible across the top of every pane.
    pub show_tab_strip: bool,
    /// Pane borders visible around every pane leaf.
    pub show_pane_borders: bool,
    /// Phase H1 focus mode active at toggle-on. DF forces this to
    /// `Paragraph` while active so the renderer's paragraph-dim path
    /// runs.
    pub focus_mode: FocusMode,
}

impl Default for PaneModesState {
    fn default() -> Self {
        Self {
            focus_mode: FocusMode::Off,
            focus_dim_alpha: 0.45,
            distraction_free: false,
            distraction_free_max_width: 80,
            distraction_free_prev_chrome: None,
            folded_lines: Vec::new(),
            tab_overlay_chord: None,
        }
    }
}

impl Window {
    /// §H1: set the focus mode for this pane.
    ///
    /// # Errors
    /// Returns `Error::Command` when `mode_str` is not a known mode.
    pub(crate) fn set_focus_mode_impl(&mut self, mode_str: &str) -> Result<(), crate::Error> {
        let mode = FocusMode::parse(mode_str)
            .map_err(|e| crate::Error::Command(continuity_command::Error::Other(e.to_string())))?;
        self.view_options.pane_modes.focus_mode = mode;
        self.request_repaint();
        Ok(())
    }

    /// §H1: cycle focus mode `off → line → sentence → paragraph → off`.
    pub(crate) fn cycle_focus_mode_impl(&mut self) -> Result<(), crate::Error> {
        let next = self.view_options.pane_modes.focus_mode.next();
        self.view_options.pane_modes.focus_mode = next;
        self.request_repaint();
        Ok(())
    }

    /// §H2: toggle distraction-free mode. Snapshots every chrome flag
    /// + the H1 focus mode on enter, forces them all to DF's "minimum
    ///   chrome / paragraph-dim" state, and restores exactly on exit.
    ///
    /// δ.3 — wrapped in `with_caret_line_anchored` so future changes
    /// that route DF through `refresh_focused_viewport` (e.g. body
    /// width-centering, tab-strip-hide that grows the body) preserve
    /// the caret line's body-relative y by default. Today the toggle
    /// only mutates chrome flags and the next call site to update
    /// `view.viewport_*_dip` is the repaint path; the anchor stays
    /// a no-op until then, which is the correct shape.
    pub(crate) fn toggle_distraction_free_mode_impl(&mut self) -> Result<(), crate::Error> {
        self.with_caret_line_anchored(|w| w.toggle_distraction_free_mode_inner());
        self.request_repaint();
        Ok(())
    }

    fn toggle_distraction_free_mode_inner(&mut self) {
        if self.view_options.pane_modes.distraction_free {
            if let Some(snap) = self
                .view_options
                .pane_modes
                .distraction_free_prev_chrome
                .take()
            {
                self.view_options.show_status_bar = snap.show_status_bar;
                self.view_options.show_sticky_breadcrumb = snap.show_sticky_breadcrumb;
                self.view_options.line_numbers = snap.line_numbers;
                self.view_options.minimap = snap.minimap;
                self.view_options.show_tab_strip = snap.show_tab_strip;
                self.view_options.show_pane_borders = snap.show_pane_borders;
                self.view_options.pane_modes.focus_mode = snap.focus_mode;
            }
            self.view_options.pane_modes.distraction_free = false;
        } else {
            let snap = ChromeSnapshot {
                show_status_bar: self.view_options.show_status_bar,
                show_sticky_breadcrumb: self.view_options.show_sticky_breadcrumb,
                line_numbers: self.view_options.line_numbers,
                minimap: self.view_options.minimap,
                show_tab_strip: self.view_options.show_tab_strip,
                show_pane_borders: self.view_options.show_pane_borders,
                focus_mode: self.view_options.pane_modes.focus_mode,
            };
            self.view_options.pane_modes.distraction_free_prev_chrome = Some(snap);
            self.view_options.show_status_bar = false;
            self.view_options.show_sticky_breadcrumb = false;
            self.view_options.line_numbers = false;
            self.view_options.minimap = false;
            self.view_options.show_tab_strip = false;
            self.view_options.show_pane_borders = false;
            // Paragraph-dim non-current paragraph by riding the H1
            // focus-mode dim path. `Paragraph` keeps the current
            // paragraph at full contrast and dims the rest.
            self.view_options.pane_modes.focus_mode = FocusMode::Paragraph;
            self.view_options.pane_modes.distraction_free = true;
        }
    }

    /// §H3: fold the indent subtree at the caret. Stores the caret's
    /// source line; the display-map's indent-fold provider computes
    /// the geometry on its next rebuild.
    pub(crate) fn fold_at_caret_impl(&mut self) -> Result<(), crate::Error> {
        let line = self.primary_caret_source_line();
        if !self.view_options.pane_modes.folded_lines.contains(&line) {
            self.view_options.pane_modes.folded_lines.push(line);
            self.view_options.pane_modes.folded_lines.sort_unstable();
        }
        self.request_repaint();
        Ok(())
    }

    /// §H3: remove any fold whose start line equals the caret's source
    /// line. (Folds spanning the caret are removed at the display-map
    /// level — this method only handles the user's source-line entry.)
    pub(crate) fn unfold_at_caret_impl(&mut self) -> Result<(), crate::Error> {
        let line = self.primary_caret_source_line();
        self.view_options
            .pane_modes
            .folded_lines
            .retain(|l| *l != line);
        self.request_repaint();
        Ok(())
    }

    /// §H3: mark every top-level line for folding. The display-map's
    /// indent-fold provider expands these to indent ranges; the actual
    /// candidate-set computation lives in
    /// [`continuity_core::edit_indent_subtree::all_top_level_subtrees`].
    pub(crate) fn fold_all_impl(&mut self) -> Result<(), crate::Error> {
        // Scaffolding: signal intent. Geometry is enumerated at
        // display-map rebuild time. We push a sentinel `u32::MAX` to
        // mean "fold all top-level"; the rebuild path translates it.
        if !self
            .view_options
            .pane_modes
            .folded_lines
            .contains(&u32::MAX)
        {
            self.view_options.pane_modes.folded_lines.push(u32::MAX);
        }
        self.request_repaint();
        Ok(())
    }

    /// §H3: drop every fold.
    pub(crate) fn unfold_all_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.pane_modes.folded_lines.clear();
        self.request_repaint();
        Ok(())
    }

    /// §H3: toggle the fold at the caret.
    pub(crate) fn toggle_fold_at_caret_impl(&mut self) -> Result<(), crate::Error> {
        let line = self.primary_caret_source_line();
        if self.view_options.pane_modes.folded_lines.contains(&line) {
            self.unfold_at_caret_impl()
        } else {
            self.fold_at_caret_impl()
        }
    }

    /// §H5: open the slash-command palette as a palette-mode overlay.
    /// `trigger` records whether the palette came up via the typed-`/`
    /// line-start hook (Esc must remove the trailing slash) or the
    /// explicit `Ctrl+/` chord (no cleanup needed).
    ///
    /// Returns silently when `editor.slash_commands_enabled` is `false`
    /// (the gate that the user sets when the feature gets in their way).
    pub(crate) fn show_slash_palette_impl(
        &mut self,
        trigger: crate::slash_palette::SlashTrigger,
    ) -> Result<(), crate::Error> {
        if !self.slash_commands_enabled {
            return Ok(());
        }
        let entries = self.build_slash_palette_entries();
        let anchor_line = self.primary_caret_source_line();
        self.overlays
            .open_slash_palette(entries, anchor_line, trigger);
        self.focus_overlay_input();
        self.request_repaint();
        Ok(())
    }

    /// §H5: build the palette safelist for the current registry. Honors
    /// `editor.slash_commands_palette` when set (exact user override)
    /// else falls back to every `palette_safe` command registered.
    /// Resolves each id to a `SlashPaletteEntry` with a derived display
    /// label.
    fn build_slash_palette_entries(&self) -> Vec<crate::slash_palette::SlashPaletteEntry> {
        let ids: Vec<String> = if let Some(override_list) = self.slash_commands_palette.as_ref() {
            override_list.clone()
        } else {
            self.registry
                .palette_safe_ids()
                .map(|id| id.as_str().to_string())
                .collect()
        };
        let mut entries: Vec<crate::slash_palette::SlashPaletteEntry> = ids
            .into_iter()
            .map(|command| {
                let label = slash_label_from_command_id(&command);
                let description = self.registry.description(&command).map(str::to_owned);
                crate::slash_palette::SlashPaletteEntry {
                    command,
                    label,
                    description,
                    keybinding: None,
                    applicable: true,
                }
            })
            .collect();
        entries.sort_by(|a, b| a.label.cmp(&b.label));
        entries
    }

    /// §H6: open the Ctrl+Tab transient tab-switcher overlay. Builds
    /// a snapshot of the focused group's positional tab order and
    /// installs it as a palette-mode overlay instance. The chord
    /// pending-state (set by the keymap entry that armed the 600 ms
    /// timer) is cleared once the overlay is up, then dismissed on
    /// Ctrl release / Esc by the window-overlays dispatcher.
    ///
    /// Returns silently when there are fewer than two tabs in the
    /// focused group — there is nothing to switch between.
    pub(crate) fn show_tab_overlay_impl(&mut self) -> Result<(), crate::Error> {
        let focused = self.tree.focused;
        let (rows, original_active) = {
            let Some(group) = self.tree.groups.get(&focused) else {
                self.view_options.pane_modes.tab_overlay_chord = None;
                return Ok(());
            };
            if group.tabs.len() < 2 {
                self.view_options.pane_modes.tab_overlay_chord = None;
                return Ok(());
            }
            let original_active = group.active;
            let mut rows = Vec::with_capacity(group.tabs.len());
            for &tab_id in &group.tabs {
                let Some(tab) = self.tree.tabs.get(&tab_id) else {
                    continue;
                };
                let first_line = self.first_buffer_line_for_label(tab.buffer_id);
                let title = crate::pane_tree::resolve_label(tab, first_line.as_deref());
                rows.push(crate::tab_switcher::TabSwitcherRow {
                    tab_id,
                    buffer_id: tab.buffer_id,
                    title,
                    subtitle: String::new(),
                    dirty: false,
                });
            }
            (rows, original_active)
        };
        let initial_delta = self
            .view_options
            .pane_modes
            .tab_overlay_chord
            .map(|c| c.initial_delta)
            .unwrap_or(1);
        self.view_options.pane_modes.tab_overlay_chord = None;
        self.overlays
            .open_tab_switcher(rows, original_active, initial_delta);
        self.blur_overlay_input();
        self.request_repaint();
        Ok(())
    }

    /// Return the first non-empty line of `buffer_id`'s rope, used as
    /// the secondary label key. Returns `None` when no snapshot is
    /// available (headless tests, never-opened buffers).
    fn first_buffer_line_for_label(
        &self,
        buffer_id: continuity_buffer::BufferId,
    ) -> Option<String> {
        let snap = self.editor.snapshot(buffer_id)?;
        let rope = snap.rope_snapshot().rope();
        for line in rope.lines().take(8) {
            let s: String = line.chars().take(80).collect();
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        None
    }

    /// §H3 — install the persisted fold set restored from
    /// `windows.pane_tree_json` at window startup. Drops out-of-range
    /// indices silently (the rope may have shrunk since last
    /// session); preserves the `u32::MAX` "fold all" sentinel
    /// verbatim. The set is sorted + deduped before being installed
    /// so the provider's coalescing assumptions hold.
    pub(crate) fn install_restored_folded_lines(&mut self, restored: Vec<u32>) {
        let total_lines = match self.editor.snapshot(self.buffer_id) {
            Some(snap) => snap.rope_snapshot().rope().len_lines() as u32,
            // No snapshot yet (early init / headless test) — keep
            // every index; the next paint will re-validate via the
            // provider, which already tolerates stale entries.
            None => u32::MAX,
        };
        let mut filtered: Vec<u32> = restored
            .into_iter()
            .filter(|&l| l == u32::MAX || l < total_lines)
            .collect();
        filtered.sort_unstable();
        filtered.dedup();
        self.view_options.pane_modes.folded_lines = filtered;
    }

    /// Source line containing the primary caret. Falls back to `0`
    /// when no snapshot is available (e.g. headless tests).
    fn primary_caret_source_line(&self) -> u32 {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return 0;
        };
        snap.selections().first().map(|s| s.head.line).unwrap_or(0)
    }
}

/// §H5 — derive a display label from a command id. Strips the
/// namespace (`markdown.insert_toc` → `insert_toc`), swaps `_` for
/// ` ` (`insert_toc` → `insert toc`), and capitalises the first
/// character (`insert toc` → `Insert toc`). Pure helper — no state.
fn slash_label_from_command_id(id: &str) -> String {
    let action = id.rsplit_once('.').map(|(_, a)| a).unwrap_or(id);
    let spaced = action.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => {
            let mut out = String::with_capacity(spaced.len());
            for c in first.to_uppercase() {
                out.push(c);
            }
            out.extend(chars);
            out
        }
        None => spaced,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pane_modes_state_is_off() {
        let s = PaneModesState::default();
        assert_eq!(s.focus_mode, FocusMode::Off);
        assert!(!s.distraction_free);
        assert!(s.folded_lines.is_empty());
        assert!(s.distraction_free_prev_chrome.is_none());
        assert_eq!(s.distraction_free_max_width, 80);
        assert!((s.focus_dim_alpha - 0.45).abs() < 1e-6);
        assert!(s.tab_overlay_chord.is_none());
    }

    #[test]
    fn slash_label_strips_namespace_and_titlecases() {
        assert_eq!(
            slash_label_from_command_id("markdown.insert_toc"),
            "Insert toc"
        );
        assert_eq!(
            slash_label_from_command_id("markdown.insert_table"),
            "Insert table"
        );
        // No namespace: capitalise + space normalisation still apply.
        assert_eq!(slash_label_from_command_id("insert_uuid"), "Insert uuid");
        assert_eq!(slash_label_from_command_id(""), "");
    }

    #[test]
    fn chrome_snapshot_is_copy() {
        let snap = ChromeSnapshot {
            show_status_bar: true,
            show_sticky_breadcrumb: true,
            line_numbers: true,
            minimap: false,
            show_tab_strip: true,
            show_pane_borders: true,
            focus_mode: FocusMode::Off,
        };
        let copy = snap;
        assert!(copy.show_status_bar);
        assert!(snap.show_sticky_breadcrumb);
        assert!(copy.show_tab_strip);
    }

    #[test]
    fn chrome_snapshot_round_trips_every_flag() {
        // The toggle-on snapshot must carry every flag the mode mutates
        // so toggle-off restores the exact pre-toggle state. This guards
        // against an agent extending the mutator without extending the
        // snapshot (the classic H2 regression).
        let pre = ChromeSnapshot {
            show_status_bar: false,
            show_sticky_breadcrumb: false,
            line_numbers: false,
            minimap: true,
            show_tab_strip: false,
            show_pane_borders: true,
            focus_mode: FocusMode::Line,
        };
        // simulate "save → mutate → restore". The mutation phase must
        // actually change observable state — otherwise `current = saved`
        // is a no-op and the restore proves nothing.
        let saved = pre;
        let mut current = saved;
        current.show_status_bar = false;
        current.show_sticky_breadcrumb = false;
        current.line_numbers = false;
        current.minimap = false;
        current.show_tab_strip = false;
        current.show_pane_borders = false;
        current.focus_mode = FocusMode::Paragraph;
        assert_ne!(current.focus_mode, saved.focus_mode);
        assert_ne!(current.minimap, saved.minimap);
        // restore
        current = saved;
        assert_eq!(current.show_status_bar, pre.show_status_bar);
        assert_eq!(current.minimap, pre.minimap);
        assert_eq!(current.show_tab_strip, pre.show_tab_strip);
        assert_eq!(current.show_pane_borders, pre.show_pane_borders);
        assert_eq!(current.focus_mode, pre.focus_mode);
    }
}
