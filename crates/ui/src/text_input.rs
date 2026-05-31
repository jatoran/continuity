//! Shared single-line text input state used by every overlay that doesn't
//! have multi-field requirements (palette / quick-open / goto-line /
//! goto-heading / find-in-all / slash / font / theme / hex pickers).
//!
//! UTF-8 boundary aware. Caret is byte-offset relative to `text`. Carries an
//! optional **selection anchor**: when `Some`, the byte range between
//! `selection_anchor` and `caret` (in either order) is the active selection.
//! Insertion or backspace/delete operations on a non-empty selection replace
//! the selected range instead of acting around the caret, so Ctrl+A followed
//! by any keystroke behaves like every native text field.

/// Editing chord recognized by [`TextInput::apply_input_chord`]. Production
/// callers translate raw VK chords (`Ctrl+A`, `Shift+Left`, …) into these
/// variants; clipboard chords (Ctrl+C/X/V) are handled by the overlay routing
/// layer since they need access to the system clipboard.
#[derive(Copy, Clone, Debug)]
pub(crate) enum InputChord {
    /// Ctrl+A — select the entire input.
    SelectAll,
    /// Shift+Left — extend selection one character left.
    ExtendLeft,
    /// Shift+Right — extend selection one character right.
    ExtendRight,
    /// Shift+Home — extend selection to the start.
    ExtendHome,
    /// Shift+End — extend selection to the end.
    ExtendEnd,
}

/// Single-line text input with a UTF-8-byte caret and an optional selection
/// anchor. Used by every overlay's text field — the focused input is exposed
/// from [`crate::Window::focused_text_input`] so the overlay routing layer can
/// service editing chords (Ctrl+A/C/X/V, Shift+Home/End/arrows, click-to-set-
/// caret) without re-implementing them per overlay.
#[derive(Debug, Default, Clone)]
pub struct TextInput {
    /// The current text.
    pub text: String,
    /// Caret byte offset within `text`. Always lies on a `char_boundary`.
    pub caret: usize,
    /// Selection anchor in bytes. When `Some`, the range
    /// `min(anchor, caret)..max(anchor, caret)` is the active selection.
    /// Always lies on a `char_boundary` when present.
    pub selection_anchor: Option<usize>,
}

impl TextInput {
    /// A fresh empty input.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace `text` and place the caret at the end. Clears any selection.
    pub fn set_text<S: Into<String>>(&mut self, text: S) {
        self.text = text.into();
        self.caret = self.text.len();
        self.selection_anchor = None;
    }

    /// Drop the selection (collapse to a caret at the current head position).
    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    /// Select the entire buffer (anchor at 0, caret at end).
    pub fn select_all(&mut self) {
        self.selection_anchor = Some(0);
        self.caret = self.text.len();
    }

    /// `(start, end)` byte range of the active selection, ordered. `None`
    /// when there is no selection or the selection is empty.
    #[must_use]
    pub fn selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.selection_anchor?;
        let (start, end) = if anchor <= self.caret {
            (anchor, self.caret)
        } else {
            (self.caret, anchor)
        };
        (start != end).then_some((start, end))
    }

    /// The selected substring, if any.
    #[must_use]
    pub fn selection_text(&self) -> Option<&str> {
        let (start, end) = self.selection_range()?;
        Some(&self.text[start..end])
    }

    /// Replace the active selection (if any) with `s`. When no selection is
    /// active this is a no-op — callers should pair this with `insert_char`
    /// for the "no selection → just insert" path.
    pub fn replace_selection(&mut self, s: &str) -> bool {
        let Some((start, end)) = self.selection_range() else {
            return false;
        };
        self.text.replace_range(start..end, s);
        self.caret = start + s.len();
        self.selection_anchor = None;
        true
    }

    /// Set caret to `byte`, snapping to the nearest valid `char_boundary`
    /// (rounding *down*). Clears selection. Used by click-to-set-caret.
    pub fn set_caret_byte(&mut self, byte: usize) {
        let mut clamped = byte.min(self.text.len());
        while clamped > 0 && !self.text.is_char_boundary(clamped) {
            clamped -= 1;
        }
        self.caret = clamped;
        self.selection_anchor = None;
    }

    /// Insert `c` at the caret. Replaces the selection if one exists.
    pub fn insert_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        if self.replace_selection(s) {
            return;
        }
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
    }

    /// Insert the literal string `s` at the caret. Replaces the selection if
    /// one exists. Used by the paste path.
    pub fn insert_str(&mut self, s: &str) {
        if self.replace_selection(s) {
            return;
        }
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
    }

    /// Delete the byte before the caret. When a selection is active, deletes
    /// the selection instead.
    pub fn delete_back(&mut self) -> bool {
        if self.replace_selection("") {
            return true;
        }
        if self.caret == 0 {
            return false;
        }
        let mut new_caret = self.caret;
        while new_caret > 0 {
            new_caret -= 1;
            if self.text.is_char_boundary(new_caret) {
                break;
            }
        }
        self.text.replace_range(new_caret..self.caret, "");
        self.caret = new_caret;
        true
    }

    /// Delete the byte after the caret. When a selection is active, deletes
    /// the selection instead.
    pub fn delete_forward(&mut self) -> bool {
        if self.replace_selection("") {
            return true;
        }
        if self.caret >= self.text.len() {
            return false;
        }
        let mut end = self.caret + 1;
        while end < self.text.len() && !self.text.is_char_boundary(end) {
            end += 1;
        }
        self.text.replace_range(self.caret..end, "");
        true
    }

    /// Move caret left one character. Collapses any selection to its left
    /// edge — matching standard text-field semantics where pressing Left
    /// with a selection jumps to the start.
    pub(crate) fn move_left(&mut self) -> bool {
        if let Some((start, _)) = self.selection_range() {
            self.caret = start;
            self.selection_anchor = None;
            return true;
        }
        self.selection_anchor = None;
        self.move_caret_left()
    }

    /// Move caret right one character. Collapses any selection to its right
    /// edge.
    pub(crate) fn move_right(&mut self) -> bool {
        if let Some((_, end)) = self.selection_range() {
            self.caret = end;
            self.selection_anchor = None;
            return true;
        }
        self.selection_anchor = None;
        self.move_caret_right()
    }

    /// Move caret to start. Clears selection.
    pub(crate) fn move_home(&mut self) {
        self.caret = 0;
        self.selection_anchor = None;
    }

    /// Move caret to end. Clears selection.
    pub(crate) fn move_end(&mut self) {
        self.caret = self.text.len();
        self.selection_anchor = None;
    }

    /// Apply a structured editing chord. Production callers in
    /// [`crate::Window::overlay_intercept_text_chord`] map raw VK chords
    /// to these variants so the overlay routing layer doesn't reimplement
    /// the caret machinery per overlay. Returns whether anything changed.
    pub(crate) fn apply_input_chord(&mut self, chord: InputChord) -> bool {
        match chord {
            InputChord::SelectAll => {
                self.select_all();
                true
            }
            InputChord::ExtendLeft => self.extend_left(),
            InputChord::ExtendRight => self.extend_right(),
            InputChord::ExtendHome => {
                self.extend_home();
                true
            }
            InputChord::ExtendEnd => {
                self.extend_end();
                true
            }
        }
    }

    fn extend_left(&mut self) -> bool {
        self.ensure_anchor();
        self.move_caret_left()
    }

    fn extend_right(&mut self) -> bool {
        self.ensure_anchor();
        self.move_caret_right()
    }

    fn extend_home(&mut self) {
        self.ensure_anchor();
        self.caret = 0;
    }

    fn extend_end(&mut self) {
        self.ensure_anchor();
        self.caret = self.text.len();
    }

    fn ensure_anchor(&mut self) {
        if self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.caret);
        }
    }

    fn move_caret_left(&mut self) -> bool {
        if self.caret == 0 {
            return false;
        }
        let mut new_caret = self.caret;
        while new_caret > 0 {
            new_caret -= 1;
            if self.text.is_char_boundary(new_caret) {
                break;
            }
        }
        self.caret = new_caret;
        true
    }

    fn move_caret_right(&mut self) -> bool {
        if self.caret >= self.text.len() {
            return false;
        }
        let mut new_caret = self.caret + 1;
        while new_caret < self.text.len() && !self.text.is_char_boundary(new_caret) {
            new_caret += 1;
        }
        self.caret = new_caret;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_delete_back() {
        let mut t = TextInput::new();
        t.insert_char('h');
        t.insert_char('i');
        assert_eq!(t.text, "hi");
        assert!(t.delete_back());
        assert_eq!(t.text, "h");
        assert_eq!(t.caret, 1);
    }

    #[test]
    fn handles_utf8_boundary() {
        let mut t = TextInput::new();
        t.insert_char('日');
        assert_eq!(t.text.len(), 3);
        assert!(t.delete_back());
        assert!(t.text.is_empty());
        assert_eq!(t.caret, 0);
    }

    #[test]
    fn move_left_right_clamp() {
        let mut t = TextInput::new();
        assert!(!t.move_left());
        t.set_text("ab");
        assert_eq!(t.caret, 2);
        assert!(t.move_left());
        assert_eq!(t.caret, 1);
        assert!(t.move_right());
        assert!(!t.move_right());
    }

    #[test]
    fn select_all_then_type_replaces_text() {
        let mut t = TextInput::new();
        t.set_text("hello");
        t.select_all();
        assert_eq!(t.selection_range(), Some((0, 5)));
        t.insert_char('x');
        assert_eq!(t.text, "x");
        assert_eq!(t.caret, 1);
        assert_eq!(t.selection_anchor, None);
    }

    #[test]
    fn select_all_then_backspace_clears() {
        let mut t = TextInput::new();
        t.set_text("hello");
        t.select_all();
        assert!(t.delete_back());
        assert!(t.text.is_empty());
        assert_eq!(t.caret, 0);
        assert_eq!(t.selection_anchor, None);
    }

    #[test]
    fn shift_right_extends_selection() {
        let mut t = TextInput::new();
        t.set_text("abcd");
        t.caret = 1;
        t.extend_right();
        t.extend_right();
        assert_eq!(t.selection_range(), Some((1, 3)));
        assert_eq!(t.selection_text(), Some("bc"));
    }

    #[test]
    fn shift_left_extends_left_then_collapses_on_plain_left() {
        let mut t = TextInput::new();
        t.set_text("abcd");
        t.caret = 4;
        t.extend_left();
        t.extend_left();
        assert_eq!(t.selection_range(), Some((2, 4)));
        // Plain Left collapses to start of selection.
        t.move_left();
        assert_eq!(t.caret, 2);
        assert_eq!(t.selection_anchor, None);
    }

    #[test]
    fn shift_home_end_extend_to_bounds() {
        let mut t = TextInput::new();
        t.set_text("abcd");
        t.caret = 2;
        t.extend_home();
        assert_eq!(t.selection_range(), Some((0, 2)));
        t.clear_selection();
        t.caret = 1;
        t.extend_end();
        assert_eq!(t.selection_range(), Some((1, 4)));
    }

    #[test]
    fn insert_str_replaces_selection() {
        let mut t = TextInput::new();
        t.set_text("hello world");
        t.caret = 0;
        t.extend_right();
        t.extend_right();
        t.extend_right();
        t.extend_right();
        t.extend_right(); // selects "hello"
        t.insert_str("YO");
        assert_eq!(t.text, "YO world");
        assert_eq!(t.caret, 2);
    }

    #[test]
    fn set_caret_byte_snaps_to_char_boundary() {
        let mut t = TextInput::new();
        t.set_text("日本");
        // Byte 1 is mid-codepoint; snaps down to 0.
        t.set_caret_byte(1);
        assert_eq!(t.caret, 0);
        t.set_caret_byte(4);
        assert_eq!(t.caret, 3);
        t.set_caret_byte(999);
        assert_eq!(t.caret, t.text.len());
    }

    #[test]
    fn delete_forward_with_selection_replaces() {
        let mut t = TextInput::new();
        t.set_text("abcd");
        t.caret = 1;
        t.extend_right();
        t.extend_right();
        assert!(t.delete_forward());
        assert_eq!(t.text, "ad");
        assert_eq!(t.caret, 1);
    }
}
