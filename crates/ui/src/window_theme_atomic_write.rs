//! Tiny shared helper for atomic file writes used by the δ.5 theme
//! workflow (theme TOML installs + settings.toml binding rewrites).
//!
//! Splitting this off keeps `window_theme_manage.rs` and
//! `window_theme_settings_edit.rs` under the 600-line cap and centralizes
//! the temp-file + rename dance so both callers share the same crash-safe
//! semantics.
//!
//! Thread ownership: stateless — both call sites run on the UI thread.

use std::path::Path;

/// Atomic write: render to `<path>.tmp` then rename to `<path>`. On
/// Windows the rename is atomic for files on the same volume. When the
/// destination already exists it is removed before the rename so a
/// re-write of the same theme file completes cleanly.
pub(crate) fn atomic_write(path: &Path, body: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic_write: path has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_parent_directory_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/dir/theme.toml");
        atomic_write(&path, b"hello").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, "hello");
    }

    #[test]
    fn replaces_existing_file_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file.toml");
        std::fs::write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }
}
