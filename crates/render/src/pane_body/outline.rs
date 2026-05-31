//! Spectator-pane outline-sidebar entry projection.

use crate::outline::OutlineEntry;
use crate::params::PaneBodyDraw;

pub(super) fn compute_spectator_outline_entries(body: &PaneBodyDraw<'_>) -> Vec<OutlineEntry> {
    let Some(decorations) = body.decorations else {
        return Vec::new();
    };
    let headings = continuity_decorate::headings(&decorations.blocks, body.rope);
    let progress =
        continuity_decorate::task_progress_per_heading(&headings, &decorations.inlines, body.rope);
    headings
        .iter()
        .enumerate()
        .map(|(idx, heading)| {
            let suffix = progress
                .get(idx)
                .and_then(|progress| progress.format_suffix())
                .map(|suffix| format!(" {suffix}"))
                .unwrap_or_default();
            OutlineEntry {
                text: format!("{}{}", heading.text, suffix),
                level: heading.level,
                target_byte: u32::try_from(heading.start_byte).unwrap_or(u32::MAX),
            }
        })
        .collect()
}
