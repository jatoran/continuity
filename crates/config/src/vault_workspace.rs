//! Portable, per-vault workspace state stored beside `vault.toml`.
//!
//! This file contains UI state rather than user-authored policy. Filesystem
//! discovery and writes remain owned by the caller; this module owns only the
//! versioned TOML shape and validation.

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::Error;

/// Filename for portable vault UI state inside `.continuity`.
pub const VAULT_WORKSPACE_FILE: &str = "workspace.toml";

const DEFAULT_FILE_TREE_WIDTH_DIP: f32 = 280.0;
const MIN_FILE_TREE_WIDTH_DIP: f32 = 140.0;
const MAX_FILE_TREE_WIDTH_DIP: f32 = 720.0;
const MAX_EXPANDED_DIRECTORIES: usize = 10_000;
const MAX_OPEN_TABS: usize = 512;

/// One restored editor tab in a vault: a file under the vault root plus the
/// vertical scroll offset it was left at. Only file-associated buffers whose
/// path lives under the vault are portable, so untitled buffers are not
/// recorded here.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct VaultTabState {
    /// File path relative to the vault root, slash-normalized.
    pub path: String,
    /// Vertical scroll offset in device-independent pixels at last save.
    pub scroll_y_dip: f32,
}

impl Default for VaultTabState {
    fn default() -> Self {
        Self {
            path: String::new(),
            scroll_y_dip: 0.0,
        }
    }
}

/// Portable file-tree + open-tab state for one vault.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct VaultWorkspaceState {
    /// Wire-format version. Version `1` is the only supported value.
    pub version: u32,
    /// Last expanded file-tree width in device-independent pixels.
    pub file_tree_width_dip: f32,
    /// Whether the file tree was expanded when last changed.
    pub file_tree_visible: bool,
    /// Expanded folder paths, relative to the vault and slash-normalized.
    pub expanded_directories: Vec<String>,
    /// Open editor tabs in positional order, restored when the vault reopens.
    pub open_tabs: Vec<VaultTabState>,
    /// Index into `open_tabs` of the focused tab at last save. Ignored when
    /// `open_tabs` is empty or the index is out of range.
    pub focused_tab: usize,
}

impl Default for VaultWorkspaceState {
    fn default() -> Self {
        Self {
            version: 1,
            file_tree_width_dip: DEFAULT_FILE_TREE_WIDTH_DIP,
            file_tree_visible: true,
            expanded_directories: Vec::new(),
            open_tabs: Vec::new(),
            focused_tab: 0,
        }
    }
}

impl VaultWorkspaceState {
    /// Parse and validate a vault workspace TOML document.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] for malformed TOML or [`Error::Invalid`]
    /// when the document contains unsupported or unsafe state.
    pub fn from_toml_validated(source: &str) -> Result<Self, Error> {
        let state: Self = toml::from_str(source)?;
        state.validate()?;
        Ok(state)
    }

    /// Serialize this validated state as stable, human-readable TOML.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for invalid state or [`Error::Serialize`]
    /// when TOML serialization fails.
    pub fn to_toml(&self) -> Result<String, Error> {
        self.validate()?;
        Ok(toml::to_string_pretty(self)?)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.version != 1 {
            return Err(Error::invalid_range(
                "vault.workspace.version",
                self.version,
                "1",
            ));
        }
        if !self.file_tree_width_dip.is_finite()
            || !(MIN_FILE_TREE_WIDTH_DIP..=MAX_FILE_TREE_WIDTH_DIP)
                .contains(&self.file_tree_width_dip)
        {
            return Err(Error::invalid_range(
                "vault.workspace.file_tree_width_dip",
                self.file_tree_width_dip,
                "140..=720",
            ));
        }
        if self.expanded_directories.len() > MAX_EXPANDED_DIRECTORIES {
            return Err(Error::invalid_range(
                "vault.workspace.expanded_directories",
                self.expanded_directories.len(),
                "0..=10000 entries",
            ));
        }
        for path in &self.expanded_directories {
            if !is_safe_relative_path(path) {
                return Err(Error::Invalid {
                    field: "vault.workspace.expanded_directories",
                    value: path.clone(),
                    allowed: "non-empty relative vault paths without `..`",
                });
            }
        }
        if self.open_tabs.len() > MAX_OPEN_TABS {
            return Err(Error::invalid_range(
                "vault.workspace.open_tabs",
                self.open_tabs.len(),
                "0..=512 entries",
            ));
        }
        for tab in &self.open_tabs {
            if !is_safe_relative_path(&tab.path) {
                return Err(Error::Invalid {
                    field: "vault.workspace.open_tabs.path",
                    value: tab.path.clone(),
                    allowed: "non-empty relative vault paths without `..`",
                });
            }
            if !tab.scroll_y_dip.is_finite() || tab.scroll_y_dip < 0.0 {
                return Err(Error::Invalid {
                    field: "vault.workspace.open_tabs.scroll_y_dip",
                    value: tab.scroll_y_dip.to_string(),
                    allowed: "a finite offset >= 0",
                });
            }
        }
        Ok(())
    }
}

fn is_safe_relative_path(value: &str) -> bool {
    if value.is_empty() || value.contains('\\') {
        return false;
    }
    let path = Path::new(value);
    !path.is_absolute()
        && path.components().all(|component| {
            matches!(component, Component::Normal(_))
                && component.as_os_str() != crate::VAULT_CONFIG_DIRECTORY
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_state_round_trips() {
        let state = VaultWorkspaceState {
            file_tree_width_dip: 412.5,
            file_tree_visible: false,
            expanded_directories: vec!["notes".into(), "projects/continuity".into()],
            ..VaultWorkspaceState::default()
        };
        let encoded = state.to_toml().expect("serialize workspace");
        assert_eq!(
            VaultWorkspaceState::from_toml_validated(&encoded).expect("parse workspace"),
            state
        );
    }

    #[test]
    fn workspace_rejects_unsafe_paths_and_widths() {
        let unsafe_path = "expanded_directories = ['../outside']";
        assert!(VaultWorkspaceState::from_toml_validated(unsafe_path).is_err());
        let invalid_width = "file_tree_width_dip = 900";
        assert!(VaultWorkspaceState::from_toml_validated(invalid_width).is_err());
    }

    #[test]
    fn open_tabs_round_trip() {
        let state = VaultWorkspaceState {
            open_tabs: vec![
                VaultTabState {
                    path: "daily/today.md".into(),
                    scroll_y_dip: 0.0,
                },
                VaultTabState {
                    path: "projects/continuity.md".into(),
                    scroll_y_dip: 512.5,
                },
            ],
            focused_tab: 1,
            ..VaultWorkspaceState::default()
        };
        let encoded = state.to_toml().expect("serialize workspace");
        assert_eq!(
            VaultWorkspaceState::from_toml_validated(&encoded).expect("parse workspace"),
            state
        );
    }

    #[test]
    fn open_tabs_reject_escapes_and_bad_scroll() {
        let escape = "[[open_tabs]]\npath = '../secrets.md'\nscroll_y_dip = 0.0";
        assert!(VaultWorkspaceState::from_toml_validated(escape).is_err());
        let config_dir = "[[open_tabs]]\npath = '.continuity/vault.toml'\nscroll_y_dip = 0.0";
        assert!(VaultWorkspaceState::from_toml_validated(config_dir).is_err());
        let negative = "[[open_tabs]]\npath = 'a.md'\nscroll_y_dip = -5.0";
        assert!(VaultWorkspaceState::from_toml_validated(negative).is_err());
    }

    #[test]
    fn absent_open_tabs_default_to_empty() {
        // Existing 0.4.x workspace files have no open-tab keys; they must
        // still parse (serde default) instead of failing vault activation.
        let legacy = "version = 1\nfile_tree_width_dip = 280.0\nfile_tree_visible = true";
        let state = VaultWorkspaceState::from_toml_validated(legacy).expect("legacy parse");
        assert!(state.open_tabs.is_empty());
        assert_eq!(state.focused_tab, 0);
    }
}
