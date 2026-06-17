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
/// This drives the line-oriented `grep_searcher` sink, so a single match can
/// never span a line boundary. For find/replace that must be able to match
/// across newlines (a literal `\n` or `(?s).` in the query), use
/// [`find_match_ranges_multiline`] instead — the dispatcher routes the regex
/// find-bar through that whole-buffer path.
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

/// Search `source` for `pattern` over the WHOLE buffer at once, returning byte
/// ranges per match that may cross line boundaries.
///
/// Unlike [`find_match_ranges`], this does not use the line-oriented
/// `grep_searcher` sink. Instead it compiles a [`RegexMatcher`] with
/// multi-line matching enabled and runs `find_iter` over the full source
/// bytes, so:
///
/// - A pattern containing a literal `\n` (the user types the two-char escape
///   `\` + `n` into the single-line find field, and the regex engine treats
///   `\n` as a newline) matches across the line break.
/// - `(?s).` matches any byte including newlines.
/// - `^` and `$` anchor to the beginning/end of each line (multi-line `m`
///   flag), matching Sublime/ripgrep semantics, rather than to the
///   beginning/end of the whole input.
///
/// `case_insensitive` toggles `(?i)` semantics. `whole_word` wraps the regex
/// in word-boundary assertions (`\b…\b`).
///
/// The returned [`MatchRange::line`] is the 1-indexed line of the match
/// *start*; a match that spans lines reports its starting line.
///
/// # Errors
///
/// Returns [`Error::InvalidRegex`] if `pattern` doesn't compile (after the
/// optional whole-word wrapping).
pub fn find_match_ranges_multiline(
    pattern: &str,
    source: &str,
    case_insensitive: bool,
    whole_word: bool,
) -> Result<Vec<MatchRange>, Error> {
    use grep_matcher::Matcher;

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
        // Multi-line: `^`/`$` anchor per line; the matcher operates over the
        // whole haystack so a match can cross newlines.
        .multi_line(true)
        .build(&effective)
        .map_err(|e| Error::InvalidRegex(e.to_string()))?;

    let bytes = source.as_bytes();
    let line_starts = compute_line_starts(bytes);
    let mut hits = Vec::new();
    // `find_iter` yields successive non-overlapping matches with absolute
    // byte offsets into the slice, and advances past zero-width matches
    // internally, so no manual cursor bookkeeping is required.
    matcher
        .find_iter(bytes, |m| {
            hits.push(MatchRange {
                line: line_number_for_byte(&line_starts, m.start()),
                start_byte: m.start(),
                end_byte: m.end(),
            });
            true
        })
        .map_err(|e| Error::InvalidRegex(e.to_string()))?;
    Ok(hits)
}

/// Build a `Vec<usize>` of line-start byte offsets. `line_starts[0]` is always
/// `0`; `line_starts[i]` for `i >= 1` is one past the i-th `\n` byte.
fn compute_line_starts(bytes: &[u8]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(bytes.len() / 64 + 1);
    starts.push(0);
    for newline_pos in memchr::memchr_iter(b'\n', bytes) {
        starts.push(newline_pos + 1);
    }
    starts
}

/// Resolve the 1-indexed line number containing `byte` by binary-searching
/// `line_starts`.
fn line_number_for_byte(line_starts: &[usize], byte: usize) -> u64 {
    line_starts.partition_point(|&start| start <= byte) as u64
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

    #[test]
    fn multiline_matches_literal_newline_across_two_lines() {
        // The user types the two-char escape `\` `n`; in a raw string
        // r"a\nb" that is exactly the pattern that reaches the engine,
        // which interprets `\n` as a newline.
        let src = "xa\nbz";
        let m = find_match_ranges_multiline(r"a\nb", src, false, false).unwrap();
        assert_eq!(m.len(), 1, "the a-newline-b span should match");
        // Match spans the newline: starts at the 'a' (byte 1), ends past
        // the 'b' (byte 4).
        assert_eq!(m[0].start_byte, 1);
        assert_eq!(m[0].end_byte, 4);
        assert_eq!(&src[m[0].start_byte..m[0].end_byte], "a\nb");
        assert_eq!(m[0].line, 1, "match start is on line 1");
    }

    #[test]
    fn line_oriented_path_cannot_match_across_newline() {
        // Regression guard: the legacy per-line path can NOT match a
        // newline, which is exactly why the multiline path exists.
        let m = find_match_ranges(r"a\nb", "a\nb", false, false).unwrap();
        assert!(m.is_empty(), "per-line sink never sees the newline");
    }

    #[test]
    fn multiline_dot_all_matches_across_lines() {
        // `(?s)` makes `.` match newlines too.
        let src = "start\nmiddle\nend";
        let m = find_match_ranges_multiline(r"(?s)start.*end", src, false, false).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].start_byte, 0);
        assert_eq!(m[0].end_byte, src.len());
    }

    #[test]
    fn multiline_caret_dollar_anchor_per_line() {
        // With multi_line enabled, `^` / `$` anchor each line.
        let src = "one\ntwo\nthree";
        let m = find_match_ranges_multiline(r"^two$", src, false, false).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(&src[m[0].start_byte..m[0].end_byte], "two");
        assert_eq!(m[0].line, 2, "match is on the second line");
    }

    #[test]
    fn multiline_reports_start_line_for_spanning_match() {
        let src = "alpha\nbeta\ngamma";
        // Match begins on line 2 and ends on line 3.
        let m = find_match_ranges_multiline(r"(?s)beta.*gam", src, false, false).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].line, 2);
    }

    #[test]
    fn multiline_case_insensitive() {
        let src = "Hello\nWORLD";
        let m = find_match_ranges_multiline(r"(?s)hello.world", src, true, false).unwrap();
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn multiline_whole_word_wraps_boundaries() {
        let src = "foobar\nfoo bar";
        let m = find_match_ranges_multiline("foo", src, false, true).unwrap();
        // Only the standalone "foo" on line 2 qualifies.
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].line, 2);
    }

    #[test]
    fn multiline_empty_pattern_returns_none() {
        let m = find_match_ranges_multiline("", "anything", false, false).unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn multiline_invalid_regex_errors() {
        assert!(find_match_ranges_multiline("(", "x", false, false).is_err());
    }

    #[test]
    fn multiline_multiple_newline_spanning_matches() {
        let src = "a\nb x a\nb";
        let m = find_match_ranges_multiline(r"a\nb", src, false, false).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(&src[m[0].start_byte..m[0].end_byte], "a\nb");
        assert_eq!(&src[m[1].start_byte..m[1].end_byte], "a\nb");
        assert_eq!(m[0].line, 1);
        assert_eq!(m[1].line, 2, "second match starts on line 2");
    }

    #[test]
    fn line_starts_and_lookup_are_one_indexed() {
        let starts = compute_line_starts(b"a\nbb\nccc");
        assert_eq!(starts, vec![0, 2, 5]);
        assert_eq!(line_number_for_byte(&starts, 0), 1);
        assert_eq!(line_number_for_byte(&starts, 2), 2);
        assert_eq!(line_number_for_byte(&starts, 5), 3);
        assert_eq!(line_number_for_byte(&starts, 7), 3);
    }
}
