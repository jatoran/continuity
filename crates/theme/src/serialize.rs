//! Deterministic TOML serialization for [`Theme`]. Powers the δ.5 commands
//! that have to write a theme TOML to disk: `theme.clone`,
//! `theme.duplicate`, `theme.create_blank`.
//!
//! The output is hand-rolled rather than going through `toml::to_string`
//! so the on-disk form has stable ordering and uses the same `#rrggbb` /
//! `#rrggbbaa` syntax humans wrote in `crates/theme/assets/*.toml`. Keys
//! emit in [`crate::keys::REQUIRED_KEYS`] order, then any extras in
//! lexicographic order so the file is reproducible across runs.
//!
//! Thread ownership: stateless — pure data transform, callable from any
//! thread.

use std::fmt::Write;

use crate::{keys::REQUIRED_KEYS, Color, Theme};

impl Theme {
    /// Render this theme as a deterministic TOML string. Required keys
    /// appear first in [`REQUIRED_KEYS`] order; any non-required extras
    /// (kept around so a `theme.clone` round-trip preserves user
    /// experiments) appear after in lexicographic order.
    ///
    /// The function never fails — `String::push_str` cannot fail and
    /// [`Color`] always round-trips through hex.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut out = String::with_capacity(2048);
        let _ = writeln!(out, "name = {}", quote_toml_string(&self.name));
        out.push('\n');
        out.push_str("[colors]\n");
        for key in REQUIRED_KEYS {
            if let Some(color) = self.colors.get(*key) {
                write_color_line(&mut out, key, *color);
            }
        }
        let required_set: std::collections::BTreeSet<&str> =
            REQUIRED_KEYS.iter().copied().collect();
        for (key, color) in &self.colors {
            if required_set.contains(key.as_str()) {
                continue;
            }
            write_color_line(&mut out, key, *color);
        }
        out
    }
}

fn write_color_line(out: &mut String, key: &str, color: Color) {
    let _ = writeln!(
        out,
        "{} = {}",
        quote_toml_string(key),
        quote_toml_string(&format_color(color)),
    );
}

fn quote_toml_string(s: &str) -> String {
    // Theme keys / names use a restricted alphabet (we control them) so a
    // basic-string escape is enough. No theme key contains `"` or `\`.
    let mut buf = String::with_capacity(s.len() + 2);
    buf.push('"');
    for ch in s.chars() {
        match ch {
            '"' => buf.push_str("\\\""),
            '\\' => buf.push_str("\\\\"),
            '\n' => buf.push_str("\\n"),
            '\r' => buf.push_str("\\r"),
            '\t' => buf.push_str("\\t"),
            c => buf.push(c),
        }
    }
    buf.push('"');
    buf
}

fn format_color(c: Color) -> String {
    if c.a == 0xff {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
    }
}

#[cfg(test)]
mod tests {
    use crate::assets::{bundled_dark, neutral_fallback};
    use crate::Theme;

    #[test]
    fn round_trips_through_load() {
        let original = bundled_dark().unwrap();
        let toml = original.to_toml();
        let reloaded = Theme::load(&toml).expect("serialized bundled theme must validate");
        assert_eq!(original.name, reloaded.name);
        for (key, color) in &original.colors {
            assert_eq!(
                reloaded.colors.get(key),
                Some(color),
                "color drift on key `{key}`",
            );
        }
    }

    #[test]
    fn neutral_fallback_serializes_and_validates() {
        let t = neutral_fallback();
        let toml = t.to_toml();
        Theme::load(&toml).expect("neutral fallback round-trips");
    }

    #[test]
    fn output_is_stable_across_calls() {
        let t = bundled_dark().unwrap();
        assert_eq!(t.to_toml(), t.to_toml());
    }

    #[test]
    fn required_keys_appear_before_extras() {
        let mut t = bundled_dark().unwrap();
        // Add an experimental color that's NOT in REQUIRED_KEYS.
        t.colors.insert(
            "experimental.ufo".into(),
            crate::Color::rgb(0xab, 0xcd, 0xef),
        );
        let toml = t.to_toml();
        let pos_required = toml
            .find("\"window.background\"")
            .expect("required key present");
        let pos_extra = toml
            .find("\"experimental.ufo\"")
            .expect("extra key present");
        assert!(pos_required < pos_extra, "required keys come first");
    }

    #[test]
    fn alpha_channel_uses_eight_digit_hex() {
        let toml = neutral_fallback().to_toml();
        // The neutral fallback has a translucent selection fill; alpha-
        // bearing colors must render with 8 hex digits, never six.
        assert!(
            toml.contains("#88888840"),
            "expected translucent editor.selection in serialized TOML",
        );
    }

    #[test]
    fn opaque_colors_use_six_digit_hex() {
        let toml = neutral_fallback().to_toml();
        // `window.background` is fully opaque in the neutral fallback.
        assert!(
            toml.contains("\"window.background\" = \"#202022\""),
            "opaque colors must render without a trailing alpha pair",
        );
    }
}
