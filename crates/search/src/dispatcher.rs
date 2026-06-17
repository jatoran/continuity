//! Find-bar pattern dispatcher.
//!
//! Routes a user-typed find query to either the SIMD-accelerated
//! literal path (see [`crate::literal::LiteralMatcher`]) or the
//! existing `grep_regex`-backed regex path (see
//! [`crate::regex::find_match_ranges`]).
//!
//! The dispatcher exists because the regex engine has non-trivial
//! setup cost (NFA construction, DFA cache, sink machinery) and
//! offers no speed advantage for the common case of a literal
//! substring query without word-boundary semantics. For literal
//! patterns we skip the engine entirely and use `memchr::memmem`
//! (SIMD, runtime CPU-feature detection).
//!
//! Thread ownership: the dispatcher is stateless — it borrows the
//! query and source and returns a [`DispatchResult`]. Safe to call
//! from any thread.

use std::time::Instant;

use crate::literal::{is_ascii_word_boundary, LiteralMatcher};
use crate::regex::{find_match_ranges, find_match_ranges_multiline, MatchRange};
use crate::Error;

/// Which engine produced the matches returned by
/// [`find_match_ranges_dispatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternPath {
    /// SIMD literal substring search via `memchr::memmem`.
    Literal,
    /// `grep_regex::RegexMatcher` + `grep_searcher`.
    Regex,
}

impl PatternPath {
    /// Stable label for trace events (`path=literal|regex`).
    #[must_use]
    pub fn as_trace_label(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Regex => "regex",
        }
    }
}

/// Result of [`find_match_ranges_dispatch`].
#[derive(Debug, Clone)]
pub struct DispatchResult {
    /// Matches in left-to-right order. Identical in shape to the
    /// existing [`find_match_ranges`] return.
    pub matches: Vec<MatchRange>,
    /// Which engine ran.
    pub path: PatternPath,
    /// Wall-clock elapsed time, including pattern classification +
    /// engine setup + scan + result construction.
    pub elapsed_us: u64,
}

/// Decide whether `query` qualifies for the literal fast path.
///
/// Returns [`PatternPath::Literal`] when **all** of the following hold:
/// - `is_regex_enabled == false` (the user did not toggle the regex
///   button), AND
/// - the query has no regex metacharacters that would change meaning
///   under the engine (covered implicitly — when the user is in
///   literal mode their input is taken verbatim), AND
/// - either `case_sensitive == true` or the query is ASCII-only
///   (so the literal path's `eq_ignore_ascii_case` covers the
///   case-fold semantics exactly).
///
/// Whole-word semantics are handled by a post-filter on the literal
/// path — they do **not** force the regex path.
#[must_use]
pub fn classify_pattern(query: &str, is_regex_enabled: bool, case_sensitive: bool) -> PatternPath {
    if is_regex_enabled {
        return PatternPath::Regex;
    }
    if !case_sensitive && !query.is_ascii() {
        // Non-ASCII case folding would require Unicode-aware
        // lowercasing; defer to the regex engine's `(?i)`.
        return PatternPath::Regex;
    }
    PatternPath::Literal
}

/// Find all matches of `query` in `source`, choosing the engine via
/// [`classify_pattern`].
///
/// `is_regex` corresponds to the find-bar regex toggle. `case_sensitive`
/// is the case-sensitivity toggle. `whole_word` wraps the query in
/// `\b…\b` semantics: on the literal path it's a post-filter; on the
/// regex path it's a syntax wrap (matches the existing
/// [`find_match_ranges`] behavior).
///
/// # Errors
///
/// Returns [`Error::InvalidRegex`] if the regex path is taken and the
/// query (post-whole-word-wrap, if any) doesn't compile. The literal
/// path never returns this error.
pub fn find_match_ranges_dispatch(
    query: &str,
    source: &str,
    is_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
) -> Result<DispatchResult, Error> {
    let started = Instant::now();
    if query.is_empty() {
        return Ok(DispatchResult {
            matches: Vec::new(),
            path: PatternPath::Literal,
            elapsed_us: started.elapsed().as_micros() as u64,
        });
    }
    let path = classify_pattern(query, is_regex, case_sensitive);
    let matches = match path {
        PatternPath::Literal => run_literal(query, source, case_sensitive, whole_word),
        PatternPath::Regex => {
            if is_regex {
                // User-toggled regex: route through the WHOLE-BUFFER
                // multi-line matcher so a literal `\n` (typed as the
                // two-char escape) or `(?s).` can match across line
                // boundaries, and `^`/`$` anchor per line. The query
                // reaches the engine verbatim — it is NOT escaped — so
                // the engine's own `\n` handling applies.
                find_match_ranges_multiline(query, source, !case_sensitive, whole_word)?
            } else {
                // Non-ASCII case-insensitive literal fallback: the query
                // is escaped to a literal byte sequence (no anchors, no
                // newline construct), so the line-oriented path is
                // sufficient and preserves prior behavior.
                let effective_pattern = crate::regex::escape_literal(query);
                find_match_ranges(&effective_pattern, source, !case_sensitive, whole_word)?
            }
        }
    };
    Ok(DispatchResult {
        matches,
        path,
        elapsed_us: started.elapsed().as_micros() as u64,
    })
}

/// Run the literal path against a contiguous source. Produces matches
/// in the same `MatchRange` shape as the regex path (1-indexed line
/// number, absolute byte offsets).
fn run_literal(
    query: &str,
    source: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Vec<MatchRange> {
    let matcher = if case_sensitive {
        LiteralMatcher::new(query.as_bytes())
    } else {
        LiteralMatcher::new_ascii_case_insensitive(query.as_bytes())
    };
    let plen = matcher.pattern_len();
    if plen == 0 {
        return Vec::new();
    }
    let bytes = source.as_bytes();
    let line_starts = compute_line_starts(bytes);
    let mut out = Vec::new();
    for start in matcher.find_iter(bytes) {
        let end = start + plen;
        if whole_word && !is_ascii_word_boundary(bytes, start, end) {
            continue;
        }
        let line = line_number_for_byte(&line_starts, start);
        out.push(MatchRange {
            line,
            start_byte: start,
            end_byte: end,
        });
    }
    out
}

/// Build a `Vec<usize>` of line-start byte offsets. `line_starts[0]`
/// is always `0`; `line_starts[i]` for `i >= 1` is one past the i-th
/// `\n` byte.
fn compute_line_starts(bytes: &[u8]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(bytes.len() / 64 + 1);
    starts.push(0);
    // memchr::memchr_iter gives us SIMD newline scanning for free.
    for newline_pos in memchr::memchr_iter(b'\n', bytes) {
        starts.push(newline_pos + 1);
    }
    starts
}

/// Resolve the 1-indexed line number containing `byte` by binary-
/// searching `line_starts`.
fn line_number_for_byte(line_starts: &[usize], byte: usize) -> u64 {
    // partition_point returns the first index whose start > `byte`;
    // subtracting 1 gives the line index (0-based), which becomes
    // 1-indexed for `MatchRange::line`.
    let idx = line_starts.partition_point(|&start| start <= byte);
    idx as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_regex_toggle_forces_regex_path() {
        assert_eq!(
            classify_pattern("foo", true, true),
            PatternPath::Regex,
            "regex toggle on -> Regex"
        );
        assert_eq!(classify_pattern(r"foo\b", true, false), PatternPath::Regex,);
    }

    #[test]
    fn classify_literal_when_regex_off_and_ascii() {
        assert_eq!(classify_pattern("foo", false, true), PatternPath::Literal);
        assert_eq!(classify_pattern("FOO", false, false), PatternPath::Literal);
        // Metachars in literal-mode are still safe; the engine isn't
        // running. ASCII-only is the only constraint when
        // case-insensitive.
        assert_eq!(classify_pattern("a.b*c", false, true), PatternPath::Literal,);
    }

    #[test]
    fn classify_non_ascii_case_insensitive_falls_back_to_regex() {
        // "café" is non-ASCII; case-insensitive literal path doesn't
        // handle Unicode case-fold, so we route to regex.
        assert_eq!(classify_pattern("café", false, false), PatternPath::Regex);
    }

    #[test]
    fn classify_non_ascii_case_sensitive_stays_literal() {
        // No case-fold needed -> literal path handles the bytes
        // exactly.
        assert_eq!(classify_pattern("café", false, true), PatternPath::Literal);
    }

    #[test]
    fn dispatch_empty_query_returns_no_matches() {
        let r = find_match_ranges_dispatch("", "anything", false, true, false).unwrap();
        assert!(r.matches.is_empty());
    }

    #[test]
    fn dispatch_literal_path_finds_matches_with_line_numbers() {
        let src = "foo bar foo\nbaz foo";
        let r = find_match_ranges_dispatch("foo", src, false, true, false).unwrap();
        assert_eq!(r.path, PatternPath::Literal);
        assert_eq!(r.matches.len(), 3);
        assert_eq!(r.matches[0].line, 1);
        assert_eq!(r.matches[0].start_byte, 0);
        assert_eq!(r.matches[1].line, 1);
        assert_eq!(r.matches[1].start_byte, 8);
        assert_eq!(r.matches[2].line, 2);
        assert_eq!(r.matches[2].start_byte, 16);
    }

    #[test]
    fn dispatch_literal_matches_regex_baseline_case_sensitive() {
        let src = "foo bar Foo foo\nFOO";
        let literal = find_match_ranges_dispatch("foo", src, false, true, false).unwrap();
        let regex = find_match_ranges("foo", src, false, false).unwrap();
        assert_eq!(literal.path, PatternPath::Literal);
        assert_eq!(literal.matches, regex);
    }

    #[test]
    fn dispatch_literal_matches_regex_baseline_case_insensitive() {
        let src = "foo Foo FOO\nfOo";
        let literal = find_match_ranges_dispatch("foo", src, false, false, false).unwrap();
        let regex = find_match_ranges("foo", src, true, false).unwrap();
        assert_eq!(literal.path, PatternPath::Literal);
        assert_eq!(literal.matches.len(), 4);
        assert_eq!(literal.matches, regex);
    }

    #[test]
    fn dispatch_literal_whole_word_filter() {
        let src = "foobar foo foo_bar foo!";
        let r = find_match_ranges_dispatch("foo", src, false, true, true).unwrap();
        assert_eq!(r.path, PatternPath::Literal);
        // Only the standalone "foo" (at byte 7) and the one before
        // "!" (at byte 19) qualify. "foobar" and "foo_bar" fail the
        // word boundary.
        let starts: Vec<usize> = r.matches.iter().map(|m| m.start_byte).collect();
        assert_eq!(starts, vec![7, 19]);
    }

    #[test]
    fn dispatch_literal_whole_word_matches_regex_baseline() {
        let src = "foobar foo foo_bar foo!";
        let literal = find_match_ranges_dispatch("foo", src, false, true, true).unwrap();
        let regex = find_match_ranges("foo", src, false, true).unwrap();
        assert_eq!(literal.matches, regex);
    }

    #[test]
    fn dispatch_regex_path_invoked_when_toggle_on() {
        // A regex-only construct: "f.o" matches "foo" / "fxo".
        let src = "foo fxo bar";
        let r = find_match_ranges_dispatch("f.o", src, true, true, false).unwrap();
        assert_eq!(r.path, PatternPath::Regex);
        assert_eq!(r.matches.len(), 2);
    }

    #[test]
    fn dispatch_regex_path_propagates_invalid_regex_error() {
        let r = find_match_ranges_dispatch("(", "x", true, true, false);
        assert!(matches!(r, Err(Error::InvalidRegex(_))));
    }

    #[test]
    fn dispatch_regex_matches_literal_newline_across_lines() {
        // With regex ON, the two-char escape `\n` reaches the engine and
        // matches across the line break (Sublime-style multi-line find).
        let src = "first line\nsecond line";
        let r = find_match_ranges_dispatch(r"line\nsecond", src, true, true, false).unwrap();
        assert_eq!(r.path, PatternPath::Regex);
        assert_eq!(r.matches.len(), 1);
        assert_eq!(
            &src[r.matches[0].start_byte..r.matches[0].end_byte],
            "line\nsecond"
        );
    }

    #[test]
    fn dispatch_regex_dot_all_spans_lines() {
        let src = "<a>\nbody\n</a>";
        let r = find_match_ranges_dispatch(r"(?s)<a>.*</a>", src, true, true, false).unwrap();
        assert_eq!(r.path, PatternPath::Regex);
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].start_byte, 0);
        assert_eq!(r.matches[0].end_byte, src.len());
    }

    #[test]
    fn dispatch_regex_caret_dollar_anchor_per_line() {
        let src = "alpha\nbeta\ngamma";
        let r = find_match_ranges_dispatch(r"^beta$", src, true, true, false).unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].line, 2);
    }

    #[test]
    fn dispatch_literal_metacharacters_are_literal_in_literal_mode() {
        // With regex off, "a.b" should match the literal three bytes
        // and **not** "axb".
        let src = "axb a.b a-b";
        let r = find_match_ranges_dispatch("a.b", src, false, true, false).unwrap();
        assert_eq!(r.path, PatternPath::Literal);
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].start_byte, 4);
    }

    #[test]
    fn line_starts_handles_no_trailing_newline() {
        let starts = compute_line_starts(b"a\nbb\nccc");
        assert_eq!(starts, vec![0, 2, 5]);
    }

    #[test]
    fn line_number_lookup_returns_one_indexed() {
        let starts = compute_line_starts(b"a\nbb\nccc");
        assert_eq!(line_number_for_byte(&starts, 0), 1);
        assert_eq!(line_number_for_byte(&starts, 1), 1);
        assert_eq!(line_number_for_byte(&starts, 2), 2);
        assert_eq!(line_number_for_byte(&starts, 4), 2);
        assert_eq!(line_number_for_byte(&starts, 5), 3);
        assert_eq!(line_number_for_byte(&starts, 7), 3);
    }
}
