//! Phase-I1 time-machine state operations on `Window`.
//!
//! Sibling of [`crate::window_time_machine`] (which carries the
//! per-pane state) and [`crate::window_time_machine_hud`] (which
//! carries the geometry / hit-test math). This file owns the
//! Window-side mutators that drive the slider's lifecycle:
//!
//! - **Preview cache** — [`TimeMachinePreview`] holds the materialized
//!   `EditorSnapshot` for the currently-pinned revision, so subsequent
//!   `on_paint` frames can substitute it without re-querying persist.
//! - **Drag state** — [`TimeMachineDrag`] tracks an in-flight mouse
//!   drag (kind that started it) so `WM_MOUSEMOVE` knows it's a slider
//!   drag rather than a normal text selection.
//! - **Lifecycle helpers** — `commit_time_machine_overlay` (Enter),
//!   `dismiss_time_machine_overlay` (Esc),
//!   `set_timeline_preview_revision` (drag tick / mouse motion),
//!   `refresh_time_machine_preview_if_needed` (paint-time refresh).
//! - **Keystroke dispatch** — `handle_time_machine_keystroke` turns
//!   the timeline-visible Enter / Esc into the right state mutation
//!   and consumes the keystroke.
//!
//! Thread ownership: UI thread of one window — `Window` is the only
//! mutator (matches the rest of the `window_*` family).

use std::sync::Arc;

use continuity_buffer::{Revision, RopeSnapshot};
use continuity_core::EditorSnapshot;
use continuity_persist::SnapshotSummaryRow;
use continuity_text::{Position, Range};
use ropey::Rope;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};

use crate::window::Window;
use crate::window_time_machine_hud::{
    compute_revision_for_x, SliderGeometry, SliderHit, SliderPaneRect,
};

/// Cached preview rope for the time-machine slider.
///
/// `revision` is the buffer revision the cached `snapshot` was
/// materialized at (via
/// [`continuity_persist::PersistClient::load_content_at_revision`]).
/// When `view_options.overlay.pinned_revision` advances or rewinds
/// past this revision, the cache is invalidated and re-loaded.
#[derive(Clone)]
pub(crate) struct TimeMachinePreview {
    /// Revision the cached snapshot represents.
    pub revision: Revision,
    /// Materialized read-only snapshot used by the renderer. Carries
    /// an empty selection set — the preview never shows the user's
    /// live selections at a past revision (that would imply we know
    /// the historical caret, which we don't).
    pub snapshot: EditorSnapshot,
}

/// In-flight mouse drag on the time-machine slider. The drag origin
/// (thumb / track / tick) doesn't influence later motion handling, so
/// the struct is currently empty — it exists purely as a presence
/// signal in `Window::time_machine_drag`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TimeMachineDrag;

impl Window {
    /// Phase-I1: Esc handler — dismiss the overlay, clear the pinned
    /// revision, drop the cached preview, restore the live head view.
    /// Read-only with respect to persistence.
    pub(crate) fn dismiss_time_machine_overlay(&mut self) {
        self.view_options.time_machine.timeline_visible = false;
        self.view_options.time_machine.timeline_preview_revision = None;
        self.view_options.overlay.pinned_revision = None;
        self.time_machine_preview = None;
        self.time_machine_drag = None;
        self.request_repaint();
    }

    /// Phase-I1: Enter handler — restore the previewed revision as a
    /// new edit at head (one undo group, content-replace op via
    /// `EditOp::replace`). Clears the overlay on success.
    ///
    /// Returns `true` when the keystroke was consumed (overlay was
    /// open). Returns `false` when the overlay isn't visible — caller
    /// should fall through to the regular keymap dispatch.
    pub(crate) fn commit_time_machine_overlay(&mut self) -> bool {
        if !self.view_options.time_machine.timeline_visible {
            return false;
        }
        let Some(target) = self.view_options.time_machine.timeline_preview_revision else {
            // Slider open but parked at head — nothing to restore;
            // treat Enter as a quiet dismiss.
            self.dismiss_time_machine_overlay();
            return true;
        };
        // Materialize the previewed content via the cached preview if
        // available, else load it now. Either way, we need a `String`
        // to use as the content-replace text.
        let content = match self.cached_preview_content_for(target) {
            Some(s) => s,
            None => match self.load_preview_content(target) {
                Some(s) => s,
                None => {
                    // Persist couldn't materialize — abort the commit
                    // but keep the keystroke consumed so the user
                    // doesn't accidentally insert a newline into the
                    // live buffer while the overlay is up.
                    return true;
                }
            },
        };
        // Build the EditOp against the *live head* rope (not the
        // preview), so the replace range covers the whole current
        // buffer.
        let head_snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return true,
        };
        let head_rope = head_snap.rope_snapshot().rope();
        let end =
            Position::from_byte_offset(head_rope, head_rope.len_bytes()).unwrap_or(Position::ZERO);
        let op = continuity_text::EditOp::replace(Range::new(Position::ZERO, end), content);
        if self.editor.apply_edit(self.buffer_id, op).is_err() {
            return true;
        }
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        self.dismiss_time_machine_overlay();
        true
    }

    /// Phase-I1: stage a preview revision (called from the slider's
    /// mouse drag handler). Mirrors the value into both
    /// `time_machine.timeline_preview_revision` (state ledger) and
    /// `view_options.overlay.pinned_revision` (renderer hook).
    /// Triggers a repaint; the next `on_paint` will refresh the
    /// preview cache if needed.
    pub(crate) fn set_timeline_preview_revision(&mut self, revision: Revision) {
        self.view_options.time_machine.timeline_preview_revision = Some(revision);
        self.view_options.overlay.pinned_revision = Some(revision);
        self.request_repaint();
    }

    /// Phase-I1: paint-time helper. If the overlay is active and the
    /// cached preview is missing or stale relative to
    /// `view_options.overlay.pinned_revision`, re-load the historical
    /// content via the persist client and rebuild the cache.
    ///
    /// No-op when the overlay is inactive or the persist client is
    /// missing (test harnesses).
    pub(crate) fn refresh_time_machine_preview_if_needed(&mut self) {
        let Some(target) = self.view_options.overlay.pinned_revision else {
            self.time_machine_preview = None;
            return;
        };
        if self
            .time_machine_preview
            .as_ref()
            .is_some_and(|p| p.revision == target)
        {
            return;
        }
        if let Some(content) = self.load_preview_content(target) {
            let rope = Rope::from_str(&content);
            let snapshot = EditorSnapshot {
                rope: RopeSnapshot::new(Arc::new(rope), target),
                selections: Vec::new(),
                file: None,
            };
            self.time_machine_preview = Some(TimeMachinePreview {
                revision: target,
                snapshot,
            });
        }
    }

    /// Phase-I1: keystroke dispatch for the time-machine overlay.
    /// Returns `true` when the keystroke was consumed by the slider.
    /// Called by `Window::on_keydown` when
    /// `time_machine.timeline_visible` is set, ahead of the regular
    /// dismiss chain / keymap dispatch.
    pub(crate) fn handle_time_machine_keystroke(&mut self, vk: u16) -> bool {
        if !self.view_options.time_machine.timeline_visible {
            return false;
        }
        if vk == VK_ESCAPE.0 {
            self.dismiss_time_machine_overlay();
            return true;
        }
        if vk == VK_RETURN.0 {
            return self.commit_time_machine_overlay();
        }
        false
    }

    /// Internal: pull the cached preview content as a `String` if it
    /// matches `target`. Returns `None` when the cache is stale.
    fn cached_preview_content_for(&self, target: Revision) -> Option<String> {
        self.time_machine_preview
            .as_ref()
            .filter(|p| p.revision == target)
            .map(|p| p.snapshot.rope_snapshot().rope().to_string())
    }

    /// Internal: synchronously load the rope content for `target` via
    /// the persist client. Returns `None` when the persist client is
    /// missing (test harness) or when the load fails / has no
    /// snapshot at-or-before `target`.
    fn load_preview_content(&self, target: Revision) -> Option<String> {
        let client = self.persist_client.as_ref()?;
        match client.load_content_at_revision(self.buffer_id, target) {
            Ok(content) => content,
            Err(e) => {
                eprintln!("time-machine: load_content_at_revision failed: {e}");
                None
            }
        }
    }

    /// Phase-I1: build the current slider geometry. `None` when the
    /// slider isn't visible, the persist client is missing, the buffer
    /// has no snapshots, or the editor doesn't know the buffer.
    pub(crate) fn build_time_machine_slider_geometry(&self) -> Option<SliderGeometry> {
        if !self.view_options.time_machine.timeline_visible {
            return None;
        }
        let summaries = self.fetch_snapshot_summaries()?;
        if summaries.is_empty() {
            return None;
        }
        let head_revision = self.editor.snapshot(self.buffer_id)?.rope.revision();
        let earliest_revision = summaries.first()?.revision.min(head_revision);
        let preview_revision = self
            .view_options
            .time_machine
            .timeline_preview_revision
            .unwrap_or(head_revision);
        let body = self.focused_body_rect();
        Some(SliderGeometry::build_in_rect(
            SliderPaneRect {
                left_dip: body.x,
                top_dip: body.y,
                width_dip: body.w.max(1.0),
                height_dip: body.h.max(1.0),
            },
            earliest_revision,
            head_revision,
            preview_revision,
            &summaries,
        ))
    }

    /// Phase-I1: WM_LBUTTONDOWN early-return for the slider. Returns
    /// `true` when the click landed inside the HUD band (consumed —
    /// caller skips the rest of the click chain). Captures the mouse
    /// when the click hits the thumb / track / a tick so subsequent
    /// `WM_MOUSEMOVE` events route into the drag handler.
    pub(crate) fn try_time_machine_slider_left_down(&mut self, x: i32, y: i32) -> bool {
        if !self.view_options.time_machine.timeline_visible {
            return false;
        }
        let Some(geometry) = self.build_time_machine_slider_geometry() else {
            self.dismiss_time_machine_overlay();
            return true;
        };
        match geometry.hit_test(x as f32, y as f32) {
            SliderHit::Outside => {
                self.dismiss_time_machine_overlay();
                true
            }
            SliderHit::Thumb => {
                self.time_machine_drag = Some(TimeMachineDrag);
                self.set_mouse_capture();
                true
            }
            SliderHit::Track { revision } => {
                self.time_machine_drag = Some(TimeMachineDrag);
                self.set_timeline_preview_revision(revision);
                self.set_mouse_capture();
                true
            }
            SliderHit::Tick(tick) => {
                self.time_machine_drag = Some(TimeMachineDrag);
                self.set_timeline_preview_revision(tick.revision);
                self.set_mouse_capture();
                true
            }
        }
    }

    /// Phase-I1: WM_MOUSEMOVE branch for the slider. Returns `true`
    /// when a slider drag is in flight — caller short-circuits the
    /// regular text-selection extension.
    pub(crate) fn try_time_machine_slider_mouse_move(&mut self, x: i32, _y: i32) -> bool {
        if self.time_machine_drag.is_none() {
            return false;
        }
        let Some(geometry) = self.build_time_machine_slider_geometry() else {
            return false;
        };
        let revision = compute_revision_for_x(
            geometry.strip_left_dip,
            geometry.strip_right_dip,
            geometry.earliest_revision,
            geometry.head_revision,
            x as f32,
        );
        self.set_timeline_preview_revision(revision);
        true
    }

    /// Phase-I1: WM_LBUTTONUP branch for the slider. Returns `true`
    /// when a drag was in flight — caller releases capture / skips
    /// other up-handling.
    pub(crate) fn try_time_machine_slider_left_up(&mut self) -> bool {
        if self.time_machine_drag.take().is_some() {
            self.release_mouse_capture();
            true
        } else {
            false
        }
    }

    /// Phase-I1: WM_MOUSEWHEEL branch for the slider. When the cursor
    /// is hovering the HUD band, each wheel notch steps the preview
    /// revision by one tick (the actual snapshot rows are the natural
    /// stops). Wheel up = forward in time (toward head); wheel down =
    /// backward (toward earliest). Returns `true` when the wheel was
    /// consumed so the caller skips the regular vertical-scroll path.
    ///
    /// `notches` is the unscaled `WHEEL_DELTA`-normalized count: +1
    /// for one detent up, -1 for one detent down, plus fractional
    /// values for high-resolution wheels (we step by `signum`).
    pub(crate) fn try_time_machine_slider_wheel(&mut self, x: i32, y: i32, notches: f32) -> bool {
        let Some(geometry) = self.build_time_machine_slider_geometry() else {
            return false;
        };
        // Only intercept the wheel when the cursor is actually over
        // the HUD band — otherwise the user expects normal buffer
        // scroll even with the slider open.
        if matches!(geometry.hit_test(x as f32, y as f32), SliderHit::Outside) {
            return false;
        }
        // Band hit but nowhere to slide (single-revision buffer): still
        // consume so the buffer doesn't scroll behind the open slider.
        if !geometry.has_drag_range() || notches == 0.0 {
            return true;
        }
        let current = geometry.preview_revision;
        let target = if notches > 0.0 {
            // Forward in time: jump to the first tick strictly newer
            // than the current preview, falling back to the head
            // revision when no later tick exists.
            geometry
                .ticks
                .iter()
                .map(|t| t.revision)
                .find(|r| *r > current)
                .unwrap_or(geometry.head_revision)
                .min(geometry.head_revision)
        } else {
            // Backward in time: last tick strictly older than current,
            // falling back to the earliest revision.
            geometry
                .ticks
                .iter()
                .rev()
                .map(|t| t.revision)
                .find(|r| *r < current)
                .unwrap_or(geometry.earliest_revision)
                .max(geometry.earliest_revision)
        };
        if target != current {
            self.set_timeline_preview_revision(target);
        }
        true
    }

    /// Phase-I1: build the render-side HUD payload from the current
    /// slider geometry + active theme. Returns `None` when the
    /// timeline is not visible (or geometry can't be built — buffer
    /// has no snapshots, persist client missing, etc.). Painted by
    /// `crates/render/src/time_machine_hud_paint.rs`.
    pub(crate) fn build_time_machine_hud_payload(
        &self,
    ) -> Option<continuity_render::TimeMachineHudDraw> {
        let geometry = self.build_time_machine_slider_geometry()?;
        let editor_colors = self.active_theme.editor_colors();
        // Translate UI-side `SliderTick` → render-side
        // `TimeMachineHudTick`. The render crate has no knowledge of
        // `Revision` / `Range` — it gets pre-positioned x-coordinates
        // and a kind flag.
        let ticks = geometry
            .ticks
            .iter()
            .map(|t| continuity_render::TimeMachineHudTick {
                x_dip: t.x_dip,
                is_named: matches!(
                    t.kind,
                    crate::window_time_machine_hud::TickKind::NamedSnapshot
                ),
            })
            .collect();
        // Color picks: band uses a darkened bg, track + edit-only ticks
        // use the line-number gutter color, named ticks pop in the
        // theme's foreground, thumb is the foreground itself. Keeps the
        // HUD readable across deep_minimal, solarized, and bring-your-
        // own themes without per-theme tuning.
        let band = darken_rgba(editor_colors.bg, 0.85, 0.94);
        let track = editor_colors.line_number;
        let tick_edit_only = editor_colors.line_number;
        let tick_named = editor_colors.fg;
        let thumb = editor_colors.fg;
        // Temporal labels: leftmost tick's date, rightmost tick's date,
        // and the previewed-revision tick's date floating above the
        // thumb. When the slider is parked at head the floating label
        // is suppressed (the live header already shows that time).
        let preview_revision = geometry.preview_revision;
        let left_label = geometry
            .ticks
            .first()
            .map(|t| format_compact_timestamp(t.created_at_ms))
            .unwrap_or_default();
        let right_label = geometry
            .ticks
            .last()
            .map(|t| format_compact_timestamp(t.created_at_ms))
            .unwrap_or_default();
        let thumb_label = if preview_revision == geometry.head_revision {
            String::new()
        } else {
            geometry
                .ticks
                .iter()
                .min_by_key(|t| {
                    (t.revision.get() as i128 - preview_revision.get() as i128).unsigned_abs()
                })
                .map(|t| format_compact_timestamp(t.created_at_ms))
                .unwrap_or_default()
        };
        Some(continuity_render::TimeMachineHudDraw {
            band_top_dip: geometry.band_top_dip,
            band_bottom_dip: geometry.band_bottom_dip,
            strip_left_dip: geometry.strip_left_dip,
            strip_right_dip: geometry.strip_right_dip,
            strip_center_y_dip: geometry.strip_center_y_dip,
            thumb_x_dip: geometry.thumb_x_dip(),
            ticks,
            band_color: band,
            track_color: track,
            tick_edit_only_color: tick_edit_only,
            tick_named_color: tick_named,
            thumb_color: thumb,
            text_color: editor_colors.line_number,
            left_label,
            right_label,
            thumb_label,
        })
    }

    fn fetch_snapshot_summaries(&self) -> Option<Vec<SnapshotSummaryRow>> {
        let client = self.persist_client.as_ref()?;
        match client.list_snapshot_summaries(self.buffer_id) {
            Ok(rows) => Some(rows),
            Err(e) => {
                eprintln!("time-machine: list_snapshot_summaries failed: {e}");
                None
            }
        }
    }

    fn set_mouse_capture(&self) {
        unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.hwnd);
        }
    }

    fn release_mouse_capture(&self) {
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
    }
}

/// Format a unix-millisecond timestamp as a compact `MMM DD HH:MM`
/// string for the HUD's temporal labels (UTC). Caption-weight
/// presentation matches the `LABEL_FONT_SIZE_DIP` cap on the render
/// side; longer formats (full year, seconds, locale) would overflow
/// the strip.
pub(crate) fn format_compact_timestamp(unix_ms: i64) -> String {
    if unix_ms <= 0 {
        return String::new();
    }
    let unix_ms_u: u64 = unix_ms as u64;
    let total_seconds = unix_ms_u / 1_000;
    let day_seconds = total_seconds % 86_400;
    let hour = (day_seconds / 3_600) as u32;
    let minute = ((day_seconds % 3_600) / 60) as u32;
    let days_since_epoch = (unix_ms_u / 86_400_000) as i64;
    let (_year, month, day) = civil_from_days(days_since_epoch);
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let m_name = MONTHS[(month as usize - 1).min(11)];
    format!("{m_name} {day:02} {hour:02}:{minute:02}")
}

/// Howard Hinnant's `civil_from_days`. Returns `(year, month, day)`
/// where `month ∈ 1..=12` and `day ∈ 1..=31`. Inlined here (rather
/// than imported from `window_metrics_paint`) so the time-machine HUD
/// path doesn't take a layer dependency on metrics paint.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (i32::try_from(y).unwrap_or(i32::MAX), m as u32, d as u32)
}

/// Scale `color`'s RGB toward black by `scale` and force alpha to
/// `alpha`. Theme-agnostic helper for picking a band fill color from
/// the editor bg without hardcoding hex values.
fn darken_rgba(color: continuity_render::Rgba, scale: f32, alpha: f32) -> continuity_render::Rgba {
    continuity_render::Rgba {
        r: color.r * scale,
        g: color.g * scale,
        b: color.b * scale,
        a: alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::format_compact_timestamp;

    #[test]
    fn format_compact_timestamp_renders_known_date() {
        // 2026-05-13 18:44:00 UTC.
        // Days since epoch: (2026-1970)*365.25 ≈ 20586 → exact: compute
        // by Howard Hinnant — but easier: 1747161840000 = 2025-05-13
        // 18:44:00 UTC; we just sanity-check shape.
        let s = format_compact_timestamp(1_747_161_840_000);
        // 2025-05-13 18:44 UTC.
        assert_eq!(s, "May 13 18:44");
    }

    #[test]
    fn format_compact_timestamp_zero_is_empty() {
        assert_eq!(format_compact_timestamp(0), "");
    }

    #[test]
    fn format_compact_timestamp_negative_is_empty() {
        assert_eq!(format_compact_timestamp(-1), "");
    }
}
