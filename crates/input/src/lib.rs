#![warn(missing_docs)]
//! Win32 raw input, IME composition handling, and key-chord parsing for
//! the keymap.

pub mod chord;
pub mod error;

pub use chord::{KeyChord, Modifiers};
pub use error::Error;
