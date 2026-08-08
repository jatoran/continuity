//! Snapshot selection for the paint path.

use continuity_core::EditorSnapshot;

use crate::window::Window;
use crate::Error;

impl Window {
    pub(crate) fn snapshot_for_paint(&mut self) -> Result<Option<EditorSnapshot>, Error> {
        let snapshot = if let Some(preview) = self.time_machine_preview.as_ref() {
            preview.snapshot.clone()
        } else {
            let Some(snapshot) = self.editor.snapshot(self.buffer_id) else {
                self.trace_missing_snapshot("paint");
                if let Some(renderer) = &self.surface.render.renderer {
                    renderer.present_clear(self.active_theme.editor_colors().bg)?;
                }
                self.inited = true;
                return Ok(None);
            };
            snapshot
        };
        self.publish_accessibility_snapshot(&snapshot);
        Ok(Some(snapshot))
    }
}
