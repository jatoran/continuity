//! Per-tab state for the buffer-history visualization.
//!
//! One [`BufferHistoryTab`] hangs off each [`crate::pane_tree::Tab`]
//! whose `kind == TabKind::BufferHistory`, keyed by `TabId`. Owns:
//!
//!   * the swimlane data (`Vec<BufferHistoryLane>`) fetched from
//!     persist via
//!     [`continuity_persist::PersistClient::list_buffer_history_timeline`],
//!   * the time-axis viewport (`viewport_start_ms..viewport_end_ms`),
//!   * the lane filter discriminant (active / all / trashed),
//!   * the keyboard / pointer selection cursor.
//!
//! Pure state — no Win32, no DirectWrite. The renderer projects from
//! this struct, and the window wire-up (`window_buffer_history_tab.rs`)
//! drives mutation in response to commands and pointer events.
//!
//! Thread ownership: created and mutated only on the UI thread of the
//! owning [`crate::Window`]. The persist query runs on the persist
//! thread; the result is `Send` and crosses back via a reply channel.

use continuity_buffer::BufferId;
use continuity_persist::{BufferHistoryLane, BufferListFilter};

/// Per-tab buffer-history-visualization state.
///
/// All time fields are unix milliseconds. The viewport spans
/// `[viewport_start_ms, viewport_end_ms)` and is clamped on every
/// mutation to a sane minimum width (a `min_zoom_ms` floor prevents
/// the user from zooming past single-millisecond resolution and
/// dividing by zero in the renderer).
#[derive(Debug, Clone)]
pub struct BufferHistoryTab {
    /// Underlying data, ordered by `last_touched DESC` (lane 0 is the
    /// most recently active buffer). Refreshed from persist whenever
    /// the tab opens or the filter cycles.
    pub lanes: Vec<BufferHistoryLane>,
    /// Active filter discriminant. Mirrors the previous-buffer-browser
    /// overlay's `BufferListFilter` so the chord (`Ctrl+T`) behaves
    /// identically on both surfaces.
    pub filter: BufferListFilter,
    /// Inclusive lower bound of the time axis (unix ms).
    pub viewport_start_ms: i64,
    /// Exclusive upper bound of the time axis (unix ms).
    pub viewport_end_ms: i64,
    /// Currently-highlighted lane index. `None` when the lane list is
    /// empty.
    pub selected_lane: Option<usize>,
    /// Currently-hovered lane index from the pointer.
    pub hovered_lane: Option<usize>,
    /// Vertical scroll position in *lanes* (0 = top). Lets the chart
    /// scroll when more lanes exist than the pane body can show.
    pub scroll_lane_offset: usize,
    /// Active drag-pan state. `Some` while the user holds the left
    /// mouse button after clicking outside a lane row (ruler band,
    /// strip background, empty area below the last lane).
    pub pan_drag: Option<PanDragState>,
}

/// State captured at the moment the user starts a drag-pan. The
/// viewport translates from the captured bounds by `delta_ms` =
/// `-(current_client_x - start_client_x) * viewport_width / panel_strip_width`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanDragState {
    /// Pointer client x at drag start, in DIPs.
    pub start_client_x: i32,
    /// Viewport lower bound captured at drag start.
    pub start_viewport_start_ms: i64,
    /// Viewport upper bound captured at drag start.
    pub start_viewport_end_ms: i64,
    /// Strip width in DIPs at drag start — used to convert the
    /// pointer delta into a timestamp delta with the same scale the
    /// renderer was using when the drag began.
    pub strip_width_dip: f32,
}

/// Minimum viewport width in milliseconds. Below this, zoom is
/// clamped so the renderer never divides by zero and lane dots stay
/// visually distinguishable from each other.
pub const MIN_VIEWPORT_WIDTH_MS: i64 = 60_000; // 1 minute

/// Default viewport when the tab opens for the first time: trailing
/// `DEFAULT_LOOKBACK_MS` from `now`.
pub const DEFAULT_LOOKBACK_MS: i64 = 30 * 24 * 60 * 60 * 1_000; // 30 days

impl BufferHistoryTab {
    /// Fresh tab state. The viewport spans the last
    /// [`DEFAULT_LOOKBACK_MS`] up to `now_ms`. Lane list is empty
    /// until the window calls [`Self::set_lanes`] with persist data.
    #[must_use]
    pub fn new(now_ms: i64) -> Self {
        Self {
            lanes: Vec::new(),
            filter: BufferListFilter::ActiveOnly,
            viewport_start_ms: now_ms.saturating_sub(DEFAULT_LOOKBACK_MS),
            viewport_end_ms: now_ms,
            selected_lane: None,
            hovered_lane: None,
            scroll_lane_offset: 0,
            pan_drag: None,
        }
    }

    /// Replace the lane list (e.g. after a fresh persist query). Auto-
    /// fits the viewport to span every lane's `created_at` →
    /// `last_touched` so a freshly-opened tab shows the full history
    /// of every persisted buffer. Resets the selection cursor to lane
    /// 0 when at least one lane exists.
    pub fn set_lanes(&mut self, lanes: Vec<BufferHistoryLane>, now_ms: i64) {
        self.lanes = lanes;
        self.selected_lane = (!self.lanes.is_empty()).then_some(0);
        self.scroll_lane_offset = 0;
        self.fit_viewport_to_data(now_ms);
    }

    /// Replace the filter discriminant. The caller is expected to
    /// re-query persist and call [`Self::set_lanes`] with the fresh
    /// result.
    pub fn set_filter(&mut self, filter: BufferListFilter) {
        self.filter = filter;
    }

    /// Cycle `ActiveOnly → All → TrashedOnly → ActiveOnly`. Returns
    /// the new value so the caller can re-query persist.
    pub fn cycle_filter(&mut self) -> BufferListFilter {
        let next = match self.filter {
            BufferListFilter::ActiveOnly => BufferListFilter::All,
            BufferListFilter::All => BufferListFilter::TrashedOnly,
            BufferListFilter::TrashedOnly => BufferListFilter::ActiveOnly,
        };
        self.filter = next;
        next
    }

    /// Fit the viewport to the persisted data window. When the lane
    /// list is empty, fall back to a trailing
    /// [`DEFAULT_LOOKBACK_MS`] up to `now_ms`. Otherwise span from the
    /// earliest `created_at` to `now_ms` (so the viewport always
    /// includes "now" — the user's intuition is anchored on the right
    /// edge of the chart).
    pub fn fit_viewport_to_data(&mut self, now_ms: i64) {
        if self.lanes.is_empty() {
            self.viewport_start_ms = now_ms.saturating_sub(DEFAULT_LOOKBACK_MS);
            self.viewport_end_ms = now_ms;
            return;
        }
        let earliest = self
            .lanes
            .iter()
            .map(|l| l.record.created_at_ms)
            .min()
            .unwrap_or(now_ms);
        // Pad 2% on the leading edge so the very first snapshot dot
        // isn't pinned flush against the left ruler.
        let span = (now_ms - earliest).max(MIN_VIEWPORT_WIDTH_MS);
        let pad = span / 50;
        self.viewport_start_ms = earliest.saturating_sub(pad);
        self.viewport_end_ms = now_ms;
        self.clamp_viewport();
    }

    /// Multiplicative zoom centered on `pivot_ms`. `factor > 1.0`
    /// zooms out (wider viewport), `factor < 1.0` zooms in (narrower).
    /// The viewport is clamped to [`MIN_VIEWPORT_WIDTH_MS`] so the
    /// renderer never divides by zero.
    pub fn zoom_about(&mut self, pivot_ms: i64, factor: f32) {
        let factor = factor.clamp(0.05, 20.0) as f64;
        let span = (self.viewport_end_ms - self.viewport_start_ms) as f64;
        let new_span = (span * factor).max(MIN_VIEWPORT_WIDTH_MS as f64);
        let left_share = ((pivot_ms - self.viewport_start_ms) as f64 / span).clamp(0.0, 1.0);
        let new_start = pivot_ms as f64 - left_share * new_span;
        let new_end = new_start + new_span;
        self.viewport_start_ms = new_start.round() as i64;
        self.viewport_end_ms = new_end.round() as i64;
        self.clamp_viewport();
    }

    /// Translate the viewport by `delta_ms` (positive = forwards in
    /// time). Width is preserved.
    pub fn pan(&mut self, delta_ms: i64) {
        self.viewport_start_ms = self.viewport_start_ms.saturating_add(delta_ms);
        self.viewport_end_ms = self.viewport_end_ms.saturating_add(delta_ms);
        self.clamp_viewport();
    }

    /// Width of the time axis in milliseconds.
    #[must_use]
    pub fn viewport_width_ms(&self) -> i64 {
        (self.viewport_end_ms - self.viewport_start_ms).max(MIN_VIEWPORT_WIDTH_MS)
    }

    /// Project a millisecond timestamp into a `[0.0, 1.0]` fraction of
    /// the visible viewport. Values < 0 or > 1 indicate off-screen
    /// positions and the renderer must clip them.
    #[must_use]
    pub fn fraction_for(&self, ts_ms: i64) -> f32 {
        let width = self.viewport_width_ms() as f64;
        let dx = (ts_ms - self.viewport_start_ms) as f64;
        (dx / width) as f32
    }

    /// Inverse of [`Self::fraction_for`]: map `[0.0, 1.0]` back to a
    /// timestamp. Used by pointer-coordinate → timestamp conversion in
    /// hover/click handlers.
    #[must_use]
    pub fn timestamp_for(&self, fraction: f32) -> i64 {
        let width = self.viewport_width_ms() as f64;
        self.viewport_start_ms + (fraction as f64 * width).round() as i64
    }

    /// Step the selected lane by `delta` rows. Clamped to the lane
    /// range. No-op when the lane list is empty.
    pub fn step_lane(&mut self, delta: i32) {
        if self.lanes.is_empty() {
            self.selected_lane = None;
            return;
        }
        let cur = self.selected_lane.unwrap_or(0) as i32;
        let next = (cur + delta).clamp(0, self.lanes.len() as i32 - 1);
        self.selected_lane = Some(next as usize);
    }

    /// `Some(buffer_id)` of the currently-selected lane, `None` when
    /// no lane is selected. Used by `Enter` (adopt the buffer as a
    /// new tab) and by mouse-click commits.
    #[must_use]
    pub fn selected_buffer(&self) -> Option<BufferId> {
        let i = self.selected_lane?;
        Some(self.lanes.get(i)?.record.id)
    }

    /// `true` once at least one lane has been loaded.
    #[must_use]
    pub fn is_populated(&self) -> bool {
        !self.lanes.is_empty()
    }

    fn clamp_viewport(&mut self) {
        if self.viewport_end_ms - self.viewport_start_ms < MIN_VIEWPORT_WIDTH_MS {
            self.viewport_end_ms = self.viewport_start_ms.saturating_add(MIN_VIEWPORT_WIDTH_MS);
        }
    }
}

/// Time-axis bucket — a coarse "today / this week / older" partition
/// used by the renderer to draw the human-readable ruler labels above
/// the swimlanes.
///
/// The bucket boundaries are computed from a single `now_ms` reference
/// passed in by the caller so all four labels line up against the same
/// instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeAxisBucket {
    /// `[now - 24h, now]`.
    Today,
    /// `[now - 7d, now - 24h)`.
    ThisWeek,
    /// `[now - 30d, now - 7d)`.
    ThisMonth,
    /// `[..., now - 30d)`.
    Older,
}

impl TimeAxisBucket {
    /// Classify a timestamp into one of the four buckets, relative to
    /// `now_ms`.
    #[must_use]
    pub fn classify(ts_ms: i64, now_ms: i64) -> Self {
        let age = now_ms.saturating_sub(ts_ms).max(0);
        const MS_PER_DAY: i64 = 24 * 60 * 60 * 1_000;
        if age < MS_PER_DAY {
            TimeAxisBucket::Today
        } else if age < 7 * MS_PER_DAY {
            TimeAxisBucket::ThisWeek
        } else if age < 30 * MS_PER_DAY {
            TimeAxisBucket::ThisMonth
        } else {
            TimeAxisBucket::Older
        }
    }

    /// Short human-readable label for the time-axis ruler header.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            TimeAxisBucket::Today => "today",
            TimeAxisBucket::ThisWeek => "this week",
            TimeAxisBucket::ThisMonth => "this month",
            TimeAxisBucket::Older => "older",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_persist::BufferRecord;

    fn lane(id: BufferId, created_at_ms: i64, last_touched_ms: i64) -> BufferHistoryLane {
        BufferHistoryLane {
            record: BufferRecord {
                id,
                title: Some("t".into()),
                created_at_ms,
                last_touched_ms,
                edit_count: 0,
                is_trashed: false,
            },
            snapshot_times_ms: Vec::new(),
            line_count: 1,
            char_count: 1,
            preview: None,
        }
    }

    #[test]
    fn new_seeds_viewport_to_trailing_lookback() {
        let t = BufferHistoryTab::new(1_000_000);
        assert_eq!(t.viewport_end_ms, 1_000_000);
        assert_eq!(t.viewport_end_ms - t.viewport_start_ms, DEFAULT_LOOKBACK_MS);
        assert!(t.selected_lane.is_none());
    }

    #[test]
    fn set_lanes_auto_fits_viewport_and_seeds_selection() {
        let mut t = BufferHistoryTab::new(1_000_000);
        let id_a = BufferId::new();
        let id_b = BufferId::new();
        t.set_lanes(
            vec![lane(id_a, 800_000, 900_000), lane(id_b, 600_000, 700_000)],
            1_000_000,
        );
        assert_eq!(t.selected_lane, Some(0));
        assert!(t.viewport_start_ms <= 600_000);
        assert_eq!(t.viewport_end_ms, 1_000_000);
    }

    #[test]
    fn cycle_filter_walks_three_values() {
        let mut t = BufferHistoryTab::new(0);
        assert_eq!(t.cycle_filter(), BufferListFilter::All);
        assert_eq!(t.cycle_filter(), BufferListFilter::TrashedOnly);
        assert_eq!(t.cycle_filter(), BufferListFilter::ActiveOnly);
    }

    #[test]
    fn fraction_for_round_trips_through_timestamp_for() {
        let mut t = BufferHistoryTab::new(2_000_000);
        t.viewport_start_ms = 1_000_000;
        t.viewport_end_ms = 2_000_000;
        let f = t.fraction_for(1_500_000);
        assert!((f - 0.5).abs() < 1e-4);
        let back = t.timestamp_for(f);
        assert!((back - 1_500_000).abs() <= 1);
    }

    #[test]
    fn zoom_about_preserves_pivot_fraction() {
        let mut t = BufferHistoryTab::new(2_000_000);
        t.viewport_start_ms = 0;
        t.viewport_end_ms = 1_000_000;
        let pivot = 250_000;
        let f_before = t.fraction_for(pivot);
        t.zoom_about(pivot, 0.5);
        let f_after = t.fraction_for(pivot);
        assert!((f_before - f_after).abs() < 1e-3);
        assert!(t.viewport_end_ms - t.viewport_start_ms < 1_000_000);
    }

    #[test]
    fn zoom_clamps_below_min_viewport_width() {
        let mut t = BufferHistoryTab::new(1_000_000);
        t.zoom_about(500_000, 0.001);
        assert!(t.viewport_end_ms - t.viewport_start_ms >= MIN_VIEWPORT_WIDTH_MS);
    }

    #[test]
    fn pan_translates_viewport_preserving_width() {
        let mut t = BufferHistoryTab::new(2_000_000);
        let width_before = t.viewport_width_ms();
        t.pan(500_000);
        assert_eq!(t.viewport_width_ms(), width_before);
        assert_eq!(t.viewport_end_ms, 2_500_000);
    }

    #[test]
    fn step_lane_clamps_at_bounds() {
        let mut t = BufferHistoryTab::new(0);
        t.set_lanes(
            vec![
                lane(BufferId::new(), 0, 0),
                lane(BufferId::new(), 0, 0),
                lane(BufferId::new(), 0, 0),
            ],
            0,
        );
        assert_eq!(t.selected_lane, Some(0));
        t.step_lane(-2);
        assert_eq!(t.selected_lane, Some(0));
        t.step_lane(50);
        assert_eq!(t.selected_lane, Some(2));
    }

    #[test]
    fn step_lane_on_empty_yields_none() {
        let mut t = BufferHistoryTab::new(0);
        t.step_lane(1);
        assert!(t.selected_lane.is_none());
    }

    #[test]
    fn selected_buffer_returns_underlying_id() {
        let mut t = BufferHistoryTab::new(0);
        let target = BufferId::new();
        t.set_lanes(vec![lane(target, 0, 0)], 0);
        assert_eq!(t.selected_buffer(), Some(target));
    }

    #[test]
    fn time_axis_bucket_classifies_age_thresholds() {
        let now = 1_000_000_000;
        assert_eq!(
            TimeAxisBucket::classify(now - 60_000, now),
            TimeAxisBucket::Today
        );
        assert_eq!(
            TimeAxisBucket::classify(now - 3 * 24 * 60 * 60 * 1_000, now),
            TimeAxisBucket::ThisWeek
        );
        assert_eq!(
            TimeAxisBucket::classify(now - 14 * 24 * 60 * 60 * 1_000, now),
            TimeAxisBucket::ThisMonth
        );
        assert_eq!(
            TimeAxisBucket::classify(now - 90 * 24 * 60 * 60 * 1_000, now),
            TimeAxisBucket::Older
        );
    }
}
