//! Phase 17.6 text-metric helpers used by the per-frame layout build.
//!
//! Holds the small DirectWrite-measurement utilities pulled out of
//! [`crate::text_helpers`] so that file stays under the 600-line cap.
//!
//! **Thread ownership**: UI thread (DirectWrite handles).
//!
//! The space/tab helpers intentionally do not retain a process-global
//! metric cache. DPI changes rebuild the window text format and the next
//! draw measures these advances again under the current render target DPI.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use ahash::AHasher;
use continuity_display_map::wrap::{
    FixedCharWidth, MeasureCacheStatus, MeasuredAdvance, WidthMeasure,
};
use continuity_display_map::{SpanRole, SpanStyle};
use continuity_layout::{FontStateId, RunCache, RunCacheKey};

use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_WEIGHT_BOLD, DWRITE_HIT_TEST_METRICS, DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE,
};

use crate::Error;

/// Measure the rendered advance of a single space glyph under `format`
/// (DIPs). Used by the renderer to align indent guides, ruler columns,
/// trailing-whitespace fills, and wrap-continuation indents to the
/// *actual* glyph width rather than the `font_size * 0.55` estimate.
/// Falls back to the estimate on DirectWrite failure.
pub fn measure_space_advance_dip(
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    base_font_size_dip: f32,
) -> f32 {
    let wide: [u16; 1] = [0x20];
    let layout = unsafe { factory.CreateTextLayout(&wide, format, f32::INFINITY, f32::INFINITY) };
    let Ok(layout) = layout else {
        return base_font_size_dip * 0.55;
    };
    let mut x = 0.0_f32;
    let mut y = 0.0_f32;
    let mut metrics = DWRITE_HIT_TEST_METRICS::default();
    let ok = unsafe {
        layout
            .HitTestTextPosition(1, false, &mut x, &mut y, &mut metrics)
            .ok()
    };
    if ok.is_none() || !x.is_finite() || x <= 0.0 {
        return base_font_size_dip * 0.55;
    }
    x
}

/// DirectWrite-backed display-map width measurer.
///
/// The display-map crate stays pure and renderer-agnostic, but the UI
/// paint/prewarm paths can use this adapter so soft-wrap decisions are
/// based on the same font family, size, weight, and heading scale that
/// `DrawTextLayout` later paints. A per-frame cache keeps repeated
/// grapheme measurements cheap.
pub struct DirectWriteWidthMeasure<'a> {
    factory: &'a IDWriteFactory,
    format: &'a IDWriteTextFormat,
    base_font_size_dip: f32,
    heading_scale: [f32; 6],
    fallback: FixedCharWidth,
    cache: HashMap<(String, SpanStyle), f32>,
    ascii_cache: HashMap<(u8, SpanStyle), f32>,
    run_cache: Option<Arc<RunCache>>,
    font_state: FontStateId,
    locale: Box<str>,
    cache_hits: u64,
    cache_misses: u64,
    layouts_created: u64,
}

/// Aggregate measure-cache counters captured per
/// [`DirectWriteWidthMeasure`] lifetime. Surfaced as
/// `event:dwrite_measure_cache` at the end of a row-count walker run.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct DirectWriteCacheStats {
    /// Calls to [`WidthMeasure::measure`] that hit either the ASCII or
    /// the string cache.
    pub hits: u64,
    /// Calls that missed and went through to `measure_uncached`.
    pub misses: u64,
    /// Successful `IDWriteFactory::CreateTextLayout` calls. Equal to
    /// `misses` minus any fallback-on-DirectWrite-failure paths.
    pub layouts_created: u64,
}

impl<'a> DirectWriteWidthMeasure<'a> {
    /// Create a DirectWrite measurer for one frame-display build.
    #[must_use]
    pub fn new(
        factory: &'a IDWriteFactory,
        format: &'a IDWriteTextFormat,
        base_font_size_dip: f32,
        heading_scale: [f32; 6],
        fallback_char_width_dip: f32,
    ) -> Self {
        Self::new_with_run_cache(
            factory,
            format,
            base_font_size_dip,
            heading_scale,
            fallback_char_width_dip,
            None,
            FontStateId::default(),
            "en-us",
        )
    }

    /// Create a DirectWrite measurer backed by a shared row-count run
    /// cache. Used by the projection worker and inline cold fallback so
    /// both paths warm the same measurement entries.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_run_cache(
        factory: &'a IDWriteFactory,
        format: &'a IDWriteTextFormat,
        base_font_size_dip: f32,
        heading_scale: [f32; 6],
        fallback_char_width_dip: f32,
        run_cache: Option<Arc<RunCache>>,
        font_state: FontStateId,
        locale: &str,
    ) -> Self {
        Self {
            factory,
            format,
            base_font_size_dip,
            heading_scale,
            fallback: FixedCharWidth::new(fallback_char_width_dip.max(1.0)),
            cache: HashMap::new(),
            ascii_cache: HashMap::new(),
            run_cache,
            font_state,
            locale: locale.into(),
            cache_hits: 0,
            cache_misses: 0,
            layouts_created: 0,
        }
    }

    /// Snapshot of the cache counters since construction. Read at the
    /// end of a row-count walker run and surfaced via the
    /// `event:dwrite_measure_cache` trace event.
    #[must_use]
    pub fn cache_stats(&self) -> DirectWriteCacheStats {
        DirectWriteCacheStats {
            hits: self.cache_hits,
            misses: self.cache_misses,
            layouts_created: self.layouts_created,
        }
    }

    fn measure_uncached(&mut self, text: &str, style: &SpanStyle) -> Option<f32> {
        if text.is_empty() {
            return Some(0.0);
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout = unsafe {
            self.factory
                .CreateTextLayout(&wide, self.format, f32::INFINITY, f32::INFINITY)
                .ok()?
        };
        self.layouts_created = self.layouts_created.saturating_add(1);
        apply_single_style(
            &layout,
            style,
            self.base_font_size_dip,
            self.heading_scale,
            wide.len() as u32,
        );
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            layout.GetMetrics(&mut metrics).ok()?;
        }
        let width = metrics.widthIncludingTrailingWhitespace;
        width.is_finite().then_some(width.max(0.0))
    }
}

impl WidthMeasure for DirectWriteWidthMeasure<'_> {
    fn measure(&mut self, text: &str, style: &SpanStyle) -> f32 {
        if text.len() == 1 && text.is_ascii() {
            let key = (text.as_bytes()[0], *style);
            if let Some(width) = self.ascii_cache.get(&key) {
                self.cache_hits = self.cache_hits.saturating_add(1);
                return *width;
            }
            self.cache_misses = self.cache_misses.saturating_add(1);
            let width = self
                .measure_uncached(text, style)
                .unwrap_or_else(|| self.fallback.measure(text, style));
            self.ascii_cache.insert(key, width);
            return width;
        }
        let key = (text.to_string(), *style);
        if let Some(width) = self.cache.get(&key) {
            self.cache_hits = self.cache_hits.saturating_add(1);
            return *width;
        }
        self.cache_misses = self.cache_misses.saturating_add(1);
        let width = self
            .measure_uncached(text, style)
            .unwrap_or_else(|| self.fallback.measure(text, style));
        self.cache.insert(key, width);
        width
    }

    fn measure_cached(
        &mut self,
        // The run-cache identity is the fragment text + font/locale/style;
        // the per-line stamp is deliberately not part of the key (see
        // `RunCacheKey` — keying on it re-measured every grapheme per line).
        _content_stamp: u64,
        text: &str,
        style: &SpanStyle,
    ) -> MeasuredAdvance {
        // Cold-walk graphemes are overwhelmingly single ASCII bytes. Serve
        // them from the per-instance `ascii_cache` (a small unsynchronized
        // map keyed by the byte) rather than the shared, shard-locked run
        // cache. The value is identical — both measure the byte in
        // isolation via `measure_uncached` — but this skips the run cache's
        // shard write-lock and multi-field key hash on every grapheme,
        // which is what dominated the walk after the per-line key fix.
        // Multi-char fragments still use the shared run cache so the
        // expensive measurements persist across builds and threads.
        if text.len() == 1 && text.is_ascii() {
            return MeasuredAdvance {
                width_dip: self.measure(text, style),
                cache_status: MeasureCacheStatus::Bypassed,
            };
        }
        let Some(run_cache) = self.run_cache.clone() else {
            return MeasuredAdvance {
                width_dip: self.measure(text, style),
                cache_status: MeasureCacheStatus::Bypassed,
            };
        };
        let key = RunCacheKey::new(
            self.font_state,
            &self.locale,
            text,
            compute_style_hash(style),
        );
        let lookup = run_cache.get_or_insert_with(key, text.len(), || {
            self.cache_misses = self.cache_misses.saturating_add(1);
            self.measure_uncached(text, style)
                .unwrap_or_else(|| self.fallback.measure(text, style))
        });
        if lookup.was_hit {
            self.cache_hits = self.cache_hits.saturating_add(1);
            MeasuredAdvance {
                width_dip: lookup.width_dip,
                cache_status: MeasureCacheStatus::Hit,
            }
        } else {
            MeasuredAdvance {
                width_dip: lookup.width_dip,
                cache_status: MeasureCacheStatus::Miss,
            }
        }
    }

    fn max_byte_advance(&self, style: &SpanStyle) -> f32 {
        // Upper bound on the advance of a single byte under this
        // measurer's font. We delegate to the `FixedCharWidth`
        // fallback that was built with `fallback_char_width_dip` —
        // the caller-provided per-char advance (`scaled_font_size *
        // 0.55` in the UI path, matching the existing
        // `projection_char_width` heuristic). That is a realistic
        // upper bound for ASCII in both Cascadia Mono (~0.55 em)
        // and Segoe UI Variable (~0.6 em); multi-byte UTF-8 chars
        // advance one glyph spread across 2-4 bytes, so per-byte
        // advance only shrinks. Heading runs scale via
        // `style.font_scale.as_f32()` (`FontScale::HEADING == 1.42`).
        //
        // The previous bound used the full em (`base_font_size_dip *
        // scale`) — ~2× too conservative for the trivial-fit fast
        // path in `count_soft_wrap_rows`. The manual-lag trace
        // captured `frame_display:cold_build 400+ ms` because most
        // markdown lines (80-160 bytes) failed the over-conservative
        // `byte_count * 14 dip ≤ wrap_width` test and fell through
        // to per-segment `IDWriteTextLayout` measure. The tighter
        // bound lets the same lines pass on byte length alone.
        self.fallback.max_byte_advance(style)
    }
}

fn compute_style_hash(style: &SpanStyle) -> u64 {
    let mut hasher = AHasher::default();
    style.hash(&mut hasher);
    hasher.finish()
}

fn apply_single_style(
    layout: &IDWriteTextLayout,
    style: &SpanStyle,
    base_font_size_dip: f32,
    heading_scale: [f32; 6],
    utf16_len: u32,
) {
    if utf16_len == 0 {
        return;
    }
    let range = DWRITE_TEXT_RANGE {
        startPosition: 0,
        length: utf16_len,
    };
    unsafe {
        if style.bold {
            let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range);
        }
        if style.italic {
            let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range);
        }
        if style.strikethrough {
            let _ = layout.SetStrikethrough(true, range);
        }
        if style.underline {
            let _ = layout.SetUnderline(true, range);
        }
        if let Some(scale) = style_font_scale(*style, heading_scale) {
            let _ = layout.SetFontSize(base_font_size_dip * scale, range);
        }
    }
}

fn style_font_scale(style: SpanStyle, heading_scale: [f32; 6]) -> Option<f32> {
    if style.font_scale.as_f32() == 1.0 {
        return None;
    }
    if let SpanRole::Heading(level) = style.role {
        let idx = usize::from(level.clamp(1, 6) - 1);
        return Some(heading_scale[idx]);
    }
    Some(style.font_scale.as_f32())
}

/// Measure the rendered advance of a single tab glyph under `format`
/// (DIPs). DirectWrite resolves `\t` to the format's default tab stop,
/// which depends on the font's design metrics — typically wider than
/// `indent_size * space_advance`. Returning the real tab advance lets
/// indent guides land under the actual first character of each line's
/// parent, even when the user mixes tabs and spaces.
///
/// Falls back to `4 * space_advance` on DirectWrite failure, matching
/// the editor's default `indent_size`.
pub(crate) fn measure_tab_advance_dip(
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    space_advance_dip: f32,
) -> f32 {
    let wide: [u16; 1] = [0x09];
    let layout = unsafe { factory.CreateTextLayout(&wide, format, f32::INFINITY, f32::INFINITY) };
    let Ok(layout) = layout else {
        return space_advance_dip * 4.0;
    };
    let mut x = 0.0_f32;
    let mut y = 0.0_f32;
    let mut metrics = DWRITE_HIT_TEST_METRICS::default();
    let ok = unsafe {
        layout
            .HitTestTextPosition(1, false, &mut x, &mut y, &mut metrics)
            .ok()
    };
    if ok.is_none() || !x.is_finite() || x <= 0.0 {
        return space_advance_dip * 4.0;
    }
    x
}

/// `CreateTextLayout` aborts the frame on a zero-length string, hiding
/// the caret on blank lines. Substitute one space when `text` is empty.
pub(crate) fn create_layout_with_empty_fallback(
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    max_layout_width: f32,
) -> Result<IDWriteTextLayout, Error> {
    let placeholder: [u16; 1] = [0x20];
    let wide_owned: Vec<u16>;
    let slice: &[u16] = if text.is_empty() {
        &placeholder
    } else {
        wide_owned = text.encode_utf16().collect();
        &wide_owned
    };
    Ok(unsafe { factory.CreateTextLayout(slice, format, max_layout_width, f32::INFINITY)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    use continuity_layout::DWriteFactory;
    use ropey::Rope;

    fn format() -> (DWriteFactory, IDWriteTextFormat) {
        let factory = DWriteFactory::new().expect("DirectWrite factory");
        let format = factory
            .text_format("Segoe UI", 14.0, "en-us")
            .expect("Segoe UI text format");
        (factory, format)
    }

    #[test]
    fn direct_write_width_measure_detects_proportional_glyphs() {
        let (factory, format) = format();
        let mut measure = DirectWriteWidthMeasure::new(
            factory.raw(),
            &format,
            14.0,
            crate::DEFAULT_HEADING_SCALE,
            8.0,
        );
        let narrow = measure.measure("iiiiiiii", &SpanStyle::body());
        let wide = measure.measure("mmmmmmmm", &SpanStyle::body());
        assert!(
            wide > narrow * 1.5,
            "Segoe UI should measure wide glyphs wider than narrow glyphs: narrow={narrow} wide={wide}",
        );
    }

    #[test]
    fn direct_write_width_measure_keeps_trailing_spaces() {
        let (factory, format) = format();
        let mut measure = DirectWriteWidthMeasure::new(
            factory.raw(),
            &format,
            14.0,
            crate::DEFAULT_HEADING_SCALE,
            8.0,
        );
        let trimmed = measure.measure("wrap", &SpanStyle::body());
        let spaced = measure.measure("wrap    ", &SpanStyle::body());
        assert!(
            spaced > trimmed,
            "trailing spaces must count for wrap width: trimmed={trimmed} spaced={spaced}",
        );
    }

    #[test]
    fn measured_frame_display_does_not_wrap_narrow_text_early() {
        let (factory, format) = format();
        let line = "i".repeat(120);
        let rope = Rope::from_str(&line);
        let mut width_measure = DirectWriteWidthMeasure::new(
            factory.raw(),
            &format,
            14.0,
            crate::DEFAULT_HEADING_SCALE,
            8.0,
        );
        let wrap_width = width_measure.measure(&line, &SpanStyle::body()).ceil() as u32;

        let mut build_measure = DirectWriteWidthMeasure::new(
            factory.raw(),
            &format,
            14.0,
            crate::DEFAULT_HEADING_SCALE,
            8.0,
        );
        let measured = crate::display_projection::FrameDisplay::build_with_options_measured(
            &rope,
            1,
            None,
            &[],
            &[],
            &[],
            wrap_width,
            &mut build_measure,
        );
        assert_eq!(measured.display_line_count_for_source(0), 1);

        let fixed =
            crate::display_projection::FrameDisplay::build(&rope, 1, None, &[], wrap_width, 8.0);
        assert!(
            fixed.display_line_count_for_source(0) > 1,
            "fixed-width scalar reproduces the early-wrap failure this test guards",
        );
    }
}
