//! γ — smart-typography preset (default-on autocorrect ruleset).
//!
//! Returns the built-in rule list consumed by the autocorrect engine
//! (see `crates/config/src/autocorrect.rs`). Activation is gated by
//! `[editor].smart_typography_enabled` in `settings.toml`; the preset
//! is prepended to user rules so user rules can override any preset
//! entry by listing the same pattern first.
//!
//! Substitutions (in the order they are matched):
//!   * `---` → `—` (em dash; must precede the en-dash rule).
//!   * `--`  → `–` (en dash).
//!   * `...` → `…` (horizontal ellipsis).
//!   * `"`   → `“` when preceded by a word boundary (opening curly).
//!   * `"`   → `”` everywhere else (closing curly fallback).
//!   * `'`   → `‘` when preceded by a word boundary (opening curly).
//!   * `'`   → `’` everywhere else (closing curly fallback / apostrophe).
//!
//! `first_match` walks rules top-down and stops on the first applicable
//! one, so the ordering above is load-bearing: the longer dash pattern
//! and the word-boundary-gated quote both need to be evaluated before
//! their fall-throughs.

use crate::autocorrect::AutocorrectRule;

/// Build the smart-typography preset as a `Vec<AutocorrectRule>`. The
/// rules are returned in evaluation order; callers should prepend them
/// to user rules so user customisation wins on identical patterns.
#[must_use]
pub fn smart_typography_rules() -> Vec<AutocorrectRule> {
    vec![
        AutocorrectRule {
            pattern: "---".into(),
            replacement: "\u{2014}".into(),
            word_boundary: false,
        },
        AutocorrectRule {
            pattern: "--".into(),
            replacement: "\u{2013}".into(),
            word_boundary: false,
        },
        AutocorrectRule {
            pattern: "...".into(),
            replacement: "\u{2026}".into(),
            word_boundary: false,
        },
        AutocorrectRule {
            pattern: "\"".into(),
            replacement: "\u{201C}".into(),
            word_boundary: true,
        },
        AutocorrectRule {
            pattern: "\"".into(),
            replacement: "\u{201D}".into(),
            word_boundary: false,
        },
        AutocorrectRule {
            pattern: "'".into(),
            replacement: "\u{2018}".into(),
            word_boundary: true,
        },
        AutocorrectRule {
            pattern: "'".into(),
            replacement: "\u{2019}".into(),
            word_boundary: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autocorrect::first_match;

    #[test]
    fn preset_is_non_empty_and_stably_ordered() {
        let rules = smart_typography_rules();
        // Longer dash pattern must precede shorter so `---` doesn't
        // get clipped to `--` + `-`.
        let idx_em = rules.iter().position(|r| r.pattern == "---").unwrap();
        let idx_en = rules.iter().position(|r| r.pattern == "--").unwrap();
        assert!(idx_em < idx_en);
    }

    #[test]
    fn em_dash_fires_on_triple_hyphen() {
        let rules = smart_typography_rules();
        let m = first_match("yes---", 6, ' ', &rules).expect("match");
        assert_eq!(m.replacement, "\u{2014}");
    }

    #[test]
    fn en_dash_fires_on_double_hyphen() {
        let rules = smart_typography_rules();
        let m = first_match("yes--", 5, ' ', &rules).expect("match");
        assert_eq!(m.replacement, "\u{2013}");
    }

    #[test]
    fn ellipsis_fires_on_three_dots() {
        let rules = smart_typography_rules();
        let m = first_match("wait...", 7, ' ', &rules).expect("match");
        assert_eq!(m.replacement, "\u{2026}");
    }

    #[test]
    fn opening_curly_at_word_boundary() {
        let rules = smart_typography_rules();
        // ` "` — boundary holds (space before the quote).
        let m = first_match("said \"", 6, 'h', &rules);
        // 'h' isn't a trigger char so first_match returns None — but
        // we want to verify the rule MATCHES on a trigger.
        assert!(m.is_none());
        let m = first_match("said \"", 6, ' ', &rules).expect("match");
        assert_eq!(m.replacement, "\u{201C}");
    }

    #[test]
    fn closing_curly_falls_through_at_word_char() {
        let rules = smart_typography_rules();
        let m = first_match("Hello\"", 6, ' ', &rules).expect("match");
        assert_eq!(m.replacement, "\u{201D}");
    }

    #[test]
    fn opening_single_quote_at_word_boundary() {
        let rules = smart_typography_rules();
        let m = first_match("said '", 6, ' ', &rules).expect("match");
        assert_eq!(m.replacement, "\u{2018}");
    }

    #[test]
    fn closing_single_quote_falls_through() {
        // `don't` — the apostrophe is preceded by a word char, so the
        // closing-curly variant fires. This is the standard apostrophe
        // case.
        let rules = smart_typography_rules();
        let m = first_match("don'", 4, ' ', &rules).expect("match");
        assert_eq!(m.replacement, "\u{2019}");
    }
}
