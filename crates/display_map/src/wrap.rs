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
