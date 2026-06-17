//! Find-bar replace operations.

use continuity_command::Error;
use continuity_text::{EditOp, Position, Range};

use crate::Window;

/// User-facing completion banner for replace-all.
#[must_use]
pub(crate) fn replace_all_banner_text(match_count: usize) -> String {
    match match_count {
        0 => "No matches to replace".to_string(),
        1 => "Replaced 1 match (Ctrl+Z to undo)".to_string(),
        n => format!("Replaced {n} matches (Ctrl+Z to undo)"),
    }
}

impl Window {
    /// Replace the current find-bar match with the replace text.
    pub(crate) fn find_replace_one_impl(&mut self) -> Result<(), Error> {
        self.ensure_find_matches_current_for_focused_pane();
        let (replacement, preserve_case, interpret_escapes, range) = match self.overlays.find_bar()
        {
            Some(fb) if fb.replace_visible => match fb.current_match() {
                Some(m) => (
                    fb.replace().to_owned(),
                    fb.preserve_case,
                    fb.regex,
                    (m.start_byte, m.end_byte),
                ),
                None => return Ok(()),
            },
            _ => return Ok(()),
        };
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return Ok(());
        };
        let rope = snap.rope_snapshot().rope().clone();
        let start = Position::from_byte_offset(&rope, range.0)
            .map_err(|_| Error::UnsupportedContext("find_replace_one"))?;
        let end = Position::from_byte_offset(&rope, range.1)
            .map_err(|_| Error::UnsupportedContext("find_replace_one"))?;
        let replacement = crate::find_replace_plan::replacement_for_one(
            &rope,
            range,
            &replacement,
            preserve_case,
            interpret_escapes,
        );
        let _ = self.editor.apply_edit(
            self.buffer_id,
            EditOp::replace(Range::new(start, end), replacement),
        );
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        self.recompute_find_matches();
        Ok(())
    }

    /// Replace every current find-bar match as one undo group.
    pub(crate) fn find_replace_all_impl(&mut self) -> Result<(), Error> {
        self.ensure_find_matches_current_for_focused_pane();
        let (replacement, preserve_case, interpret_escapes, ranges) = match self.overlays.find_bar()
        {
            Some(fb) if fb.replace_visible => (
                fb.replace().to_owned(),
                fb.preserve_case,
                fb.regex,
                fb.matches
                    .iter()
                    .map(|m| (m.start_byte, m.end_byte))
                    .collect::<Vec<_>>(),
            ),
            _ => return Ok(()),
        };
        let match_count = ranges.len();
        if ranges.is_empty() {
            let now = self.now_ms();
            self.file_banner = Some(crate::window_file::FileBanner::transient(
                replace_all_banner_text(0),
                now,
            ));
            return Ok(());
        }
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return Ok(());
        };
        let rope = snap.rope_snapshot().rope().clone();
        let (ops, selections_after) = crate::find_replace_plan::build_replace_all_plan(
            &rope,
            &ranges,
            &replacement,
            preserve_case,
            interpret_escapes,
        );
        if ops.is_empty() {
            return Ok(());
        }
        let _ = self.editor.apply_edit_group(
            self.buffer_id,
            ops,
            selections_after,
            "editor.find_replace_all",
        );
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        self.recompute_find_matches();
        let now = self.now_ms();
        self.file_banner = Some(crate::window_file::FileBanner::transient(
            replace_all_banner_text(match_count),
            now,
        ));
        self.start_motion_timer();
        Ok(())
    }
}
