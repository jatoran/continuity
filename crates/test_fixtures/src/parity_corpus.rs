//! Frozen semantic fixtures shared by every editor-engine host.
//!
//! These values are deliberately plain Rust data: native core, WASM, and
//! future language bindings can consume the same inputs and compare their
//! externally visible results without depending on another implementation.

/// Initial text for the multi-cursor edit case.
pub const MULTI_CURSOR_INITIAL_TEXT: &str = "alpha\nbeta\n";

/// Text after inserting `!` at the end of both non-empty lines.
pub const MULTI_CURSOR_EXPECTED_TEXT: &str = "alpha!\nbeta!\n";

/// Expected caret positions after the multi-cursor edit, as `(line, byte)`.
pub const MULTI_CURSOR_EXPECTED_CARETS: &[(u32, u32)] = &[(0, 6), (1, 5)];

/// Single-character typing burst used to verify undo coalescing.
pub const TYPING_BURST: &[&str] = &["a", "b", "c"];

/// Expected content after [`TYPING_BURST`].
pub const TYPING_BURST_EXPECTED_TEXT: &str = "abc";

/// Prefix, abandoned branch, and replacement branch for undo-tree parity.
pub const UNDO_BRANCH_INPUTS: &[&str] = &["old", "branch", "new"];

/// Expected content on the replacement branch.
pub const UNDO_BRANCH_REPLACEMENT_TEXT: &str = "oldnew";

/// Expected content after selecting the alternate branch.
pub const UNDO_BRANCH_ALTERNATE_TEXT: &str = "oldbranch";

/// Markdown source used by decoration and display-projection parity tests.
pub const MARKDOWN_SOURCE: &str = "# Heading\n\nThis is **bold** and *italic*.\n";

/// Visible lines after structural Markdown markers are projected away.
pub const MARKDOWN_DISPLAY_LINES: &[&str] = &["Heading", "", "This is bold and *italic*.", ""];

/// Expected block fingerprint `(kind, start_byte, end_byte)`.
pub const MARKDOWN_BLOCKS: &[(&str, usize, usize)] = &[("heading:1", 0, 10), ("paragraph", 11, 42)];

/// Expected inline fingerprint `(kind, start_byte, end_byte)`.
pub const MARKDOWN_INLINES: &[(&str, usize, usize)] = &[
    ("marker:heading", 0, 2),
    ("marker:emphasis", 19, 21),
    ("strong", 21, 25),
    ("marker:emphasis", 25, 27),
    ("marker:emphasis", 32, 33),
    ("emphasis", 33, 39),
    ("marker:emphasis", 39, 40),
];

/// Serialized cross-runtime engine, decoration, and display-map fixture.
pub const WASM_PARITY_FIXTURE_JSON: &str = include_str!("../fixtures/wasm_engine_parity.json");
