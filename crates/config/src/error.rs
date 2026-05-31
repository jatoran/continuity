//! Errors for the `continuity-config` crate.

use thiserror::Error;

/// Errors that can arise while loading, validating, or watching settings.
#[derive(Debug, Error)]
pub enum Error {
    /// The settings TOML failed to parse.
    #[error("settings parse: {0}")]
    Parse(#[from] toml::de::Error),

    /// A filesystem watch operation failed.
    #[error("watch: {0}")]
    Watch(#[from] notify::Error),

    /// File I/O for the settings file failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// A required environment variable was missing when a typed
    /// accessor tried to expand a `%ENV%`-prefixed path.
    #[error("settings: missing environment variable `{0}`")]
    MissingEnv(&'static str),

    /// A field carried a value outside its accepted set / range.
    #[error("settings: invalid `{field}` = `{value}` (allowed: {allowed})")]
    Invalid {
        /// Dotted TOML path of the offending field (e.g. `persistence.mode`).
        field: &'static str,
        /// The rejected value, formatted for display.
        value: String,
        /// Human-readable description of what's allowed.
        allowed: &'static str,
    },
}

impl Error {
    /// Helper for fixed-set enum violations.
    #[must_use]
    pub(crate) fn invalid_enum(field: &'static str, value: &str, allowed: &'static str) -> Self {
        Self::Invalid {
            field,
            value: value.to_string(),
            allowed,
        }
    }

    /// Helper for numeric range violations.
    #[must_use]
    pub(crate) fn invalid_range(
        field: &'static str,
        value: impl ToString,
        allowed: &'static str,
    ) -> Self {
        Self::Invalid {
            field,
            value: value.to_string(),
            allowed,
        }
    }
}
