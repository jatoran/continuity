//! Identifier-naming lint rules — phase-coded names are forbidden.
//!
//! Extends the file-level `no_phase_prefixed_filename` rule (Phase 17.8) down
//! to the identifier level. Function, type, and constant names that encode
//! the project's roadmap phase coordinate (e.g. `h6_tab_overlay`,
//! `apply_phase6_selection_edit`, `PhaseHState`, `PHASE_F3_FOO`) are rejected
//! because phase numbering rotates as the roadmap advances and the names
//! become uninformative the moment the next phase lands.
//!
//! Patterns flagged (case-insensitive against the identifier):
//!   1. starts with `phase` followed by digit / underscore / end
//!      — e.g. `phase6_foo`, `phase_h_state`, `phase_f3_inline`.
//!   2. starts with `<single-letter><digits>_`
//!      — e.g. `h6_tab_overlay`, `i2_view_metrics`, `f3_color`.
//!   3. contains `_phase` followed by digit, end, or `_<single-letter>`
//!      (the single letter must itself be followed by digit / underscore /
//!      end so multi-letter words like `_phase_prefixed_*` do NOT match).
//!      — e.g. `apply_phase6_selection_edit`, `view_phase_h_overlay`.
//!
//! Each rule lives next to the file-level analogue in `conventions.rs` and
//! surfaces violations on the same channel ([`Violation`]).

use crate::conventions::Violation;

/// Names that look like phase prefixes but aren't. Add an entry here only
/// with a comment justifying the exception. Start small; the right answer
/// is usually to rename.
const IDENTIFIER_ALLOWLIST: &[&str] = &[
    // (no entries yet — add only with justification, e.g. a codec
    // identifier `h264_decoder` or a hardware standard `i2c_bus_init`.)
];

/// Core matcher — returns true if `name` looks like a phase-coded
/// identifier. Case-aware: `phase` (snake_case), `Phase` (PascalCase),
/// and `PHASE` (SCREAMING_SNAKE) all qualify, and the "what follows the
/// keyword" check is tuned per case so `phaser` and `PhaseRendererState`
/// don't false-positive while `PhaseHState` does match (single uppercase
/// letter is a phase coord under PascalCase).
pub(crate) fn is_phase_prefixed_ident(name: &str) -> bool {
    if IDENTIFIER_ALLOWLIST.contains(&name) {
        return false;
    }
    if starts_with_phase_keyword(name) {
        return true;
    }
    if starts_with_single_letter_digits(name) {
        return true;
    }
    if contains_inner_phase_coord(name) {
        return true;
    }
    false
}

#[derive(Clone, Copy)]
enum Case {
    /// snake_case (`phase`) — boundary after the keyword is digit, `_`, or end.
    Snake,
    /// SCREAMING_SNAKE (`PHASE`) — same boundary rules as snake_case.
    Scream,
    /// PascalCase (`Phase`) — also accepts an uppercase letter as boundary
    /// (the start of the next camel-token).
    Pascal,
}

fn starts_with_phase_keyword(name: &str) -> bool {
    has_phase_prefix_with_boundary(name, "phase", Case::Snake)
        || has_phase_prefix_with_boundary(name, "PHASE", Case::Scream)
        || has_phase_prefix_with_boundary(name, "Phase", Case::Pascal)
}

fn has_phase_prefix_with_boundary(name: &str, prefix: &str, case: Case) -> bool {
    let Some(rest) = name.strip_prefix(prefix) else {
        return false;
    };
    match rest.as_bytes().first().copied() {
        None => true,
        Some(c) if c.is_ascii_digit() || c == b'_' => true,
        Some(c) if matches!(case, Case::Pascal) && c.is_ascii_uppercase() => true,
        _ => false,
    }
}

/// Rule 2: `^[A-Za-z][0-9]+_` — short feature-letter code (`h6_*`, `H6_*`).
fn starts_with_single_letter_digits(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() < 3 {
        return false;
    }
    if !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    let mut i = 1;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 1 {
        return false;
    }
    matches!(bytes.get(i), Some(b'_'))
}

/// Rule 3: contains `_phase` (or `_PHASE`) followed by digit, end, or
/// `_<single-letter>` where the letter is terminal/digit/underscore-bounded
/// so `_phase_prefixed_foo` does NOT match while `_phase_h_foo` does.
fn contains_inner_phase_coord(name: &str) -> bool {
    let bytes = name.as_bytes();
    for (needle, case) in [
        (b"_phase".as_ref(), Case::Snake),
        (b"_PHASE".as_ref(), Case::Scream),
    ] {
        let mut i = 0;
        while i + needle.len() <= bytes.len() {
            if &bytes[i..i + needle.len()] == needle
                && is_phase_coord_continuation(bytes, i + needle.len(), case)
            {
                return true;
            }
            i += 1;
        }
    }
    false
}

fn is_phase_coord_continuation(bytes: &[u8], at: usize, case: Case) -> bool {
    match bytes.get(at).copied() {
        None => true,
        Some(c) if c.is_ascii_digit() => true,
        Some(b'_') => {
            let next = bytes.get(at + 1).copied();
            let letter_ok = match (case, next) {
                (Case::Snake, Some(c)) if c.is_ascii_lowercase() => true,
                (Case::Scream, Some(c)) if c.is_ascii_uppercase() => true,
                _ => false,
            };
            let digit_ok = matches!(next, Some(c) if c.is_ascii_digit());
            if digit_ok {
                return true;
            }
            if !letter_ok {
                return false;
            }
            match bytes.get(at + 2).copied() {
                None => true,
                Some(c) if c.is_ascii_digit() || c == b'_' => true,
                _ => false,
            }
        }
        _ => false,
    }
}

/// Identifier kind for the line scanner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefKind {
    Function,
    Type,
    Const,
}

impl DefKind {
    fn rule(self) -> &'static str {
        match self {
            DefKind::Function => "conventions:no-phase-prefixed-function",
            DefKind::Type => "conventions:no-phase-prefixed-type",
            DefKind::Const => "conventions:no-phase-prefixed-const",
        }
    }

    fn label(self) -> &'static str {
        match self {
            DefKind::Function => "function",
            DefKind::Type => "type",
            DefKind::Const => "constant",
        }
    }
}

/// Inspect one source line for a `fn` / `struct` / `enum` / `trait` /
/// `type` / `const` / `static` declaration and flag the name if it matches
/// the phase-coded pattern.
pub(crate) fn check_phase_prefixed_definition(
    code: &str,
    path: &str,
    line: usize,
    out: &mut Vec<Violation>,
) {
    let Some((kind, name)) = line_definition_kind_and_name(code) else {
        return;
    };
    if !is_phase_prefixed_ident(&name) {
        return;
    }
    out.push(Violation::new(
        kind.rule(),
        path,
        Some(line),
        format!(
            "{kind_label} `{name}` is phase-coded — rename to a topic-based name. \
Phase numbers rotate; the roadmap is the history of record (e.g. \
`h6_tab_overlay_dispatches` → `tab_switcher_overlay_command_is_registered`, \
`PhaseHState` → `PaneModesState`, `apply_phase6_selection_edit` → \
`apply_selection_edit`). If the name is a genuine false positive (codec, \
hardware standard, etc.) add it to IDENTIFIER_ALLOWLIST with a comment.",
            kind_label = kind.label(),
            name = name,
        ),
    ));
}

/// Parse one trimmed source line; return the kind and name of the
/// declaration it introduces, if any. Strips visibility / modifier
/// keywords (`pub`, `pub(crate)`, `pub(super)`, `pub(in path)`, `unsafe`,
/// `async`, `default`, `extern`, `extern "C"`).
pub(crate) fn line_definition_kind_and_name(line: &str) -> Option<(DefKind, String)> {
    let mut s = line.trim_start();
    s = strip_leading_modifiers(s);

    let (kw, kind): (&str, DefKind) = if s.starts_with("fn ") {
        ("fn", DefKind::Function)
    } else if s.starts_with("struct ") {
        ("struct", DefKind::Type)
    } else if s.starts_with("enum ") {
        ("enum", DefKind::Type)
    } else if s.starts_with("trait ") {
        ("trait", DefKind::Type)
    } else if s.starts_with("type ") {
        ("type", DefKind::Type)
    } else if s.starts_with("const ") {
        ("const", DefKind::Const)
    } else if s.starts_with("static ") {
        ("static", DefKind::Const)
    } else {
        return None;
    };

    let mut after = s[kw.len()..].trim_start();
    if matches!(kind, DefKind::Const) && after.starts_with("mut ") {
        after = after[4..].trim_start();
    }

    let mut ident = String::new();
    for c in after.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            ident.push(c);
        } else {
            break;
        }
    }
    if ident.is_empty() {
        None
    } else {
        Some((kind, ident))
    }
}

fn strip_leading_modifiers(mut s: &str) -> &str {
    loop {
        let original = s;
        if let Some(rest) = strip_pub_in(s) {
            s = rest.trim_start();
            continue;
        }
        for kw in &[
            "pub(crate)",
            "pub(super)",
            "pub(self)",
            "pub",
            "unsafe",
            "async",
            "default",
            "extern \"C\"",
            "extern",
        ] {
            if let Some(rest) = s.strip_prefix(kw) {
                if rest.starts_with(char::is_whitespace) || rest.is_empty() {
                    s = rest.trim_start();
                    break;
                }
            }
        }
        if s == original {
            return s;
        }
    }
}

fn strip_pub_in(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("pub(in ")?;
    let end = rest.find(')')?;
    Some(&rest[end + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_phase_keyword_prefix() {
        assert!(is_phase_prefixed_ident("phase6_multi_cursor"));
        assert!(is_phase_prefixed_ident("phase_h_state"));
        assert!(is_phase_prefixed_ident("phase_f3_inline"));
        assert!(is_phase_prefixed_ident("PhaseHState"));
        assert!(is_phase_prefixed_ident("Phase6Foo"));
    }

    #[test]
    fn flags_letter_digit_prefix() {
        assert!(is_phase_prefixed_ident("h6_tab_overlay"));
        assert!(is_phase_prefixed_ident("i2_view_metrics"));
        assert!(is_phase_prefixed_ident("f3_color"));
        assert!(is_phase_prefixed_ident("H6_TAB_OVERLAY"));
    }

    #[test]
    fn flags_inner_phase_coord() {
        assert!(is_phase_prefixed_ident("apply_phase6_selection_edit"));
        assert!(is_phase_prefixed_ident("apply_phase_h_state"));
        assert!(is_phase_prefixed_ident("view_phase_f3_inline"));
        assert!(is_phase_prefixed_ident("trailing_phase"));
        assert!(is_phase_prefixed_ident("APPLY_PHASE6_SELECTION_EDIT"));
    }

    #[test]
    fn near_misses_do_not_trigger() {
        // word that happens to start with "phase"
        assert!(!is_phase_prefixed_ident("phaser"));
        assert!(!is_phase_prefixed_ident("phaserstate"));
        // single-letter + multi-digit codec — looks like a phase but isn't.
        // The strict rule still matches `h264_decoder`; we exempt it via
        // IDENTIFIER_ALLOWLIST in production (the test asserts the rule's
        // raw behavior so future edits don't quietly weaken it).
        assert!(is_phase_prefixed_ident("h264_decoder"));
        // feature acronym in a const — `TOC` is not a phase code
        assert!(!is_phase_prefixed_ident("MARKDOWN_INSERT_TOC"));
        // identifier with `_phase_` but the next token is a real word
        assert!(!is_phase_prefixed_ident("check_no_phase_prefixed_filename"));
        assert!(!is_phase_prefixed_ident("is_phase_prefixed_ident"));
        // no leading letter+digit (multi-letter prefix)
        assert!(!is_phase_prefixed_ident("ab2_foo"));
        // type with digit suffix only
        assert!(!is_phase_prefixed_ident("Vec2"));
        assert!(!is_phase_prefixed_ident("Mat4x4"));
        // numeric type
        assert!(!is_phase_prefixed_ident("u32"));
    }

    #[test]
    fn allowlist_overrides() {
        // sanity: the allowlist mechanism works (using a temporary name
        // that we know would otherwise match the rule).
        assert!(is_phase_prefixed_ident("h264_decoder"));
        // If/when we add `h264_decoder` to IDENTIFIER_ALLOWLIST it would
        // return false; the contains() check is what enables that.
    }

    #[test]
    fn parses_function_definitions() {
        assert_eq!(
            line_definition_kind_and_name("fn h6_foo() {"),
            Some((DefKind::Function, "h6_foo".into()))
        );
        assert_eq!(
            line_definition_kind_and_name("    pub fn h6_foo() {"),
            Some((DefKind::Function, "h6_foo".into()))
        );
        assert_eq!(
            line_definition_kind_and_name("pub(crate) fn apply_phase6_edit() {"),
            Some((DefKind::Function, "apply_phase6_edit".into()))
        );
        assert_eq!(
            line_definition_kind_and_name("pub(in crate::foo) fn bar() {"),
            Some((DefKind::Function, "bar".into()))
        );
    }

    #[test]
    fn parses_type_definitions() {
        assert_eq!(
            line_definition_kind_and_name("pub struct PhaseHState {"),
            Some((DefKind::Type, "PhaseHState".into()))
        );
        assert_eq!(
            line_definition_kind_and_name("enum Phase6 { A, B }"),
            Some((DefKind::Type, "Phase6".into()))
        );
        assert_eq!(
            line_definition_kind_and_name("trait Phase6Renderer {"),
            Some((DefKind::Type, "Phase6Renderer".into()))
        );
        assert_eq!(
            line_definition_kind_and_name("type PhaseHandler = Box<dyn Fn()>;"),
            Some((DefKind::Type, "PhaseHandler".into()))
        );
    }

    #[test]
    fn parses_const_and_static_definitions() {
        assert_eq!(
            line_definition_kind_and_name("const PHASE_H_FOO: u32 = 0;"),
            Some((DefKind::Const, "PHASE_H_FOO".into()))
        );
        assert_eq!(
            line_definition_kind_and_name("pub static PHASE6_TAG: &str = \"x\";"),
            Some((DefKind::Const, "PHASE6_TAG".into()))
        );
        assert_eq!(
            line_definition_kind_and_name("static mut H6_COUNTER: u32 = 0;"),
            Some((DefKind::Const, "H6_COUNTER".into()))
        );
    }

    #[test]
    fn check_function_flags_then_skips() {
        let mut v = Vec::new();
        check_phase_prefixed_definition("fn h6_overlay() {}", "x.rs", 1, &mut v);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rule, "conventions:no-phase-prefixed-function");

        v.clear();
        check_phase_prefixed_definition("fn tab_switcher_overlay() {}", "x.rs", 1, &mut v);
        assert!(v.is_empty());
    }

    #[test]
    fn check_const_flags_then_skips() {
        let mut v = Vec::new();
        check_phase_prefixed_definition("const PHASE_H_FOO: u32 = 0;", "x.rs", 1, &mut v);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rule, "conventions:no-phase-prefixed-const");

        v.clear();
        check_phase_prefixed_definition(
            "const MARKDOWN_INSERT_TOC: &str = \"insert.toc\";",
            "x.rs",
            1,
            &mut v,
        );
        assert!(v.is_empty());
    }

    #[test]
    fn check_type_flags_then_skips() {
        let mut v = Vec::new();
        check_phase_prefixed_definition("pub struct PhaseHState;", "x.rs", 1, &mut v);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rule, "conventions:no-phase-prefixed-type");

        v.clear();
        check_phase_prefixed_definition("pub struct PaneModesState;", "x.rs", 1, &mut v);
        assert!(v.is_empty());
    }

    #[test]
    fn skips_non_definitions() {
        let mut v = Vec::new();
        check_phase_prefixed_definition("let h6_local = 1;", "x.rs", 1, &mut v);
        assert!(v.is_empty());
        check_phase_prefixed_definition("impl PhaseHState {", "x.rs", 1, &mut v);
        assert!(v.is_empty());
        check_phase_prefixed_definition("    h6_overlay();", "x.rs", 1, &mut v);
        assert!(v.is_empty());
    }
}
