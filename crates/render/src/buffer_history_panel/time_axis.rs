//! Time-axis tick computation for the buffer-history panel's ruler
//! band. Sibling of `buffer_history_panel.rs`; lifted out to keep the
//! parent paint module under the 600-line cap.

/// One time-axis tick produced by [`compute_time_axis_ticks`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeAxisTick {
    /// Unix millis the tick anchors to.
    pub ts_ms: i64,
    /// Human-readable label (granularity-aware — `"14:30"`,
    /// `"May 8"`, `"May 2026"`).
    pub label: String,
}

#[derive(Copy, Clone, Debug)]
enum TickGranularity {
    HourMinute,
    DayWithHour,
    DayOnly,
    Month,
}

/// Compute evenly-spaced time-axis ticks across the visible
/// viewport. The label granularity scales with the span: minutes/
/// hours for short ranges, day-with-hour for multi-day, day-only
/// for multi-week, month for multi-year.
///
/// Returns an empty vector when the strip is too narrow or the
/// viewport collapses (which only happens during pathological
/// resizes).
///
/// `scale` is the global text-zoom multiplier: the minimum spacing
/// between adjacent labels grows with it so the larger date glyphs do
/// not overlap when the user has zoomed in.
#[must_use]
pub(crate) fn compute_time_axis_ticks(
    start_ms: i64,
    end_ms: i64,
    strip_w_dip: f32,
    scale: f32,
) -> Vec<TimeAxisTick> {
    let scale = if scale.is_finite() {
        scale.clamp(0.25, 8.0)
    } else {
        1.0
    };
    if strip_w_dip < 60.0 * scale || end_ms <= start_ms {
        return Vec::new();
    }
    let min_label_spacing = 110.0 * scale;
    let max_ticks = ((strip_w_dip / min_label_spacing).floor() as usize).clamp(2, 7);
    let span = end_ms - start_ms;
    const HOUR: i64 = 3_600_000;
    const DAY: i64 = 24 * HOUR;
    let gran = if span < 4 * HOUR {
        TickGranularity::HourMinute
    } else if span < 5 * DAY {
        TickGranularity::DayWithHour
    } else if span < 120 * DAY {
        TickGranularity::DayOnly
    } else {
        TickGranularity::Month
    };
    let step = span / max_ticks as i64;
    let mut out = Vec::with_capacity(max_ticks);
    for i in 0..max_ticks {
        let ts = start_ms + step * i as i64 + step / 2;
        out.push(TimeAxisTick {
            ts_ms: ts,
            label: format_tick_label(ts, gran),
        });
    }
    out
}

fn format_tick_label(ts_ms: i64, gran: TickGranularity) -> String {
    let (y, m, d, hh, mm) = unix_ms_to_ymdhm(ts_ms);
    let month = month_short(m);
    match gran {
        TickGranularity::HourMinute => format!("{hh:02}:{mm:02}"),
        TickGranularity::DayWithHour => format!("{month} {d} {hh:02}:00"),
        TickGranularity::DayOnly => format!("{month} {d}"),
        TickGranularity::Month => format!("{month} {y}"),
    }
}

/// Civil-from-days conversion lifted from
/// `crates/ui/src/previous_buffer_browser.rs::unix_secs_to_ymd` and
/// extended with `(hour, minute)`. Public-domain Howard Hinnant
/// algorithm; avoids pulling in `chrono` for one date helper.
fn unix_ms_to_ymdhm(ts_ms: i64) -> (i32, u32, u32, u32, u32) {
    let secs = ts_ms.div_euclid(1_000);
    let days = secs.div_euclid(86_400);
    let remaining = secs.rem_euclid(86_400);
    let hour = (remaining / 3_600) as u32;
    let minute = ((remaining % 3_600) / 60) as u32;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d, hour, minute)
}

fn month_short(m: u32) -> &'static str {
    match m {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "???",
    }
}
