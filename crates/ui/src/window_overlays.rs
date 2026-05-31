//! Per-overlay input dispatch + Context-overlay-method implementations for
//! [`crate::Window`].
//!
//! Split from `window_commanding.rs` to keep both files under the 600-line
//! cap. The overlay key handling matrix lives here; the `Context` overlay
//! methods delegate to the `Overlays` state machine and refresh derived
//! state (palette candidates, quick-open list, heading walks).

use continuity_input::KeyChord;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F3, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR,
    VK_RETURN, VK_RIGHT, VK_TAB, VK_UP,
};

use crate::overlays::Overlays;
use crate::Window;

/// δ.4 — virtual-key code for `T` (used by the previous-buffer
/// browser's `Ctrl+T` filter-cycle chord). The `windows` crate
/// surfaces letter VKs as numeric constants rather than enum
/// symbols, so we name them locally to keep the intercept readable.
const VK_LETTER_T: u16 = 0x54;
/// δ.4 — virtual-key code for `R` (Ctrl+R timeline-on-closed-buffer chord).
const VK_LETTER_R: u16 = 0x52;
const VK_LETTER_C: u16 = 0x43;
const VK_LETTER_P: u16 = 0x50;
const VK_LETTER_S: u16 = 0x53;
const VK_LETTER_W: u16 = 0x57;

impl Window {
    /// Route a typed character to the active overlay. Returns whether the
    /// overlay consumed the input (caller should invalidate the window).
    pub(crate) fn overlay_on_char(&mut self, ch: char) -> bool {
        if !self.overlays.is_active() {
            return false;
        }
        if (ch as u32) < 0x20 {
            return true;
        }
        match &mut self.overlays {
            Overlays::Idle => false,
            Overlays::Find(fb) => {
                fb.insert_char(ch);
                self.recompute_find_matches();
                true
            }
            Overlays::FindInAll(fia) => {
                fia.input.insert_char(ch);
                self.recompute_find_in_all();
                true
            }
            Overlays::Palette(p) => {
                p.input.insert_char(ch);
                p.refilter();
                true
            }
            Overlays::QuickOpen(q) => {
                q.input.insert_char(ch);
                q.refilter();
                true
            }
            Overlays::GotoLine(g) => {
                g.input.insert_char(ch);
                true
            }
            Overlays::GotoHeading(g) => {
                g.input.insert_char(ch);
                g.refilter();
                true
            }
            Overlays::FontPicker(fp) => {
                fp.input.insert_char(ch);
                fp.refilter();
                // §E3: don't preview here — only the highlight change
                // triggers a preview swap (throttle to one per row, not
                // per keystroke).
                true
            }
            Overlays::ThemePicker(tp) => {
                tp.input.insert_char(ch);
                tp.refilter();
                // §E4: same throttle rule — preview only on row change.
                true
            }
            // §H6 — the tab switcher has no text input. Swallow chars
            // so they don't fall through to the editor while Ctrl+Tab
            // is held; the chord routing in `on_keydown` is the only
            // way to step the selection.
            Overlays::TabSwitcher(_) => true,
            // §H5 — slash palette: typed chars filter the safelist
            // and promote out of the "Backspace dismisses" zero-typed
            // state.
            Overlays::SlashPalette(sp) => {
                sp.input.insert_char(ch);
                sp.note_filter_char();
                sp.refilter();
                true
            }
            // Phase F3 — hex picker accepts only `[0-9a-fA-F]`; every
            // other character is silently dropped by `insert_char`.
            Overlays::HexPicker(hp) => {
                hp.insert_char(ch);
                true
            }
            // δ.4 — previous-buffer browser: standard fuzzy filter
            // over the title column.
            Overlays::PreviousBufferBrowser(b) => {
                b.input.insert_char(ch);
                b.refilter();
                true
            }
        }
    }

    /// Route a key chord to the active overlay. Returns whether handled.
    pub(crate) fn overlay_on_keydown(&mut self, vk: u16, chord: &KeyChord) -> bool {
        if !self.overlays.is_active() {
            return false;
        }
        if vk == VK_ESCAPE.0 {
            // §E3: cancelling the font picker reverts to the family in
            // effect when the picker opened.
            if let Some(fp) = self.overlays.font_picker() {
                let family = fp.original_family.clone();
                self.set_font_family(family);
            }
            // §E4: cancelling the theme picker reverts the installed
            // `ThemeSet` to its open-time snapshot.
            if let Some(tp) = self.overlays.theme_picker() {
                let revert = tp.revert_set();
                self.active_theme.set_installed(revert);
                self.invalidate(self.hwnd);
            }
            // §H6: cancelling the tab switcher restores the tab that
            // was active when the overlay opened. (`activate` is fine
            // here — the original tab was the MRU front already, so
            // re-promoting it is a no-op for the stack.)
            if let Some(original) = self.overlays.tab_switcher().and_then(|t| t.original_active) {
                self.restore_tab_after_tab_switcher(original);
            }
            // §H5: cancelling the slash palette also removes the
            // trailing `/` when the trigger was the typed-`/` hook
            // (Backspace path leaves the slash; Esc cleans it up).
            let remove_slash = matches!(
                self.overlays.slash_palette(),
                Some(sp) if sp.trigger == crate::slash_palette::SlashTrigger::TypedSlash
            );
            self.overlays.dismiss();
            if remove_slash {
                self.delete_trailing_slash_from_typed_trigger();
            }
            self.blur_overlay_input();
            return true;
        }
        // δ.4 — chord intercepts that only apply while the
        // previous-buffer browser overlay is open. Ctrl+T cycles the
        // filter (Active → All → Trash); Ctrl+R opens the
        // time-machine slider against the highlighted closed buffer.
        if matches!(self.overlays, Overlays::PreviousBufferBrowser(_))
            && chord.modifiers.ctrl
            && !chord.modifiers.shift
            && !chord.modifiers.alt
        {
            if vk == VK_LETTER_T {
                self.cycle_previous_buffer_browser_filter();
                return true;
            }
            if vk == VK_LETTER_R {
                self.open_timeline_for_highlighted_closed_buffer();
                return true;
            }
        }
        if matches!(self.overlays, Overlays::Find(_)) {
            if vk == VK_RETURN.0 && chord.modifiers.ctrl && chord.modifiers.alt {
                let _ = self.find_replace_all_impl();
                return true;
            }
            if chord.modifiers.alt && !chord.modifiers.ctrl && !chord.modifiers.shift {
                let handled = match vk {
                    VK_LETTER_C => {
                        self.find_toggle_mode_impl("case");
                        true
                    }
                    VK_LETTER_W => {
                        self.find_toggle_mode_impl("word");
                        true
                    }
                    VK_LETTER_R => {
                        self.find_toggle_mode_impl("regex");
                        true
                    }
                    VK_LETTER_P => {
                        self.find_toggle_mode_impl("preserve");
                        true
                    }
                    VK_LETTER_S => {
                        self.find_toggle_mode_impl("scope");
                        true
                    }
                    v if v == VK_RETURN.0 => {
                        self.find_matches_to_cursors_impl();
                        true
                    }
                    _ => false,
                };
                if handled {
                    return true;
                }
            }
            if vk == VK_RETURN.0 && !chord.modifiers.alt {
                let replace_visible = self
                    .overlays
                    .find_bar()
                    .is_some_and(|fb| fb.replace_visible);
                if chord.modifiers.ctrl && chord.modifiers.shift {
                    if replace_visible {
                        let _ = self.find_replace_all_impl();
                    }
                    return true;
                }
                if chord.modifiers.ctrl {
                    if replace_visible {
                        let _ = self.find_replace_one_impl();
                    }
                    return true;
                }
                if chord.modifiers.shift {
                    self.step_find_bar(-1);
                    return true;
                }
            }
        }
        // F3 / Shift+F3 always work (scoped to the active find bar). The
        // command registry surface is also bound to the same chord.
        if vk == VK_F3.0 && self.overlays.find_bar().is_some() {
            self.step_find_bar(if chord.modifiers.shift { -1 } else { 1 });
            return true;
        }
        // δ.5 — theme-picker inline row actions. Ctrl+E edits the
        // highlighted theme, Ctrl+D duplicates it, Ctrl+Backspace
        // soft-deletes a custom theme. Bundled rows reject delete with a
        // banner so the picker stays open. Scoped strictly to when the
        // theme picker is the active overlay so the chords don't bleed
        // into any other input surface.
        if matches!(&self.overlays, crate::overlays::Overlays::ThemePicker(_))
            && chord.modifiers.ctrl
            && !chord.modifiers.alt
            && self.theme_picker_row_action(vk, chord.modifiers.shift)
        {
            return true;
        }
        match vk {
            v if v == VK_BACK.0 => {
                self.overlay_delete_back();
                true
            }
            v if v == VK_DELETE.0 => {
                self.overlay_delete_forward();
                true
            }
            v if v == VK_LEFT.0 => {
                self.overlay_caret_horizontal(-1);
                true
            }
            v if v == VK_RIGHT.0 => {
                self.overlay_caret_horizontal(1);
                true
            }
            v if v == VK_HOME.0 => {
                self.overlay_caret_home();
                true
            }
            v if v == VK_END.0 => {
                self.overlay_caret_end();
                true
            }
            v if v == VK_UP.0 || v == VK_PRIOR.0 => {
                self.overlay_step_selection(-1);
                true
            }
            v if v == VK_DOWN.0 || v == VK_NEXT.0 => {
                self.overlay_step_selection(1);
                true
            }
            v if v == VK_TAB.0 => {
                if let Some(fb) = self.overlays.find_bar_mut() {
                    fb.toggle_focus();
                    return true;
                }
                // §H6 — while the tab switcher overlay is visible,
                // `Tab` (with Ctrl still held) advances the cursor;
                // `Shift+Tab` walks it backwards. This mirrors the
                // expectation that the chord that opened the overlay
                // keeps driving its selection without forcing the
                // user to retrain onto arrow keys.
                if matches!(self.overlays, crate::overlays::Overlays::TabSwitcher(_)) {
                    let delta = if chord.modifiers.shift { -1 } else { 1 };
                    self.tab_switcher_step_via_chord(delta);
                    return true;
                }
                false
            }
            v if v == VK_RETURN.0 => {
                self.overlay_confirm();
                true
            }
            _ => false,
        }
    }

    fn overlay_delete_back(&mut self) {
        // §H5 — backspace at zero filter chars dismisses the slash
        // palette and leaves the literal `/` in source (Esc is the
        // path that *removes* the slash). Promote this check above
        // the standard text-input op so the trailing slash never
        // gets fed to `apply_input_op` on an empty buffer.
        if matches!(
            self.overlays.slash_palette(),
            Some(sp) if !sp.has_filter_chars
        ) {
            self.overlays.dismiss();
            self.blur_overlay_input();
            return;
        }
        let needs_refresh = self.overlay_apply_input_op(InputOp::DeleteBack);
        if needs_refresh.matches {
            self.recompute_find_matches();
        }
        if needs_refresh.find_in_all {
            self.recompute_find_in_all();
        }
    }

    fn overlay_delete_forward(&mut self) {
        let needs_refresh = self.overlay_apply_input_op(InputOp::DeleteForward);
        if needs_refresh.matches {
            self.recompute_find_matches();
        }
        if needs_refresh.find_in_all {
            self.recompute_find_in_all();
        }
    }

    fn overlay_caret_horizontal(&mut self, delta: i32) {
        let _ = self.overlay_apply_input_op(InputOp::Caret(delta));
    }

    fn overlay_caret_home(&mut self) {
        let _ = self.overlay_apply_input_op(InputOp::CaretHome);
    }

    fn overlay_caret_end(&mut self) {
        let _ = self.overlay_apply_input_op(InputOp::CaretEnd);
    }

    fn overlay_step_selection(&mut self, delta: i32) {
        if matches!(self.overlays, Overlays::Find(_)) {
            self.step_find_bar(delta);
            return;
        }
        let mut font_preview: Option<String> = None;
        let mut theme_preview: Option<crate::theme_picker::ThemeEntry> = None;
        match &mut self.overlays {
            Overlays::FindInAll(fia) => fia.step(delta),
            Overlays::Palette(p) => p.step(delta),
            Overlays::QuickOpen(q) => q.step(delta),
            Overlays::GotoHeading(g) => g.step(delta),
            Overlays::FontPicker(fp) => {
                fp.step(delta);
                font_preview = fp.next_preview_family();
            }
            Overlays::ThemePicker(tp) => {
                tp.step(delta);
                theme_preview = tp.next_preview().cloned();
            }
            // §H6 — tab switcher: advance the selection cursor and
            // preview the newly highlighted tab in the focused pane.
            // Preview-activate uses `set_active_for_preview` so the
            // MRU stack is left untouched; the commit path on Ctrl
            // release / Enter is the only one that promotes the tab.
            Overlays::TabSwitcher(ts) => {
                ts.step(delta);
                if let Some(tab) = ts.selected_row().map(|r| r.tab_id) {
                    self.preview_tab_via_switcher(tab);
                }
            }
            Overlays::SlashPalette(sp) => sp.step(delta),
            Overlays::PreviousBufferBrowser(b) => b.step(delta),
            _ => {}
        }
        if let Some(family) = font_preview {
            self.set_font_family(family);
        }
        if let Some(entry) = theme_preview {
            self.apply_theme_entry(&entry);
        }
    }

    fn overlay_apply_input_op(&mut self, op: InputOp) -> InputOpEffect {
        let mut effect = InputOpEffect::default();
        match &mut self.overlays {
            Overlays::Find(fb) => {
                effect.matches = match op {
                    InputOp::DeleteBack => fb.delete_back(),
                    InputOp::DeleteForward => fb.delete_forward(),
                    InputOp::Caret(d) => {
                        if d < 0 {
                            fb.move_left()
                        } else {
                            fb.move_right()
                        }
                    }
                    InputOp::CaretHome => {
                        fb.move_home();
                        false
                    }
                    InputOp::CaretEnd => {
                        fb.move_end();
                        false
                    }
                };
            }
            Overlays::FindInAll(fia) => {
                effect.find_in_all = apply_input_op(&mut fia.input, op);
            }
            Overlays::Palette(p) => {
                let changed = apply_input_op(&mut p.input, op);
                if changed {
                    p.refilter();
                }
            }
            Overlays::QuickOpen(q) => {
                let changed = apply_input_op(&mut q.input, op);
                if changed {
                    q.refilter();
                }
            }
            Overlays::GotoLine(g) => {
                apply_input_op(&mut g.input, op);
            }
            Overlays::GotoHeading(g) => {
                let changed = apply_input_op(&mut g.input, op);
                if changed {
                    g.refilter();
                }
            }
            Overlays::FontPicker(fp) => {
                let changed = apply_input_op(&mut fp.input, op);
                if changed {
                    fp.refilter();
                }
            }
            Overlays::ThemePicker(tp) => {
                let changed = apply_input_op(&mut tp.input, op);
                if changed {
                    tp.refilter();
                }
            }
            // §H6 — tab switcher has no text input; swallow the op.
            Overlays::TabSwitcher(_) => {}
            // §H5 — slash palette: standard text-input editing, but
            // backspace at zero filter chars dismisses the palette
            // (handled in `overlay_delete_back` so the policy lives
            // next to the routing).
            Overlays::SlashPalette(sp) => {
                let changed = apply_input_op(&mut sp.input, op);
                if changed {
                    sp.refilter();
                }
            }
            // Phase F3 — hex picker. Backspace / arrows operate on the
            // standard `TextInput`; no refilter needed.
            Overlays::HexPicker(hp) => {
                apply_input_op(&mut hp.input, op);
            }
            // δ.4 — previous-buffer browser: standard editing on the
            // filter input, refilter on text-changing ops.
            Overlays::PreviousBufferBrowser(b) => {
                let changed = apply_input_op(&mut b.input, op);
                if changed {
                    b.refilter();
                }
            }
            Overlays::Idle => {}
        }
        effect
    }

    // `overlay_confirm` and every `confirm_*` method (plus the
    // tab-switcher preview helpers and `palette_math_copy`) live in
    // `window_overlay_confirm.rs` so this file stays under the
    // conventions cap.
}

// Text-input op helpers moved to `window_overlays_input_op.rs`
// (Phase H5 split) to keep this file under the 600-line cap.
use crate::window_overlays_input_op::{apply_input_op, InputOp, InputOpEffect};
