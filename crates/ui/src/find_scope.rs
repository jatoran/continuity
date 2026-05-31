//! Selection-scope filtering for find-bar matches.

use continuity_search::MatchRange;
use continuity_text::Selection;
use ropey::Rope;

/// Convert non-empty selections to byte ranges over `rope`.
pub(crate) fn selected_byte_ranges(rope: &Rope, selections: &[Selection]) -> Vec<(usize, usize)> {
    selections
        .iter()
        .filter(|selection| !selection.is_collapsed())
        .filter_map(|selection| {
            let range = selection.ordered_range();
            let start = range.start.to_byte_offset(rope).ok()?;
            let end = range.end.to_byte_offset(rope).ok()?;
            (start < end).then_some((start, end))
        })
        .collect()
}

/// Keep matches fully contained by at least one selected byte range.
pub(crate) fn retain_matches_in_ranges(matches: &mut Vec<MatchRange>, ranges: &[(usize, usize)]) {
    matches.retain(|m| {
        ranges
            .iter()
            .any(|(start, end)| m.start_byte >= *start && m.end_byte <= *end)
    });
}
