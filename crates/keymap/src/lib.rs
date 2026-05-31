#![warn(missing_docs)]
//! TOML keymap loader and conflict checker.

/// Compiled-in default keymap shipped with the binary.
pub const DEFAULT_KEYMAP_TOML: &str = include_str!("../assets/default.toml");

pub mod binding;
pub mod conflict;
pub mod error;
pub mod keymap;

pub use binding::Binding;
pub use conflict::Conflict;
pub use error::Error;
pub use keymap::{Keymap, SequenceChainMatch, SequenceMatch};
