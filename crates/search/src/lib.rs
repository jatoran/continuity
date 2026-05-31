#![warn(missing_docs)]
//! Cross-buffer search (ripgrep) and FTS5 quick-open index.

pub mod dispatcher;
pub mod error;
pub mod fuzzy;
pub mod index;
pub mod literal;
pub mod regex;

pub use dispatcher::{classify_pattern, find_match_ranges_dispatch, DispatchResult, PatternPath};
pub use error::Error;
pub use fuzzy::{rank, score, FuzzyMatch};
pub use index::{SearchHit, SearchIndex};
pub use literal::{is_ascii_word_boundary, LiteralMatcher};
pub use regex::{
    compile_regex, escape_literal, find_match_ranges, find_matches, CompiledRegex, MatchRange,
    MatchSpan,
};
