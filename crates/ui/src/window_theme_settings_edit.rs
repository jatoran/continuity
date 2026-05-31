//! δ.5 — in-place TOML editing helpers used by the theme-management
//! commands when they need to update `[ui] theme_dark` / `theme_light`
//! in the user's `settings.toml`.
//!
//! The helpers operate line-by-line so comments, formatting, and
//! unrelated keys survive untouched. Two entry points:
//!
//! - [`update_settings_theme_binding_if`] rewrites the binding only
//!   when the existing value satisfies a predicate. Used by
//!   `theme.rename` and `theme.delete` to surgically retarget the
//!   binding when (and only when) it pointed at the affected theme.
//! - [`write_settings_theme_binding`] unconditionally sets the binding
//!   on the supplied slot, inserting the line — or a fresh `[ui]`
//!   section — when none exists. Used by `theme.clone`,
//!   `theme.duplicate`, and `theme.create_blank` where activation
//!   always changes the binding.
//!
//! Thread ownership: stateless — pure data transforms over file
//! contents. The callers serialize writes through the UI thread.

use std::path::Path;

use crate::window_theme_atomic_write::atomic_write;

/// Settings slot a theme is bound to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ThemeSlot {
    /// `[ui] theme_dark`.
    Dark,
    /// `[ui] theme_light`.
    Light,
}

impl ThemeSlot {
    pub(crate) fn settings_key(self) -> &'static str {
        match self {
            ThemeSlot::Dark => "theme_dark",
            ThemeSlot::Light => "theme_light",
        }
    }
}

/// Update either `theme_dark` or `theme_light` in `settings.toml` to
/// `new_value` when the existing value matches `predicate`. Preserves
/// comments and unrelated keys by editing the file line by line; if the
/// `[ui]` section is absent the file is left untouched (no implicit
/// rewrites).
pub(crate) fn update_settings_theme_binding_if<F>(
    settings_path: &Path,
    predicate: F,
    new_value: &str,
) -> std::io::Result<()>
where
    F: Fn(&str) -> bool,
{
    let text = match std::fs::read_to_string(settings_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut updated = false;
    let new_text = rewrite_ui_theme_bindings(&text, &predicate, new_value, &mut updated);
    if !updated {
        return Ok(());
    }
    atomic_write(settings_path, new_text.as_bytes())
}

/// Forcibly set `[ui].<slot>` to `new_value`, inserting the line under
/// `[ui]` if it isn't present yet, and inserting a fresh `[ui]` section
/// at the end of the file if necessary. Used by the install path
/// (`theme.clone`, `theme.duplicate`, `theme.create_blank`) where the
/// binding always changes regardless of the old value.
pub(crate) fn write_settings_theme_binding(
    settings_path: &Path,
    slot: ThemeSlot,
    new_value: &str,
) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(settings_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let key = slot.settings_key();
    let new_text = rewrite_or_insert_ui_key(&text, key, new_value);
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(settings_path, new_text.as_bytes())
}

/// Rewrite every line of the form `theme_dark = "name"` (or
/// `theme_light = ...`) inside the `[ui]` section whose existing value
/// satisfies `predicate`. Lines outside `[ui]`, comments, and whitespace
/// are preserved verbatim. Sets `*updated = true` when any replacement
/// happened.
fn rewrite_ui_theme_bindings<F>(
    text: &str,
    predicate: &F,
    new_value: &str,
    updated: &mut bool,
) -> String
where
    F: Fn(&str) -> bool,
{
    let mut out = String::with_capacity(text.len());
    let mut in_ui = false;
    for line in text.split_inclusive('\n') {
        let stripped = line.trim_end_matches(['\r', '\n']);
        let trimmed = stripped.trim_start();
        // Section header tracking. `[ui]` is the only section we touch;
        // every other header switches us out.
        if let Some(rest) = trimmed.strip_prefix('[') {
            if let Some(name) = rest.strip_suffix(']') {
                in_ui = name.trim() == "ui";
                out.push_str(line);
                continue;
            }
        }
        if in_ui
            && !trimmed.starts_with('#')
            && (trimmed.starts_with("theme_dark") || trimmed.starts_with("theme_light"))
        {
            if let Some((key, value)) = parse_key_value(trimmed) {
                if (key == "theme_dark" || key == "theme_light") && predicate(value) {
                    let prefix_len = stripped.len() - trimmed.len();
                    let prefix = &stripped[..prefix_len];
                    let line_ending = &line[stripped.len()..];
                    out.push_str(prefix);
                    out.push_str(key);
                    out.push_str(" = \"");
                    out.push_str(new_value);
                    out.push('"');
                    out.push_str(line_ending);
                    *updated = true;
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    out
}

/// Like [`rewrite_ui_theme_bindings`] but unconditionally sets the named
/// key. Inserts the line under an existing `[ui]` header, or appends a
/// fresh `[ui]` section at the end of the file when none exists.
fn rewrite_or_insert_ui_key(text: &str, key: &str, new_value: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut in_ui = false;
    let mut wrote_key = false;
    let mut seen_ui_section = false;
    let mut last_ui_line_idx: Option<usize> = None;

    for line in text.split_inclusive('\n') {
        let stripped = line.trim_end_matches(['\r', '\n']);
        let trimmed = stripped.trim_start();
        if let Some(rest) = trimmed.strip_prefix('[') {
            if let Some(name) = rest.strip_suffix(']') {
                if in_ui && !wrote_key {
                    if let Some(idx) = last_ui_line_idx {
                        out.insert_str(idx, &format!("{key} = \"{new_value}\"\n"));
                        wrote_key = true;
                    } else {
                        out.push_str(&format!("{key} = \"{new_value}\"\n"));
                        wrote_key = true;
                    }
                }
                in_ui = name.trim() == "ui";
                if in_ui {
                    seen_ui_section = true;
                }
                out.push_str(line);
                last_ui_line_idx = Some(out.len());
                continue;
            }
        }
        if in_ui && !trimmed.starts_with('#') {
            if let Some((existing_key, _)) = parse_key_value(trimmed) {
                if existing_key == key {
                    let prefix_len = stripped.len() - trimmed.len();
                    let prefix = &stripped[..prefix_len];
                    let line_ending = &line[stripped.len()..];
                    out.push_str(prefix);
                    out.push_str(key);
                    out.push_str(" = \"");
                    out.push_str(new_value);
                    out.push('"');
                    out.push_str(line_ending);
                    wrote_key = true;
                    last_ui_line_idx = Some(out.len());
                    continue;
                }
            }
        }
        out.push_str(line);
        last_ui_line_idx = Some(out.len());
    }
    if !wrote_key {
        if seen_ui_section {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&format!("{key} = \"{new_value}\"\n"));
        } else {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&format!("\n[ui]\n{key} = \"{new_value}\"\n"));
        }
    }
    out
}

/// Parse `key = "value"` or `key = 'value'` returning `(key, value)`.
/// Returns `None` for anything that doesn't look like a string assignment
/// (numbers, arrays, tables, etc. — none of which apply to
/// `theme_dark` / `theme_light`).
fn parse_key_value(line: &str) -> Option<(&str, &str)> {
    let eq = line.find('=')?;
    let (lhs, rhs) = line.split_at(eq);
    let key = lhs.trim();
    let value = rhs[1..].trim();
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))?;
    Some((key, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_replaces_matching_theme_dark() {
        let text = "[ui]\ntheme_dark = \"old\"\ntheme_light = \"paper\"\n";
        let mut updated = false;
        let out = rewrite_ui_theme_bindings(text, &|v| v == "old", "new", &mut updated);
        assert!(updated);
        assert!(out.contains("theme_dark = \"new\""));
        assert!(out.contains("theme_light = \"paper\""));
    }

    #[test]
    fn rewrite_leaves_non_matching_alone() {
        let text = "[ui]\ntheme_dark = \"keep_me\"\n";
        let mut updated = false;
        let out = rewrite_ui_theme_bindings(text, &|v| v == "old", "new", &mut updated);
        assert!(!updated);
        assert!(out.contains("theme_dark = \"keep_me\""));
    }

    #[test]
    fn rewrite_preserves_comments() {
        let text = "# header comment\n[ui]\n# inline comment\ntheme_dark = \"old\"\n";
        let mut updated = false;
        let out = rewrite_ui_theme_bindings(text, &|_| true, "new", &mut updated);
        assert!(out.contains("# header comment"));
        assert!(out.contains("# inline comment"));
        assert!(out.contains("theme_dark = \"new\""));
    }

    #[test]
    fn rewrite_skips_other_sections() {
        let text = "[editor]\ntheme_dark = \"foo\"\n[ui]\ntheme_dark = \"bar\"\n";
        let mut updated = false;
        let out = rewrite_ui_theme_bindings(text, &|_| true, "new", &mut updated);
        assert!(out.contains("[editor]\ntheme_dark = \"foo\""));
        assert!(out.contains("[ui]\ntheme_dark = \"new\""));
    }

    #[test]
    fn rewrite_or_insert_creates_ui_section() {
        let text = "[editor]\nfont_size = 14\n";
        let out = rewrite_or_insert_ui_key(text, "theme_dark", "my-theme");
        assert!(out.contains("[ui]"));
        assert!(out.contains("theme_dark = \"my-theme\""));
    }

    #[test]
    fn rewrite_or_insert_replaces_existing_value() {
        let text = "[ui]\ntheme_dark = \"old\"\n";
        let out = rewrite_or_insert_ui_key(text, "theme_dark", "new");
        assert_eq!(out.matches("theme_dark = ").count(), 1);
        assert!(out.contains("theme_dark = \"new\""));
    }

    #[test]
    fn rewrite_or_insert_appends_under_existing_ui_section() {
        let text = "[ui]\nshow_minimap = false\n";
        let out = rewrite_or_insert_ui_key(text, "theme_dark", "new");
        assert!(out.contains("[ui]"));
        assert!(out.contains("show_minimap = false"));
        assert!(out.contains("theme_dark = \"new\""));
    }

    #[test]
    fn parse_key_value_unwraps_double_quotes() {
        assert_eq!(
            parse_key_value("theme_dark = \"foo\""),
            Some(("theme_dark", "foo")),
        );
    }

    #[test]
    fn parse_key_value_rejects_non_string() {
        assert!(parse_key_value("font_size = 14").is_none());
    }
}
