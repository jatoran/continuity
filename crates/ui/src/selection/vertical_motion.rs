//! Up/Down caret motion with sticky intended-column memory and
//! soft-wrap-aware [`FrameDisplay`](continuity_render::FrameDisplay)
//! reuse.
//!
//! **Caret screen-y anchoring** lives here (see CLAUDE.md
//! principles): the math sequence — fingerprint check → seed of
//! `intended_columns` and `intended_display_columns` →
//! `head_display_byte_in_row` → `move_visual_row` /
//! `move_line_with_column` — must not be reordered, because that
//! would change which rounding step "wins" when a wrap row's
//! display-byte column lands between two source-byte boundaries.

use continuity_text::Selection;
use ropey::Rope;

use crate::paint_trace::{is_trace_enabled, log_event, EventScope};
use crate::selection_motion_cache::motion_cache_realized_covers_caret;
use crate::selection_vertical::{head_display_byte_in_row, move_line_with_column, move_visual_row};
use crate::Window;

impl Window {
    /// Vertical-motion selection update with sticky intended-column memory
    /// (Phase B2).
    ///
    /// On each call, if the current selection heads still match the
    /// fingerprint stored in `intended_columns_for` the previous intended
    /// columns are reused — so moving Down through a short line then onto
    /// a wider line restores the original column. Any horizontal move,
    /// edit, mouse click, etc. perturbs the head fingerprint, which
    /// causes the intended-column memory to be reseeded from the current
    /// heads on the next vertical step.
    pub(crate) fn move_line_selection(&mut self, delta: i32, extend: bool) -> bool {
        let Some(snapshot) = self.current_snapshot() else {
            return false;
        };
        let rope = snapshot.rope_snapshot().rope();
        let selections = snapshot.selections().to_vec();

        let fingerprint_ok = self.intended_columns_for.len() == selections.len()
            && self
                .intended_columns_for
                .iter()
                .zip(selections.iter())
                .all(|(p, s)| *p == s.head);

        // Soft-wrap branch: build a per-call FrameDisplay so vertical
        // motion steps by display rows rather than source lines.
        // Without this, Shift+Up on a wrapped paragraph jumps past
        // every continuation row at once.
        let frame_display = self.maybe_build_motion_frame_display(rope, &selections);

        if !fingerprint_ok {
            self.intended_columns = selections.iter().map(|s| s.head.byte_in_line).collect();
            self.intended_display_columns = selections
                .iter()
                .map(|s| {
                    frame_display
                        .as_ref()
                        .and_then(|fd| head_display_byte_in_row(rope, fd, s.head))
                        .unwrap_or(s.head.byte_in_line)
                })
                .collect();
        }

        let mut new_selections = Vec::with_capacity(selections.len());
        for (i, selection) in selections.iter().enumerate() {
            let intended = self
                .intended_columns
                .get(i)
                .copied()
                .unwrap_or(selection.head.byte_in_line);
            let new_head = if let Some(fd) = frame_display.as_ref() {
                // Sticky display-byte column survives multi-step motion
                // through narrow rows — fingerprint mismatch is what
                // invalidates it (handled above), not the row width.
                let intended_db = self
                    .intended_display_columns
                    .get(i)
                    .copied()
                    .or_else(|| head_display_byte_in_row(rope, fd, selection.head))
                    .unwrap_or(intended);
                move_visual_row(rope, fd, selection.head, delta, intended_db)
            } else {
                move_line_with_column(rope, selection.head, delta, intended)
            };
            let new_sel = if extend {
                Selection::new(selection.anchor, new_head, selection.kind)
            } else {
                Selection::caret_at(new_head)
            };
            new_selections.push(new_sel);
        }

        self.intended_columns_for = new_selections.iter().map(|s| s.head).collect();
        let changed = new_selections.as_slice() != selections.as_slice();
        let count = new_selections.len();
        let _scope = is_trace_enabled().then(|| {
            EventScope::with_detail(
                "selection_set",
                format!(
                    "entry=move_line_selection delta={delta} extend={extend} changed={changed} selections={count}"
                ),
            )
        });
        let result = self.editor.set_selections(self.buffer_id, new_selections);
        if is_trace_enabled() {
            log_event(
                "selection_update",
                &format!(
                    "entry=move_line_selection delta={delta} extend={extend} changed={changed} selections={count} ok={}",
                    result.is_ok()
                ),
            );
        }
        result.is_ok()
    }

    /// Resolve a [`FrameDisplay`] for soft-wrap-aware vertical motion.
    ///
    /// Returns `None` when wrap is off (the source-line path handles
    /// that case without needing a projection). Otherwise, prefers
    /// the most recent painted projection on
    /// [`crate::Window::last_painted_frame_display`] when its query
    /// is `is_compatible_for_motion` with the current rope /
    /// decoration / wrap / font / fold context — Up/Down on a
    /// 6000-line buffer used to pay an O(document) `FrameDisplay::build`
    /// per keystroke before this hit path.
    ///
    /// On a cache miss the method returns `None`, which downgrades
    /// `move_line_selection` to source-line stepping for that one
    /// step (approximate under soft-wrap, but the next paint reseeds
    /// the cache so subsequent steps are exact). This trades a small
    /// transient correctness drift for input responsiveness against
    /// very large buffers, per the first-pass large-buffer plan.
    fn maybe_build_motion_frame_display(
        &self,
        rope: &Rope,
        selections: &[Selection],
    ) -> Option<continuity_render::FrameDisplay> {
        if !self.view.soft_wrap {
            return None;
        }
        let metrics =
            self.display_projection_metrics(self.current_search_minimap_active(), rope.len_lines());
        if metrics.wrap_width_dip == 0 {
            return None;
        }
        let snap = self.current_snapshot()?;
        let revision = snap.rope_snapshot().revision().get();
        let decoration_id = self.buffer_id.as_uuid().as_u128();
        let decorations = self.decoration_cache.get(decoration_id);
        let caret_bytes: Vec<usize> = selections
            .iter()
            .map(|s| {
                let line = s.head.line as usize;
                let line_start = if line < rope.len_lines() {
                    rope.line_to_byte(line)
                } else {
                    rope.len_bytes()
                };
                line_start + s.head.byte_in_line as usize
            })
            .collect();
        let heading_lines =
            self.cached_heading_lines_for_projection(self.buffer_id, rope, revision, decorations);
        let folds = self.display_projection_folds(rope, &heading_lines, &caret_bytes);
        let query = crate::display_prewarm_cache::PrewarmQuery::new(
            self.buffer_id,
            revision,
            decorations.map(|d| d.revision),
            &caret_bytes,
            &folds,
            metrics.wrap_width_dip,
            self.font_state,
        );
        if let Some((cached_query, cached_fd)) = self.last_painted_frame_display.as_ref() {
            if cached_query.is_compatible_for_motion(&query)
                && motion_cache_realized_covers_caret(cached_fd, selections)
            {
                return Some(cached_fd.clone());
            }
        }
        // Cache miss: skip the O(document) rebuild and fall back to
        // source-line stepping for this one keystroke. The next paint
        // reseeds [`Window::last_painted_frame_display`] so any
        // remaining motion in the burst hits the cache.
        None
    }
}
