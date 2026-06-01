//! Lightweight syntax highlighting for fenced code-block bodies and code files.
//!
//! The syntax-highlighting allowlist is intentionally small: `rust`, `json`,
//! `toml`, and `markdown`. A heavier tree-sitter-grammars + highlight-query
//! approach is recorded as a follow-up; for now a hand-rolled scanner per
//! language produces span colors keyed to a small palette.
//!
//! Pure function: `(language_tag, source) -> Vec<HighlightSpan>` with byte
//! offsets relative to the input. The caller (decoration pipeline) is
//! responsible for shifting them into document-absolute coordinates.

/// What kind of token a [`HighlightSpan`] describes. Maps to a small
/// palette the renderer will translate into Rgba.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum HighlightKind {
    /// Language keyword (`fn`, `if`, `else`, `let`, `return`, `match`,
    /// `pub`, `mod`, `use`, …).
    Keyword,
    /// Type-like identifier (`String`, `Vec`, leading capital).
    Type,
    /// String literal contents (including the surrounding quotes).
    String,
    /// Numeric literal.
    Number,
    /// Line/block comment.
    Comment,
    /// Function-call name (identifier immediately followed by `(`).
    Function,
    /// Punctuation / structural (delimiters, operators) — currently unused
    /// but reserved for Phase-11 theming finesse.
    Punctuation,
}

/// One highlight span: byte range + token kind.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HighlightSpan {
    /// Inclusive byte start within the input.
    pub start: usize,
    /// Exclusive byte end within the input.
    pub end: usize,
    /// Token classification.
    pub kind: HighlightKind,
}

/// Syntax-highlight `source` per `language_tag` from a fence info string or
/// detected file extension. Languages outside the allowlist return an empty
/// vector — no highlighting, no error.
#[must_use]
pub fn highlight(language_tag: &str, source: &str) -> Vec<HighlightSpan> {
    let lang = normalize_language(language_tag);
    match lang {
        "rust" | "rs" => highlight_rust(source),
        "json" => highlight_json(source),
        "toml" => highlight_toml(source),
        "markdown" | "md" => highlight_markdown_inline(source),
        _ => Vec::new(),
    }
}

fn normalize_language(tag: &str) -> &str {
    let tag = tag.trim();
    // Strip args after a space (e.g. `rust ignore`).
    tag.split_whitespace().next().unwrap_or("")
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

fn highlight_rust(src: &str) -> Vec<HighlightSpan> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // Line comments
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            out.push(HighlightSpan {
                start,
                end: i,
                kind: HighlightKind::Comment,
            });
            continue;
        }
        // Block comments
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            out.push(HighlightSpan {
                start,
                end: i,
                kind: HighlightKind::Comment,
            });
            continue;
        }
        // String literals (incl. raw-strings approximated as quoted)
        if b == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(HighlightSpan {
                start,
                end: i,
                kind: HighlightKind::String,
            });
            continue;
        }
        // Char literals
        if b == b'\'' && i + 2 < bytes.len() {
            let start = i;
            // Lifetime vs char heuristic: char ends with a `'` within 6 bytes.
            let mut j = i + 1;
            while j < bytes.len() && j < i + 6 {
                if bytes[j] == b'\'' {
                    out.push(HighlightSpan {
                        start,
                        end: j + 1,
                        kind: HighlightKind::String,
                    });
                    i = j + 1;
                    break;
                }
                j += 1;
            }
            if i != j + 1 {
                i += 1;
            }
            continue;
        }
        // Numbers
        if b.is_ascii_digit() {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_')
            {
                i += 1;
            }
            out.push(HighlightSpan {
                start,
                end: i,
                kind: HighlightKind::Number,
            });
            continue;
        }
        // Identifiers / keywords
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];
            let kind = if RUST_KEYWORDS.contains(&word) {
                HighlightKind::Keyword
            } else if word
                .chars()
                .next()
                .map(|c| c.is_ascii_uppercase())
                .unwrap_or(false)
            {
                HighlightKind::Type
            } else if i < bytes.len() && bytes[i] == b'(' {
                HighlightKind::Function
            } else {
                continue;
            };
            out.push(HighlightSpan {
                start,
                end: i,
                kind,
            });
            continue;
        }
        i += 1;
    }
    out
}

fn highlight_json(src: &str) -> Vec<HighlightSpan> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            // Distinguish keys (followed by `:`) from values; both are strings.
            out.push(HighlightSpan {
                start,
                end: i,
                kind: HighlightKind::String,
            });
            continue;
        }
        if b.is_ascii_digit() || b == b'-' {
            let start = i;
            if b == b'-' {
                i += 1;
            }
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || bytes[i] == b'.'
                    || bytes[i] == b'e'
                    || bytes[i] == b'E'
                    || bytes[i] == b'+'
                    || bytes[i] == b'-')
            {
                i += 1;
            }
            if i > start + (b == b'-') as usize {
                out.push(HighlightSpan {
                    start,
                    end: i,
                    kind: HighlightKind::Number,
                });
            }
            continue;
        }
        if b.is_ascii_alphabetic() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            let word = &src[start..i];
            if matches!(word, "true" | "false" | "null") {
                out.push(HighlightSpan {
                    start,
                    end: i,
                    kind: HighlightKind::Keyword,
                });
            }
            continue;
        }
        i += 1;
    }
    out
}

fn highlight_toml(src: &str) -> Vec<HighlightSpan> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'#' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            out.push(HighlightSpan {
                start,
                end: i,
                kind: HighlightKind::Comment,
            });
            continue;
        }
        if b == b'[' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b']' {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b']' {
                i += 1;
            }
            out.push(HighlightSpan {
                start,
                end: i,
                kind: HighlightKind::Keyword,
            });
            continue;
        }
        if b == b'"' || b == b'\'' {
            let quote = b;
            let start = i;
            i += 1;
            while i < bytes.len() {
                if quote == b'"' && bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(HighlightSpan {
                start,
                end: i,
                kind: HighlightKind::String,
            });
            continue;
        }
        if b.is_ascii_digit() || b == b'-' || b == b'+' {
            let start = i;
            if b == b'-' || b == b'+' {
                i += 1;
            }
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || matches!(bytes[i], b'.' | b'_' | b':' | b'-' | b'+'))
            {
                i += 1;
            }
            let signed = matches!(b, b'-' | b'+');
            let signed_len = if signed { 1 } else { 0 };
            if i > start + signed_len {
                out.push(HighlightSpan {
                    start,
                    end: i,
                    kind: HighlightKind::Number,
                });
            }
            continue;
        }
        if is_toml_key_start(b) {
            let start = i;
            while i < bytes.len() && is_toml_key_continue(bytes[i]) {
                i += 1;
            }
            let word = &src[start..i];
            if matches!(word, "true" | "false") {
                out.push(HighlightSpan {
                    start,
                    end: i,
                    kind: HighlightKind::Keyword,
                });
                continue;
            }
            let mut j = i;
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'=' {
                out.push(HighlightSpan {
                    start,
                    end: i,
                    kind: HighlightKind::Type,
                });
            }
            continue;
        }
        i += 1;
    }
    out
}

fn is_toml_key_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'-'
}

fn is_toml_key_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')
}

fn highlight_markdown_inline(src: &str) -> Vec<HighlightSpan> {
    // For markdown shown *inside* a fenced block we just light up backticked
    // inline code as Code-style runs; the renderer doesn't double-style
    // because fenced bodies are styled by the markdown decoration pass.
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'`' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            out.push(HighlightSpan {
                start,
                end: i,
                kind: HighlightKind::String,
            });
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_highlights_keywords_and_strings() {
        let src = r#"fn main() { let s = "hello"; }"#;
        let spans = highlight("rust", src);
        assert!(spans.iter().any(|s| s.kind == HighlightKind::Keyword));
        assert!(spans.iter().any(|s| s.kind == HighlightKind::String));
    }

    #[test]
    fn rust_function_call() {
        let src = "do_thing(x);";
        let spans = highlight("rust", src);
        assert!(spans.iter().any(|s| s.kind == HighlightKind::Function));
    }

    #[test]
    fn json_string_and_number() {
        let src = r#"{"name": "x", "age": 42}"#;
        let spans = highlight("json", src);
        let strings = spans
            .iter()
            .filter(|s| s.kind == HighlightKind::String)
            .count();
        assert!(strings >= 2);
        assert!(spans.iter().any(|s| s.kind == HighlightKind::Number));
    }

    #[test]
    fn json_keywords() {
        let src = r#"{"ok": true, "err": null, "active": false}"#;
        let spans = highlight("json", src);
        assert!(
            spans
                .iter()
                .filter(|s| s.kind == HighlightKind::Keyword)
                .count()
                >= 3
        );
    }

    #[test]
    fn toml_keys_values_and_comments() {
        let src = "[package]\nname = \"continuity\"\nworkers = 4\n# local\n";
        let spans = highlight("toml", src);
        assert!(spans.iter().any(|s| s.kind == HighlightKind::Keyword));
        assert!(spans.iter().any(|s| s.kind == HighlightKind::Type));
        assert!(spans.iter().any(|s| s.kind == HighlightKind::String));
        assert!(spans.iter().any(|s| s.kind == HighlightKind::Number));
        assert!(spans.iter().any(|s| s.kind == HighlightKind::Comment));
    }

    #[test]
    fn unknown_language_returns_empty() {
        assert!(highlight("python", "print('hi')").is_empty());
    }

    #[test]
    fn rust_line_comment() {
        let src = "// a comment\nfn x() {}";
        let spans = highlight("rust", src);
        assert!(spans.iter().any(|s| s.kind == HighlightKind::Comment));
    }
}
