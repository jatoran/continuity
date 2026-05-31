//! Build the status-bar segment list for one paint frame.
//!
//! Owns the segment-text formatting: each variant in
//! `continuity_config::StatusBarSegment` maps to one segment in the
//! render layer's [`continuity_render::StatusBarSegmentDraw`] vec. The
//! computation runs on the UI thread inside `on_paint`; the resulting
//! `Vec`s are stored on the stack across one renderer call.
//!
//! The cached previous-frame [`continuity_render::StatusBarLayout`]
//! lets `WM_LBUTTONDOWN` consult prior bounds to route clicks via
//! [`hit_test`]; click dispatch itself lives in
//! [`crate::window_status_bar::click`].
//!
//! The right-hand chip lane composes warning chips (from
//! [`crate::window_status_chips::detect_chips`]) with notice chips
//! ([`crate::window_status_notice::append_notice_chips`]) and the
//! persist-queue chip (see [`persist_queue`]).
//!
//! Thread ownership: UI thread of one window.

use continuity_buffer::FileAssociation;
use continuity_render::{StatusBarColors, StatusBarSegmentDraw};
use continuity_text::Selection;
use ropey::Rope;
use windows::Win32::System::SystemInformation::GetTickCount64;

use crate::window_theme::rgba_from_color;
use crate::Window;

mod click;
pub(crate) mod hit_test;
mod persist_queue;
mod rope_counts;
mod segments;

#[cfg(test)]
mod tests;

pub(crate) use self::rope_counts::RopeStatusCounts;

// Brought into scope so the `tests` child module's `use super::*;`
// glob picks them up. None are referenced directly by this file.
#[cfg(test)]
use self::segments::{
    count_words, format_count, format_lines, format_numeric_sum, format_position, format_selection,
};
#[cfg(test)]
use crate::window_view_options::StatusCountMode;
#[cfg(test)]
use continuity_render::{SegmentBounds, StatusBarSegmentKind};

/// One frame's worth of status-bar payload. The renderer borrows from
/// the `Vec`s inside this struct via [`continuity_render::StatusBarData`].
pub(crate) struct StatusBarBuild {
    /// Left-aligned segments, in paint order.
    pub segments: Vec<StatusBarSegmentDraw>,
    /// Right-aligned chips (warnings, notices, persist queue).
    pub chips: Vec<StatusBarSegmentDraw>,
    /// Theme-derived colors.
    pub colors: StatusBarColors,
}

impl Window {
    /// Compose the per-frame [`StatusBarBuild`] from `self` and the
    /// active snapshot. Cheap — every per-segment helper is O(rope
    /// chunks) or smaller and the segment list is short.
    pub(crate) fn build_status_bar(
        &self,
        rope: &Rope,
        rope_revision: u64,
        selections: &[Selection],
        file: Option<&FileAssociation>,
    ) -> StatusBarBuild {
        let mut segment_draws: Vec<StatusBarSegmentDraw> =
            Vec::with_capacity(self.view_options.status_bar_segments.len());
        let idle_ms = unsafe { GetTickCount64() }.saturating_sub(self.last_input_tick);
        let count_mode = self.view_options.status_count_mode;
        // Refresh the rope-counts cache when the revision advances.
        // The cache is keyed by [`BufferId`] so a focus switch into a
        // different buffer doesn't invalidate the prior buffer's
        // counts — switching back hits the cache instead of paying
        // another O(N) recompute. The borrow scope is kept narrow so
        // `build_segment` can re-borrow immutably below.
        let buffer_id = self.buffer_id;
        {
            let mut slot = self.status_bar_rope_counts.borrow_mut();
            let stale = slot
                .get(&buffer_id)
                .is_none_or(|c| c.rope_revision != rope_revision);
            if stale {
                slot.insert(buffer_id, RopeStatusCounts::compute(rope, rope_revision));
            }
        }
        let cache = self.status_bar_rope_counts.borrow();
        let counts = cache
            .get(&buffer_id)
            .expect("invariant: status-bar counts cache populated above");
        for kind in &self.view_options.status_bar_segments {
            if let Some(s) =
                segments::build_segment(*kind, rope, selections, file, count_mode, idle_ms, counts)
            {
                segment_draws.push(s);
            }
        }
        let theme = &self.active_theme.current;
        let colors = StatusBarColors {
            bg: rgba_from_color(theme.status_background()),
            fg: rgba_from_color(theme.status_foreground()),
            warn: rgba_from_color(theme.status_warn()),
            error: rgba_from_color(theme.status_error()),
        };
        let mut chips = crate::window_status_chips::detect_chips(rope);
        // Persist-queue chip sits before the notice-chip lane so a
        // transient save-confirm doesn't shove it around.
        if let Some(chip) = self.persist_queue_chip() {
            chips.push(chip);
        }
        crate::window_status_notice::append_notice_chips(
            &mut chips,
            &self.status_notices,
            self.now_ms(),
            self.motion_policy().is_reduced_motion(),
        );
        StatusBarBuild {
            segments: segment_draws,
            chips,
            colors,
        }
    }
}
