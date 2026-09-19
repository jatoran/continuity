//! The updater's tiny persisted state: when it last checked and which
//! version the user asked it to stop offering. One JSON file next to the
//! database; a missing or malformed file reads as defaults, so nothing
//! here can ever block startup.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// `updates.json` contents.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct UpdateState {
    /// UNIX-epoch milliseconds of the last completed check (success or
    /// failure — a failing network must not be retried every wake).
    pub(crate) last_check_ms: u64,
    /// Version the user chose *Skip this version* on; offered again only
    /// on an explicit `help.check_for_updates`.
    pub(crate) skipped_version: Option<String>,
}

impl UpdateState {
    /// Read the state file, falling back to defaults on any error.
    pub(crate) fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Write the state file atomically (temp + rename).
    pub(crate) fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, json)?;
        std::fs::rename(&temp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::UpdateState;

    #[test]
    fn round_trips_and_defaults_on_garbage() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("updates.json");
        assert_eq!(UpdateState::load(&path), UpdateState::default());
        let state = UpdateState {
            last_check_ms: 1_700_000_000_000,
            skipped_version: Some("0.4.12".into()),
        };
        state.save(&path).expect("save");
        assert_eq!(UpdateState::load(&path), state);
        std::fs::write(&path, b"{ not json").expect("corrupt");
        assert_eq!(UpdateState::load(&path), UpdateState::default());
    }
}
