//! Regex helper data for the find bar.

/// Find-bar chrome controls that can be hovered or clicked.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum FindControl {
    /// Case-sensitive search toggle.
    Case,
    /// Whole-word search toggle.
    Word,
    /// Regex search toggle.
    Regex,
    /// Preserve matched-text case while replacing.
    PreserveCase,
    /// Buffer vs selection scope toggle.
    Scope,
    /// Replace-field visibility toggle.
    Replace,
    /// Replace the current match.
    ReplaceOne,
    /// Replace all matches.
    ReplaceAll,
    /// Step to the previous match.
    Previous,
    /// Step to the next match.
    Next,
    /// Convert matches to cursors.
    Cursors,
}

/// One clickable regex helper row.
pub(crate) struct RegexSnippet {
    /// Visible syntax sample.
    pub label: &'static str,
    /// Text inserted into the find field.
    pub insert: &'static str,
    /// Plain-language description.
    pub description: &'static str,
}

/// Common regex snippets shown from the regex toggle hover panel.
pub(crate) const REGEX_SNIPPETS: &[RegexSnippet] = &[
    RegexSnippet {
        label: ".",
        insert: ".",
        description: "any one character",
    },
    RegexSnippet {
        label: "\\d+",
        insert: "\\d+",
        description: "one or more digits",
    },
    RegexSnippet {
        label: "\\w+",
        insert: "\\w+",
        description: "one or more word chars",
    },
    RegexSnippet {
        label: "\\s+",
        insert: "\\s+",
        description: "one or more spaces",
    },
    RegexSnippet {
        label: "^",
        insert: "^",
        description: "start of a line",
    },
    RegexSnippet {
        label: "$",
        insert: "$",
        description: "end of a line",
    },
    RegexSnippet {
        label: ".*?",
        insert: ".*?",
        description: "shortest any text",
    },
    RegexSnippet {
        label: "(one|two)",
        insert: "(one|two)",
        description: "one term or another",
    },
];
