//! §I2: paint the metrics-buffer surface and supporting projections.
//!
//! Sibling of [`crate::window_time_machine`] — extracted so that file
//! stays under the 600-line cap once the I2 paint dispatch + panel
//! data builders landed. The runtime tap (keystroke recording, 1 Hz
//! flush, timer arming) still lives in `window_time_machine`; this
//! file owns the per-paint projection that turns persisted
//! `metrics_daily` rows + theme colors into a layout the renderer
//! consumes.
//!
//! Thread ownership: every function runs on the owning window's UI
//! thread. `Window` is the only mutator of `metrics_pending`,
//! `wpm_tracker`, and `view_options.time_machine`.

use crate::window::Window;

impl Window {
    /// §I2: overlay the metrics panel onto the supplied DIP viewport
    /// without presenting. Called after `Renderer::draw_buffer_no_present`
    /// has painted the regular chrome + an empty body backdrop, so the
    /// tab strip, status bar, and pane borders remain visible.
    ///
    /// No-op when the renderer or text format isn't ready during an
    /// early frame; the caller still owns the final present.
    pub(crate) fn paint_metrics_buffer_overlay_no_present(
        &mut self,
        viewport: continuity_render::metrics_panel::PanelRect,
    ) -> Result<(), crate::Error> {
        // Compute every mutable-borrow input *before* we take the
        // shared borrow of `self.renderer`: `metrics_display_wpm`
        // takes `&mut self` to update `wpm_frozen`, so any longer-
        // lived borrow blocks it.
        let now_ms = self.now_ms();
        let days = build_metrics_panel_days(self.persist_client.as_ref(), now_ms);
        let top_buffers = load_top_buffers(self.persist_client.as_ref(), now_ms);
        let colors = metrics_panel_colors_for(&self.active_theme);
        let text_format = match self.text_format.as_ref().cloned() {
            Some(f) => f,
            None => return Ok(()),
        };
        let live_wpm = self.metrics_display_wpm();
        let inputs = continuity_render::metrics_panel::MetricsPanelInputs {
            days,
            live_wpm,
            viewport,
            colors,
            top_buffers,
        };
        let layout =
            continuity_render::metrics_panel::layout::compute_metrics_panel_layout(&inputs);
        let Some(renderer) = self.renderer.as_ref() else {
            return Ok(());
        };
        continuity_render::metrics_panel_paint::paint_metrics_panel_no_present(
            renderer,
            &layout,
            colors,
            &text_format,
        )?;
        // Repaint cycle has been served; clear the dirty token until
        // the 1 Hz timer or a fresh keystroke flips it again.
        self.view_options.time_machine.metrics_repaint_due = false;
        Ok(())
    }

    /// §I2: read the WPM for paint while honoring the idle-freeze
    /// rule. If the user hasn't typed in [`METRICS_WPM_IDLE_THRESHOLD_MS`],
    /// show the frozen value captured at the last keystroke instead of
    /// letting the rolling window decay toward zero.
    fn metrics_display_wpm(&mut self) -> u32 {
        let now_ms = self.now_ms();
        let last = self.metrics_last_keystroke_ms;
        if last == 0 {
            return 0;
        }
        if now_ms.saturating_sub(last) >= METRICS_WPM_IDLE_THRESHOLD_MS {
            // Idle ⇒ frozen value from the last burst.
            self.wpm_frozen
        } else {
            let live = self.wpm_tracker.wpm_now(now_ms);
            self.wpm_frozen = live;
            live
        }
    }
}

/// §I2: any gap longer than this between keystrokes counts as
/// "idle" — the displayed WPM freezes at its last live reading
/// instead of decaying as the rolling window drains. 2 s is short
/// enough not to mask a real slowdown (a fast typist's inter-key
/// gap is well under it) but long enough to ride out pauses for
/// thinking.
pub(crate) const METRICS_WPM_IDLE_THRESHOLD_MS: u64 = 2_000;

/// §I2: pull up to 90 days of metrics rows from the persist client
/// (ascending by `day_iso`) and pad missing days with zeros so the
/// caller can iterate exactly 90 entries oldest-first. When no client
/// is available (test harnesses) the result is an empty vec — the
/// renderer paints the empty-state heatmap.
pub(crate) fn build_metrics_panel_days(
    client: Option<&continuity_persist::PersistClient>,
    now_ms: u64,
) -> Vec<continuity_render::metrics_panel::MetricsDay> {
    let Some(client) = client else {
        return Vec::new();
    };
    let today = day_iso_from_unix_ms(now_ms);
    let earliest = day_iso_from_unix_ms(now_ms.saturating_sub(89 * 86_400_000));
    let rows = match client.load_metrics_range(earliest.clone(), today.clone()) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("paint_metrics_buffer: load_metrics_range failed: {e}");
            return Vec::new();
        }
    };
    // Walk 90 calendar days from `earliest` forward, splicing in
    // matching rows. Missing days are left zero-filled.
    let mut by_day: std::collections::HashMap<String, &continuity_persist::MetricsDailyRow> =
        std::collections::HashMap::new();
    for row in &rows {
        by_day.insert(row.day_iso.clone(), row);
    }
    let mut out = Vec::with_capacity(90);
    for i in 0..90u64 {
        let day_ms = now_ms.saturating_sub((89 - i) * 86_400_000);
        let day_iso = day_iso_from_unix_ms(day_ms);
        let day = match by_day.get(&day_iso) {
            Some(row) => {
                let avg = if row.wpm_samples == 0 {
                    0
                } else {
                    (row.wpm_sum / row.wpm_samples).min(u64::from(u32::MAX)) as u32
                };
                continuity_render::metrics_panel::MetricsDay {
                    day_iso: row.day_iso.clone(),
                    keystrokes: row.keystrokes,
                    chars_typed: row.chars_typed,
                    chars_deleted: row.chars_deleted,
                    active_ms: row.active_ms,
                    wpm_average: avg,
                    wpm_peak: row.wpm_peak,
                }
            }
            None => continuity_render::metrics_panel::MetricsDay {
                day_iso,
                ..Default::default()
            },
        };
        out.push(day);
    }
    out
}

/// §I2: derive the metrics-panel palette from the active theme. The
/// editor's bg / fg map naturally; the heatmap intensity ramp + sparkline
/// pick a contrasting accent color so the surface stays legible across
/// the bundled themes.
pub(crate) fn metrics_panel_colors_for(
    theme: &crate::window_theme::ActiveTheme,
) -> continuity_render::metrics_panel::MetricsPanelColors {
    let editor = theme.editor_colors();
    let bg = rgba_to_argb(editor.bg);
    let fg = rgba_to_argb(editor.fg);
    let accent = rgba_to_argb(editor.caret);
    let quiet = blend_argb(bg, fg, 0.12);
    continuity_render::metrics_panel::MetricsPanelColors {
        background: bg,
        foreground: fg,
        // Sub-headings, axis labels, "max" / value chips — sit at a
        // muted midpoint between bg and fg so they read as captions
        // rather than competing with the live header.
        muted_foreground: blend_argb(bg, fg, 0.55),
        heatmap_empty: quiet,
        heatmap_full: blend_argb(quiet, accent, 0.82),
        sparkline: accent,
    }
}

/// §I2: load the top edit-count buffers for the metrics panel's
/// "Most edited this week" surface. Returns at most 5 rows, ranked by
/// edit count descending. Falls back to an empty vec when there's no
/// persist client (test harnesses) or the query fails.
pub(crate) fn load_top_buffers(
    client: Option<&continuity_persist::PersistClient>,
    now_ms: u64,
) -> Vec<continuity_render::metrics_panel::TopBufferEntry> {
    let Some(client) = client else {
        return Vec::new();
    };
    let week_ms = 7 * 86_400_000i64;
    let now_i64 = i64::try_from(now_ms).unwrap_or(i64::MAX);
    let start_ms = now_i64.saturating_sub(week_ms);
    let rows = match client.load_top_buffers_by_edits(start_ms, now_i64, 5) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("paint_metrics_buffer: load_top_buffers_by_edits failed: {e}");
            return Vec::new();
        }
    };
    rows.into_iter()
        .map(|r| continuity_render::metrics_panel::TopBufferEntry {
            title: top_buffer_display_title(&r),
            edit_count: r.edit_count,
        })
        .collect()
}

fn top_buffer_display_title(row: &continuity_persist::TopBufferRow) -> String {
    if let Some(title) = row.title.as_deref().filter(|s| !s.trim().is_empty()) {
        return title.to_string();
    }
    if let Some(path) = row.file_path.as_deref() {
        if let Some(name) = std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.trim().is_empty())
        {
            return name.to_string();
        }
    }
    format!("buffer {}", short_buffer_id(row.buffer_id))
}

/// First 8 hex chars of a buffer's UUIDv7 — enough to disambiguate in
/// a 5-row list without dragging in the full 32-char id.
fn short_buffer_id(id: continuity_buffer::BufferId) -> String {
    let s = id.as_uuid().simple().to_string();
    s.chars().take(8).collect()
}

/// Convert a [`continuity_render::params::Rgba`] into the
/// `0xAARRGGBB` packing the metrics panel expects.
pub(crate) fn rgba_to_argb(rgba: continuity_render::params::Rgba) -> u32 {
    let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    (to_u8(rgba.a) << 24) | (to_u8(rgba.r) << 16) | (to_u8(rgba.g) << 8) | to_u8(rgba.b)
}

/// Linear interpolation between two `0xAARRGGBB` packed colors.
pub(crate) fn blend_argb(a: u32, b: u32, t: f32) -> u32 {
    let mix = |sa: u8, sb: u8| -> u8 {
        let fa = f32::from(sa);
        let fb = f32::from(sb);
        let m = fa + (fb - fa) * t.clamp(0.0, 1.0);
        m.round().clamp(0.0, 255.0) as u8
    };
    let a_a = ((a >> 24) & 0xFF) as u8;
    let r_a = ((a >> 16) & 0xFF) as u8;
    let g_a = ((a >> 8) & 0xFF) as u8;
    let b_a = (a & 0xFF) as u8;
    let a_b = ((b >> 24) & 0xFF) as u8;
    let r_b = ((b >> 16) & 0xFF) as u8;
    let g_b = ((b >> 8) & 0xFF) as u8;
    let b_b = (b & 0xFF) as u8;
    (u32::from(mix(a_a, a_b)) << 24)
        | (u32::from(mix(r_a, r_b)) << 16)
        | (u32::from(mix(g_a, g_b)) << 8)
        | u32::from(mix(b_a, b_b))
}

/// Convert a unix-millisecond timestamp to a UTC `YYYY-MM-DD` string.
///
/// Uses Howard Hinnant's "days from civil" inverse to avoid pulling in
/// the `chrono` / `time` crates (the project explicitly bans `tokio`
/// and prefers minimal dependencies). The output is **UTC**, not local
/// time — keeping `metrics_daily` keyed by a fixed timezone avoids the
/// "two-rows-on-DST" edge case at the cost of cross-midnight users
/// seeing a slightly skewed day boundary. The spec §I2 doesn't pin
/// either choice; UTC is the simpler invariant to test.
#[must_use]
pub(crate) fn day_iso_from_unix_ms(ms: u64) -> String {
    let days_since_epoch: i64 = (ms / 86_400_000) as i64;
    let (y, m, d) = civil_from_days(days_since_epoch);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's `civil_from_days`. Returns `(year, month, day)`
/// where `month ∈ 1..=12` and `day ∈ 1..=31`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_iso_unix_epoch_is_1970_01_01() {
        assert_eq!(day_iso_from_unix_ms(0), "1970-01-01");
    }

    #[test]
    fn day_iso_handles_leap_day() {
        // 2024-02-29 UTC midnight = 1709164800000 ms since epoch.
        assert_eq!(day_iso_from_unix_ms(1_709_164_800_000), "2024-02-29");
    }

    #[test]
    fn day_iso_matches_known_2026_date() {
        // 2026-05-12 UTC midnight = 1778544000000 ms since epoch
        // (20585 days × 86_400_000 ms/day).
        assert_eq!(day_iso_from_unix_ms(1_778_544_000_000), "2026-05-12");
    }

    #[test]
    fn build_panel_days_no_client_returns_empty() {
        let days = build_metrics_panel_days(None, 1_778_544_000_000);
        assert!(days.is_empty());
    }

    #[test]
    fn rgba_to_argb_round_trip_canonical() {
        let opaque_red = continuity_render::params::Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        assert_eq!(rgba_to_argb(opaque_red), 0xFF_FF_00_00);
        let transparent_black = continuity_render::params::Rgba::default();
        assert_eq!(rgba_to_argb(transparent_black), 0x00_00_00_00);
    }

    #[test]
    fn blend_argb_lerps_alpha_channel_too() {
        let a = 0x00_00_00_00;
        let b = 0xFF_FF_FF_FF;
        assert_eq!(blend_argb(a, b, 0.0), a);
        assert_eq!(blend_argb(a, b, 1.0), b);
        let mid = blend_argb(a, b, 0.5);
        let chans = [
            ((mid >> 24) & 0xFF) as u8,
            ((mid >> 16) & 0xFF) as u8,
            ((mid >> 8) & 0xFF) as u8,
            (mid & 0xFF) as u8,
        ];
        for c in chans {
            assert!(
                (127..=128).contains(&c),
                "mid channel {c} should be ~half full"
            );
        }
    }
}
