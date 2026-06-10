//! Soft-wrap configuration + width-measurement trait.
//!
//! The display map is built on a worker thread that doesn't own
//! `IDWriteTextLayout`, so the builder receives a [`WidthMeasure`] callback
//! that returns the rendered width of a candidate display string in DIPs.
//! Tests pass a deterministic char-count measurer; production code passes
//! a DirectWrite-backed one.

use crate::style::SpanStyle;

/// Attribution for a cached measurement lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeasureCacheStatus {
    /// The measurement came from a shared cache entry.
    Hit,
    /// The measurement was computed and inserted into a shared cache.
    Miss,
    /// The measurer has no shared run cache for this call.
    Bypassed,
}

/// Width measurement with cache-attribution metadata.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeasuredAdvance {
    /// Rendered width in DIPs.
    pub width_dip: f32,
    /// Cache status for this measurement.
    pub cache_status: MeasureCacheStatus,
}

/// Soft-wrap configuration. `width_dip == 0` disables wrap (the renderer
/// builds one display line per source line, regardless of measured width).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Default, Hash)]
pub struct WrapConfig {
    /// Soft-wrap width in DIPs.
    pub width_dip: u32,
}

impl WrapConfig {
    /// Wrap-disabled configuration.
    pub const NONE: WrapConfig = WrapConfig { width_dip: 0 };

    /// Build from a DIP width. `0` disables wrap.
    #[must_use]
    pub const fn new(width_dip: u32) -> Self {
        Self { width_dip }
    }

    /// `true` if soft-wrap is active.
    #[must_use]
    pub const fn enabled(self) -> bool {
        self.width_dip > 0
    }
}

/// Width-measurement callback. The builder calls `measure` once per
/// candidate display fragment when soft-wrap is considering a break.
pub trait WidthMeasure {
    /// Return the rendered width of `text` in DIPs under `style`.
    ///
    /// Implementations must return a finite, non-negative value. Returning
    /// `NaN` or a negative value surfaces as [`crate::Error::BadMeasurement`].
    fn measure(&mut self, text: &str, style: &SpanStyle) -> f32;

    /// Return the rendered width with optional shared-cache attribution.
    ///
    /// The default implementation delegates to [`Self::measure`]. UI and
    /// projection-worker DirectWrite measurers override this so the
    /// row-count walker can count run-cache hits and misses without knowing
    /// about the layout crate.
    fn measure_cached(
        &mut self,
        _content_stamp: u64,
        text: &str,
        style: &SpanStyle,
    ) -> MeasuredAdvance {
        MeasuredAdvance {
            width_dip: self.measure(text, style),
            cache_status: MeasureCacheStatus::Bypassed,
        }
    }

    /// Upper bound on the rendered advance of one byte under `style`,
    /// in DIPs. Used by the soft-wrap row-count walker to skip the
    /// per-segment `measure` call when a line is **trivially short**:
    /// when `total_byte_count * max_byte_advance(style) ≤ wrap_width`,
    /// the line obviously fits in one display row and no measurement
    /// is necessary.
    ///
    /// On a 9 k-line markdown buffer most lines are short ASCII and
    /// hit this fast path — without it, the walker created an
    /// `IDWriteTextLayout` per segment per line and the cheap
    /// row-count walker cost ~450 ms in release builds (see
    /// `perf-snapshots/manual-lag_after-coalesce_20260517-235814.tsv`).
    ///
    /// Returning [`f32::INFINITY`] disables the fast path (the
    /// default — safe for any measurer whose advance is not bounded
    /// by a known scalar).
    fn max_byte_advance(&self, _style: &SpanStyle) -> f32 {
        f32::INFINITY
    }
}

/// Largest fraction of the wrap width the hanging indent of a wrap
/// continuation row may consume when budgeting. Mirrored by the
/// painter's indent clamp so a deeply indented line keeps at least a
/// quarter of the column for content and the painted right edge stays
/// inside the text column.
pub const MAX_HANG_INDENT_FRACTION: f32 = 0.75;

/// Rendered column count of a leading markdown list marker (`- ` /
/// `* ` / `+ ` render as the two-column `• ` glyph; `12. ` / `3) `
/// render their literal text), or `0` when `after_indent` is not a
/// list item. Shared with the renderer's hanging-indent math — the
/// painter and the soft-wrap budget must agree on the marker width.
#[must_use]
pub fn list_marker_display_columns(after_indent: &str) -> usize {
    let bytes = after_indent.as_bytes();
    match bytes.first() {
        Some(b'-' | b'*' | b'+') if bytes.get(1) == Some(&b' ') => 2,
        Some(b'0'..=b'9') => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            match bytes.get(digits) {
                Some(b'.' | b')') if bytes.get(digits + 1) == Some(&b' ') => digits + 2,
                _ => 0,
            }
        }
        _ => 0,
    }
}

/// Hanging indent (DIPs) the painter applies to soft-wrap continuation
/// rows of `line_text`: leading spaces and tabs at their rendered
/// advances plus the rendered list-marker width. Uses the same
/// per-char formula as the renderer's
/// `FrameDisplay::hanging_indent_advance_dip` — `measure(" ")` is the
/// painter's `column_advance` and `measure("\t")` is the resolved
/// incremental tab stop, so budget and paint offset stay in lockstep.
///
/// Returns `0.0` without measuring when the line has no leading
/// whitespace and no list marker (the common case).
pub fn hanging_indent_dip(line_text: &str, measure: &mut dyn WidthMeasure) -> f32 {
    let bytes = line_text.as_bytes();
    let mut leading_spaces = 0usize;
    let mut leading_tabs = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b' ' => leading_spaces += 1,
            b'\t' => leading_tabs += 1,
            _ => break,
        }
        idx += 1;
    }
    let marker_columns = list_marker_display_columns(&line_text[idx..]);
    if leading_spaces == 0 && leading_tabs == 0 && marker_columns == 0 {
        return 0.0;
    }
    let body = SpanStyle::body();
    let space_advance = measure.measure(" ", &body);
    let tab_advance = if leading_tabs > 0 {
        measure.measure("\t", &body)
    } else {
        0.0
    };
    (leading_spaces + marker_columns) as f32 * space_advance + leading_tabs as f32 * tab_advance
}

/// Soft-wrap budget for wrap *continuation* rows of a line whose
/// hanging indent is `hang_indent_dip`: the painter shifts those rows
/// right by the indent, so their usable width is the wrap column minus
/// the indent — floored at `1 - MAX_HANG_INDENT_FRACTION` of the
/// column so a pathologically deep indent cannot collapse the budget
/// to zero. The first row of a line always budgets the full wrap
/// width.
#[must_use]
pub fn continuation_wrap_budget_dip(max_width_dip: f32, hang_indent_dip: f32) -> f32 {
    (max_width_dip - hang_indent_dip).max(max_width_dip * (1.0 - MAX_HANG_INDENT_FRACTION))
}

/// Deterministic measurer for tests / docs: every char counts as
/// `char_width_dip` DIPs regardless of style. Default `char_width_dip` is
/// `8.0`.
#[derive(Clone, Debug)]
pub struct FixedCharWidth {
    /// Width of one char, in DIPs.
    pub char_width_dip: f32,
}

impl FixedCharWidth {
    /// Construct with the given per-char width.
    #[must_use]
    pub fn new(char_width_dip: f32) -> Self {
        Self { char_width_dip }
    }
}

impl Default for FixedCharWidth {
    fn default() -> Self {
        Self {
            char_width_dip: 8.0,
        }
    }
}

impl WidthMeasure for FixedCharWidth {
    fn measure(&mut self, text: &str, style: &SpanStyle) -> f32 {
        // Honour the segment's font-size scale (heading runs are
        // 1.42× body, superscript runs are 0.70×) so the soft-wrap
        // measurer matches what DirectWrite actually paints. Without
        // this, headings overflow the wrap_width because the measurer
        // tallies them at body width while the painter renders them
        // ~40 % wider.
        text.chars().count() as f32 * self.char_width_dip * style.font_scale.as_f32()
    }

    fn max_byte_advance(&self, style: &SpanStyle) -> f32 {
        // One byte never exceeds one char of advance under this
        // measurer (multi-byte UTF-8 chars count once via
        // `chars().count()` in `measure`, so the per-byte upper
        // bound equals the per-char advance).
        self.char_width_dip * style.font_scale.as_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_disabled_when_width_is_zero() {
        assert!(!WrapConfig::NONE.enabled());
        assert!(WrapConfig::new(80).enabled());
    }

    #[test]
    fn fixed_char_width_counts_graphemes_as_chars() {
        let mut m = FixedCharWidth::new(10.0);
        assert_eq!(m.measure("abc", &SpanStyle::body()), 30.0);
        assert_eq!(m.measure("a•c", &SpanStyle::body()), 30.0);
        assert_eq!(m.measure("", &SpanStyle::body()), 0.0);
    }
}
