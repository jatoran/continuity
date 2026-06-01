//! Per-buffer language identification.
//!
//! Phase 10 needs the `language` context atom to actually fire for markdown
//! buffers so existing `when = "language == 'markdown'"` keymap bindings
//! activate. Detection is conservative: extension first, then a tiny
//! content sniff against the first few non-empty lines. Known code
//! extensions return syntax tags; default falls back to `"plain"`.

/// Identifier used in `Context::lookup("language")`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Language {
    /// Plain text (default).
    Plain,
    /// Markdown.
    Markdown,
    /// Non-markdown code-like file syntax-highlighted as this language tag.
    Code(&'static str),
}

impl Language {
    /// Stable string identifier consumed by the predicate grammar.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Language::Plain => "plain",
            Language::Markdown => "markdown",
            Language::Code(tag) => tag,
        }
    }

    /// Language tag accepted by [`crate::syntax::highlight`] for
    /// whole-file code highlighting.
    #[must_use]
    pub const fn syntax_tag(self) -> Option<&'static str> {
        match self {
            Language::Code(tag) => Some(tag),
            Language::Plain | Language::Markdown => None,
        }
    }
}

/// Detect a buffer's language from an optional file path and the buffer
/// content.
///
/// Per spec §3 ("The buffer is plain markdown text. Always."), the
/// markdown-first default kicks in whenever the caller cannot prove the
/// buffer is something else:
///
/// - extension is a known markdown one → [`Language::Markdown`]
/// - extension is a known code one → [`Language::Code`]
/// - extension is present but unknown → [`Language::Plain`]
///   (the file's name is the authoritative hint)
/// - no extension and the content sniff flags markdown → [`Language::Markdown`]
/// - no extension and the content looks like plain text → [`Language::Markdown`]
///   (fresh / untitled buffers are notes, and notes are markdown)
#[must_use]
pub fn detect(path_extension: Option<&str>, content: &str) -> Language {
    if let Some(ext) = path_extension {
        return match ext.to_ascii_lowercase().as_str() {
            "md" | "markdown" | "mdown" | "mkd" | "mkdn" => Language::Markdown,
            "rs" => Language::Code("rust"),
            "json" => Language::Code("json"),
            "toml" => Language::Code("toml"),
            _ => Language::Plain,
        };
    }
    if sniff_markdown(content) {
        return Language::Markdown;
    }
    // Untitled buffers are notes; notes are markdown by spec default.
    Language::Markdown
}

fn sniff_markdown(content: &str) -> bool {
    let mut score = 0i32;
    for (n, line) in content.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        if n >= 16 {
            break;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("# ")
            || trimmed.starts_with("## ")
            || trimmed.starts_with("### ")
            || trimmed.starts_with("#### ")
            || trimmed.starts_with("##### ")
            || trimmed.starts_with("###### ")
        {
            score += 3;
        }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
            score += 1;
        }
        if trimmed.starts_with("> ") {
            score += 1;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            score += 2;
        }
        if line.contains("**") || line.contains("__") {
            score += 1;
        }
        if line.contains("](http") || line.contains("](#") {
            score += 1;
        }
    }
    score >= 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_md_detects_markdown() {
        assert_eq!(detect(Some("md"), ""), Language::Markdown);
        assert_eq!(detect(Some("MARKDOWN"), ""), Language::Markdown);
    }

    #[test]
    fn code_extensions_detect_code_languages() {
        // Spec §3 markdown-first default only applies to *untitled* buffers.
        // A file with a non-markdown extension keeps its name's authority.
        assert_eq!(detect(Some("txt"), "hello"), Language::Plain);
        assert_eq!(
            detect(Some("toml"), "# still literal"),
            Language::Code("toml")
        );
        assert_eq!(detect(Some("rs"), "fn main()"), Language::Code("rust"));
        assert_eq!(
            detect(Some("json"), r#"{"ok": true}"#),
            Language::Code("json")
        );
    }

    #[test]
    fn content_sniff_picks_up_markdown() {
        let src = "# Title\n\n## Sub\n\n- a\n- b\n\n```\ncode\n```\n";
        assert_eq!(detect(None, src), Language::Markdown);
    }

    #[test]
    fn untitled_buffer_defaults_to_markdown() {
        // Spec §3: "The buffer is plain markdown text. Always."
        // Empty / unstructured buffers without a file path are notes — and
        // notes are markdown by default so `language == 'markdown'`
        // keybindings (Ctrl+B, Ctrl+I, headings, …) fire from the first
        // keystroke.
        assert_eq!(detect(None, ""), Language::Markdown);
        assert_eq!(
            detect(None, "Just some prose without any structure."),
            Language::Markdown
        );
    }

    #[test]
    fn as_str_matches_predicate_grammar() {
        assert_eq!(Language::Markdown.as_str(), "markdown");
        assert_eq!(Language::Plain.as_str(), "plain");
    }
}
