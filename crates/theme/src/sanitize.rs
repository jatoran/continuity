//! Theme-name sanitization rules shared by every δ.5 workflow command
//! (`theme.clone`, `theme.duplicate`, `theme.rename`, `theme.create_blank`).
//!
//! Theme names map directly to filenames under `%APPDATA%\continuity\themes\`,
//! so user input must be validated before any disk operation. The rules:
//!
//! - allowed character set: ASCII alphanumeric plus `_` and `-`,
//! - max length 64 chars,
//! - no leading `.`,
//! - rejected when it collides with any bundled theme name (checked
//!   programmatically against [`crate::assets::BUNDLED_NAMES`] so adding a
//!   new bundled theme automatically reserves the name).
//!
//! Leading and trailing whitespace is trimmed before validation; the
//! cleaned value is what the caller writes to disk. Rejection produces a
//! human-readable diagnostic suitable for banner text.
//!
//! Thread ownership: stateless — pure data validation, callable from any
//! thread.

use crate::assets::BUNDLED_NAMES;

/// Maximum theme-name length in characters. Picked to leave room for
/// `.toml` plus the trash-rename timestamp suffix without exceeding the
/// 260-character Windows path-component soft cap.
pub const MAX_NAME_LEN: usize = 64;

/// Outcome of validating a theme name. The `Ok` variant carries the
/// trimmed name that callers should use on disk; the `Err` variant
/// carries a one-line user-facing diagnostic.
#[derive(Debug)]
pub enum NameCheck {
    /// Name passes every rule; the trimmed value is canonical.
    Ok(String),
    /// Name violates one of the rules; the contained diagnostic is
    /// suitable for a banner.
    Rejected(String),
}

/// Validate a user-supplied theme name against the δ.5 sanitization rules.
///
/// Returns [`NameCheck::Ok`] with the canonical (trimmed) form, or
/// [`NameCheck::Rejected`] with a banner-ready reason.
#[must_use]
pub fn check_theme_name(input: &str) -> NameCheck {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return NameCheck::Rejected("theme name must not be empty".to_string());
    }
    if trimmed.chars().count() > MAX_NAME_LEN {
        return NameCheck::Rejected(format!(
            "theme name must be {MAX_NAME_LEN} characters or fewer",
        ));
    }
    if trimmed.starts_with('.') {
        return NameCheck::Rejected("theme name must not start with `.`".to_string());
    }
    if let Some(bad) = trimmed
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-'))
    {
        return NameCheck::Rejected(format!(
            "theme name may only contain letters, digits, `_`, and `-` (got `{bad}`)",
        ));
    }
    if is_reserved_name(trimmed) {
        return NameCheck::Rejected(format!(
            "`{trimmed}` is reserved for a bundled theme — pick another name",
        ));
    }
    NameCheck::Ok(trimmed.to_string())
}

/// Return whether `name` collides with any bundled theme name. The check
/// is case-insensitive: filesystems on Windows are case-insensitive by
/// default, so `Paper` and `paper` would land in the same place.
#[must_use]
pub fn is_reserved_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    BUNDLED_NAMES
        .iter()
        .any(|n| n.eq_ignore_ascii_case(lower.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> String {
        match check_theme_name(input) {
            NameCheck::Ok(n) => n,
            NameCheck::Rejected(r) => panic!("expected Ok for `{input}`, got: {r}"),
        }
    }

    fn rejected(input: &str) -> String {
        match check_theme_name(input) {
            NameCheck::Ok(n) => panic!("expected Rejected for `{input}`, got Ok({n})"),
            NameCheck::Rejected(r) => r,
        }
    }

    #[test]
    fn accepts_basic_names() {
        assert_eq!(ok("my-theme"), "my-theme");
        assert_eq!(ok("Theme_42"), "Theme_42");
        assert_eq!(ok("A"), "A");
    }

    #[test]
    fn trims_whitespace_before_validating() {
        assert_eq!(ok("  hello  "), "hello");
    }

    #[test]
    fn rejects_empty_after_trim() {
        rejected("");
        rejected("   ");
    }

    #[test]
    fn rejects_disallowed_characters() {
        assert!(rejected("has space").contains("letters, digits"));
        assert!(rejected("name.with.dot").contains("letters, digits"));
        assert!(rejected("slash/here").contains("letters, digits"));
        assert!(rejected("back\\slash").contains("letters, digits"));
    }

    #[test]
    fn rejects_leading_dot() {
        assert!(rejected(".hidden").contains("must not start"));
    }

    #[test]
    fn rejects_overlong_name() {
        let long = "a".repeat(MAX_NAME_LEN + 1);
        assert!(rejected(&long).contains("characters or fewer"));
    }

    #[test]
    fn accepts_max_length_exact() {
        let name = "a".repeat(MAX_NAME_LEN);
        assert_eq!(ok(&name), name);
    }

    #[test]
    fn rejects_every_bundled_name() {
        for bundled in BUNDLED_NAMES {
            let reason = rejected(bundled);
            assert!(
                reason.contains("reserved"),
                "expected `{bundled}` to be rejected as reserved, got: {reason}",
            );
        }
    }

    #[test]
    fn rejects_bundled_name_case_insensitive() {
        rejected("Deep_Minimal");
        rejected("PAPER");
        rejected("Solarized_Dark");
    }

    #[test]
    fn is_reserved_name_matches_bundled_set() {
        assert!(is_reserved_name("deep_minimal"));
        assert!(is_reserved_name("paper"));
        assert!(!is_reserved_name("custom_theme"));
    }
}
