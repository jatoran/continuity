//! Per-tick prewarm stage processor — the warm-cache build step.
//!
//! [`Window::process_one_display_prewarm_stage`] pops one queued
//! prewarm work item, builds the corresponding `FrameDisplay`, and
//! inserts it into the prewarm cache. Called by
//! [`Window::on_display_prewarm_tick`] in [`super::tick`].
//!
//! Runs on the [`crate::Window`]-owning UI thread; never invoked from
//! a worker or the core thread.

use crate::display_prewarm_cache::{PrewarmQuery, PrewarmStage};
use crate::window::Window;

impl Window {
    pub(super) fn process_one_display_prewarm_stage(&mut self) {
        let Some(work) = self.display_map_prewarm.pop_work() else {
            return;
        };
        if !self.is_focused_mru_target(work.buffer_id) {
            return;
        }
        let Some(snapshot) = self.editor.snapshot(work.buffer_id) else {
            return;
        };
        let rope = snapshot.rope_snapshot().rope();
        let rope_revision = snapshot.rope_snapshot().revision().get();
        let document = work.buffer_id.as_uuid().as_u128();
        self.display_map_prewarm
            .invalidate_rope_revision(document, rope_revision);
        let caret_bytes = Self::caret_bytes_for_projection(rope, snapshot.selections());
        let search_minimap_active = self
            .overlays
            .find_bar()
            .is_some_and(|find_bar| !find_bar.matches.is_empty());
        let metrics = self.display_projection_metrics(search_minimap_active, rope.len_lines());
        let (wrap_width_dip, decorations) = match work.stage {
            PrewarmStage::Caret => (0, None),
            PrewarmStage::Viewport => (metrics.wrap_width_dip, None),
            PrewarmStage::Decoration => {
                let current = self
                    .decoration_cache
                    .get(document)
                    .filter(|decorations| decorations.revision == rope_revision);
                if current.is_none() {
                    self.submit_decoration_for_buffer(work.buffer_id);
                    self.display_map_prewarm
                        .push_work(work.buffer_id, PrewarmStage::Decoration);
                    return;
                }
                (metrics.wrap_width_dip, current)
            }
        };
        let heading_lines = Self::heading_lines_for_projection(rope, decorations);
        let folds = self.display_projection_folds(rope, &heading_lines, &caret_bytes);
        let frame_display = self.build_frame_display_with_options(
            rope,
            rope_revision,
            decorations,
            &caret_bytes,
            &folds,
            &[],
            wrap_width_dip,
            metrics.char_width_dip,
        );
        let query = PrewarmQuery::new(
            work.buffer_id,
            rope_revision,
            decorations.map(|decorations| decorations.revision),
            &caret_bytes,
            &folds,
            wrap_width_dip,
            self.font_state,
        );
        self.display_map_prewarm
            .insert(query, work.stage, frame_display);
        self.display_map_prewarm
            .push_next_stage(work.buffer_id, work.stage);
    }
}
