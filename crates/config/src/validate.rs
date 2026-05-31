//! `Settings::validate` — checks every enum field and numeric range.
//!
//! Validation runs after deserialization. Defaults are valid by
//! construction (verified by tests in this module).

use crate::mode::{
    CaretStyle, FocusMode, MarkdownDialect, PersistenceMode, RevealMode, StatusBarSegment,
    TabCloseButton, ThemeMode,
};
use crate::{Error, Settings};

impl Settings {
    /// Validate every enum and numeric range.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error::Invalid`] encountered. Iteration order is
    /// stable (sections in declaration order, fields top-down) so error
    /// messages don't churn between runs.
    pub fn validate(&self) -> Result<(), Error> {
        // -- persistence ------------------------------------------------
        PersistenceMode::parse(&self.persistence.mode)?;
        check_range(
            "persistence.debounce_ms",
            self.persistence.debounce_ms,
            10..=10_000,
            "10..=10000",
        )?;
        check_range(
            "persistence.snapshot_every_edits",
            self.persistence.snapshot_every_edits,
            1..=100_000,
            "1..=100000",
        )?;
        check_range(
            "persistence.snapshot_every_bytes",
            self.persistence.snapshot_every_bytes,
            1024..=u32::MAX,
            "1024..=u32::MAX",
        )?;
        check_range(
            "persistence.trash_retention_days",
            self.persistence.trash_retention_days,
            0..=3650,
            "0..=3650",
        )?;

        // -- backup -----------------------------------------------------
        check_range(
            "backup.interval_minutes",
            self.backup.interval_minutes,
            1..=1440,
            "1..=1440",
        )?;
        check_range(
            "backup.hourly_retention",
            self.backup.hourly_retention,
            0..=240,
            "0..=240",
        )?;
        check_range(
            "backup.daily_retention",
            self.backup.daily_retention,
            0..=365,
            "0..=365",
        )?;

        // -- workers ----------------------------------------------------
        check_range(
            "workers.decoration_watchdog_ms",
            self.workers.decoration_watchdog_ms,
            100..=600_000,
            "100..=600000",
        )?;

        // -- editor -----------------------------------------------------
        check_f32_range(
            "editor.font_size",
            self.editor.font_size,
            6.0..=72.0,
            "6.0..=72.0",
        )?;
        check_f32_range(
            "editor.line_height",
            self.editor.line_height,
            0.8..=3.0,
            "0.8..=3.0",
        )?;
        CaretStyle::parse(&self.editor.caret_style)?;
        check_range(
            "editor.caret_blink_ms",
            self.editor.caret_blink_ms,
            0..=10_000,
            "0..=10000",
        )?;
        check_range(
            "editor.caret_width_px",
            self.editor.caret_width_px,
            1..=16,
            "1..=16",
        )?;
        check_range(
            "editor.caret_typing_pause_ms",
            self.editor.caret_typing_pause_ms,
            0..=10_000,
            "0..=10000",
        )?;
        check_range(
            "editor.caret_long_idle_ms",
            self.editor.caret_long_idle_ms,
            0..=600_000,
            "0..=600000",
        )?;
        check_range(
            "editor.caret_tween_threshold_rows",
            self.editor.caret_tween_threshold_rows,
            0..=1_000,
            "0..=1000",
        )?;
        check_range(
            "editor.caret_tween_duration_ms",
            self.editor.caret_tween_duration_ms,
            0..=2_000,
            "0..=2000",
        )?;
        // Phase B17: glyph must be exactly one character (renderer paints
        // it once per continuation row; allowing multi-char gets weird).
        if self.editor.soft_wrap_indicator_glyph.chars().count() != 1 {
            return Err(Error::Invalid {
                field: "editor.soft_wrap_indicator_glyph",
                value: self.editor.soft_wrap_indicator_glyph.clone(),
                allowed: "exactly one character",
            });
        }
        validate_caret_color("editor.caret_color", &self.editor.caret_color)?;
        validate_caret_color(
            "editor.caret_secondary_color",
            &self.editor.caret_secondary_color,
        )?;
        check_f32_range(
            "editor.mouse_wheel_scroll_speed",
            self.editor.mouse_wheel_scroll_speed,
            0.25..=8.0,
            "0.25..=8.0",
        )?;
        check_range(
            "editor.zoom_step_pct",
            self.editor.zoom_step_pct,
            1..=100,
            "1..=100",
        )?;
        for (i, col) in self.editor.ruler_columns.iter().enumerate() {
            if *col == 0 || *col > 1024 {
                return Err(Error::Invalid {
                    field: "editor.ruler_columns",
                    value: format!("[{i}]={col}"),
                    allowed: "1..=1024 per element",
                });
            }
        }

        // -- markdown ---------------------------------------------------
        RevealMode::parse(&self.markdown.reveal_mode)?;
        MarkdownDialect::parse(&self.markdown.dialect)?;
        if self.markdown.heading_scale.len() != 6 {
            return Err(Error::Invalid {
                field: "markdown.heading_scale",
                value: format!("len={}", self.markdown.heading_scale.len()),
                allowed: "exactly 6 entries (one per heading level)",
            });
        }
        for (i, scale) in self.markdown.heading_scale.iter().enumerate() {
            if !(0.5..=8.0).contains(scale) {
                return Err(Error::Invalid {
                    field: "markdown.heading_scale",
                    value: format!("[{i}]={scale}"),
                    allowed: "0.5..=8.0 per element",
                });
            }
        }

        // -- ui ---------------------------------------------------------
        ThemeMode::parse(&self.ui.theme)?;
        TabCloseButton::parse(&self.ui.tab_close_button)?;

        // -- statusbar --------------------------------------------------
        for s in &self.statusbar.segments {
            StatusBarSegment::parse(s)?;
        }

        // -- focus (Phase H1/H2) ---------------------------------------
        FocusMode::parse(&self.focus.initial_mode)?;
        check_f32_range(
            "focus.dim_alpha",
            self.focus.dim_alpha,
            0.0..=1.0,
            "0.0..=1.0",
        )?;
        check_range(
            "focus.max_column_width",
            self.focus.max_column_width,
            20..=300,
            "20..=300",
        )?;

        Ok(())
    }
}

fn check_range(
    field: &'static str,
    value: u32,
    range: std::ops::RangeInclusive<u32>,
    allowed: &'static str,
) -> Result<(), Error> {
    if range.contains(&value) {
        Ok(())
    } else {
        Err(Error::invalid_range(field, value, allowed))
    }
}

fn check_f32_range(
    field: &'static str,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    allowed: &'static str,
) -> Result<(), Error> {
    if range.contains(&value) {
        Ok(())
    } else {
        Err(Error::invalid_range(field, value, allowed))
    }
}

/// Phase B4 caret-color validator. Accepts:
///   * empty string (fall through to theme key `editor.cursor.primary`)
///   * `#rrggbb` or `#rrggbbaa` literals (ASCII hex)
///   * any dotted theme-key reference (e.g. `editor.cursor.primary`)
///
/// The theme-key path is checked syntactically — the actual key
/// existence is the theme loader's concern, since themes are
/// pluggable.
fn validate_caret_color(field: &'static str, value: &str) -> Result<(), Error> {
    if value.is_empty() {
        return Ok(());
    }
    if let Some(hex) = value.strip_prefix('#') {
        let len_ok = matches!(hex.len(), 6 | 8);
        let all_hex = hex.chars().all(|c| c.is_ascii_hexdigit());
        if len_ok && all_hex {
            return Ok(());
        }
        return Err(Error::Invalid {
            field,
            value: value.to_string(),
            allowed: "#rrggbb | #rrggbbaa | theme-key | empty",
        });
    }
    if value.contains('.')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
    {
        return Ok(());
    }
    Err(Error::Invalid {
        field,
        value: value.to_string(),
        allowed: "#rrggbb | #rrggbbaa | theme-key | empty",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_pass_validation() {
        Settings::default().validate().expect("defaults are valid");
    }

    #[test]
    fn rejects_bad_persistence_mode() {
        let mut s = Settings::default();
        s.persistence.mode = "safe".into();
        let err = s.validate().unwrap_err();
        assert!(format!("{err}").contains("persistence.mode"));
    }

    #[test]
    fn rejects_too_small_font() {
        let mut s = Settings::default();
        s.editor.font_size = 4.0;
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_wrong_heading_scale_len() {
        let mut s = Settings::default();
        s.markdown.heading_scale = vec![1.0, 1.0, 1.0];
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_zero_ruler_column() {
        let mut s = Settings::default();
        s.editor.ruler_columns = vec![80, 0];
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_caret_width_out_of_range() {
        let mut s = Settings::default();
        s.editor.caret_width_px = 0;
        assert!(s.validate().is_err());
        let mut s = Settings::default();
        s.editor.caret_width_px = 64;
        assert!(s.validate().is_err());
    }

    #[test]
    fn accepts_caret_color_hex_and_theme_key() {
        let mut s = Settings::default();
        s.editor.caret_color = "#ff8800".into();
        s.editor.caret_secondary_color = "editor.cursor.secondary".into();
        s.validate().expect("both forms valid");
    }

    #[test]
    fn accepts_caret_color_with_alpha() {
        let mut s = Settings::default();
        s.editor.caret_color = "#ff8800cc".into();
        s.validate().expect("rgba hex valid");
    }

    #[test]
    fn rejects_malformed_caret_color() {
        let mut s = Settings::default();
        s.editor.caret_color = "#zz".into();
        assert!(s.validate().is_err());
        let mut s = Settings::default();
        s.editor.caret_color = "notATheme".into();
        assert!(s.validate().is_err());
    }

    #[test]
    fn empty_caret_color_falls_through() {
        let mut s = Settings::default();
        s.editor.caret_color.clear();
        s.editor.caret_secondary_color.clear();
        s.validate().expect("empty allowed");
    }

    #[test]
    fn rejects_empty_soft_wrap_glyph() {
        let mut s = Settings::default();
        s.editor.soft_wrap_indicator_glyph.clear();
        assert!(s.validate().is_err());
        let mut s = Settings::default();
        s.editor.soft_wrap_indicator_glyph = "→→".into();
        assert!(s.validate().is_err());
    }

    #[test]
    fn caret_typing_pause_zero_is_valid() {
        let mut s = Settings::default();
        s.editor.caret_typing_pause_ms = 0;
        s.validate().expect("0 ok");
    }

    #[test]
    fn rejects_mouse_wheel_scroll_speed_out_of_range() {
        let mut s = Settings::default();
        s.editor.mouse_wheel_scroll_speed = 0.0;
        assert!(s.validate().is_err());
        let mut s = Settings::default();
        s.editor.mouse_wheel_scroll_speed = 20.0;
        assert!(s.validate().is_err());
    }

    #[test]
    fn from_toml_validated_pipeline() {
        let s = Settings::from_toml_validated(
            r#"[persistence]
mode = "max_safety"
"#,
        )
        .expect("ok");
        assert_eq!(s.persistence.mode, "max_safety");
    }

    #[test]
    fn rejects_unknown_statusbar_segment() {
        let mut s = Settings::default();
        s.statusbar.segments.push("weather".into());
        assert!(s.validate().is_err());
    }

    #[test]
    fn accepts_empty_statusbar_segments_list() {
        let mut s = Settings::default();
        s.statusbar.segments.clear();
        s.validate()
            .expect("empty list is valid (status bar paints blank strip)");
    }

    #[test]
    fn rejects_focus_dim_alpha_out_of_range() {
        let mut s = Settings::default();
        s.focus.dim_alpha = 1.5;
        assert!(s.validate().is_err());
        let mut s = Settings::default();
        s.focus.dim_alpha = -0.1;
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_unknown_focus_initial_mode() {
        let mut s = Settings::default();
        s.focus.initial_mode = "typewriter".into();
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_decoration_watchdog_timeout_out_of_range() {
        let mut s = Settings::default();
        s.workers.decoration_watchdog_ms = 99;
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_focus_max_column_width_out_of_range() {
        let mut s = Settings::default();
        s.focus.max_column_width = 5;
        assert!(s.validate().is_err());
        let mut s = Settings::default();
        s.focus.max_column_width = 500;
        assert!(s.validate().is_err());
    }

    #[test]
    fn from_toml_validated_rejects_bad_enum() {
        let err = Settings::from_toml_validated(
            r#"[persistence]
mode = "wat"
"#,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("persistence.mode"));
    }
}
