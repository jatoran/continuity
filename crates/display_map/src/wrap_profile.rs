//! Width-independent line-wrap profile interpretation.
//!
//! Consumes the per-break-candidate cumulative-width pair stored on a
//! populated [`WrapCacheEntry`] (see `wrap_cache.rs` for the cache
//! value contract) and derives the soft-wrap row count at an
//! arbitrary `wrap_width_dip` without re-measuring any glyphs.
//!
//! P18.12a (2026-05-22) introduces this helper alongside the slow
//! path's new profile emission; P18.12b will route paint-time row
//! count queries through it for wrap_widths other than the one the
//! cache entry was originally built at, so a window-drag tick can
//! reflow large spectator buffers without re-running the slow walker.
//!
//! ## Contract
//!
//! [`row_count_from_profile`] returns:
//!
//! - `Some(rows)` matching the slow path's row count for the queried
//!   `wrap_width_dip`, including the slow path's "overshoot" case
//!   (a row of width slightly greater than `wrap_width_dip` when the
//!   trigger fires at the trailing whitespace itself), and including
//!   the slow path's trailing-empty-row idiosyncrasy on lines that end
//!   with whitespace.
//! - `None` when the slow path would have placed a cut at a non-
//!   whitespace grapheme (mid-word cut). The cached profile does not
//!   carry per-grapheme widths, so the caller must fall back to the
//!   slow walker. This case fires when a single token between two
//!   break candidates is itself wider than `wrap_width_dip`.
//! - `None` when the entry has no profile populated (e.g. it was
//!   constructed via [`crate::wrap_cache::WrapCacheEntry::row_count_only`]).
//!   The caller should fall back to the slow walker.

use crate::wrap_cache::{WrapCache, WrapCacheEntry, WrapCacheKey};

/// Derive the soft-wrap row count from a populated [`WrapCacheEntry`]
/// at the given `wrap_width_dip`, without re-measuring any glyphs.
///
/// `continuation_budget_dip` is the reduced budget continuation rows
/// get (the wrap width minus the line's hanging indent — see
/// [`crate::wrap::continuation_wrap_budget_dip`]); the first row always
/// budgets the full `wrap_width_dip`. Pass `wrap_width_dip as f32` for
/// lines with no hanging indent.
///
/// See module docs for the contract on `Some` / `None`.
#[must_use]
pub fn row_count_from_profile(
    entry: &WrapCacheEntry,
    wrap_width_dip: u32,
    continuation_budget_dip: f32,
) -> Option<u16> {
    let breaks = &entry.break_points;
    let prefix = &entry.prefix_advances_bits;
    let pre_ws = &entry.pre_whitespace_advances_bits;
    if breaks.is_empty() {
        return None;
    }
    if breaks.len() != prefix.len() || breaks.len() != pre_ws.len() {
        return None;
    }
    let n = breaks.len();
    let first_row_budget = wrap_width_dip as f32;
    let total_width = f32::from_bits(prefix[n - 1]);
    if total_width <= first_row_budget {
        return Some(1);
    }

    let mut rows: u16 = 1;
    let mut line_start_advance = 0.0_f32;
    let mut candidates_in_current_row: u32 = 0;
    let mut i: usize = 0;

    while i < n {
        // The row currently being filled: the first row budgets the
        // full wrap width, every continuation row the hang-indent-
        // reduced width (mirrors the slow path's `row_budget` switch).
        let max_width = if rows == 1 {
            first_row_budget
        } else {
            continuation_budget_dip
        };
        let post_ws_advance = f32::from_bits(prefix[i]);
        let pre_ws_advance = f32::from_bits(pre_ws[i]);
        let row_through_pre_ws = pre_ws_advance - line_start_advance;
        let row_through_post_ws = post_ws_advance - line_start_advance;

        if row_through_pre_ws > max_width {
            // The trigger fires before the trailing whitespace of
            // candidate `i` — at some non-whitespace grapheme between
            // the previous break candidate and this one. The slow path
            // cuts at the previous break (if any in this row) or at a
            // mid-grapheme position (if none).
            if candidates_in_current_row == 0 {
                return None;
            }
            line_start_advance = f32::from_bits(prefix[i - 1]);
            rows = rows.checked_add(1)?;
            candidates_in_current_row = 0;
            continue;
        }

        if row_through_post_ws > max_width {
            // Trigger fires AT the trailing whitespace of candidate
            // `i`. Slow path cuts here, producing a row of width
            // exactly `row_through_post_ws` (overshoots `max_width` by
            // the trailing whitespace's width).
            //
            // Two sub-cases mirror the slow path's `byte_off >
            // line_start_byte` guard plus its end-of-line semantics:
            if pre_ws_advance == line_start_advance {
                // The trailing whitespace is the first grapheme of
                // this row. The slow path's guard suppresses the cut
                // (running gains only the whitespace's width and we
                // continue). For us: consume candidate `i` into the
                // current row and advance.
                candidates_in_current_row = candidates_in_current_row.saturating_add(1);
                i += 1;
                continue;
            }
            if i == n - 1 {
                // Last candidate. If it is the end-of-line sentinel
                // (`pre_ws == post_ws`) we cannot reach this branch
                // because `row_through_post_ws > max_width` would
                // already have implied `row_through_pre_ws > max_width`
                // above. So this is a *real* trailing whitespace at
                // the line's end. The slow path increments `breaks`
                // and returns `breaks + 1`, leaving a notionally-empty
                // trailing row in the count.
                rows = rows.checked_add(1)?;
                return Some(rows);
            }
            line_start_advance = post_ws_advance;
            rows = rows.checked_add(1)?;
            candidates_in_current_row = 0;
            i += 1;
            continue;
        }

        candidates_in_current_row = candidates_in_current_row.saturating_add(1);
        i += 1;
    }

    Some(rows)
}

/// P18.12b — compose [`WrapCache::get_any_width`] +
/// [`row_count_from_profile`] + [`WrapCache::insert`] into a single
/// pre-slow-walk lookup. Returns the soft-wrap row count when a
/// sibling-bucket entry carries a usable width-independent profile at
/// the queried `wrap_width_dip`; the donor entry is also re-inserted
/// under `key`'s exact-width bucket so subsequent paint cycles at the
/// same `wrap_width_dip` skip even the cross-bucket scan.
///
/// Returns `None` when there is no sibling-bucket donor for
/// `(content_stamp, font_state, locale)`, or when the donor's profile
/// is insufficient to determine the row count (mid-word cut required).
/// The caller must then fall through to the slow walker.
pub(crate) fn try_serve_via_profile(
    wrap_cache: &WrapCache,
    content_stamp: u64,
    font_state: u64,
    locale: &str,
    wrap_width_dip: u32,
    continuation_budget_dip: f32,
    key: WrapCacheKey,
) -> Option<u16> {
    let donor = wrap_cache.get_any_width(content_stamp, font_state, locale)?;
    let rows = row_count_from_profile(&donor, wrap_width_dip, continuation_budget_dip)?;
    wrap_cache.insert(
        key,
        WrapCacheEntry {
            row_count: rows,
            break_points: donor.break_points.clone(),
            prefix_advances_bits: donor.prefix_advances_bits.clone(),
            pre_whitespace_advances_bits: donor.pre_whitespace_advances_bits.clone(),
        },
    );
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Build a `WrapCacheEntry` from a single-style line `text` where
    /// each grapheme has unit width, mirroring the convention used by
    /// the proptest's slow-path emulation. `row_count` is whatever the
    /// slow path would have returned at the wrap_width the entry was
    /// originally built at — irrelevant for `row_count_from_profile`,
    /// which only reads the profile fields.
    fn build_profile_unit_widths(text: &str, row_count: u16) -> WrapCacheEntry {
        let mut breaks: Vec<u32> = Vec::new();
        let mut prefix: Vec<u32> = Vec::new();
        let mut pre_ws: Vec<u32> = Vec::new();
        let mut cum: f32 = 0.0;
        let len_bytes = text.len();
        for (byte_off, ch) in text.char_indices() {
            let w = 1.0_f32;
            if ch.is_whitespace() {
                pre_ws.push(cum.to_bits());
                prefix.push((cum + w).to_bits());
                breaks.push((byte_off + ch.len_utf8()) as u32);
            }
            cum += w;
        }
        if breaks.last() != Some(&(len_bytes as u32)) {
            breaks.push(len_bytes as u32);
            prefix.push(cum.to_bits());
            pre_ws.push(cum.to_bits());
        }
        WrapCacheEntry {
            row_count,
            break_points: Arc::from(breaks),
            prefix_advances_bits: Arc::from(prefix),
            pre_whitespace_advances_bits: Arc::from(pre_ws),
        }
    }

    #[test]
    fn empty_profile_returns_none() {
        let entry = WrapCacheEntry::row_count_only(1);
        assert_eq!(row_count_from_profile(&entry, 10, 10.0), None);
    }

    #[test]
    fn mismatched_array_lengths_returns_none() {
        let entry = WrapCacheEntry {
            row_count: 1,
            break_points: Arc::from(vec![1_u32, 2_u32]),
            prefix_advances_bits: Arc::from(vec![1.0_f32.to_bits()]),
            pre_whitespace_advances_bits: Arc::from(vec![1.0_f32.to_bits()]),
        };
        assert_eq!(row_count_from_profile(&entry, 10, 10.0), None);
    }

    #[test]
    fn line_that_fits_in_one_row() {
        let entry = build_profile_unit_widths("AB CDE", 1);
        assert_eq!(row_count_from_profile(&entry, 10, 10.0), Some(1));
        assert_eq!(row_count_from_profile(&entry, 6, 6.0), Some(1));
    }

    /// Regression case from `P18.12a_populate_wrap_profile_20260522-160308.md`.
    /// Without `pre_whitespace_advances_bits` this returned `Some(3)`.
    #[test]
    fn overshoot_one_a_bb_ccc_max_4() {
        let entry = build_profile_unit_widths("A BB CCC", 2);
        assert_eq!(row_count_from_profile(&entry, 4, 4.0), Some(2));
    }

    /// Regression case from the same report.
    #[test]
    fn overshoot_two_aaaa_bb_cc_ddd_max_5() {
        let entry = build_profile_unit_widths("AAAA BB CC DDD", 3);
        assert_eq!(row_count_from_profile(&entry, 5, 5.0), Some(3));
    }

    /// Regression case from the same report.
    #[test]
    fn overshoot_three_aaaa_bb_cc_ddd_max_7() {
        let entry = build_profile_unit_widths("AAAA BB CC DDD", 2);
        assert_eq!(row_count_from_profile(&entry, 7, 7.0), Some(2));
    }

    #[test]
    fn mid_word_cut_returns_none() {
        // "ABC DEF GHI" at max_width=2 needs to cut INSIDE "ABC"
        // because "ABC" alone is wider than wrap_width and the first
        // whitespace is at advance=4 > 2. The slow path uses a
        // mid-grapheme cut; profile-only cannot. Note max_width=3 is
        // NOT mid-word — it's the slow-path overshoot case (cut at
        // trailing whitespace produces a row of width 4 > 3), which
        // Option A correctly handles; see `overshoot_at_every_break`.
        let entry = build_profile_unit_widths("ABC DEF GHI", 6);
        assert_eq!(row_count_from_profile(&entry, 2, 2.0), None);
    }

    #[test]
    fn overshoot_at_every_break() {
        // "ABC DEF GHI" at max_width=3: slow path overshoots at each
        // of the two whitespaces (rows are 4 wide each), producing 3
        // rows total. Profile must match.
        let entry = build_profile_unit_widths("ABC DEF GHI", 3);
        assert_eq!(row_count_from_profile(&entry, 3, 3.0), Some(3));
    }

    #[test]
    fn non_overshoot_matches() {
        // "ABC DEF GHI" widths=1 each at max_width=4: slow path = 3.
        let entry = build_profile_unit_widths("ABC DEF GHI", 3);
        assert_eq!(row_count_from_profile(&entry, 4, 4.0), Some(3));
        assert_eq!(row_count_from_profile(&entry, 5, 5.0), Some(3));
        assert_eq!(row_count_from_profile(&entry, 7, 7.0), Some(2));
    }

    #[test]
    fn trailing_whitespace_at_line_end() {
        // "AB " ending with whitespace, max_width=2: slow path cuts
        // at the trailing whitespace, leaving a trailing empty row
        // → 2 rows.
        let entry = build_profile_unit_widths("AB ", 2);
        assert_eq!(row_count_from_profile(&entry, 2, 2.0), Some(2));
        // At max_width=3 the line fits in one row.
        assert_eq!(row_count_from_profile(&entry, 3, 3.0), Some(1));
    }
}
