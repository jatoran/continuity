//! Snapshot selection for the paint path.

use continuity_core::EditorSnapshot;

use crate::window::Window;
use crate::Error;

impl Window {
    pub(crate) fn snapshot_for_paint(&mut self) -> Result<Option<EditorSnapshot>, Error> {
        if let Some(preview) = self.time_machine_preview.as_ref() {
            return Ok(Some(preview.snapshot.clone()));
        }
        let Some(snapshot) = self.editor.snapshot(self.buffer_id) else {
            self.trace_missing_snapshot("paint");
            if let Some(renderer) = &self.renderer {
                renderer.present_clear(self.active_theme.editor_colors().bg)?;
            }
            self.inited = true;
            return Ok(None);
        };
        Ok(Some(snapshot))
    }
}
