#![warn(missing_docs)]
//! TOML theme loader, required-key validation, dark/light/system mode
//! resolution, and bundled defaults (`deep_minimal` + `paper`).
//!
//! Phase 11 surface:
//!
//! - [`Theme::load`] parses + validates every required key from
//!   [`keys::REQUIRED_KEYS`].
//! - [`Theme::*`] typed accessors return a [`Color`] for every required key
//!   without `Option` plumbing (validated at load time).
//! - [`assets`] exposes the bundled `deep_minimal` (dark) and `paper`
//!   (light) themes plus a hard-coded neutral fallback.
//! - [`Mode`] / [`ThemeSet`] resolve dark/light/system, sampling an OS
//!   `system_dark` flag from the UI thread.

pub mod assets;
pub mod color;
pub mod error;
pub mod keys;
pub mod mode;
pub mod sanitize;
pub mod serialize;
pub mod theme;

pub use color::Color;
pub use error::Error;
pub use mode::{Mode, ThemeSet};
pub use sanitize::{check_theme_name, is_reserved_name, NameCheck, MAX_NAME_LEN};
pub use theme::Theme;
