//! Literal substring matcher backed by `memchr::memmem::Finder` (SIMD,
//! runtime CPU-feature detection).
//!
//! Two modes:
//!
//! - **Case-sensitive (default)** — a raw [`memchr::memmem::Finder`] is
//!   used directly. Throughput approaches `memcpy` on AVX2 hardware.
//! - **ASCII case-insensitive** — `memchr::memchr2` finds candidate
//!   positions (lower / upper of the pattern's first byte) and a manual
//!   pattern compare with [`u8::eq_ignore_ascii_case`] confirms each.
//!   Non-ASCII bytes are compared byte-for-byte (no case-fold).
//!
//! The matcher is contiguous-buffer friendly via [`LiteralMatcher::find_iter`]
//! and rope-chunk friendly via [`LiteralMatcher::find_in_chunks`], which
//! carries an overlap of `pattern.len() - 1` bytes between chunks so a
//! match that straddles a chunk boundary is still found.
//!
//! Thread ownership: stateless — `LiteralMatcher` is `Send + Sync` and a
//! single instance may be shared across the search thread and any
//! caller-owned buffer pass.

use memchr::memmem::Finder;

/// Owns a `memmem::Finder` plus the pattern bytes used for confirmation
/// on the case-insensitive path. Reusable across haystacks.
#[derive(Debug, Clone)]
pub struct LiteralMatcher {
    /// Lowercased-ASCII pattern bytes when `case_insensitive_ascii`,
    /// otherwise the raw pattern.
    pattern: Box<[u8]>,
    /// `memmem` finder built on `pattern`. Owned, so the matcher has no
    /// borrowed lifetime.
    finder: Finder<'static>,
    /// When true, candidate matches are confirmed with
    /// `eq_ignore_ascii_case` and the case-insensitive chunked scan
    /// uses `memchr2(first_lower, first_upper, …)`.
    case_insensitive_ascii: bool,
}

impl LiteralMatcher {
    /// Build a case-sensitive matcher.
    #[must_use]
    pub fn new(pattern: &[u8]) -> Self {
        let pattern: Box<[u8]> = pattern.into();
        let finder = Finder::new(pattern.as_ref()).into_owned();
        Self {
            pattern,
            finder,
            case_insensitive_ascii: false,
        }
    }

    /// Build an ASCII case-insensitive matcher. Non-ASCII bytes in
    /// `pattern` are kept verbatim and compared byte-for-byte.
    #[must_use]
    pub fn new_ascii_case_insensitive(pattern: &[u8]) -> Self {
        let lower: Box<[u8]> = pattern.iter().map(u8::to_ascii_lowercase).collect();
        let finder = Finder::new(lower.as_ref()).into_owned();
        Self {
            pattern: lower,
            finder,
            case_insensitive_ascii: true,
        }
    }

    /// Pattern length in bytes (post-lowercase for the case-insensitive
    /// constructor — same as the input length).
    #[must_use]
    pub fn pattern_len(&self) -> usize {
        self.pattern.len()
    }

    /// `true` when the matcher was built with the case-insensitive
    /// constructor.
    #[must_use]
    pub fn is_ascii_case_insensitive(&self) -> bool {
        self.case_insensitive_ascii
    }

    /// Iterate non-overlapping match start offsets in `haystack`.
    ///
    /// Matches are returned in left-to-right order. After yielding a
    /// match at `start`, the iterator advances by `max(1,
    /// pattern.len())`, mirroring the regex engine's non-overlapping
    /// semantics.
    pub fn find_iter<'h>(&'h self, haystack: &'h [u8]) -> LiteralMatchIter<'h> {
        LiteralMatchIter {
            matcher: self,
            haystack,
            pos: 0,
        }
    }

    /// Collect non-overlapping match start offsets across a sequence of
    /// rope chunks, returning **global** byte offsets (sum of preceding
    /// chunk lengths plus in-chunk match start).
    ///
    /// An overlap window of `pattern.len() - 1` bytes is carried from
    /// the tail of each chunk into the head of the next so a match that
    /// straddles a chunk boundary is still found.
    ///
    /// Empty pattern returns no matches (mirrors `find_iter`'s behavior
    /// — `Finder::new(b"")` would loop without bound).
    #[must_use]
    pub fn find_in_chunks<'a, I>(&self, chunks: I) -> Vec<usize>
    where
        I: IntoIterator<Item = &'a [u8]>,
    {
        let mut out = Vec::new();
        if self.pattern.is_empty() {
            return out;
        }
        let plen = self.pattern.len();
        // Carry buffer keeps the trailing `plen - 1` bytes of the
        // already-scanned region plus the new chunk, so straddling
        // matches see contiguous bytes. `carry_global_start` is the
        // global byte offset that corresponds to `carry[0]`.
        let overlap_len = plen - 1;
        let mut carry: Vec<u8> = Vec::new();
        let mut carry_global_start: usize = 0;
        let mut consumed_global: usize = 0;
        let mut next_allowed_global_start: usize = 0;
        for chunk in chunks {
            let chunk_global_start = consumed_global;
            consumed_global += chunk.len();
            if carry.is_empty() {
                // No carry — scan the chunk directly, then snapshot
                // its tail into the carry for the next iteration.
                for start in self.find_iter(chunk) {
                    let global_start = chunk_global_start + start;
                    if global_start >= next_allowed_global_start {
                        out.push(global_start);
                        next_allowed_global_start = global_start + plen;
                    }
                }
                if chunk.len() >= overlap_len {
                    let tail_offset = chunk.len() - overlap_len;
                    carry.extend_from_slice(&chunk[tail_offset..]);
                    carry_global_start = chunk_global_start + tail_offset;
                } else {
                    carry.extend_from_slice(chunk);
                    carry_global_start = chunk_global_start;
                }
                continue;
            }
            // Stitch the new chunk onto the carry and scan the seam.
            carry.extend_from_slice(chunk);
            for start in self.find_iter(&carry) {
                let global_start = carry_global_start + start;
                if global_start >= next_allowed_global_start {
                    out.push(global_start);
                    next_allowed_global_start = global_start + plen;
                }
            }
            // Reset carry to the tail of (carry ++ chunk).
            let combined_len = carry.len();
            let new_carry_start_in_combined = combined_len.saturating_sub(overlap_len);
            let combined_global_start = carry_global_start;
            let tail = carry.split_off(new_carry_start_in_combined);
            carry = tail;
            carry_global_start = combined_global_start + new_carry_start_in_combined;
        }
        out
    }

    /// Confirm a candidate position in `haystack` matches the pattern.
    /// On the case-sensitive path this is a single `memmem` slice
    /// compare; on the case-insensitive path it walks bytes with
    /// `eq_ignore_ascii_case`.
    fn matches_at(&self, haystack: &[u8], start: usize) -> bool {
        let end = start.saturating_add(self.pattern.len());
        if end > haystack.len() {
            return false;
        }
        if self.case_insensitive_ascii {
            haystack[start..end]
                .iter()
                .zip(self.pattern.iter())
                .all(|(h, p)| h.eq_ignore_ascii_case(p))
        } else {
            &haystack[start..end] == self.pattern.as_ref()
        }
    }
}

/// Iterator yielded by [`LiteralMatcher::find_iter`].
#[derive(Debug)]
pub struct LiteralMatchIter<'h> {
    matcher: &'h LiteralMatcher,
    haystack: &'h [u8],
    pos: usize,
}

impl Iterator for LiteralMatchIter<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let plen = self.matcher.pattern.len();
        if plen == 0 {
            // Mirror regex `find_iter` which produces no matches for
            // an empty regex via the search-crate entry point.
            return None;
        }
        if self.matcher.case_insensitive_ascii {
            let first_lo = self.matcher.pattern[0];
            let first_up = first_lo.to_ascii_uppercase();
            while self.pos + plen <= self.haystack.len() {
                let rel = if first_lo == first_up {
                    memchr::memchr(first_lo, &self.haystack[self.pos..])
                } else {
                    memchr::memchr2(first_lo, first_up, &self.haystack[self.pos..])
                };
                let Some(rel) = rel else {
                    self.pos = self.haystack.len();
                    return None;
                };
                let start = self.pos + rel;
                if start + plen > self.haystack.len() {
                    self.pos = self.haystack.len();
                    return None;
                }
                if self.matcher.matches_at(self.haystack, start) {
                    self.pos = start + plen;
                    return Some(start);
                }
                self.pos = start + 1;
            }
            self.pos = self.haystack.len();
            None
        } else {
            let rel = self.matcher.finder.find(&self.haystack[self.pos..])?;
            let start = self.pos + rel;
            self.pos = start + plen;
            Some(start)
        }
    }
}

/// True when `[start, end)` in `haystack` is bordered by an ASCII
/// word-boundary on both sides — i.e. the byte before `start` and the
/// byte at `end` are either out-of-range or not an ASCII word
/// character (`[0-9A-Za-z_]`).
///
/// Mirrors `grep_regex`'s `\b…\b` semantics for ASCII input; non-ASCII
/// neighbors are treated as word characters (conservative — matches
/// what the regex engine does on the same input).
#[must_use]
pub fn is_ascii_word_boundary(haystack: &[u8], start: usize, end: usize) -> bool {
    fn is_word(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'_' || !byte.is_ascii()
    }
    let before_ok = start == 0 || !is_word(haystack[start - 1]);
    let after_ok = end >= haystack.len() || !is_word(haystack[end]);
    before_ok && after_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_sensitive_finds_all_non_overlapping_positions() {
        let m = LiteralMatcher::new(b"foo");
        let starts: Vec<usize> = m.find_iter(b"foo bar foo fooo").collect();
        assert_eq!(starts, vec![0, 8, 12]);
    }

    #[test]
    fn case_sensitive_misses_wrong_case() {
        let m = LiteralMatcher::new(b"Foo");
        let starts: Vec<usize> = m.find_iter(b"foo Foo foo").collect();
        assert_eq!(starts, vec![4]);
    }

    #[test]
    fn ascii_case_insensitive_finds_mixed_case() {
        let m = LiteralMatcher::new_ascii_case_insensitive(b"foo");
        let starts: Vec<usize> = m.find_iter(b"FOO foo Foo bAr fOo").collect();
        assert_eq!(starts, vec![0, 4, 8, 16]);
    }

    #[test]
    fn case_insensitive_single_byte_pattern_uses_memchr() {
        // Exercises the `first_lo == first_up` branch (non-letter
        // first byte) to ensure we don't try to upper-case a digit.
        let m = LiteralMatcher::new_ascii_case_insensitive(b"7");
        let starts: Vec<usize> = m.find_iter(b"a7b7c").collect();
        assert_eq!(starts, vec![1, 3]);
    }

    #[test]
    fn empty_pattern_yields_no_matches() {
        let m = LiteralMatcher::new(b"");
        let starts: Vec<usize> = m.find_iter(b"anything").collect();
        assert!(starts.is_empty());
        assert!(m.find_in_chunks([b"anything".as_slice()]).is_empty());
    }

    #[test]
    fn find_in_chunks_matches_contiguous_scan_when_no_split() {
        let m = LiteralMatcher::new(b"foo");
        let chunks = [b"foo bar foo".as_slice()];
        assert_eq!(m.find_in_chunks(chunks), vec![0, 8]);
    }

    #[test]
    fn find_in_chunks_finds_match_straddling_boundary() {
        // Pattern "needle" straddles the seam between "nee" and
        // "dle in haystack".
        let m = LiteralMatcher::new(b"needle");
        let chunks = [b"abc nee".as_slice(), b"dle in haystack".as_slice()];
        let got = m.find_in_chunks(chunks);
        assert_eq!(got, vec![4]); // "needle" starts at byte 4 globally
    }

    #[test]
    fn find_in_chunks_deduplicates_across_carry_and_next_scan() {
        // A match that starts at the very first byte of the second
        // chunk must be reported exactly once. The seam scan should
        // skip it (start >= pre_len), the next-chunk scan picks it up.
        let m = LiteralMatcher::new(b"abc");
        let chunks = [b"xxxx".as_slice(), b"abc yyy".as_slice()];
        assert_eq!(m.find_in_chunks(chunks), vec![4]);
    }

    #[test]
    fn find_in_chunks_handles_many_small_chunks() {
        // Pattern length 5 against text "abcdeabcde" split into
        // 1-byte chunks. Should still find both matches at 0 and 5.
        let m = LiteralMatcher::new(b"abcde");
        let text = b"abcdeabcde";
        let chunks: Vec<&[u8]> = text.iter().map(std::slice::from_ref).collect();
        assert_eq!(m.find_in_chunks(chunks), vec![0, 5]);
    }

    #[test]
    fn find_in_chunks_match_entirely_inside_one_chunk_after_carry() {
        let m = LiteralMatcher::new(b"foo");
        // Chunk 1 has no match; chunk 2 has the only one. Pattern
        // does not straddle.
        let chunks = [b"hello".as_slice(), b"world foo bar".as_slice()];
        assert_eq!(m.find_in_chunks(chunks), vec![11]);
    }

    #[test]
    fn case_insensitive_chunked_finds_straddling_match() {
        let m = LiteralMatcher::new_ascii_case_insensitive(b"Hello");
        let chunks = [b"abc HEL".as_slice(), b"LO world".as_slice()];
        assert_eq!(m.find_in_chunks(chunks), vec![4]);
    }

    #[test]
    fn word_boundary_at_string_edges() {
        let h = b"foo bar";
        assert!(is_ascii_word_boundary(h, 0, 3)); // "foo" at start
        assert!(is_ascii_word_boundary(h, 4, 7)); // "bar" at end
    }

    #[test]
    fn word_boundary_rejects_partial_word_match() {
        let h = b"foobar";
        assert!(!is_ascii_word_boundary(h, 0, 3)); // "foo" inside "foobar"
        assert!(!is_ascii_word_boundary(h, 3, 6)); // "bar" inside "foobar"
    }

    #[test]
    fn word_boundary_treats_underscore_as_word_char() {
        let h = b"foo_bar";
        assert!(!is_ascii_word_boundary(h, 0, 3));
        assert!(!is_ascii_word_boundary(h, 4, 7));
    }

    #[test]
    fn word_boundary_punctuation_neighbors_are_boundaries() {
        let h = b"(foo).bar!";
        assert!(is_ascii_word_boundary(h, 1, 4)); // "foo" between ( and )
        assert!(is_ascii_word_boundary(h, 6, 9)); // "bar" between . and !
    }
}
