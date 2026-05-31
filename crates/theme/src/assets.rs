//! Bundled themes compiled into the binary via `include_str!`.
//!
//! The two bundled defaults are `deep_minimal` (dark) and `paper` (light).
//! The crate also exposes a hard-coded `neutral_fallback` used only when a
//! user-installed theme fails validation and the bundled themes themselves
//! cannot be loaded — guaranteed by tests in this module.
//!
//! Thread ownership: stateless, callable from any thread.

use crate::{Mode, Theme, ThemeSet};

/// `deep_minimal` (dark) source TOML, baked into the binary.
pub(crate) const DEEP_MINIMAL_TOML: &str = include_str!("../assets/deep_minimal.toml");
/// `paper` (light) source TOML, baked into the binary.
pub(crate) const PAPER_TOML: &str = include_str!("../assets/paper.toml");

// §E5 — Solarized family.
/// Canonical Ethan Schoonover Solarized Dark, baked into the binary.
pub(crate) const SOLARIZED_DARK_TOML: &str = include_str!("../assets/solarized_dark.toml");
/// Higher-contrast Solarized variant (pure-black background) baked into
/// the binary.
pub(crate) const SOLARIZED_DARKER_TOML: &str = include_str!("../assets/solarized_darker.toml");
/// Canonical Solarized Light, baked into the binary.
pub(crate) const SOLARIZED_LIGHT_TOML: &str = include_str!("../assets/solarized_light.toml");

/// Classic Sublime Text Monokai palette, baked into the binary.
pub(crate) const MONOKAI_TOML: &str = include_str!("../assets/monokai.toml");
/// Canonical Rose Pine palette, baked into the binary.
pub(crate) const ROSE_PINE_TOML: &str = include_str!("../assets/rose_pine.toml");

/// Catppuccin Mocha (dark) palette, baked into the binary.
pub(crate) const CATPPUCCIN_MOCHA_TOML: &str = include_str!("../assets/catppuccin_mocha.toml");
/// Catppuccin Macchiato (dark) palette, baked into the binary.
pub(crate) const CATPPUCCIN_MACCHIATO_TOML: &str =
    include_str!("../assets/catppuccin_macchiato.toml");
/// Catppuccin Frappé (dark) palette, baked into the binary.
pub(crate) const CATPPUCCIN_FRAPPE_TOML: &str = include_str!("../assets/catppuccin_frappe.toml");
/// Catppuccin Latte (light) palette, baked into the binary.
pub(crate) const CATPPUCCIN_LATTE_TOML: &str = include_str!("../assets/catppuccin_latte.toml");
/// Tokyo Night (storm) palette, baked into the binary.
pub(crate) const TOKYO_NIGHT_TOML: &str = include_str!("../assets/tokyo_night.toml");
/// Nord (arctic) palette, baked into the binary.
pub(crate) const NORD_TOML: &str = include_str!("../assets/nord.toml");
/// One Dark (Atom-origin) palette, baked into the binary.
pub(crate) const ONE_DARK_TOML: &str = include_str!("../assets/one_dark.toml");
/// Gruvbox Dark (hard) palette, baked into the binary.
pub(crate) const GRUVBOX_DARK_TOML: &str = include_str!("../assets/gruvbox_dark.toml");
/// Gruvbox Light (hard) palette, baked into the binary.
pub(crate) const GRUVBOX_LIGHT_TOML: &str = include_str!("../assets/gruvbox_light.toml");
/// Dracula palette, baked into the binary.
pub(crate) const DRACULA_TOML: &str = include_str!("../assets/dracula.toml");

/// Names of every bundled theme (`deep_minimal`, `paper`,
/// `solarized_dark`, `solarized_darker`, `solarized_light`, `monokai`,
/// `rose_pine`, `catppuccin_mocha`, `catppuccin_macchiato`,
/// `catppuccin_frappe`, `catppuccin_latte`, `tokyo_night`, `nord`,
/// `one_dark`, `gruvbox_dark`, `gruvbox_light`, `dracula`). Used by the
/// E4 picker enumeration so callers can list every bundled theme
/// without keeping their own copy of the list.
pub const BUNDLED_NAMES: &[&str] = &[
    "deep_minimal",
    "paper",
    "solarized_dark",
    "solarized_darker",
    "solarized_light",
    "monokai",
    "rose_pine",
    "catppuccin_mocha",
    "catppuccin_macchiato",
    "catppuccin_frappe",
    "catppuccin_latte",
    "tokyo_night",
    "nord",
    "one_dark",
    "gruvbox_dark",
    "gruvbox_light",
    "dracula",
];

/// Resolve a bundled theme name to its source TOML, or `None` when the
/// name is not bundled.
#[must_use]
pub(crate) fn bundled_toml(name: &str) -> Option<&'static str> {
    Some(match name {
        "deep_minimal" => DEEP_MINIMAL_TOML,
        "paper" => PAPER_TOML,
        "solarized_dark" => SOLARIZED_DARK_TOML,
        "solarized_darker" => SOLARIZED_DARKER_TOML,
        "solarized_light" => SOLARIZED_LIGHT_TOML,
        "monokai" => MONOKAI_TOML,
        "rose_pine" => ROSE_PINE_TOML,
        "catppuccin_mocha" => CATPPUCCIN_MOCHA_TOML,
        "catppuccin_macchiato" => CATPPUCCIN_MACCHIATO_TOML,
        "catppuccin_frappe" => CATPPUCCIN_FRAPPE_TOML,
        "catppuccin_latte" => CATPPUCCIN_LATTE_TOML,
        "tokyo_night" => TOKYO_NIGHT_TOML,
        "nord" => NORD_TOML,
        "one_dark" => ONE_DARK_TOML,
        "gruvbox_dark" => GRUVBOX_DARK_TOML,
        "gruvbox_light" => GRUVBOX_LIGHT_TOML,
        "dracula" => DRACULA_TOML,
        _ => return None,
    })
}

/// Load a bundled theme by name.
///
/// # Errors
///
/// Returns [`crate::Error::UnknownTheme`] when `name` is not bundled,
/// or any error returned by [`crate::Theme::load`] for the matching
/// TOML.
pub fn bundled_named(name: &str) -> Result<crate::Theme, crate::Error> {
    let toml = bundled_toml(name).ok_or_else(|| crate::Error::UnknownTheme(name.to_string()))?;
    crate::Theme::load(toml)
}

/// Load the bundled `deep_minimal` (dark) theme.
///
/// # Errors
///
/// Returns a `theme::Error` only if `assets/deep_minimal.toml` is malformed
/// or missing keys; the canary test in this module guarantees it loads.
pub fn bundled_dark() -> Result<Theme, crate::Error> {
    Theme::load(DEEP_MINIMAL_TOML)
}

/// Load the bundled `paper` (light) theme.
///
/// # Errors
///
/// Returns a `theme::Error` only if `assets/paper.toml` is malformed
/// or missing keys; the canary test in this module guarantees it loads.
pub(crate) fn bundled_light() -> Result<Theme, crate::Error> {
    Theme::load(PAPER_TOML)
}

/// Load the bundled dark + light pair.
///
/// # Errors
///
/// Mirrors [`bundled_dark`] / [`bundled_light`] — the canary tests
/// guarantee both succeed.
pub fn bundled_set() -> Result<ThemeSet, crate::Error> {
    Ok(ThemeSet {
        dark: bundled_dark()?,
        light: bundled_light()?,
    })
}

/// Hard-coded fallback theme — every required key resolves but the palette
/// is intentionally bland. Used when a user-installed theme fails parse and
/// the bundled defaults cannot be reached. Validates all required keys.
///
/// # Panics
///
/// Doesn't, in practice — the const-shaped TOML is verified by the
/// `neutral_fallback_validates` test. The `expect` reflects that invariant.
#[must_use]
pub fn neutral_fallback() -> Theme {
    Theme::load(NEUTRAL_TOML).expect("invariant: neutral fallback theme must validate")
}

/// Build the active theme for `mode` / `system_dark`, falling back through
/// `installed -> bundled -> neutral` so the editor always paints something.
#[must_use]
pub fn resolve_active(installed: Option<&ThemeSet>, mode: Mode, system_dark: bool) -> Theme {
    if let Some(set) = installed {
        return set.active(mode, system_dark).clone();
    }
    if let Ok(set) = bundled_set() {
        return set.active(mode, system_dark).clone();
    }
    neutral_fallback()
}

/// Neutral palette: middle-grey on off-white. Picked so that no missing-key
/// crash will ever happen — the editor always renders.
const NEUTRAL_TOML: &str = r##"
name = "neutral"

[colors]
"window.background"                 = "#202022"
"window.foreground"                 = "#d0d0d0"

"panel.background"                  = "#262628"
"panel.foreground"                  = "#c0c0c0"
"panel.active_tab.background"       = "#303033"
"panel.active_tab.foreground"       = "#f0f0f0"
"panel.inactive_tab.background"     = "#1c1c1e"
"panel.inactive_tab.foreground"     = "#888888"

"pane.border"                       = "#303033"
"pane.border_active"                = "#888888"

"editor.background"                 = "#202022"
"editor.foreground"                 = "#d0d0d0"
"editor.cursor.primary"             = "#cccccc"
"editor.cursor.secondary"           = "#888888"
"editor.selection"                  = "#88888840"
"editor.selection_inactive"         = "#88888820"
"editor.line_highlight"             = "#262628"
"editor.line_number"                = "#666666"
"editor.line_number_active"         = "#aaaaaa"
"editor.indent_guide"               = "#2a2a2c"
"editor.indent_guide_active"        = "#3a3a3c"
"editor.search_match"               = "#dddd6680"
"editor.search_match_active"        = "#ffee88b0"
"editor.find_bar.background"        = "#262628"
"editor.search_minimap.background"  = "#1c1c1ec0"
"editor.search_minimap.match"       = "#dddd66b0"
"editor.search_minimap.match_active" = "#ffee88ff"
"editor.minimap.background"          = "#1c1c1ec0"
"editor.minimap.foreground"          = "#cfcfcfaa"
"editor.minimap.viewport_indicator"  = "#ffffff14"
"editor.loading_overlay.background"  = "#1c1c1ed8"
"editor.loading_overlay.foreground"  = "#d0d0d0"
"editor.loading_overlay.border"      = "#3a3a3c"
"editor.caret_jump_glow"            = "#ffd86a40"
"editor.edit_pulse"                 = "#9ecbff30"
"editor.pair_rainbow.0"             = "#dddddd"
"editor.pair_rainbow.1"             = "#bbbbbb"
"editor.pair_rainbow.2"             = "#999999"
"editor.pair_rainbow.3"             = "#bbbbbb"
"editor.pair_rainbow.4"             = "#dddddd"
"editor.pair_rainbow.5"             = "#bbbbbb"
"editor.soft_wrap_indicator"        = "#666666"
"editor.breadcrumb.foreground"      = "#888888"
"editor.breadcrumb.separator"       = "#555555"
"editor.breadcrumb.active"          = "#cccccc"
"editor.outline.background"         = "#262628"
"editor.outline.foreground"         = "#a0a0a0"
"editor.outline.foreground_active"  = "#e0e0e0"
"editor.outline.separator"          = "#303033"
"editor.inline_highlight.foreground" = "#1a1a1c"
"editor.inline_highlight.background" = "#e6c66060"
"editor.focus_dim_alpha"             = "#00000073"
"editor.foreground_dim"              = "#6a6a6a"

"markdown.heading.1"                = "#dddddd"
"markdown.heading.2"                = "#c8c8c8"
"markdown.heading.3"                = "#b8b8b8"
"markdown.heading.4"                = "#a8a8a8"
"markdown.heading.5"                = "#989898"
"markdown.heading.6"                = "#888888"
"markdown.bold"                     = "#f0f0f0"
"markdown.italic"                   = "#cccccc"
"markdown.strikethrough"            = "#777777"
"markdown.code.foreground"          = "#dddddd"
"markdown.code.background"          = "#2c2c2e"
"markdown.code_block.background"    = "#26262a"
"markdown.code_block.border"        = "#3a3a3e"
"markdown.blockquote.foreground"    = "#a0a0a0"
"markdown.blockquote.bar"           = "#666666"
"markdown.link"                     = "#9090c0"
"markdown.footnote"                 = "#9090c0"
"markdown.url"                      = "#7070a0"
"markdown.image_alt"                = "#909090"
"markdown.list_marker"              = "#cccccc"
"markdown.checkbox.checked"         = "#80c080"
"markdown.checkbox.unchecked"       = "#888888"
"markdown.hr"                       = "#666666"
"markdown.table.border"             = "#3a3a3e"
"markdown.table.header_bg"          = "#2a2a2e"
"markdown.table.alignment_bg"       = "#1f1f24"
"markdown.table.active_cell_outline" = "#cccccc"
"markdown.formula.value"            = "#80c080"
"markdown.formula.error"            = "#e06060"

"status.background"                 = "#1c1c1e"
"status.foreground"                 = "#c0c0c0"
"status.error"                      = "#cc6666"
"status.warn"                       = "#cccc66"
"status.info"                       = "#6666cc"

"overlay.background"                = "#26262a"
"overlay.shadow"                    = "#00000080"

"palette.background"                = "#26262a"
"palette.match_highlight"           = "#cccc66"
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_minimal_loads_and_validates() {
        let t = bundled_dark().expect("deep_minimal.toml validates");
        assert_eq!(t.name, "deep_minimal");
    }

    #[test]
    fn paper_loads_and_validates() {
        let t = bundled_light().expect("paper.toml validates");
        assert_eq!(t.name, "paper");
    }

    #[test]
    fn bundled_set_loads_both() {
        let set = bundled_set().expect("bundled set validates");
        assert_eq!(set.dark.name, "deep_minimal");
        assert_eq!(set.light.name, "paper");
    }

    #[test]
    fn neutral_fallback_validates() {
        let t = neutral_fallback();
        assert_eq!(t.name, "neutral");
    }

    #[test]
    fn resolve_active_uses_installed_when_present() {
        let set = bundled_set().unwrap();
        let t = resolve_active(Some(&set), Mode::Dark, false);
        assert_eq!(t.name, "deep_minimal");
    }

    #[test]
    fn resolve_active_falls_back_to_bundled_when_no_installed() {
        let t = resolve_active(None, Mode::Light, false);
        assert_eq!(t.name, "paper");
    }

    #[test]
    fn solarized_family_loads_and_validates() {
        let dark = bundled_named("solarized_dark").expect("solarized_dark validates");
        assert_eq!(dark.name, "solarized_dark");
        let darker = bundled_named("solarized_darker").expect("solarized_darker validates");
        assert_eq!(darker.name, "solarized_darker");
        let light = bundled_named("solarized_light").expect("solarized_light validates");
        assert_eq!(light.name, "solarized_light");
    }

    #[test]
    fn monokai_loads_and_validates() {
        let t = bundled_named("monokai").expect("monokai.toml validates");
        assert_eq!(t.name, "monokai");
    }

    #[test]
    fn rose_pine_loads_and_validates() {
        let t = bundled_named("rose_pine").expect("rose_pine.toml validates");
        assert_eq!(t.name, "rose_pine");
    }

    #[test]
    fn catppuccin_family_loads_and_validates() {
        for name in [
            "catppuccin_mocha",
            "catppuccin_macchiato",
            "catppuccin_frappe",
            "catppuccin_latte",
        ] {
            let t = bundled_named(name).expect("catppuccin variant validates");
            assert_eq!(t.name, name);
        }
    }

    #[test]
    fn tokyo_night_loads_and_validates() {
        let t = bundled_named("tokyo_night").expect("tokyo_night.toml validates");
        assert_eq!(t.name, "tokyo_night");
    }

    #[test]
    fn nord_loads_and_validates() {
        let t = bundled_named("nord").expect("nord.toml validates");
        assert_eq!(t.name, "nord");
    }

    #[test]
    fn one_dark_loads_and_validates() {
        let t = bundled_named("one_dark").expect("one_dark.toml validates");
        assert_eq!(t.name, "one_dark");
    }

    #[test]
    fn gruvbox_family_loads_and_validates() {
        let dark = bundled_named("gruvbox_dark").expect("gruvbox_dark validates");
        assert_eq!(dark.name, "gruvbox_dark");
        let light = bundled_named("gruvbox_light").expect("gruvbox_light validates");
        assert_eq!(light.name, "gruvbox_light");
    }

    #[test]
    fn dracula_loads_and_validates() {
        let t = bundled_named("dracula").expect("dracula.toml validates");
        assert_eq!(t.name, "dracula");
    }

    #[test]
    fn bundled_named_unknown_errors() {
        let err = bundled_named("not_a_theme").unwrap_err();
        assert!(matches!(err, crate::Error::UnknownTheme(_)));
    }

    #[test]
    fn bundled_names_lists_every_bundled() {
        let names = BUNDLED_NAMES.to_vec();
        for name in &names {
            // Every name resolves.
            let _ = bundled_named(name).expect("bundled");
        }
        // Exactly the expected set, no extras.
        assert!(names.contains(&"deep_minimal"));
        assert!(names.contains(&"paper"));
        assert!(names.contains(&"solarized_dark"));
        assert!(names.contains(&"solarized_darker"));
        assert!(names.contains(&"solarized_light"));
        assert!(names.contains(&"monokai"));
        assert!(names.contains(&"rose_pine"));
        assert!(names.contains(&"catppuccin_mocha"));
        assert!(names.contains(&"catppuccin_macchiato"));
        assert!(names.contains(&"catppuccin_frappe"));
        assert!(names.contains(&"catppuccin_latte"));
        assert!(names.contains(&"tokyo_night"));
        assert!(names.contains(&"nord"));
        assert!(names.contains(&"one_dark"));
        assert!(names.contains(&"gruvbox_dark"));
        assert!(names.contains(&"gruvbox_light"));
        assert!(names.contains(&"dracula"));
    }
}
