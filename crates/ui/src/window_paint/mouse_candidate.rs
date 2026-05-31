//! Paint-time promotion for the mouse hit-test frame cache.
//!
//! A first click into a large pane can force hit-testing to build a
//! current `FrameDisplay` before paint. The following `WM_PAINT` should
//! treat that frame as a rebuild source instead of cold-building the
//! same row index again.

use crate::display_prewarm_cache::PrewarmQuery;
use crate::window::Window;
use crate::window_mouse_hit_test_cache::MouseHitTestFrameCacheEntry;

impl Window {
    pub(crate) fn mouse_hit_test_paint_candidate(
        &mut self,
        display_query: &PrewarmQuery,
        revision_for_projection: u64,
        source_line_count: u32,
    ) -> Option<MouseHitTestFrameCacheEntry> {
        let entry = self.mouse_hit_test_frame_cache.borrow().clone()?;
        let Some(mismatch) = entry.query().hit_test_compat_mismatch(display_query) else {
            let stamps = entry.frame_display().row_index().stamps();
            if stamps.rope_revision > revision_for_projection {
                log_mouse_candidate("miss=future_rope_revision");
                return None;
            }
            if stamps.rope_revision == revision_for_projection
                && entry.frame_display().row_index().source_line_count() != source_line_count
            {
                log_mouse_candidate("miss=source_line_count");
                return None;
            }
            if entry
                .decorations()
                .map(|decorations| decorations.revision == stamps.decoration_revision)
                .unwrap_or(true)
            {
                self.last_painted_decorations = entry.decorations().cloned();
                self.last_painted_decoration_parse_revision = entry.parse_revision();
                log_mouse_candidate("hit=rebuild_candidate shadowed=true");
            } else {
                log_mouse_candidate("hit=rebuild_candidate shadowed=false");
            }
            return Some(entry);
        };
        log_mouse_candidate(&format!("miss=field_{mismatch}"));
        None
    }
}

fn log_mouse_candidate(detail: &str) {
    if crate::paint_trace::is_trace_enabled() {
        crate::paint_trace::log_event("mouse_hit_test_frame_display", detail);
    }
}
