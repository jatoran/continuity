//! Wrapper around `IDWriteFactory` that builds `IDWriteTextLayout`s.

use windows::core::HSTRING;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
};

use crate::Error;

/// Owned `IDWriteFactory` (shared mode).
pub struct DWriteFactory(IDWriteFactory);

impl DWriteFactory {
    /// Borrow the underlying `IDWriteFactory` — used by callers that want
    /// to build a one-shot layout outside the cache (e.g. UI hit-testing).
    #[must_use]
    pub fn raw(&self) -> &IDWriteFactory {
        &self.0
    }

    /// Construct a shared factory.
    ///
    /// # Errors
    ///
    /// Returns the wrapped `windows::core::Error` if the factory cannot be
    /// created (typically because the system lacks DirectWrite, which on
    /// Win10 should never happen).
    pub fn new() -> Result<Self, Error> {
        let factory: IDWriteFactory =
            unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)? };
        Ok(Self(factory))
    }

    /// Build an `IDWriteTextFormat` describing a font + size.
    pub fn text_format(
        &self,
        family: &str,
        size_dip: f32,
        locale: &str,
    ) -> Result<IDWriteTextFormat, Error> {
        let fmt = unsafe {
            self.0.CreateTextFormat(
                &HSTRING::from(family),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size_dip,
                &HSTRING::from(locale),
            )?
        };
        Ok(fmt)
    }

    /// Enumerate the system font collection, returning every available
    /// font family name in the locale (or first available locale) order.
    /// Names are deduplicated and sorted case-insensitively. Used by the
    /// font-picker palette (E3).
    ///
    /// # Errors
    ///
    /// Returns the wrapped `windows::core::Error` if DirectWrite fails to
    /// produce a system font collection or any family name. Empty
    /// collections (no fonts installed) return `Ok(vec![])` rather than
    /// erroring.
    pub fn system_font_families(&self) -> Result<Vec<String>, Error> {
        let mut collection: Option<IDWriteFontCollection> = None;
        unsafe { self.0.GetSystemFontCollection(&mut collection, false)? };
        let Some(collection) = collection else {
            return Ok(Vec::new());
        };
        let count = unsafe { collection.GetFontFamilyCount() };
        let mut out: Vec<String> = Vec::with_capacity(count as usize);
        for i in 0..count {
            let Ok(family) = (unsafe { collection.GetFontFamily(i) }) else {
                continue;
            };
            let Ok(names) = (unsafe { family.GetFamilyNames() }) else {
                continue;
            };
            // Pick the user's locale if available, else fall back to first.
            let mut idx = 0u32;
            let mut exists = windows::Win32::Foundation::BOOL(0);
            let locale = HSTRING::from("en-us");
            unsafe {
                let _ = names.FindLocaleName(&locale, &mut idx, &mut exists);
            }
            let chosen = if exists.as_bool() { idx } else { 0 };
            let len = match unsafe { names.GetStringLength(chosen) } {
                Ok(n) => n as usize,
                Err(_) => continue,
            };
            let mut buf = vec![0u16; len + 1];
            if unsafe { names.GetString(chosen, &mut buf) }.is_err() {
                continue;
            }
            // Trim the trailing NUL.
            if buf.last() == Some(&0) {
                buf.pop();
            }
            let name = String::from_utf16_lossy(&buf);
            if !name.is_empty() {
                out.push(name);
            }
        }
        out.sort_by_key(|a| a.to_ascii_lowercase());
        out.dedup();
        Ok(out)
    }

    /// Build an `IDWriteTextLayout` for `text` constrained to a max width/height.
    pub fn text_layout(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        max_width: f32,
        max_height: f32,
    ) -> Result<IDWriteTextLayout, Error> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout = unsafe {
            self.0
                .CreateTextLayout(&wide, format, max_width, max_height)?
        };
        Ok(layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS;

    #[test]
    fn factory_creates_text_layout() {
        let f = DWriteFactory::new().unwrap();
        let fmt = f.text_format("Segoe UI", 14.0, "en-us").unwrap();
        let layout = f
            .text_layout("Hello, world!", &fmt, 1024.0, 1024.0)
            .unwrap();
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            layout.GetMetrics(&mut metrics).unwrap();
        }
        assert!(metrics.width > 0.0);
        assert!(metrics.height > 0.0);
    }

    #[test]
    fn empty_string_layout_has_zero_width() {
        let f = DWriteFactory::new().unwrap();
        let fmt = f.text_format("Segoe UI", 14.0, "en-us").unwrap();
        let layout = f.text_layout("", &fmt, 1024.0, 1024.0).unwrap();
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            layout.GetMetrics(&mut metrics).unwrap();
        }
        assert_eq!(metrics.width, 0.0);
    }

    #[test]
    fn nonexistent_font_family_falls_back() {
        // DirectWrite returns a layout even for unknown families; it falls
        // back at draw time. So this should not error.
        let f = DWriteFactory::new().unwrap();
        let fmt = f
            .text_format("This Font Does Not Exist", 12.0, "en-us")
            .unwrap();
        let _layout = f.text_layout("hi", &fmt, 100.0, 100.0).unwrap();
    }
}
