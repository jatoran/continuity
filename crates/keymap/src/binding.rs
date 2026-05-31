//! A single keymap binding.

use continuity_input::KeyChord;

/// One binding: a *sequence* of chords (`keys`) that fires `command`,
/// optionally gated by `when`.
///
/// The sequence semantic supports prefix-style shortcuts like
/// `Ctrl+K, Ctrl+R` — the user presses `Ctrl+K` first, then `Ctrl+R`
/// within the chord window, and the binding fires. Single-chord
/// shortcuts (the common case) are sequences of length 1.
#[derive(Debug, Clone)]
pub struct Binding {
    /// Sequence of chords the user types to fire `command`. Length 1
    /// for ordinary single-key shortcuts; length 2 for `Ctrl+K`-style
    /// prefix sequences.
    pub keys: Vec<KeyChord>,
    /// The command id (e.g., `"editor.move_line_up"`).
    pub command: String,
    /// Optional context predicate (e.g., `"language == 'markdown'"`).
    pub when: Option<String>,
}
