//! Phase F5 — typed path accessors for [`crate::MarkdownConfig`].
//!
//! Extracted into a sibling module so [`crate::settings`] stays under
//! the 600-line cap. The accessor lives on `MarkdownConfig` itself
//! (Rust allows the impl block to live in a different module of the
//! same crate); call sites remain unchanged.

use std::path::PathBuf;

use crate::settings_markdown::MarkdownConfig;
use crate::Error;

const APPDATA_PREFIX: &str = "%APPDATA%";

impl MarkdownConfig {
    /// Resolve [`Self::images_dir`] to an absolute [`PathBuf`],
    /// expanding the single supported `%APPDATA%` prefix. Mirrors the
    /// persist crate's `data_dir` expansion pattern.
    ///
    /// Resolution:
    /// * `%APPDATA%\…` → expanded against `std::env::var("APPDATA")`.
    /// * Any other path → returned verbatim (caller can supply an
    ///   absolute path, a relative path, or a different env prefix —
    ///   we deliberately do not implement a generic `%X%` expander
    ///   here; that would conflate two policies and surprise users
    ///   whose paths legitimately contain percent signs).
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingEnv`] when the configured path starts
    /// with `%APPDATA%` and the env var is unset.
    pub fn resolve_images_dir(&self) -> Result<PathBuf, Error> {
        if let Some(rest) = self.images_dir.strip_prefix(APPDATA_PREFIX) {
            let base = std::env::var_os("APPDATA").ok_or(Error::MissingEnv("APPDATA"))?;
            let trimmed = rest.trim_start_matches(['\\', '/']);
            return Ok(PathBuf::from(base).join(trimmed));
        }
        Ok(PathBuf::from(&self.images_dir))
    }
}
