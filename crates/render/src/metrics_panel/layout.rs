//! Pure layout math for the metrics panel.

use std::cmp::Ordering;

use super::calendar::{short_month_day, weekday_index_from_iso};
use super::format::{
    chars_to_words, format_duration, lerp_color, plural_chars, plural_keystrokes, plural_words,
};
use super::{
    ActivityBar, HeatmapCell, LabelDraw, MetricCardDraw, MetricsDay, MetricsPanelColors,
    MetricsPanelInputs, MetricsPanelLayout, PanelRect, TopBufferRowDraw,
};

const PANEL_PAD_DIP: f32 = 16.0;
const SECTION_GAP_DIP: f32 = 18.0;
const HEADING_HEIGHT_DIP: f32 = 18.0;
const CAPTION_HEIGHT_DIP: f32 = 16.0;
const CARD_HEIGHT_DIP: f32 = 64.0;
const ACTIVITY_HEIGHT_DIP: f32 = 78.0;
const TOP_BUFFER_ROW_HEIGHT_DIP: f32 = 24.0;

/// Clamp a raw zoom factor to a sane range so a degenerate input never
/// collapses or explodes the layout. Mirrors the buffer-history panel's
/// guard.
fn resolve_scale(raw: f32) -> f32 {
    if raw.is_finite() {
        raw.clamp(0.25, 8.0)
    } else {
        1.0
    }
}

/// Compute every rectangle + label the renderer needs to paint the
/// metrics panel this frame.
///
/// All vertical / horizontal geometry is multiplied by
/// [`MetricsPanelInputs::scale`] so the dashboard grows together with
/// the zoom-scaled glyphs the renderer paints into each cell.
#[must_use]
pub fn compute_metrics_panel_layout(inputs: &MetricsPanelInputs) -> MetricsPanelLayout {
    let rect = inputs.viewport;
    let scale = resolve_scale(inputs.scale);
    let pad = (PANEL_PAD_DIP * scale)
        .min(rect.width() * 0.08)
        .max(8.0 * scale);
    let inner_left = rect.left + pad;
    let inner_right = (rect.right - pad).max(inner_left);
    let inner_width = (inner_right - inner_left).max(0.0);
    let mut cursor_y = rect.top + pad;

    let totals = Totals::from_days(&inputs.days, inputs.live_wpm);
    let header = LabelDraw {
        rect: PanelRect {
            left: inner_left,
            top: cursor_y,
            right: inner_right,
            bottom: cursor_y + 28.0 * scale,
        },
        text: "Metrics - local writing activity".into(),
    };
    cursor_y = header.rect.bottom + 14.0 * scale;

    let summary_cards = layout_summary_cards(
        PanelRect {
            left: inner_left,
            top: cursor_y,
            right: inner_right,
            bottom: cursor_y + summary_card_block_height(inner_width, scale),
        },
        &totals,
        scale,
    );
    cursor_y = summary_cards
        .iter()
        .map(|card| card.rect.bottom)
        .fold(cursor_y, f32::max)
        + SECTION_GAP_DIP * scale;

    let activity_heading = label(
        inner_left,
        cursor_y,
        inner_right,
        HEADING_HEIGHT_DIP * scale,
        "Recent writing",
    );
    cursor_y = activity_heading.rect.bottom;
    let activity_caption = label(
        inner_left,
        cursor_y,
        inner_right,
        CAPTION_HEIGHT_DIP * scale,
        format!("{} in the last 14 days", plural_words(totals.words_last_14)),
    );
    cursor_y = activity_caption.rect.bottom + 6.0 * scale;
    let activity_rect = PanelRect {
        left: inner_left,
        top: cursor_y,
        right: inner_right,
        bottom: cursor_y + ACTIVITY_HEIGHT_DIP * scale,
    };
    let (activity_bars, activity_axis_labels) = layout_activity(activity_rect, &inputs.days, scale);
    cursor_y = activity_rect.bottom + SECTION_GAP_DIP * scale;

    let top_buffers_heading = label(
        inner_left,
        cursor_y,
        inner_right,
        HEADING_HEIGHT_DIP * scale,
        "Most edited this week",
    );
    cursor_y = top_buffers_heading.rect.bottom + 6.0 * scale;
    let (top_buffers_rows, top_buffers_empty, top_bottom) = layout_top_buffers(
        inner_left,
        inner_right,
        cursor_y,
        &inputs.top_buffers,
        scale,
    );
    cursor_y = top_bottom + SECTION_GAP_DIP * scale;

    let heatmap_heading = label(
        inner_left,
        cursor_y,
        inner_right,
        HEADING_HEIGHT_DIP * scale,
        "90-day activity",
    );
    cursor_y = heatmap_heading.rect.bottom;
    let heatmap_caption = label(
        inner_left,
        cursor_y,
        inner_right,
        CAPTION_HEIGHT_DIP * scale,
        format!("{} total", plural_keystrokes(totals.keystrokes_last_90)),
    );
    cursor_y = heatmap_caption.rect.bottom + 4.0 * scale;

    let dow_height = 14.0 * scale;
    let dow_labels = layout_dow_axis(inner_left, inner_right, cursor_y, dow_height);
    let grid_top = cursor_y + dow_height + 4.0 * scale;
    let grid_bottom = (rect.bottom - pad).max(grid_top);
    let heatmap = layout_heatmap(
        PanelRect {
            left: inner_left,
            top: grid_top,
            right: inner_right,
            bottom: grid_bottom,
        },
        &inputs.days,
        inputs.colors,
    );

    let empty_state = if totals.keystrokes_last_90 == 0 && inputs.top_buffers.is_empty() {
        Some(label(
            inner_left,
            header.rect.bottom + 2.0 * scale,
            inner_right,
            18.0 * scale,
            "No writing metrics recorded yet.",
        ))
    } else {
        None
    };

    MetricsPanelLayout {
        header,
        summary_cards,
        activity_heading,
        activity_caption,
        activity_bars,
        activity_axis_labels,
        top_buffers_heading,
        top_buffers_rows,
        top_buffers_empty,
        heatmap_heading,
        heatmap_caption,
        dow_labels,
        heatmap,
        empty_state,
        background_rect: rect,
    }
}

/// Sort a snapshot of [`MetricsDay`] in ascending `day_iso` order.
pub fn sort_days_ascending(days: &mut [MetricsDay]) {
    days.sort_by(|a, b| a.day_iso.cmp(&b.day_iso).then(Ordering::Equal));
}

fn summary_card_block_height(width: f32, scale: f32) -> f32 {
    if width >= 540.0 * scale {
        CARD_HEIGHT_DIP * scale
    } else {
        CARD_HEIGHT_DIP * 2.0 * scale + 8.0 * scale
    }
}

fn layout_summary_cards(rect: PanelRect, totals: &Totals, scale: f32) -> Vec<MetricCardDraw> {
    let columns = if rect.width() >= 540.0 * scale { 4 } else { 2 };
    let rows = 4_usize.div_ceil(columns);
    let gap = 8.0 * scale;
    let card_width = (rect.width() - gap * (columns.saturating_sub(1) as f32)) / columns as f32;
    let card_height = (rect.height() - gap * (rows.saturating_sub(1) as f32)) / rows as f32;
    let values = [
        (
            "Today",
            plural_words(totals.words_today),
            format_duration(totals.active_today_ms),
        ),
        (
            "Pace",
            format!("{} WPM", totals.live_wpm),
            format!("peak {}", totals.peak_wpm_today),
        ),
        (
            "7 days",
            plural_words(totals.words_last_7),
            plural_keystrokes(totals.keystrokes_last_7),
        ),
        (
            "Revisions",
            plural_chars(totals.chars_edited_today),
            "typed + deleted".to_string(),
        ),
    ];
    values
        .iter()
        .enumerate()
        .map(|(idx, (name, value, detail))| {
            let col = idx % columns;
            let row = idx / columns;
            let left = rect.left + col as f32 * (card_width + gap);
            let top = rect.top + row as f32 * (card_height + gap);
            let card = PanelRect {
                left,
                top,
                right: left + card_width,
                bottom: top + card_height,
            };
            let inset = 10.0 * scale;
            MetricCardDraw {
                rect: card,
                label: label(
                    card.left + inset,
                    card.top + 6.0 * scale,
                    card.right - inset,
                    16.0 * scale,
                    *name,
                ),
                value: label(
                    card.left + inset,
                    card.top + 24.0 * scale,
                    card.right - inset,
                    22.0 * scale,
                    value.clone(),
                ),
                detail: label(
                    card.left + inset,
                    card.bottom - 20.0 * scale,
                    card.right - inset,
                    16.0 * scale,
                    detail.clone(),
                ),
            }
        })
        .collect()
}

fn layout_activity(
    rect: PanelRect,
    days: &[MetricsDay],
    scale: f32,
) -> (Vec<ActivityBar>, Vec<LabelDraw>) {
    let recent: Vec<&MetricsDay> = days
        .iter()
        .rev()
        .take(14)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if recent.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let label_height = 16.0 * scale;
    let chart = PanelRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: (rect.bottom - label_height).max(rect.top),
    };
    let gap = 4.0 * scale;
    let count = recent.len();
    let bar_width =
        ((chart.width() - gap * count.saturating_sub(1) as f32) / count as f32).max(2.0);
    let max_words = recent
        .iter()
        .map(|day| chars_to_words(day.chars_typed))
        .max()
        .unwrap_or(0)
        .max(1);
    let bars = recent
        .iter()
        .enumerate()
        .map(|(idx, day)| {
            let left = chart.left + idx as f32 * (bar_width + gap);
            let track = PanelRect {
                left,
                top: chart.top,
                right: (left + bar_width).min(chart.right),
                bottom: chart.bottom,
            };
            let words = chars_to_words(day.chars_typed);
            let ratio = (words as f32 / max_words as f32).clamp(0.0, 1.0);
            let fill_height = if words == 0 {
                0.0
            } else {
                (track.height() * ratio).max(2.0)
            };
            ActivityBar {
                track_rect: track,
                fill_rect: PanelRect {
                    left: track.left,
                    top: track.bottom - fill_height,
                    right: track.right,
                    bottom: track.bottom,
                },
                words,
            }
        })
        .collect();
    let axis_label_w = 120.0 * scale;
    let mut axis = Vec::new();
    if let Some(first) = recent.first() {
        axis.push(label(
            rect.left,
            rect.bottom - label_height,
            rect.left + axis_label_w,
            label_height,
            short_month_day(&first.day_iso),
        ));
    }
    if let Some(last) = recent.last() {
        axis.push(label(
            (rect.right - axis_label_w).max(rect.left),
            rect.bottom - label_height,
            rect.right,
            label_height,
            short_month_day(&last.day_iso),
        ));
    }
    (bars, axis)
}

fn layout_top_buffers(
    left: f32,
    right: f32,
    top: f32,
    top_buffers: &[super::TopBufferEntry],
    scale: f32,
) -> (Vec<TopBufferRowDraw>, Option<LabelDraw>, f32) {
    let row_height = TOP_BUFFER_ROW_HEIGHT_DIP * scale;
    if top_buffers.is_empty() {
        let empty = label(left, top, right, row_height, "No edits this week yet.");
        return (Vec::new(), Some(empty), top + row_height);
    }
    let max_edits = top_buffers
        .iter()
        .map(|entry| entry.edit_count)
        .max()
        .unwrap_or(1)
        .max(1);
    let label_height = 18.0 * scale;
    let count_gap = 8.0 * scale;
    let rows: Vec<TopBufferRowDraw> = top_buffers
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let row_top = top + idx as f32 * row_height;
            let row_rect = PanelRect {
                left,
                top: row_top,
                right,
                bottom: row_top + row_height,
            };
            let count_width = (92.0 * scale).min(row_rect.width() * 0.35);
            let track = PanelRect {
                left,
                top: row_rect.bottom - 5.0 * scale,
                right,
                bottom: row_rect.bottom - 3.0 * scale,
            };
            let ratio = (entry.edit_count as f32 / max_edits as f32).clamp(0.0, 1.0);
            TopBufferRowDraw {
                row_rect,
                title: label(
                    left,
                    row_top,
                    (right - count_width - count_gap).max(left),
                    label_height,
                    entry.title.clone(),
                ),
                count: label(
                    (right - count_width).max(left),
                    row_top,
                    right,
                    label_height,
                    format!("{} edits", entry.edit_count),
                ),
                bar_track: track,
                bar_fill: PanelRect {
                    right: track.left + track.width() * ratio,
                    ..track
                },
            }
        })
        .collect();
    let bottom = top + rows.len() as f32 * row_height;
    (rows, None, bottom)
}

fn layout_dow_axis(left: f32, right: f32, top: f32, height: f32) -> Vec<LabelDraw> {
    const LABELS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];
    let cell_w = ((right - left) / 7.0).max(0.0);
    LABELS
        .iter()
        .enumerate()
        .map(|(i, &c)| LabelDraw {
            rect: PanelRect {
                left: left + i as f32 * cell_w,
                top,
                right: left + (i as f32 + 1.0) * cell_w,
                bottom: top + height,
            },
            text: c.into(),
        })
        .collect()
}

fn layout_heatmap(
    rect: PanelRect,
    days: &[MetricsDay],
    colors: MetricsPanelColors,
) -> Vec<HeatmapCell> {
    if days.is_empty() {
        return Vec::new();
    }
    const COLS: usize = 7;
    let start_col = days
        .first()
        .and_then(|day| weekday_index_from_iso(&day.day_iso))
        .unwrap_or(0);
    let rows = (start_col + days.len()).div_ceil(COLS).max(1);
    let cell_w = rect.width() / COLS as f32;
    let cell_h = rect.height() / rows as f32;
    let gap = 2.0_f32.min(cell_w * 0.25).min(cell_h * 0.25);
    let max_keystrokes = days.iter().map(|d| d.keystrokes).max().unwrap_or(0);
    let mut cells = Vec::with_capacity(days.len());
    for (i, day) in days.iter().enumerate() {
        let slot = start_col + i;
        let col = slot % COLS;
        let row = slot / COLS;
        let x0 = rect.left + col as f32 * cell_w;
        let y0 = rect.top + row as f32 * cell_h;
        let intensity = if max_keystrokes == 0 {
            0.0
        } else {
            (day.keystrokes as f32 / max_keystrokes as f32).clamp(0.0, 1.0)
        };
        cells.push(HeatmapCell {
            rect: PanelRect {
                left: x0 + gap,
                top: y0 + gap,
                right: x0 + cell_w - gap,
                bottom: y0 + cell_h - gap,
            },
            color: lerp_color(colors.heatmap_empty, colors.heatmap_full, intensity),
            keystrokes: day.keystrokes,
        });
    }
    cells
}

fn label(left: f32, top: f32, right: f32, height: f32, text: impl Into<String>) -> LabelDraw {
    LabelDraw {
        rect: PanelRect {
            left,
            top,
            right,
            bottom: top + height,
        },
        text: text.into(),
    }
}

#[derive(Debug, Default)]
struct Totals {
    words_today: u64,
    active_today_ms: u64,
    peak_wpm_today: u32,
    chars_edited_today: u64,
    live_wpm: u32,
    words_last_7: u64,
    words_last_14: u64,
    keystrokes_last_7: u64,
    keystrokes_last_90: u64,
}

impl Totals {
    fn from_days(days: &[MetricsDay], live_wpm: u32) -> Self {
        let today = days.last().cloned().unwrap_or_default();
        let mut totals = Self {
            words_today: chars_to_words(today.chars_typed),
            active_today_ms: today.active_ms,
            peak_wpm_today: today.wpm_peak.max(today.wpm_average).max(live_wpm),
            chars_edited_today: today.chars_typed.saturating_add(today.chars_deleted),
            live_wpm,
            ..Default::default()
        };
        for (idx, day) in days.iter().rev().enumerate() {
            totals.keystrokes_last_90 = totals.keystrokes_last_90.saturating_add(day.keystrokes);
            if idx < 14 {
                totals.words_last_14 = totals
                    .words_last_14
                    .saturating_add(chars_to_words(day.chars_typed));
            }
            if idx < 7 {
                totals.words_last_7 = totals
                    .words_last_7
                    .saturating_add(chars_to_words(day.chars_typed));
                totals.keystrokes_last_7 = totals.keystrokes_last_7.saturating_add(day.keystrokes);
            }
        }
        totals
    }
}
