//! `Mode` (dark / light / system) and `ThemeSet` (a paired dark + light
//! theme that resolves to one based on the current `Mode` and an OS
//! `system_dark` flag).
//!
//! Thread ownership: `Mode` is plain data; the `ThemeSet` owner is the UI
//! thread of each window (one resolved theme per window). The OS `system_dark`
//! flag is sampled by the UI thread on `WM_SETTINGCHANGE` /
//! `ImmersiveColorSet`.

use serde::Deserialize;

use crate::Theme;

/// Which theme the editor should display.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Always show the dark theme.
    Dark,
    /// Always show the light theme.
    Light,
    /// Follow the OS theme (Windows light/dark setting).
    #[default]
    System,
}

impl Mode {
    /// Parse a settings-string value (`"dark"` / `"light"` / `"system"`).
    /// Unrecognised values fall back to [`Mode::System`].
    #[must_use]
    pub fn from_str_or_system(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "dark" => Self::Dark,
            "light" => Self::Light,
            _ => Self::System,
        }
    }
}

/// Paired dark + light themes the UI rotates between depending on `Mode`.
///
/// Loaders validate both halves up front via [`Theme::validate_required`],
/// so the typed accessors on `active(...)` cannot panic on a missing key.
#[derive(Debug, Clone)]
pub struct ThemeSet {
    /// Theme used when [`Mode`] resolves to dark.
    pub dark: Theme,
    /// Theme used when [`Mode`] resolves to light.
    pub light: Theme,
}

impl ThemeSet {
    /// Pick the active theme for the given mode and OS state.
    #[must_use]
    pub fn active(&self, mode: Mode, system_dark: bool) -> &Theme {
        match mode {
            Mode::Dark => &self.dark,
            Mode::Light => &self.light,
            Mode::System => {
                if system_dark {
                    &self.dark
                } else {
                    &self.light
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets;

    #[test]
    fn mode_parses_known_values() {
        assert_eq!(Mode::from_str_or_system("dark"), Mode::Dark);
        assert_eq!(Mode::from_str_or_system("LIGHT"), Mode::Light);
        assert_eq!(Mode::from_str_or_system("system"), Mode::System);
        assert_eq!(Mode::from_str_or_system("nonsense"), Mode::System);
    }

    #[test]
    fn theme_set_picks_dark_in_dark_mode() {
        let set = assets::bundled_set().unwrap();
        assert_eq!(set.active(Mode::Dark, false).name, "deep_minimal");
        assert_eq!(set.active(Mode::Dark, true).name, "deep_minimal");
    }

    #[test]
    fn theme_set_picks_light_in_light_mode() {
        let set = assets::bundled_set().unwrap();
        assert_eq!(set.active(Mode::Light, true).name, "paper");
    }

    #[test]
    fn theme_set_follows_os_in_system_mode() {
        let set = assets::bundled_set().unwrap();
        assert_eq!(set.active(Mode::System, true).name, "deep_minimal");
        assert_eq!(set.active(Mode::System, false).name, "paper");
    }
}
