//! Placeholder-only [`FrameDisplay`] constructor.
//!
//! The UI uses this when a spectator pane's real projection is already
//! pending on the worker but has not reached the UI-thread cache yet.
//! It gives the renderer a stamped whole-document row index with an
//! empty realized window, so the existing scroll-placeholder painter
//! can fill the visible body without materializing any display specs.

use std::sync::Arc;

use continuity_display_map::{DisplayMap, DisplayRowIndex, FoldSignature, IndexStamps};

use super::FrameDisplay;

impl FrameDisplay {
    /// Build a placeholder frame with one estimated row per source
    /// line and no realized display-line specs.
    ///
    /// This frame is paint-only: callers must not seed projection
    /// caches with it because it intentionally carries approximate row
    /// counts. The stamps let downstream trace / validation code name
    /// the document geometry that the pending worker result is expected
    /// to replace.
    #[must_use]
    pub fn placeholder_unrealized(
        source_line_count: usize,
        revision: u64,
        decoration_revision: Option<u64>,
        wrap_width_dip: u32,
        font_state: u64,
    ) -> Self {
        let source_lines = source_line_count.max(1);
        let stamps = IndexStamps {
            rope_revision: revision,
            decoration_revision: decoration_revision.unwrap_or(revision),
            wrap_width_dip,
            font_state,
            fold_signature: FoldSignature::EMPTY,
        };
        let row_index = Arc::new(DisplayRowIndex::from_row_counts(
            vec![1u16; source_lines],
            stamps,
        ));
        Self {
            map: Arc::new(DisplayMap::from_parts_viewport(
                revision,
                wrap_width_dip,
                row_index,
                Vec::new(),
                0,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_has_empty_realized_window() {
        let frame = FrameDisplay::placeholder_unrealized(3, 7, Some(9), 480, 11);

        assert_eq!(frame.display_line_count(), 3);
        assert_eq!(frame.realized_row_range(), 0..0);
        assert!(frame.display_line_by_index(0).is_none());
        let stamps = frame.row_index().stamps();
        assert_eq!(stamps.rope_revision, 7);
        assert_eq!(stamps.decoration_revision, 9);
        assert_eq!(stamps.wrap_width_dip, 480);
        assert_eq!(stamps.font_state, 11);
    }
}
