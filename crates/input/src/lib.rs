#![warn(missing_docs)]
//! Platform-neutral key-chord parsing for the keymap.
//!
//! Physical keyboard translation belongs to each host adapter. Native
//! Windows virtual-key translation lives in `continuity-ui`.

pub mod chord;
pub mod error;

pub use chord::{KeyChord, Modifiers};
pub use error::Error;
