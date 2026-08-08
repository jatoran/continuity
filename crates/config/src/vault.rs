//! Per-folder Continuity vault configuration and path matching.
//!
//! A vault is identified by `.continuity/vault.toml`. This module is
//! deliberately data-only: callers own filesystem discovery and watching,
//! while [`VaultConfig`] owns parsing, validation, and relative-path policy.

use std::path::Path;

use serde::Deserialize;

use crate::Error;

/// Directory holding portable configuration for one vault.
pub const VAULT_CONFIG_DIRECTORY: &str = ".continuity";
/// Marker/config filename within [`VAULT_CONFIG_DIRECTORY`].
pub const VAULT_CONFIG_FILE: &str = "vault.toml";
/// Initial configuration written by vault initialization.
pub const DEFAULT_VAULT_TOML: &str = r##"version = 1

[save]
autosave = true
delay_ms = 750

[files]
hide_markdown_extensions = true
folders_first = true
sort = "name"
descending = false
ignore = [
    ".trash",
    # "*.tmp",
    # "archive/**",
    # "!archive/keep.md",
]

# Optional path-specific colors; the last matching rule wins.
# [[files.styles]]
# pattern = "daily/**"
# kind = "file" # "any", "file", or "folder"
# color = "#b58900"

[appearance]
theme = "solarized_dark"
file_color = "#839496"
folder_color = "#268bd2"

# Optional stable theme-token overrides.
# [appearance.colors]
# "editor.caret_line_highlight" = "#073642"
"##;

/// Complete per-vault configuration.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct VaultConfig {
    /// Wire-format version. Version `1` is the only supported value.
    pub version: u32,
    /// Continuous-export settings.
    pub save: VaultSaveConfig,
    /// File-tree membership and presentation settings.
    pub files: VaultFilesConfig,
    /// Vault-local theme and file-tree color settings.
    pub appearance: VaultAppearanceConfig,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            version: 1,
            save: VaultSaveConfig::default(),
            files: VaultFilesConfig::default(),
            appearance: VaultAppearanceConfig::default(),
        }
    }
}

impl VaultConfig {
    /// Parse and validate a vault TOML document.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] for malformed TOML or [`Error::Invalid`]
    /// when a validated field is outside its supported range.
    pub fn from_toml_validated(source: &str) -> Result<Self, Error> {
        let config: Self = toml::from_str(source)?;
        config.validate()?;
        Ok(config)
    }

    /// Return whether a relative vault path is excluded by ordered ignore
    /// patterns. `.continuity` is always excluded from the content tree.
    #[must_use]
    pub fn is_path_ignored(&self, relative: &Path) -> bool {
        let normalized = normalize_relative(relative);
        if normalized == VAULT_CONFIG_DIRECTORY
            || normalized.starts_with(&format!("{VAULT_CONFIG_DIRECTORY}/"))
        {
            return true;
        }
        let mut ignored = false;
        for raw_pattern in &self.files.ignore {
            let trimmed = raw_pattern.trim();
            if trimmed.is_empty() {
                continue;
            }
            let (is_negated, pattern) = trimmed
                .strip_prefix('!')
                .map_or((false, trimmed), |pattern| (true, pattern));
            if pattern_matches_path(pattern, &normalized) {
                ignored = !is_negated;
            }
        }
        ignored
    }

    fn validate(&self) -> Result<(), Error> {
        if self.version != 1 {
            return Err(Error::invalid_range("vault.version", self.version, "1"));
        }
        if !(100..=60_000).contains(&self.save.delay_ms) {
            return Err(Error::invalid_range(
                "vault.save.delay_ms",
                self.save.delay_ms,
                "100..=60000",
            ));
        }
        for (field, value) in [
            ("vault.appearance.file_color", &self.appearance.file_color),
            (
                "vault.appearance.folder_color",
                &self.appearance.folder_color,
            ),
        ] {
            if !value.is_empty() && !is_hex_color(value) {
                return Err(Error::Invalid {
                    field,
                    value: value.clone(),
                    allowed: "empty | #RRGGBB | #RRGGBBAA",
                });
            }
        }
        for style in &self.files.styles {
            if style.pattern.trim().is_empty() {
                return Err(Error::Invalid {
                    field: "vault.files.styles.pattern",
                    value: style.pattern.clone(),
                    allowed: "non-empty relative glob",
                });
            }
            if !style.color.is_empty() && !is_hex_color(&style.color) {
                return Err(Error::Invalid {
                    field: "vault.files.styles.color",
                    value: style.color.clone(),
                    allowed: "empty | #RRGGBB | #RRGGBBAA",
                });
            }
        }
        for value in self.appearance.colors.values() {
            if !is_hex_color(value) {
                return Err(Error::Invalid {
                    field: "vault.appearance.colors",
                    value: value.clone(),
                    allowed: "#RRGGBB | #RRGGBBAA",
                });
            }
        }
        Ok(())
    }
}

/// Continuous-export settings for vault-owned files.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct VaultSaveConfig {
    /// Whether edits to vault-owned file buffers are exported automatically.
    pub autosave: bool,
    /// Idle debounce before an automatic export.
    pub delay_ms: u64,
}

impl Default for VaultSaveConfig {
    fn default() -> Self {
        Self {
            autosave: true,
            delay_ms: 750,
        }
    }
}

/// File-tree membership and presentation settings.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct VaultFilesConfig {
    /// Hide a terminal `.md` suffix in tree labels.
    pub hide_markdown_extensions: bool,
    /// Sort directories before files.
    pub folders_first: bool,
    /// Primary sort key.
    pub sort: VaultSort,
    /// Reverse the configured ordering.
    pub descending: bool,
    /// Ordered relative glob rules. A leading `!` re-includes a path.
    pub ignore: Vec<String>,
    /// Optional path-pattern color rules, last match wins.
    pub styles: Vec<VaultFileStyle>,
}

impl Default for VaultFilesConfig {
    fn default() -> Self {
        Self {
            hide_markdown_extensions: true,
            folders_first: true,
            sort: VaultSort::Name,
            descending: false,
            ignore: Vec::new(),
            styles: Vec::new(),
        }
    }
}

/// Supported directory listing sort keys.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum VaultSort {
    /// Case-insensitive display name.
    #[default]
    Name,
    /// Last-modified timestamp.
    Modified,
    /// Creation timestamp where the filesystem provides it.
    Created,
}

/// One optional file-tree path color rule.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct VaultFileStyle {
    /// Relative glob pattern.
    pub pattern: String,
    /// `any`, `file`, or `folder`.
    pub kind: VaultFileStyleKind,
    /// Hex color override.
    pub color: String,
}

impl VaultFileStyle {
    /// Return whether this rule applies to a relative path and entry kind.
    #[must_use]
    pub fn matches(&self, relative: &Path, is_directory: bool) -> bool {
        let kind_matches = match self.kind {
            VaultFileStyleKind::Any => true,
            VaultFileStyleKind::File => !is_directory,
            VaultFileStyleKind::Folder => is_directory,
        };
        kind_matches && pattern_matches_path(&self.pattern, &normalize_relative(relative))
    }
}

/// Entry-kind selector for a [`VaultFileStyle`].
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum VaultFileStyleKind {
    /// Files and folders.
    #[default]
    Any,
    /// Files only.
    File,
    /// Folders only.
    Folder,
}

/// Vault-local appearance defaults.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct VaultAppearanceConfig {
    /// Optional base theme name. Empty inherits global settings.
    pub theme: String,
    /// Default file label color. Empty inherits the active theme.
    pub file_color: String,
    /// Default folder label color. Empty inherits the active theme.
    pub folder_color: String,
    /// Partial active-theme color overrides keyed by stable theme token.
    pub colors: std::collections::BTreeMap<String, String>,
}

fn normalize_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn pattern_matches_path(pattern: &str, normalized_path: &str) -> bool {
    let pattern = pattern.trim().trim_start_matches('/').replace('\\', "/");
    if pattern.is_empty() {
        return false;
    }
    if let Some(directory) = pattern.strip_suffix('/') {
        return normalized_path == directory
            || normalized_path.starts_with(&format!("{directory}/"));
    }
    if !pattern.contains('/') {
        return normalized_path
            .split('/')
            .any(|component| wildcard_matches(&pattern, component));
    }
    wildcard_matches(&pattern, normalized_path)
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for token in pattern {
        let mut next = vec![false; value.len() + 1];
        if token == '*' {
            next[0] = previous[0];
            for index in 1..=value.len() {
                next[index] = previous[index] || next[index - 1];
            }
        } else {
            for index in 1..=value.len() {
                next[index] = previous[index - 1]
                    && (token == '?' || token.eq_ignore_ascii_case(&value[index - 1]));
            }
        }
        previous = next;
    }
    previous[value.len()]
}

fn is_hex_color(value: &str) -> bool {
    matches!(value.len(), 7 | 9)
        && value.starts_with('#')
        && value[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialized_document_has_sensible_illustrative_defaults() {
        let config = VaultConfig::from_toml_validated(DEFAULT_VAULT_TOML)
            .expect("default vault config parses");
        assert!(config.files.ignore.iter().any(|rule| rule == ".trash"));
        assert_eq!(config.appearance.theme, "solarized_dark");
        assert_eq!(config.appearance.file_color, "#839496");
        assert_eq!(config.appearance.folder_color, "#268bd2");
    }

    #[test]
    fn ignore_rules_are_ordered_and_config_directory_is_always_hidden() {
        let config = VaultConfig {
            files: VaultFilesConfig {
                ignore: vec!["archive/**".into(), "!archive/keep.md".into()],
                ..VaultFilesConfig::default()
            },
            ..VaultConfig::default()
        };
        assert!(config.is_path_ignored(Path::new("archive/old.md")));
        assert!(!config.is_path_ignored(Path::new("archive/keep.md")));
        assert!(config.is_path_ignored(Path::new(".continuity/vault.toml")));
    }

    #[test]
    fn component_pattern_matches_at_any_depth() {
        let config = VaultConfig {
            files: VaultFilesConfig {
                ignore: vec!["*.tmp".into()],
                ..VaultFilesConfig::default()
            },
            ..VaultConfig::default()
        };
        assert!(config.is_path_ignored(Path::new("nested/scratch.tmp")));
        assert!(!config.is_path_ignored(Path::new("nested/scratch.md")));
    }

    #[test]
    fn invalid_delay_and_color_are_rejected() {
        assert!(VaultConfig::from_toml_validated("[save]\ndelay_ms = 10").is_err());
        assert!(
            VaultConfig::from_toml_validated("[appearance]\nfile_color = 'not-a-color'").is_err()
        );
    }
}
