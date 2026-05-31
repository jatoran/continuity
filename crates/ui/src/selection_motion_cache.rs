//! Cache-coverage check for soft-wrap-aware vertical motion.
//!
//! Extracted from `selection.rs` to keep that file under the 600-line
//! conventions cap. The motion path reuses the last painted
//! `FrameDisplay` to step Up/Down through soft-wrap continuations
//! without rebuilding the whole-document projection. ε.2 narrowed
//! that projection to the painted viewport, so a viewport-built cache
//! entry can only serve motion if the caret's primary source line
//! falls inside the cached realized window. Outside that window, the
//! motion path falls back to source-line stepping.

use continuity_display_map::SourceLine;
use continuity_render::FrameDisplay;
use continuity_text::Selection;

/// `true` when the cached frame's realized window includes the
/// primary caret's source line. The motion path takes a single
/// Up/Down step at a time, so the 20-row overscan applied by
/// `build_viewport` keeps adjacent rows available too — a covered
/// caret implies covered step targets.
pub(crate) fn motion_cache_realized_covers_caret(
    cached: &FrameDisplay,
    selections: &[Selection],
) -> bool {
    let Some(primary) = selections.first() else {
        return false;
    };
    let source_line = primary.head.line;
    let caret_row = cached
        .row_index()
        .first_display_row_of_source_line(SourceLine(source_line))
        .raw();
    let realized = cached.realized_row_range();
    realized.start <= caret_row && caret_row < realized.end
}
