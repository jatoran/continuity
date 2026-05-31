//! Errors for the `continuity-theme` crate.

use thiserror::Error;

/// Errors that can arise while loading a theme.
#[derive(Debug, Error)]
pub enum Error {
    /// The theme TOML failed to parse.
    #[error("theme parse: {0}")]
    Parse(#[from] toml::de::Error),

    /// A required theme key was missing.
    #[error("missing required theme key `{0}`")]
    MissingKey(&'static str),

    /// A color value did not parse as a recognized format.
    #[error("invalid color `{0}`")]
    InvalidColor(String),

    /// `assets::bundled_named` was asked for a theme that is not in
    /// [`assets::BUNDLED_NAMES`].
    #[error("unknown bundled theme `{0}`")]
    UnknownTheme(String),
}
