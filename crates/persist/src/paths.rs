//! Resolve filesystem paths for the live database and backup directory.
//!
//! Per spec §14 the default install location is `%APPDATA%\continuity\` for
//! the live database and `%LOCALAPPDATA%\continuity\backups\` for hot
//! backups. Portable mode is implemented by the app binary through
//! `CONTINUITY_DATA_DIR` and `CONTINUITY_BACKUPS_DIR` process overrides.

use std::path::PathBuf;

use crate::Error;

/// The on-disk database file.
///
/// Resolution order:
/// 1. `$CONTINUITY_DATA_DIR/continuity.db` — Phase 17.9 §E test override.
///    When set, both the live DB and backups live under this single
///    directory so the §E crash-recovery e2e can isolate state in a
///    tempdir without touching `%APPDATA%`. Mirrors the Phase-18
///    `--portable` flag's eventual semantics.
/// 2. `%APPDATA%\continuity\continuity.db` — production default.
///
/// Creates the parent directory if it doesn't exist.
///
/// # Errors
///
/// Returns [`Error::MissingEnv`] when neither override nor `%APPDATA%`
/// is set. Propagates filesystem errors via [`Error::Compression`].
pub fn db_path() -> Result<PathBuf, Error> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("continuity.db"))
}

/// The tutorial-seen sentinel file.
///
/// Path: `$CONTINUITY_DATA_DIR/.tutorial_seen` (when the override is
/// set) or `%APPDATA%\continuity\.tutorial_seen` otherwise. Existence
/// is the only signal — the file's contents are irrelevant.
///
/// Created the first time the app dispatches `help.tutorial` on
/// launch so subsequent launches do not auto-open the tutorial tab.
/// Deletion (e.g. via "Reset all settings") re-arms the first-launch
/// behaviour for the next start.
///
/// Creates the parent directory if it doesn't exist (idempotent —
/// `db_path` already does this on production runs, but a test that
/// only calls this helper deserves the same behaviour).
///
/// # Errors
///
/// Returns [`Error::MissingEnv`] when neither `$CONTINUITY_DATA_DIR`
/// nor `%APPDATA%` is set. Propagates filesystem errors via
/// [`Error::Compression`].
pub fn tutorial_seen_path() -> Result<PathBuf, Error> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(".tutorial_seen"))
}

/// The hot-backup directory.
///
/// Resolution order:
/// 1. `$CONTINUITY_BACKUPS_DIR` — app-owned portable-mode override.
/// 2. `$CONTINUITY_DATA_DIR/backups` — §E test override.
/// 3. `%LOCALAPPDATA%\continuity\backups` — production default.
///
/// Creates the directory if it doesn't exist.
///
/// # Errors
///
/// Returns [`Error::MissingEnv`] when neither override nor
/// `%LOCALAPPDATA%` is set. Propagates filesystem errors via
/// [`Error::Compression`].
pub fn backups_dir() -> Result<PathBuf, Error> {
    let dir = if let Some(d) = backups_dir_override() {
        d
    } else if let Some(d) = data_dir_override() {
        d.join("backups")
    } else {
        appdata_subdir("LOCALAPPDATA")?.join("backups")
    };
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn data_dir() -> Result<PathBuf, Error> {
    if let Some(d) = data_dir_override() {
        return Ok(d);
    }
    appdata_subdir("APPDATA")
}

fn data_dir_override() -> Option<PathBuf> {
    std::env::var_os("CONTINUITY_DATA_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

fn backups_dir_override() -> Option<PathBuf> {
    std::env::var_os("CONTINUITY_BACKUPS_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

fn appdata_subdir(env_key: &'static str) -> Result<PathBuf, Error> {
    let base = std::env::var_os(env_key).ok_or(Error::MissingEnv(env_key))?;
    Ok(PathBuf::from(base).join("continuity"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_env_yields_typed_error() {
        // This succeeds on a Windows host (where APPDATA is set in the
        // process env); on CI / non-Windows hosts where APPDATA might be
        // unset, we still expect a typed error rather than a panic.
        match std::env::var_os("BOGUS_KEY_NOT_SET") {
            Some(_) => {}
            None => {
                let err = appdata_subdir("BOGUS_KEY_NOT_SET").unwrap_err();
                assert!(matches!(err, Error::MissingEnv("BOGUS_KEY_NOT_SET")));
            }
        }
    }
}
