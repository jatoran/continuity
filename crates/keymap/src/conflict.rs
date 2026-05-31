//! Conflict detection across a keymap.

use continuity_input::KeyChord;

/// Two bindings claim the same chord under the same `when` predicate.
#[derive(Debug, Clone)]
pub struct Conflict {
    /// The colliding chord.
    pub chord: KeyChord,
    /// The first command bound to it.
    pub a: String,
    /// The second command bound to it.
    pub b: String,
    /// The `when` predicate (or `None` if both bindings are global).
    pub when: Option<String>,
}
