//! Fuzzy substring scoring for the command palette and quick-open.
//!
//! Implements a small sub-sequence scorer in the spirit of fzf's "v1" path:
//! every query character must appear in order in the candidate, and matches
//! are scored to favor (a) prefix hits, (b) word-boundary hits, (c)
//! consecutive runs, and (d) tighter overall span. Case-insensitive by
//! default; an exact-case substring hit gets a small bonus on top of the
//! usual fold-matched score.
//!
//! The scorer returns a [`FuzzyMatch`] containing the score and the byte
//! indices in the candidate that matched, so callers can underline / bold
//! those characters when rendering.

use std::cmp::Ordering;

/// A single ranked fuzzy match.
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch {
    /// The candidate's score. Higher is better.
    pub score: i32,
    /// The byte indices into the candidate that matched, in order.
    pub matched_indices: Vec<usize>,
}

impl Eq for FuzzyMatch {}

impl Ord for FuzzyMatch {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score.cmp(&other.score)
    }
}

impl PartialOrd for FuzzyMatch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Score `candidate` against `query`. Returns `None` when no match exists.
///
/// Scoring rules:
/// - +16 per matched character
/// - +24 if the matched character starts the candidate
/// - +12 if the matched character is at a word boundary (preceded by a
///   non-alphanumeric)
/// - +8 if the previous query character matched the immediately-preceding
///   candidate character (consecutive run)
/// - -2 per skipped non-match character (encourages tight matches)
/// - +4 bonus per character that matched in the original case
///
/// Empty `query` matches every candidate with `score = 0`.
#[must_use]
pub fn score(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    if query.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            matched_indices: Vec::new(),
        });
    }
    if candidate.is_empty() {
        return None;
    }

    let cand_chars: Vec<(usize, char)> = candidate.char_indices().collect();
    let mut total: i32 = 0;
    let mut matched_indices = Vec::new();
    let mut cursor = 0usize;
    let mut prev_match_idx: Option<usize> = None;

    for q_ch in query.chars() {
        let q_lower = q_ch.to_ascii_lowercase();
        let mut found_at: Option<usize> = None;
        for (i, (_, c_ch)) in cand_chars.iter().enumerate().skip(cursor) {
            if c_ch.to_ascii_lowercase() == q_lower {
                found_at = Some(i);
                break;
            }
        }
        let i = found_at?;
        let (byte_idx, c_ch) = cand_chars[i];
        let mut delta: i32 = 16;
        let skipped = i.saturating_sub(cursor) as i32;
        delta -= 2 * skipped;
        if i == 0 {
            delta += 24;
        } else {
            let prev_ch = cand_chars[i - 1].1;
            if !prev_ch.is_alphanumeric() {
                delta += 12;
            }
        }
        if let Some(p) = prev_match_idx {
            if i == p + 1 {
                delta += 8;
            }
        }
        if c_ch == q_ch {
            delta += 4;
        }
        total = total.saturating_add(delta);
        matched_indices.push(byte_idx);
        cursor = i + 1;
        prev_match_idx = Some(i);
    }

    Some(FuzzyMatch {
        score: total,
        matched_indices,
    })
}

/// Convenience: rank `candidates` by `query`, returning matched entries
/// (non-matchers are dropped). Stable sort, highest score first.
#[must_use]
pub fn rank<'a, I, S>(query: &str, candidates: I) -> Vec<(usize, FuzzyMatch)>
where
    I: IntoIterator<Item = (usize, &'a S)>,
    S: AsRef<str> + 'a,
{
    let mut scored: Vec<(usize, FuzzyMatch)> = candidates
        .into_iter()
        .filter_map(|(idx, c)| score(query, c.as_ref()).map(|m| (idx, m)))
        .collect();
    scored.sort_by(|a, b| b.1.score.cmp(&a.1.score));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_with_zero_score() {
        let m = score("", "anything").unwrap();
        assert_eq!(m.score, 0);
        assert!(m.matched_indices.is_empty());
    }

    #[test]
    fn empty_candidate_does_not_match_nonempty_query() {
        assert!(score("a", "").is_none());
    }

    #[test]
    fn missing_char_returns_none() {
        assert!(score("xyz", "abc").is_none());
    }

    #[test]
    fn prefix_beats_midword() {
        let prefix = score("ed", "editor").unwrap();
        let midword = score("ed", "buffered").unwrap();
        assert!(prefix.score > midword.score);
    }

    #[test]
    fn consecutive_run_beats_scattered() {
        let consec = score("abc", "abcxyz").unwrap();
        let scattered = score("abc", "axbxcx").unwrap();
        assert!(consec.score > scattered.score);
    }

    #[test]
    fn word_boundary_match_scores_higher_than_inner() {
        let boundary = score("p", "command palette").unwrap();
        let inner = score("p", "shapes").unwrap();
        assert!(boundary.score > inner.score);
    }

    #[test]
    fn case_match_bonus() {
        let exact = score("E", "Editor").unwrap();
        let folded = score("e", "Editor").unwrap();
        assert!(exact.score > folded.score);
    }

    #[test]
    fn matched_indices_track_byte_positions() {
        let m = score("ab", "xaxb").unwrap();
        assert_eq!(m.matched_indices, vec![1, 3]);
    }

    #[test]
    fn rank_sorts_by_score_desc() {
        let candidates = ["editor.move_char_forward", "view.toggle_minimap", "abc"];
        let ranked = rank("edit", candidates.iter().enumerate());
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].0, 0);
    }

    #[test]
    fn rank_respects_score_order() {
        let candidates = ["editor.find", "edit_distance", "predicate"];
        let ranked = rank("edit", candidates.iter().enumerate());
        assert_eq!(ranked.len(), 3);
        // "editor.find" and "edit_distance" both prefix; "predicate" is mid-word.
        assert_ne!(ranked[2].0, 0);
    }
}
