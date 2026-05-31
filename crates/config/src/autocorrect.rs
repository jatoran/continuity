//! Phase B18 user-editable autocorrect rules.
//!
//! Loaded from `%APPDATA%\continuity\autocorrect.toml`. Format:
//!
//! ```toml
//! [[rule]]
//! pattern = "teh"
//! replacement = "the"
//! word_boundary = true   # default true
//!
//! [[rule]]
//! pattern = "->"
//! replacement = "→"
//! word_boundary = false
//! ```
//!
//! Triggered on space / punctuation following a matched pattern.
//! Pure literal substitution — no regex, no expression evaluation
//! (§16.4 preserved). Hot-reloaded by the existing settings watcher.

use serde::Deserialize;

/// One autocorrect rule.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct AutocorrectRule {
    /// Literal text that should be replaced when the user types a
    /// trigger character right after it.
    pub pattern: String,
    /// Literal replacement text (no template expansion).
    pub replacement: String,
    /// `true` → only fire when `pattern` sits at a word boundary
    /// (preceded by start-of-line or non-word char). Defaults to
    /// `true` if omitted in TOML.
    #[serde(default = "default_true")]
    pub word_boundary: bool,
}

fn default_true() -> bool {
    true
}

/// Top-level TOML schema for the autocorrect file.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AutocorrectRuleset {
    /// One entry per `[[rule]]` block.
    #[serde(default, rename = "rule")]
    pub rules: Vec<AutocorrectRule>,
}

impl AutocorrectRuleset {
    /// Parse a TOML document into a ruleset. Empty / missing TOML
    /// yields an empty ruleset.
    ///
    /// # Errors
    ///
    /// Returns a [`toml::de::Error`] when the TOML is malformed.
    pub fn from_toml(src: &str) -> Result<Self, toml::de::Error> {
        if src.trim().is_empty() {
            return Ok(Self::default());
        }
        toml::from_str(src)
    }
}

/// A single applicable replacement located against a source buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutocorrectMatch {
    /// Byte offset where `pattern` starts in the source.
    pub start: usize,
    /// Byte offset where `pattern` ends (exclusive).
    pub end: usize,
    /// The replacement text to insert in place of `[start..end]`.
    pub replacement: String,
}

/// Walk `rules` and look for one that matches `text` ending exactly
/// at `caret_byte` — i.e. the user has just finished typing a token
/// and the trigger character `trigger` follows. Returns the first
/// matching rule's replacement plan. `None` on no match.
pub fn first_match(
    text: &str,
    caret_byte: usize,
    trigger: char,
    rules: &[AutocorrectRule],
) -> Option<AutocorrectMatch> {
    if !is_autocorrect_trigger(trigger) {
        return None;
    }
    if caret_byte > text.len() {
        return None;
    }
    let prefix = &text[..caret_byte];
    for rule in rules {
        if rule.pattern.is_empty() {
            continue;
        }
        if !prefix.ends_with(&rule.pattern) {
            continue;
        }
        let start = caret_byte - rule.pattern.len();
        if rule.word_boundary && !is_word_boundary_before(text, start) {
            continue;
        }
        return Some(AutocorrectMatch {
            start,
            end: caret_byte,
            replacement: rule.replacement.clone(),
        });
    }
    None
}

/// Characters that count as autocorrect *triggers*: completing the
/// preceding token implies the user is done typing the word.
#[must_use]
pub fn is_autocorrect_trigger(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}')
}

fn is_word_boundary_before(text: &str, byte: usize) -> bool {
    if byte == 0 {
        return true;
    }
    let prev = text[..byte].chars().next_back();
    match prev {
        None => true,
        Some(c) => !c.is_alphanumeric() && c != '_',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(pairs: &[(&str, &str, bool)]) -> Vec<AutocorrectRule> {
        pairs
            .iter()
            .map(|(p, r, wb)| AutocorrectRule {
                pattern: (*p).into(),
                replacement: (*r).into(),
                word_boundary: *wb,
            })
            .collect()
    }

    #[test]
    fn parses_toml_with_default_word_boundary() {
        let src = r#"
            [[rule]]
            pattern = "teh"
            replacement = "the"

            [[rule]]
            pattern = "->"
            replacement = "→"
            word_boundary = false
        "#;
        let rs = AutocorrectRuleset::from_toml(src).expect("parses");
        assert_eq!(rs.rules.len(), 2);
        assert!(rs.rules[0].word_boundary); // default true
        assert!(!rs.rules[1].word_boundary);
    }

    #[test]
    fn empty_toml_yields_no_rules() {
        let rs = AutocorrectRuleset::from_toml("").expect("ok");
        assert!(rs.rules.is_empty());
    }

    #[test]
    fn trigger_check_recognises_space_and_punct() {
        assert!(is_autocorrect_trigger(' '));
        assert!(is_autocorrect_trigger('\t'));
        assert!(is_autocorrect_trigger('.'));
        assert!(is_autocorrect_trigger(')'));
        assert!(!is_autocorrect_trigger('a'));
        assert!(!is_autocorrect_trigger('-'));
    }

    #[test]
    fn first_match_replaces_word() {
        let r = rules(&[("teh", "the", true)]);
        let m = first_match("see teh", 7, ' ', &r).expect("match");
        assert_eq!(m.start, 4);
        assert_eq!(m.end, 7);
        assert_eq!(m.replacement, "the");
    }

    #[test]
    fn first_match_respects_word_boundary() {
        let r = rules(&[("teh", "the", true)]);
        // `tehXteh ` — second `teh` is preceded by `X` (word char),
        // so word-boundary check skips the pattern at byte 0..3 path
        // but accepts the boundary check at byte 4..7? No — the
        // second `teh` is preceded by `X`, an alphanumeric, so the
        // boundary check fails. With word_boundary=true the rule
        // shouldn't fire.
        let res = first_match("Xteh", 4, ' ', &r);
        assert!(res.is_none());
    }

    #[test]
    fn first_match_word_boundary_off_fires_anywhere() {
        let r = rules(&[("->", "→", false)]);
        let m = first_match("x->", 3, ' ', &r).expect("match");
        assert_eq!(m.start, 1);
        assert_eq!(m.replacement, "→");
    }

    #[test]
    fn first_match_ignores_non_trigger() {
        let r = rules(&[("teh", "the", true)]);
        assert!(first_match("see teh", 7, 'X', &r).is_none());
    }

    #[test]
    fn first_match_first_rule_wins() {
        let r = rules(&[("a", "FIRST", true), ("a", "SECOND", true)]);
        let m = first_match("a", 1, ' ', &r).expect("match");
        assert_eq!(m.replacement, "FIRST");
    }

    #[test]
    fn first_match_skips_empty_pattern() {
        let r = rules(&[("", "x", false), ("teh", "the", true)]);
        let m = first_match("see teh", 7, ' ', &r).expect("match");
        assert_eq!(m.replacement, "the");
    }

    #[test]
    fn first_match_handles_caret_past_eof() {
        let r = rules(&[("teh", "the", true)]);
        let res = first_match("see teh", 99, ' ', &r);
        assert!(res.is_none());
    }

    #[test]
    fn boundary_after_underscore_is_word() {
        // `_teh` — `_` counts as a word char (same convention as the
        // existing word-motion machinery).
        let r = rules(&[("teh", "the", true)]);
        assert!(first_match("a_teh", 5, ' ', &r).is_none());
    }
}
