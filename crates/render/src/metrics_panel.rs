//! Pure-data layout for the metrics buffer surface.
//!
//! The renderer dispatches to this module when the focused tab's
//! buffer id matches `time_machine.metrics_buffer_id`. The buffer rope
//! itself is never painted. Persisted `metrics_daily` rows and weekly
//! edit counts are projected into a quiet dashboard:
//!
//! 1. a four-value summary strip,
//! 2. a compact recent-writing trend,
//! 3. a ranked "most edited this week" list,
//! 4. a secondary 90-day activity calendar.
//!
//! This file owns the public data shapes. Layout math lives in
//! [`layout`], and the Direct2D paint pass lives in
//! [`crate::metrics_panel_paint`].
//!
//! Thread ownership: pure - every function takes its inputs by value,
//! so the layout can be computed on any thread the renderer chooses.

mod calendar;
mod format;
pub mod layout;
#[cfg(test)]
mod tests;

/// One row's worth of per-day metrics, mirrored from
/// `continuity_persist::MetricsDailyRow` so this crate does not have
/// to depend on `continuity-persist`.
#[derive(Debug, Clone, Default)]
pub struct MetricsDay {
    /// Calendar day in `YYYY-MM-DD` (UTC, per
    /// `window_metrics_paint::day_iso_from_unix_ms`).
    pub day_iso: String,
    /// Total keystrokes recorded for the day.
    pub keystrokes: u64,
    /// Characters typed (additions, not net).
    pub chars_typed: u64,
    /// Characters deleted.
    pub chars_deleted: u64,
    /// Active milliseconds.
    pub active_ms: u64,
    /// Average WPM for the day (= `wpm_sum / wpm_samples`, rounded).
    pub wpm_average: u32,
    /// Peak WPM over any rolling 60 s window on this day.
    pub wpm_peak: u32,
}

/// One row in the "Most edited this week" list.
#[derive(Debug, Clone)]
pub struct TopBufferEntry {
    /// Display name (derived title, file name, or a stable fallback).
    pub title: String,
    /// Edit-log rows attributed to this buffer over the window.
    pub edit_count: u64,
}

/// Inputs to [`layout::compute_metrics_panel_layout`].
#[derive(Debug, Clone)]
pub struct MetricsPanelInputs {
    /// Most-recent 90 days of activity. `days[0]` is the oldest, the
    /// last entry is "today". Missing days are zeroed [`MetricsDay`]s.
    pub days: Vec<MetricsDay>,
    /// Live trailing WPM for the rightmost pace value - already
    /// idle-frozen by the caller.
    pub live_wpm: u32,
    /// Panel viewport in DIPs (the focused pane's body rect, not the
    /// full window).
    pub viewport: PanelRect,
    /// Resolved theme colors for this paint frame.
    pub colors: MetricsPanelColors,
    /// Buffers ranked by edit count over the trailing 7 days.
    pub top_buffers: Vec<TopBufferEntry>,
    /// Global text-zoom multiplier (`continuity_layout::ViewState::
    /// font_size_scale`). The `text_format` the renderer paints this
    /// panel with is built at `base_font * scale`, so every text-
    /// bearing geometric quantity (heading / caption / card / row
    /// heights, paddings, section gaps, bar and heatmap-cell sizing)
    /// is multiplied by this factor so the dashboard scales coherently
    /// and no value overflows its cell. `1.0` is no zoom.
    pub scale: f32,
}

/// Axis-aligned DIP rectangle.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PanelRect {
    /// Left edge (DIPs).
    pub left: f32,
    /// Top edge.
    pub top: f32,
    /// Right edge.
    pub right: f32,
    /// Bottom edge.
    pub bottom: f32,
}

impl PanelRect {
    /// Width in DIPs.
    #[must_use]
    pub fn width(self) -> f32 {
        (self.right - self.left).max(0.0)
    }

    /// Height in DIPs.
    #[must_use]
    pub fn height(self) -> f32 {
        (self.bottom - self.top).max(0.0)
    }
}

/// Theme-driven palette for the panel. Every color is a packed
/// `0xAARRGGBB` value so this module stays Win32-agnostic.
#[derive(Debug, Clone, Copy)]
pub struct MetricsPanelColors {
    /// Panel background fill.
    pub background: u32,
    /// Heading + value text.
    pub foreground: u32,
    /// Muted text (section labels, captions, axes).
    pub muted_foreground: u32,
    /// "No activity" cell / quiet panel fill.
    pub heatmap_empty: u32,
    /// Maximum activity accent. Intermediate cells are linearly
    /// interpolated between [`Self::heatmap_empty`] and this.
    pub heatmap_full: u32,
    /// Recent-activity trend and ranked-list bar fill.
    pub sparkline: u32,
}

impl Default for MetricsPanelColors {
    fn default() -> Self {
        Self {
            background: 0xFF_1E_1E_1E,
            foreground: 0xFF_E0_E0_E0,
            muted_foreground: 0xFF_90_90_90,
            heatmap_empty: 0xFF_2A_2A_2A,
            heatmap_full: 0xFF_4F_C3_F7,
            sparkline: 0xFF_FF_CA_28,
        }
    }
}

/// A piece of text to render with its target DIP rectangle.
#[derive(Debug, Clone)]
pub struct LabelDraw {
    /// Rectangle in panel DIP coordinates.
    pub rect: PanelRect,
    /// Text payload (UTF-8).
    pub text: String,
}

/// One top summary value.
#[derive(Debug, Clone)]
pub struct MetricCardDraw {
    /// Card/background rectangle.
    pub rect: PanelRect,
    /// Muted label, e.g. `"Today"`.
    pub label: LabelDraw,
    /// Primary value, e.g. `"420 words"`.
    pub value: LabelDraw,
    /// Secondary detail, e.g. `"18 min active"`.
    pub detail: LabelDraw,
}

/// One recent-activity bar. These are not stacked; each bar represents
/// one day of typed-word volume.
#[derive(Debug, Clone, Copy)]
pub struct ActivityBar {
    /// Full quiet track rectangle.
    pub track_rect: PanelRect,
    /// Filled value rectangle.
    pub fill_rect: PanelRect,
    /// Source value in 5-character words.
    pub words: u64,
}

/// One heatmap cell ready for the renderer to fill.
#[derive(Debug, Clone, Copy)]
pub struct HeatmapCell {
    /// Cell rectangle in panel DIP coordinates.
    pub rect: PanelRect,
    /// Linearly-interpolated fill color, `0xAARRGGBB`.
    pub color: u32,
    /// `keystrokes` for the represented day.
    pub keystrokes: u64,
}

/// One ranked top-buffer row.
#[derive(Debug, Clone)]
pub struct TopBufferRowDraw {
    /// Full row rectangle.
    pub row_rect: PanelRect,
    /// Left title label.
    pub title: LabelDraw,
    /// Right count label.
    pub count: LabelDraw,
    /// Quiet comparison track.
    pub bar_track: PanelRect,
    /// Proportional comparison fill.
    pub bar_fill: PanelRect,
}

/// Kept for source compatibility with the original metrics surface.
#[derive(Debug, Clone, Copy)]
pub struct SparklineVertex {
    /// X coordinate in DIPs (panel-relative).
    pub x: f32,
    /// Y coordinate in DIPs (panel-relative).
    pub y: f32,
}

/// The full panel layout - every rectangle + label the renderer needs
/// to paint a single frame.
#[derive(Debug, Clone)]
pub struct MetricsPanelLayout {
    /// Top-strip header text.
    pub header: LabelDraw,
    /// Four primary metric cards.
    pub summary_cards: Vec<MetricCardDraw>,
    /// "Recent writing" section heading.
    pub activity_heading: LabelDraw,
    /// Activity section caption.
    pub activity_caption: LabelDraw,
    /// One bar per recent day.
    pub activity_bars: Vec<ActivityBar>,
    /// Sparse axis labels for the activity bar strip.
    pub activity_axis_labels: Vec<LabelDraw>,
    /// "Most edited this week" sub-heading.
    pub top_buffers_heading: LabelDraw,
    /// One rendered row per top buffer.
    pub top_buffers_rows: Vec<TopBufferRowDraw>,
    /// Placeholder when the top-buffer list is empty.
    pub top_buffers_empty: Option<LabelDraw>,
    /// "90-day activity" sub-heading above the calendar grid.
    pub heatmap_heading: LabelDraw,
    /// Small caption above the calendar.
    pub heatmap_caption: LabelDraw,
    /// Seven day-of-week labels ("S","M","T","W","T","F","S").
    pub dow_labels: Vec<LabelDraw>,
    /// Calendar-aligned 90-day heatmap cells.
    pub heatmap: Vec<HeatmapCell>,
    /// Empty-state placeholder for a brand-new metrics store.
    pub empty_state: Option<LabelDraw>,
    /// Region the renderer should paint with [`MetricsPanelColors::background`].
    pub background_rect: PanelRect,
}
