//! Cross-overlay text-input focus routing.
//!
//! Every overlay with a text field (find bar / palette / quick-open / goto
//! line / goto heading / find-in-all / font / theme / slash / hex pickers)
//! exposes a [`TextInput`] — the focused one is funnelled through
//! [`Window::focused_text_input`]. The overlay branch of `Window::on_keydown`
//! calls [`Window::overlay_intercept_text_chord`] *before* the editor's
//! chord engine sees the key, so editing chords (Ctrl+A/C/X/V, Shift+
//! Home/End/arrows) act on the overlay input instead of leaking through to
//! the buffer behind it. Click-to-focus is handled by
//! [`Window::overlay_input_hit_test`] which the mouse path consults before
//! dispatching to the editor.
//!
//! Per-overlay side-effects (refilter, recompute matches) live in
//! [`Window::overlay_after_input_mutation`] — every chord that mutates the
//! focused input calls it so palettes and find results stay in sync.

use continuity_input::KeyChord;
use continuity_render::{OverlayDraw, Rect};
use continuity_win::clipboard;
use serde_json::Value;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_A, VK_C, VK_END, VK_HOME, VK_LEFT, VK_RIGHT, VK_V, VK_X,
};

use crate::find_bar::FindFocus;
use crate::find_regex_help::{FindControl, REGEX_SNIPPETS};
use crate::overlay_render_find::{hit_test_find_bar, hover_find_control, FindBarHit};
use crate::overlays::Overlays;
use crate::pane_layout::metrics;
use crate::text_input::{InputChord, TextInput};
use crate::Window;

fn rect_contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

fn hit_list_row(draw: &OverlayDraw, x: f32, y: f32) -> Option<usize> {
    draw.list_rows
        .iter()
        .position(|row| rect_contains(row.rect, x, y))
}

/// Cursor shape requested by an active overlay at a client point.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum OverlayCursor {
    /// Plain panel/background cursor.
    Arrow,
    /// Clickable control cursor.
    Hand,
    /// Editable text-field cursor.
    IBeam,
}

impl Window {
    /// Let `Ctrl+F` / `Ctrl+H` retarget an already-open find bar even
    /// while one of its text fields owns keyboard focus.
    pub(crate) fn overlay_dispatch_find_mode_chord(&mut self, chord: &KeyChord) -> bool {
        if self.overlays.find_bar().is_none() {
            return false;
        }
        let Some(command) = self.keymap.lookup(chord, self).and_then(|binding| {
            let command = binding.command.as_str();
            (command == continuity_command::EDITOR_FIND.as_str()
                || command == continuity_command::EDITOR_REPLACE.as_str())
            .then(|| binding.command.clone())
        }) else {
            return false;
        };
        self.dispatch_command(&command, &Value::Null)
    }

    /// Mutable handle to the focused text input across every overlay
    /// variant that has one. Returns `None` for the tab switcher (chord-
    /// driven, no text field) and when no overlay is open.
    ///
    /// **Ordering**: matches the [`Overlays`] enum so a renderer reordering
    /// is a compile-time mismatch rather than a silent gap.
    pub(crate) fn focused_text_input(&mut self) -> Option<&mut TextInput> {
        if !self.overlay_input_focused {
            return None;
        }
        match &mut self.overlays {
            Overlays::Idle => None,
            Overlays::Find(fb) => Some(fb.focused_input_mut()),
            Overlays::FindInAll(fia) => Some(&mut fia.input),
            Overlays::Palette(p) => Some(&mut p.input),
            Overlays::QuickOpen(q) => Some(&mut q.input),
            Overlays::GotoLine(g) => Some(&mut g.input),
            Overlays::GotoHeading(g) => Some(&mut g.input),
            Overlays::FontPicker(fp) => Some(&mut fp.input),
            Overlays::ThemePicker(tp) => Some(&mut tp.input),
            Overlays::TabSwitcher(_) => None,
            Overlays::SlashPalette(sp) => Some(&mut sp.input),
            Overlays::HexPicker(hp) => Some(&mut hp.input),
            Overlays::PreviousBufferBrowser(b) => Some(&mut b.input),
        }
    }

    /// Returns `true` when an active overlay should receive keyboard input.
    pub(crate) fn overlay_has_keyboard_focus(&self) -> bool {
        self.overlay_input_focused || matches!(self.overlays, Overlays::TabSwitcher(_))
    }

    /// Move keyboard focus to the active overlay text input, when it has one.
    pub(crate) fn focus_overlay_input(&mut self) {
        if matches!(self.overlays, Overlays::Idle | Overlays::TabSwitcher(_)) {
            return;
        }
        self.overlay_input_focused = true;
    }

    /// Return keyboard focus to the editor body while leaving the overlay open.
    pub(crate) fn blur_overlay_input(&mut self) {
        self.overlay_input_focused = false;
    }

    /// Dismiss the active overlay and return keyboard focus to the editor
    /// body. Use this anywhere an overlay closes after committing — bare
    /// [`Overlays::dismiss`] flips the discriminant to `Idle` but leaves
    /// [`Self::overlay_input_focused`] set, so the next `WM_KEYDOWN` hits
    /// [`Self::overlay_has_keyboard_focus`] and gets swallowed (every
    /// keystroke dies until the user clicks back into the editor).
    pub(crate) fn dismiss_overlay_and_blur(&mut self) {
        self.overlays.dismiss();
        self.overlay_input_focused = false;
    }

    /// Service a text-editing chord against the focused overlay input.
    /// Returns `true` when the chord was claimed — the on_keydown caller
    /// must then *not* fall through to the editor's chord engine, since
    /// the overlay (still open behind the chord) owns input until dismissed.
    ///
    /// Handles: Ctrl+A (select-all), Ctrl+C (copy), Ctrl+X (cut),
    /// Ctrl+V (paste), Shift+Left/Right/Home/End (extend selection).
    /// Anything else returns `false` so the existing overlay routing
    /// (palette step / Enter confirm / Esc dismiss) still runs.
    pub(crate) fn overlay_intercept_text_chord(&mut self, vk: u16, chord: &KeyChord) -> bool {
        let ctrl = chord.modifiers.ctrl;
        let shift = chord.modifiers.shift;
        let alt = chord.modifiers.alt;
        // Alt-modified chords are reserved for overlay-specific toggles
        // (e.g. Alt+C/W/R on the find bar). Don't shadow them here.
        if alt {
            return false;
        }
        if self.focused_text_input().is_none() {
            return false;
        }
        if ctrl && !shift {
            match vk {
                v if v == VK_A.0 => {
                    if let Some(input) = self.focused_text_input() {
                        input.apply_input_chord(InputChord::SelectAll);
                    }
                    return true;
                }
                v if v == VK_C.0 => return self.overlay_copy_focused_input(),
                v if v == VK_X.0 => return self.overlay_cut_focused_input(),
                v if v == VK_V.0 => return self.overlay_paste_focused_input(),
                _ => {}
            }
        }
        if shift && !ctrl {
            let mapped = match vk {
                v if v == VK_LEFT.0 => Some(InputChord::ExtendLeft),
                v if v == VK_RIGHT.0 => Some(InputChord::ExtendRight),
                v if v == VK_HOME.0 => Some(InputChord::ExtendHome),
                v if v == VK_END.0 => Some(InputChord::ExtendEnd),
                _ => None,
            };
            if let Some(c) = mapped {
                let mut moved = false;
                if let Some(input) = self.focused_text_input() {
                    moved = input.apply_input_chord(c);
                }
                if moved {
                    // Extending selection doesn't change the input *text*,
                    // so no refilter is required — but invalidate so the
                    // renderer redraws the new selection range.
                    self.invalidate(self.hwnd);
                }
                return true;
            }
        }
        false
    }

    fn overlay_copy_focused_input(&mut self) -> bool {
        let text = self
            .focused_text_input()
            .and_then(|input| input.selection_text().map(str::to_owned));
        if let Some(t) = text {
            if !t.is_empty() {
                let _ = clipboard::write_text(self.hwnd, &t);
            }
        }
        // Always consume Ctrl+C while an input is focused so the editor's
        // copy binding doesn't fire on the buffer behind the overlay.
        true
    }

    fn overlay_cut_focused_input(&mut self) -> bool {
        let text = self
            .focused_text_input()
            .and_then(|input| input.selection_text().map(str::to_owned));
        if let Some(t) = text {
            if !t.is_empty() {
                let _ = clipboard::write_text(self.hwnd, &t);
                if let Some(input) = self.focused_text_input() {
                    input.replace_selection("");
                }
                self.overlay_after_input_mutation();
            }
        }
        true
    }

    fn overlay_paste_focused_input(&mut self) -> bool {
        // Best-effort: an OS read failure leaves the input untouched but
        // still consumes the chord so the editor doesn't paste behind.
        let Ok(Some(raw)) = clipboard::read_text(self.hwnd) else {
            return true;
        };
        // Overlay text fields are single-line. Drop CR/LF so a multi-line
        // clipboard doesn't smear an embedded newline into the input.
        let sanitized: String = raw.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        if sanitized.is_empty() {
            return true;
        }
        if let Some(input) = self.focused_text_input() {
            input.insert_str(&sanitized);
        }
        self.overlay_after_input_mutation();
        true
    }

    /// Click-to-focus for overlay text inputs. Returns `true` when the
    /// click lands inside the active overlay's panel — the caller skips
    /// the editor's caret-placement path so a click on a palette / find
    /// bar / picker can't double as a buffer caret move.
    ///
    /// When the click hits one of the overlay's text-input rects, this
    /// also updates the input's focus state: for the dual-field find bar
    /// that means flipping `FindBar::focus` to whichever rect was clicked.
    /// Precise caret-by-X positioning inside the input is intentionally
    /// not done here — DirectWrite text-measurement plumbing would have
    /// to be threaded through, and the v1 ask is just "make clicking the
    /// input focus it." The caret keeps its prior position within the
    /// field; subsequent Shift+arrow / Home / End operations work from
    /// there.
    pub(crate) fn overlay_input_click(&mut self, x: i32, y: i32) -> bool {
        if !self.overlays.is_active() {
            return false;
        }
        let draw = crate::overlay_render::build_overlay_draw(
            &self.overlays,
            &self.keymap,
            &self.registry,
            self.client_width_dip(),
            self.client_height_dip(),
            true,
        );
        let Some(draw) = draw else {
            return false;
        };
        let xf = x as f32;
        let yf = y as f32;
        let find_hit = self
            .overlays
            .find_bar()
            .and_then(|fb| hit_test_find_bar(fb, draw.panel.rect, xf, yf));
        if let Some(hit) = find_hit {
            self.handle_find_bar_hit(hit);
            self.invalidate(self.hwnd);
            return true;
        }
        let find_hover = self
            .overlays
            .find_bar()
            .and_then(|fb| hover_find_control(fb, draw.panel.rect, xf, yf));
        if find_hover == Some(FindControl::Regex) {
            return true;
        }
        if matches!(self.overlays, Overlays::Palette(_)) {
            if let Some(row_idx) = hit_list_row(&draw, xf, yf) {
                let has_row = if let Some(palette) = self.overlays.palette_mut() {
                    let has_row = palette.virtual_row_for_visible(row_idx).is_some();
                    if has_row {
                        palette.select_visible_row(row_idx);
                    }
                    has_row
                } else {
                    false
                };
                if has_row {
                    self.overlay_confirm();
                    self.invalidate(self.hwnd);
                    return true;
                }
            }
        }
        if !rect_contains(draw.panel.rect, xf, yf) {
            if matches!(self.overlays, Overlays::Palette(_)) {
                self.dismiss_overlay_and_blur();
                self.invalidate(self.hwnd);
                return false;
            }
            if self.is_inside_pane_body(xf, yf) {
                self.blur_overlay_input();
                self.invalidate(self.hwnd);
            }
            return false;
        }
        // §G4: find-bar dual-field hit-test. Clicking the unfocused
        // field flips `FindBar::focus`; clicking the already-focused
        // field is a no-op beyond consuming the click.
        if let Some(secondary) = draw.secondary_field.as_ref() {
            if rect_contains(secondary.rect, xf, yf) {
                if let Some(fb) = self.overlays.find_bar_mut() {
                    fb.toggle_focus();
                }
                self.focus_overlay_input();
                self.invalidate(self.hwnd);
                return true;
            }
        }
        if draw
            .focus_field
            .as_ref()
            .is_some_and(|field| rect_contains(field.rect, xf, yf))
        {
            self.focus_overlay_input();
            self.invalidate(self.hwnd);
            return true;
        }
        // Inputs other than the find bar's dual fields are single-input
        // overlays — the click is already "on the input" semantically;
        // there's nothing to switch.
        true
    }

    fn handle_find_bar_hit(&mut self, hit: FindBarHit) {
        match hit {
            FindBarHit::Control(FindControl::Case) => self.find_toggle_mode_impl("case"),
            FindBarHit::Control(FindControl::Word) => self.find_toggle_mode_impl("word"),
            FindBarHit::Control(FindControl::Regex) => self.find_toggle_mode_impl("regex"),
            FindBarHit::Control(FindControl::PreserveCase) => {
                self.find_toggle_mode_impl("preserve");
            }
            FindBarHit::Control(FindControl::Scope) => self.find_toggle_mode_impl("scope"),
            FindBarHit::Control(FindControl::Replace) => {
                let with_replace = self
                    .overlays
                    .find_bar()
                    .is_some_and(|fb| !fb.replace_visible);
                let _ = self.open_find_impl(with_replace);
            }
            FindBarHit::Control(FindControl::ReplaceOne) => {
                let _ = self.find_replace_one_impl();
            }
            FindBarHit::Control(FindControl::ReplaceAll) => {
                let _ = self.find_replace_all_impl();
            }
            FindBarHit::Control(FindControl::Previous) => self.step_find_bar_from_button(-1),
            FindBarHit::Control(FindControl::Next) => self.step_find_bar_from_button(1),
            FindBarHit::Control(FindControl::Cursors) => self.find_matches_to_cursors_impl(),
            FindBarHit::RegexSnippet(index) => self.insert_regex_snippet(index),
        }
    }

    fn step_find_bar_from_button(&mut self, delta: i32) {
        self.step_find_bar(delta);
    }

    fn insert_regex_snippet(&mut self, index: usize) {
        let Some(snippet) = REGEX_SNIPPETS.get(index) else {
            return;
        };
        if let Some(fb) = self.overlays.find_bar_mut() {
            fb.focus = FindFocus::Find;
            fb.regex = true;
            fb.query_input.insert_str(snippet.insert);
        }
        self.focus_overlay_input();
        self.recompute_find_matches();
    }

    /// Update find-bar hover state for controls and regex helper rows.
    pub(crate) fn update_overlay_hover_from_pixel(&mut self, x: i32, y: i32) -> bool {
        let Some(draw) = crate::overlay_render::build_overlay_draw(
            &self.overlays,
            &self.keymap,
            &self.registry,
            self.client_width_dip(),
            self.client_height_dip(),
            self.overlay_input_focused,
        ) else {
            return false;
        };
        if matches!(self.overlays, Overlays::Palette(_)) {
            let hover = hit_list_row(&draw, x as f32, y as f32);
            let Some(palette) = self.overlays.palette_mut() else {
                return false;
            };
            let Some(row_idx) = hover else {
                return false;
            };
            if !palette.select_visible_row(row_idx) {
                return false;
            }
            self.invalidate(self.hwnd);
            return true;
        }
        let hover = self
            .overlays
            .find_bar()
            .and_then(|fb| hover_find_control(fb, draw.panel.rect, x as f32, y as f32));
        let Some(fb) = self.overlays.find_bar_mut() else {
            return false;
        };
        if fb.hovered_control == hover {
            return false;
        }
        fb.hovered_control = hover;
        self.invalidate(self.hwnd);
        true
    }

    /// Scroll command-palette results when the wheel is over the overlay.
    pub(crate) fn try_palette_mouse_wheel(&mut self, x: i32, y: i32, notches: f32) -> bool {
        if !matches!(self.overlays, Overlays::Palette(_)) {
            return false;
        }
        let Some(draw) = crate::overlay_render::build_overlay_draw(
            &self.overlays,
            &self.keymap,
            &self.registry,
            self.client_width_dip(),
            self.client_height_dip(),
            self.overlay_input_focused,
        ) else {
            return false;
        };
        if !rect_contains(draw.panel.rect, x as f32, y as f32) {
            return false;
        }
        let row_delta = (-notches * 3.0).round() as i32;
        if row_delta != 0
            && self
                .overlays
                .palette_mut()
                .is_some_and(|palette| palette.scroll_visible_rows(row_delta))
        {
            self.invalidate(self.hwnd);
        }
        true
    }

    /// Returns `true` when an active overlay panel owns this client point.
    /// Wheel dispatch uses this as a no-op claim so scrolling never leaks
    /// through a palette / find / picker panel into a pane underneath.
    pub(crate) fn overlay_claims_pointer(&self, x: i32, y: i32) -> bool {
        if !self.overlays.is_active() {
            return false;
        }
        let Some(draw) = crate::overlay_render::build_overlay_draw(
            &self.overlays,
            &self.keymap,
            &self.registry,
            self.client_width_dip(),
            self.client_height_dip(),
            self.overlay_input_focused,
        ) else {
            return false;
        };
        if rect_contains(draw.panel.rect, x as f32, y as f32) {
            return true;
        }
        self.overlays
            .find_bar()
            .is_some_and(|fb| hover_find_control(fb, draw.panel.rect, x as f32, y as f32).is_some())
    }

    /// Cursor routing for active overlay panels. Returns `None` outside
    /// overlay-owned regions so the editor body can keep its normal cursor.
    pub(crate) fn overlay_cursor_at(&self, x: f32, y: f32) -> Option<OverlayCursor> {
        if !self.overlays.is_active() {
            return None;
        }
        let draw = crate::overlay_render::build_overlay_draw(
            &self.overlays,
            &self.keymap,
            &self.registry,
            self.client_width_dip(),
            self.client_height_dip(),
            self.overlay_input_focused,
        )?;
        if let Some(fb) = self.overlays.find_bar() {
            if hit_test_find_bar(fb, draw.panel.rect, x, y).is_some()
                || hover_find_control(fb, draw.panel.rect, x, y).is_some()
            {
                return Some(OverlayCursor::Hand);
            }
        }
        if matches!(self.overlays, Overlays::Palette(_)) {
            if let Some(row_idx) = hit_list_row(&draw, x, y) {
                if draw.list_rows.get(row_idx).is_some_and(|row| !row.disabled) {
                    return Some(OverlayCursor::Hand);
                }
            }
        }
        if draw
            .focus_field
            .as_ref()
            .is_some_and(|field| rect_contains(field.rect, x, y))
            || draw
                .secondary_field
                .as_ref()
                .is_some_and(|field| rect_contains(field.rect, x, y))
        {
            return Some(OverlayCursor::IBeam);
        }
        rect_contains(draw.panel.rect, x, y).then_some(OverlayCursor::Arrow)
    }

    fn is_inside_pane_body(&self, x: f32, y: f32) -> bool {
        self.pane_outer_rects()
            .into_iter()
            .any(|(_, rect)| rect.contains(x, y) && y >= rect.y + metrics::TAB_STRIP_HEIGHT_DIP)
    }

    /// Run the per-overlay side-effects that follow a text mutation on the
    /// focused input — refilter palettes / recompute matches. Pulled out
    /// of the chord intercept so cut / paste / future helpers share one
    /// dispatch table.
    ///
    /// **Borrow shape**: the per-overlay refilters operate on `&mut self.
    /// overlays` only, while the find recompute paths need full `&mut
    /// self`. We split the two so the borrow on `self.overlays` is
    /// dropped before the recompute call.
    pub(crate) fn overlay_after_input_mutation(&mut self) {
        match &mut self.overlays {
            Overlays::Palette(p) => p.refilter(),
            Overlays::QuickOpen(q) => q.refilter(),
            Overlays::GotoHeading(g) => g.refilter(),
            Overlays::FontPicker(fp) => fp.refilter(),
            Overlays::ThemePicker(tp) => tp.refilter(),
            Overlays::SlashPalette(sp) => sp.refilter(),
            // Find / FindInAll need full `&mut self` — handled below
            // after the `self.overlays` borrow ends.
            // GotoLine, HexPicker, TabSwitcher, Idle: no list to refilter.
            _ => {}
        }
        match self.overlays.kind() {
            crate::overlays::OverlayKind::Find => self.recompute_find_matches(),
            crate::overlays::OverlayKind::FindInAll => self.recompute_find_in_all(),
            _ => {}
        }
    }
}
