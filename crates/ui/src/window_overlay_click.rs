//! Mouse click / double-click routing for overlay text inputs.
//!
//! Split from [`crate::window_overlay_input`] to keep both files under the
//! 600-line cap. Focus routing and keyboard chord interception stay there;
//! this module owns the pointer side: caret-by-X inversion, single-click
//! focus + caret placement, double-click select-all, and find-bar control
//! dispatch. **Thread ownership**: the window's UI thread (sole HWND owner).

use continuity_render::FocusField;

use crate::find_bar::FindFocus;
use crate::find_regex_help::{FindControl, REGEX_SNIPPETS};
use crate::overlay_render_find::{hit_test_find_bar, hover_find_control, FindBarHit};
use crate::overlays::Overlays;
use crate::window_overlay_input::{hit_list_row, rect_contains, OverlayCursor};
use crate::Window;

impl Window {
    /// Invert a client X coordinate inside `field` to a UTF-8 byte offset in
    /// the field's text, mirroring the overlay painter's byte→x mapping.
    ///
    /// The painter draws field text at `field.rect.inset(8.0, 4.0)` and at the
    /// *unzoomed* base font size (it divides body zoom back out), so this
    /// subtracts the 8-DIP left inset and hit-tests at the same unzoomed size
    /// via [`continuity_render::hit_test_x_to_byte_sized`]. Single-line fields
    /// never wrap, so the wrap width is `f32::INFINITY`. Returns `None` before
    /// the first paint has a live `text_format`.
    fn overlay_caret_byte_at_x(&self, field: &FocusField, client_x: f32) -> Option<usize> {
        let format = self.text_format.as_ref()?;
        let inner_left = field.rect.x + 8.0;
        let x_in_text = (client_x - inner_left).max(0.0);
        let overlay_font = self.scaled_font_size() / self.view.font_size_scale.max(0.01);
        continuity_render::hit_test_x_to_byte_sized(
            self.dwrite.raw(),
            format,
            &field.text,
            x_in_text,
            f32::INFINITY,
            overlay_font,
        )
    }

    /// Click-to-focus + caret placement for overlay text inputs. Returns
    /// `true` when the click lands inside the active overlay's panel — the
    /// caller skips the editor's caret-placement path so a click on a palette
    /// / find bar / picker can't double as a buffer caret move.
    ///
    /// When the click hits one of the overlay's text-input rects, this updates
    /// the input's focus state (for the dual-field find bar, flipping
    /// `FindBar::focus` to whichever rect was clicked) and positions the caret
    /// at the clicked X via [`Window::overlay_caret_byte_at_x`] +
    /// [`crate::text_input::TextInput::set_caret_byte`]. The placement clears
    /// any prior selection, matching native click behavior.
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
                // The clicked field is now the focused field; place the caret
                // at the clicked X within it.
                if let Some(byte) = self.overlay_caret_byte_at_x(secondary, xf) {
                    if let Some(input) = self.focused_text_input() {
                        input.set_caret_byte(byte);
                    }
                }
                self.invalidate(self.hwnd);
                return true;
            }
        }
        if let Some(field) = draw
            .focus_field
            .as_ref()
            .filter(|field| rect_contains(field.rect, xf, yf))
        {
            self.focus_overlay_input();
            if let Some(byte) = self.overlay_caret_byte_at_x(field, xf) {
                if let Some(input) = self.focused_text_input() {
                    input.set_caret_byte(byte);
                }
            }
            self.invalidate(self.hwnd);
            return true;
        }
        // Inputs other than the find bar's dual fields are single-input
        // overlays — the click is already "on the input" semantically;
        // there's nothing to switch.
        true
    }

    /// Double-click routing for overlay text inputs. Returns `true` when the
    /// double-click lands inside the active overlay's panel so the caller
    /// skips the buffer's `select_word` path. When the double-click is inside
    /// a text field, the focused input is selected whole (native "double-click
    /// to select" feel); otherwise it routes through
    /// [`Window::overlay_input_click`] so controls / list rows still respond.
    pub(crate) fn overlay_input_dbl_click(&mut self, x: i32, y: i32) -> bool {
        if !self.overlays.is_active() {
            return false;
        }
        let over_field = self
            .overlay_cursor_at(x as f32, y as f32)
            .is_some_and(|cursor| cursor == OverlayCursor::IBeam);
        let claimed = self.overlay_input_click(x, y);
        if claimed && over_field {
            // overlay_input_click already focused the clicked field; select
            // all of it so the next keystroke overtypes the whole value.
            if let Some(input) = self.focused_text_input() {
                input.select_all();
            }
            self.invalidate(self.hwnd);
        }
        claimed
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
}
