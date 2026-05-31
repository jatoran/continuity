//! Errors for the `continuity-keymap` crate.

use thiserror::Error;

/// Errors that can arise while loading or applying a keymap.
#[derive(Debug, Error)]
pub enum Error {
    /// The keymap TOML failed to parse.
    #[error("keymap parse: {0}")]
    Parse(#[from] toml::de::Error),

    /// A chord string was syntactically invalid.
    #[error("chord: {0}")]
    Chord(#[from] continuity_input::Error),

    /// Two bindings collide in the same context predicate.
    #[error("keymap conflict: chord `{chord}` bound to both `{a}` and `{b}`")]
    Conflict {
        /// The conflicting key chord.
        chord: String,
        /// First command bound to it.
        a: String,
        /// Second command bound to it.
        b: String,
    },
}
