//! `grep-regex` + `grep-searcher` wrappers for in-memory text search.
//!
//! The full editor uses these libraries to walk multiple buffers from the
//! search/index thread; this module exposes a single-buffer helper sufficient
//! for proving the dependency links and tests can find matches.

use grep_regex::RegexMatcher;
use grep_searcher::{sinks::UTF8, Searcher, SearcherBuilder, Sink, SinkMatch};

use crate::Error;

/// One match in a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchSpan {
    /// 1-indexed line number (matches `grep_searcher`'s convention).
    pub line: u64,
    /// The full text of the matching line.
    pub line_text: String,
}

/// Search `source` for occurrences of `pattern` (a regex).
///
/// `case_insensitive = true` enables `(?i)` semantics.
///
/// # Errors
///
/// Returns [`Error::InvalidRegex`] if `pattern` doesn't compile.
pub fn find_matches(
    pattern: &str,
    source: &str,
    case_insensitive: bool,
) -> Result<Vec<MatchSpan>, Error> {
    let matcher = grep_regex::RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .build(pattern)
        .map_err(|e| Error::InvalidRegex(e.to_string()))?;
    find_with_matcher(&matcher, source)
}

fn find_with_matcher(matcher: &RegexMatcher, source: &str) -> Result<Vec<MatchSpan>, Error> {
    let mut hits = Vec::new();
    SearcherBuilder::new()
        .line_number(true)
        .build()
        .search_slice(
            matcher,
            source.as_bytes(),
            UTF8(|line, text| {
                hits.push(MatchSpan {
                    line,
                    line_text: text.trim_end_matches('\n').to_string(),
                });
                Ok(true)
            }),
        )?;
    Ok(hits)
}

/// One match in a buffer, expressed as a byte range over the full source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchRange {
    /// 1-indexed line number.
    pub line: u64,
    /// Inclusive start byte offset into the source.
    pub start_byte: usize,
    /// Exclusive end byte offset into the source.
    pub end_byte: usize,
}

/// Search `source` for `pattern`, returning byte ranges per match.
///
/// `case_insensitive` toggles `(?i)` semantics. `whole_word` wraps the regex
/// in word-boundary assertions (`\b…\b`).
///
/// # Errors
///
/// Returns [`Error::InvalidRegex`] if `pattern` doesn't compile (after the
/// optional whole-word wrapping).
pub fn find_match_ranges(
    pattern: &str,
    source: &str,
    case_insensitive: bool,
    whole_word: bool,
) -> Result<Vec<MatchRange>, Error> {
    if pattern.is_empty() {
        return Ok(Vec::new());
    }
    let effective = if whole_word {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern.to_string()
    };
    let matcher = grep_regex::RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .build(&effective)
        .map_err(|e| Error::InvalidRegex(e.to_string()))?;
    let mut hits = Vec::new();
    let sink = RangeSink {
        matcher: &matcher,
        out: &mut hits,
    };
    SearcherBuilder::new()
        .line_number(true)
        .build()
        .search_slice(&matcher, source.as_bytes(), sink)?;
    Ok(hits)
}

struct RangeSink<'a> {
    matcher: &'a RegexMatcher,
    out: &'a mut Vec<MatchRange>,
}

impl Sink for RangeSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        sink_match: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        let line_no = sink_match.line_number().unwrap_or(0);
        let line_start = sink_match.absolute_byte_offset() as usize;
        let bytes = sink_match.bytes();
        use grep_matcher::Matcher;
        let mut start_in_line = 0usize;
        loop {
            let region = self
                .matcher
                .find_at(bytes, start_in_line)
                .map_err(std::io::Error::other)?;
            let Some(m) = region else { break };
            self.out.push(MatchRange {
                line: line_no,
                start_byte: line_start + m.start(),
                end_byte: line_start + m.end(),
            });
            if m.end() == start_in_line {
                start_in_line = m.end() + 1;
            } else {
                start_in_line = m.end();
            }
            if start_in_line > bytes.len() {
                break;
            }
        }
        Ok(true)
    }
}

/// Escape `literal` so it can be used as a regex matching the original text.
#[must_use]
pub fn escape_literal(literal: &str) -> String {
    grep_regex::RegexMatcherBuilder::new();
    regex_syntax::escape(literal)
}

/// G5 — a compiled regex suitable for in-memory match / find_iter over
/// byte slices. Thin newtype over [`grep_regex::RegexMatcher`] so
/// callers don't import grep types directly.
#[derive(Debug)]
pub struct CompiledRegex(RegexMatcher);

impl CompiledRegex {
    /// `true` when `haystack` contains a match.
    #[must_use]
    pub fn is_match(&self, haystack: &[u8]) -> bool {
        use grep_matcher::Matcher;
        self.0.is_match(haystack).unwrap_or(false)
    }

    /// All non-overlapping match byte ranges within `haystack`, in
    /// left-to-right order.
    #[must_use]
    pub fn find_ranges(&self, haystack: &[u8]) -> Vec<(usize, usize)> {
        use grep_matcher::Matcher;
        let mut out = Vec::new();
        let _ = self.0.find_iter(haystack, |m| {
            out.push((m.start(), m.end()));
            true
        });
        out
    }
}

/// G5 — compile a regex pattern for [`CompiledRegex`]. Mirrors the
/// behavior of [`find_match_ranges`] for input syntax.
///
/// # Errors
///
/// Returns [`Error::InvalidRegex`] when `pattern` doesn't compile.
pub fn compile_regex(pattern: &str) -> Result<CompiledRegex, Error> {
    let m = grep_regex::RegexMatcherBuilder::new()
        .build(pattern)
        .map_err(|e| Error::InvalidRegex(e.to_string()))?;
    Ok(CompiledRegex(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_literal_match() {
        let m = find_matches("rope", "ropey is a rope library\nstd has Vec", false).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].line, 1);
        assert!(m[0].line_text.contains("rope"));
    }

    #[test]
    fn case_insensitive_match() {
        let m = find_matches("RUST", "rust", true).unwrap();
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn case_sensitive_misses() {
        let m = find_matches("RUST", "rust", false).unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn invalid_regex_errors() {
        assert!(find_matches("(", "x", false).is_err());
    }

    #[test]
    fn line_numbers_are_one_indexed() {
        let m = find_matches("hit", "miss\nhit\nmiss", false).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].line, 2);
    }

    #[test]
    fn finds_multiple_matches_across_lines() {
        let m = find_matches("the", "the cat\nis on\nthe mat", false).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].line, 1);
        assert_eq!(m[1].line, 3);
    }

    #[test]
    fn match_ranges_returns_byte_offsets() {
        let src = "foo bar foo\nbaz foo";
        let m = find_match_ranges("foo", src, false, false).unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].start_byte, 0);
        assert_eq!(m[0].end_byte, 3);
        assert_eq!(m[1].start_byte, 8);
        assert_eq!(m[2].line, 2);
        assert_eq!(&src[m[2].start_byte..m[2].end_byte], "foo");
    }

    #[test]
    fn match_ranges_whole_word_filters_partial() {
        let m = find_match_ranges("foo", "foobar foo", false, true).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].start_byte, 7);
    }

    #[test]
    fn match_ranges_case_insensitive() {
        let m = find_match_ranges("HELLO", "hello Hello HELLO", true, false).unwrap();
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn match_ranges_empty_pattern() {
        let m = find_match_ranges("", "anything", false, false).unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn escape_literal_handles_metacharacters() {
        let p = escape_literal("a.b*c");
        let m = find_match_ranges(&p, "a.b*c not abxc", false, false).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].start_byte, 0);
    }
}
