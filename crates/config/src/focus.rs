//! `[focus]` settings section — Phase H1/H2 focus modes.
//!
//! Pulled out of [`crate::settings`] so that file stays under the
//! 600-line cap.

use serde::Deserialize;

/// `[focus]` section — Phase H1/H2 focus modes.
///
/// `dim_alpha` is preferred over the theme key `editor.focus_dim_alpha`
/// when set non-zero (so user-tuning doesn't require a theme reload).
/// `max_column_width` is consumed by distraction-free mode (§H2) to
/// center the body column within the window.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FocusConfig {
    /// §H1 — dim alpha for non-focused source ranges (0.0..=1.0).
    /// Default `0.45`. `0.0` makes the theme key authoritative.
    pub dim_alpha: f32,
    /// §H2 — body column width cap (in characters) when distraction-free
    /// mode is active. Default `80`.
    pub max_column_width: u32,
    /// §H1 — initial focus mode at window startup. One of
    /// `"off" | "line" | "sentence" | "paragraph"`. Default `"off"`.
    pub initial_mode: String,
    /// §H2 — start every window in distraction-free mode. Default `false`.
    pub distraction_free_on_launch: bool,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self {
            dim_alpha: 0.45,
            max_column_width: 80,
            initial_mode: "off".into(),
            distraction_free_on_launch: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::FocusMode;

    #[test]
    fn focus_defaults_align_with_pane_modes_spec() {
        let f = FocusConfig::default();
        assert!((f.dim_alpha - 0.45).abs() < 1e-6);
        assert_eq!(f.max_column_width, 80);
        assert_eq!(f.initial_mode, "off");
        assert!(!f.distraction_free_on_launch);
    }

    #[test]
    fn initial_mode_round_trips_through_focus_mode_parse() {
        // §H1 — the ui-side `apply_focus_initial_mode` parses
        // `s.focus.initial_mode` and writes the result to
        // `PaneModesState.focus_mode`. Verifies the contract: every
        // valid token round-trips back to the same `FocusMode` value
        // and the resulting `as_str()` matches the input.
        for token in ["off", "line", "sentence", "paragraph"] {
            let f = FocusConfig {
                initial_mode: token.into(),
                ..FocusConfig::default()
            };
            let parsed = FocusMode::parse(&f.initial_mode).expect("token parses");
            assert_eq!(parsed.as_str(), token);
        }
    }

    #[test]
    fn dim_alpha_zero_means_theme_wins() {
        // §H1 — `[focus].dim_alpha == 0.0` is the documented signal to
        // fall through to the theme key `editor.focus_dim_alpha`. The
        // ui-side resolver compares against `f32::EPSILON`; verifies
        // exact zero stays below the threshold.
        let f = FocusConfig {
            dim_alpha: 0.0,
            ..FocusConfig::default()
        };
        assert!(f.dim_alpha <= f32::EPSILON);
    }
}
