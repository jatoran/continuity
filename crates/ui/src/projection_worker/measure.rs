// ε.5 ships the worker foundation only; until the integration slice
// wires `Window::on_paint` to dispatch + validate worker results,
// these types read "never used".
#![allow(dead_code)]
//! Worker-side measurement backend.
//!
//! [`MeasureMode`] picks DirectWrite (production, matches the renderer)
//! or fixed-width (deterministic test fallback). [`SendCom`] wraps the
//! immutable `IDWriteFactory` / `IDWriteTextFormat` COM handles so they
//! can cross thread boundaries; per-build `IDWriteTextLayout` objects
//! are created and dropped on the worker thread without crossing back.

use std::sync::Arc;

use continuity_display_map::wrap::{FixedCharWidth, WidthMeasure};
use continuity_layout::{FontStateId, RunCache};
use continuity_render::DirectWriteWidthMeasure;
use windows::Win32::Graphics::DirectWrite::IDWriteFactory;

use super::schema::WorkerFontMetrics;

/// Measurement backend the worker uses to build the projection.
///
/// `DirectWrite` is the production path (matches the renderer's glyph
/// advances). `FixedCharWidth` is the deterministic fallback used in
/// unit tests so they don't depend on system fonts.
pub(crate) enum MeasureMode {
    /// Production: DirectWrite-backed measurer. Font size + text format
    /// are NOT baked here — they arrive per request via
    /// [`WorkerFontMetrics`], so a font-family / font-size change is
    /// reflected on the next build without respawning the worker (RC1).
    DirectWrite {
        /// Shared factory (thread-safe per DirectWrite docs).
        factory: SendCom<IDWriteFactory>,
        /// Shared row-count run cache.
        run_cache: Arc<RunCache>,
        /// DirectWrite locale.
        locale: &'static str,
    },
    /// Tests/fallback: fixed-width measurer.
    Fixed,
}

impl MeasureMode {
    /// Build a `Box<dyn WidthMeasure>` for one projection build. The
    /// returned box is dropped after the build completes — measurer
    /// caches do not survive across requests. The lifetime is tied to
    /// both `&self` (factory / run cache) and `font_metrics` (the
    /// per-request text format the measurer borrows).
    ///
    /// In `DirectWrite` mode the measurement uses the request's live
    /// `font_metrics` (format + size + heading scale), not values baked
    /// at spawn — this is the RC1 fix for stale-font soft-wrap overflow.
    /// A request without a format falls back to the fixed-width scalar.
    pub(super) fn build_measure<'a>(
        &'a self,
        font_metrics: &'a WorkerFontMetrics,
        fallback_char_width_dip: f32,
        font_state: FontStateId,
    ) -> Box<dyn WidthMeasure + 'a> {
        match self {
            Self::DirectWrite {
                factory,
                run_cache,
                locale,
            } => match font_metrics.format.as_ref() {
                Some(format) => Box::new(DirectWriteWidthMeasure::new_with_run_cache(
                    factory.as_ref(),
                    format.as_ref(),
                    font_metrics.font_size_dip,
                    font_metrics.heading_scale,
                    fallback_char_width_dip,
                    Some(Arc::clone(run_cache)),
                    font_state,
                    locale,
                )),
                None => Box::new(FixedCharWidth::new(fallback_char_width_dip.max(1.0))),
            },
            Self::Fixed => Box::new(FixedCharWidth::new(fallback_char_width_dip.max(1.0))),
        }
    }
}

/// Send-safe wrapper around a COM interface handle.
///
/// # Safety
///
/// Wrapping `IDWriteFactory` and `IDWriteTextFormat` in `Send + Sync`
/// is sound because Microsoft documents both as thread-safe (see
/// <https://learn.microsoft.com/en-us/windows/win32/directwrite/multi-threading>):
/// the factory is "fully thread-safe" and immutable text-format objects
/// can be shared freely. This wrapper is **not** safe for mutable COM
/// interfaces like `IDWriteTextLayout` (which the layout cache
/// explicitly notes are non-`Send`).
#[derive(Clone)]
pub(crate) struct SendCom<T>(T);

impl<T> SendCom<T> {
    /// Wrap a COM handle. Caller asserts thread-safety of `T`.
    ///
    /// # Safety
    ///
    /// `T` must be a COM interface whose Microsoft documentation
    /// guarantees thread-safety. Currently only used with
    /// `IDWriteFactory` and `IDWriteTextFormat`.
    pub(crate) unsafe fn new(handle: T) -> Self {
        Self(handle)
    }

    /// Borrow the wrapped handle.
    pub(crate) fn as_ref(&self) -> &T {
        &self.0
    }
}

// SAFETY: see [`SendCom`] safety docs.
unsafe impl<T> Send for SendCom<T> {}
// SAFETY: see [`SendCom`] safety docs.
unsafe impl<T> Sync for SendCom<T> {}
