//! Per-segment style baked into the layout once at build time.
//!
//! The renderer maps each `SpanStyle` to DirectWrite attributes (weight,
//! italic, strikethrough, size scale) and to a theme colour via
//! [`SpanRole`]. Per spec §9 the display-map builder records all style up
//! front so the renderer needs no second pass.

/// Style attributes applied to one `Visible` or `Replace` segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SpanStyle {
    /// `DWRITE_FONT_WEIGHT_BOLD` when `true`, normal otherwise.
    pub bold: bool,
    /// `DWRITE_FONT_STYLE_ITALIC` when `true`, normal otherwise.
    pub italic: bool,
    /// `IDWriteTextLayout::SetStrikethrough(true, …)` when `true`.
    pub strikethrough: bool,
    /// `IDWriteTextLayout::SetUnderline(true, …)` when `true`.
    pub underline: bool,
    /// Multiplier on the *body* font size in DIPs. `1.0` = base. Heading
    /// runs get the per-level value from the theme's heading scale.
    pub font_scale: FontScale,
    /// Theme role; resolves to a colour at paint time.
    pub role: SpanRole,
}

impl SpanStyle {
    /// The plain-body default — no decoration, body font size.
    #[must_use]
    pub const fn body() -> Self {
        Self {
            bold: false,
            italic: false,
            strikethrough: false,
            underline: false,
            font_scale: FontScale::ONE,
            role: SpanRole::Body,
        }
    }

    /// Strong (bold) inline run.
    #[must_use]
    pub const fn strong() -> Self {
        Self {
            bold: true,
            ..Self::body()
        }
    }

    /// Emphasis (italic) inline run.
    #[must_use]
    pub const fn emphasis() -> Self {
        Self {
            italic: true,
            ..Self::body()
        }
    }

    /// Strike inline run.
    #[must_use]
    pub const fn strike() -> Self {
        Self {
            strikethrough: true,
            ..Self::body()
        }
    }

    /// Inline code run (background panel painted by the renderer).
    #[must_use]
    pub const fn code() -> Self {
        Self {
            role: SpanRole::Code,
            ..Self::body()
        }
    }

    /// Link visible text run.
    #[must_use]
    pub const fn link() -> Self {
        Self {
            role: SpanRole::Link,
            ..Self::body()
        }
    }

    /// Footnote reference / definition label run.
    #[must_use]
    pub const fn footnote() -> Self {
        Self {
            font_scale: FontScale::SUPERSCRIPT,
            role: SpanRole::Footnote,
            ..Self::body()
        }
    }

    /// Marker run (heading hashes, fence ticks, etc. *when revealed*).
    #[must_use]
    pub const fn marker() -> Self {
        Self {
            role: SpanRole::Marker,
            ..Self::body()
        }
    }

    /// Bullet-glyph replacement (e.g. `•`).
    #[must_use]
    pub const fn bullet() -> Self {
        Self {
            role: SpanRole::Bullet,
            ..Self::body()
        }
    }

    /// Checkbox-glyph replacement (e.g. `☐` / `☑`).
    #[must_use]
    pub const fn checkbox() -> Self {
        Self {
            role: SpanRole::Checkbox,
            ..Self::body()
        }
    }

    /// Image-label replacement.
    #[must_use]
    pub const fn image_label() -> Self {
        Self {
            italic: true,
            role: SpanRole::ImageLabel,
            ..Self::body()
        }
    }

    /// Heading text run; bold and scaled.
    #[must_use]
    pub const fn heading(level: u8) -> Self {
        Self {
            bold: true,
            font_scale: FontScale::HEADING,
            role: SpanRole::Heading(level),
            ..Self::body()
        }
    }

    /// Heading text while the source markers are revealed: keep the heading
    /// role and weight, but use body-size glyphs so edit mode matches source
    /// text metrics.
    #[must_use]
    pub const fn heading_revealed(level: u8) -> Self {
        Self {
            bold: true,
            role: SpanRole::Heading(level),
            ..Self::body()
        }
    }

    /// Combine `self` with the additive attributes of `other` (logical OR
    /// for booleans). Used by the builder when an inline emphasis overlaps
    /// a block heading.
    #[must_use]
    pub fn merge(mut self, other: SpanStyle) -> Self {
        self.bold |= other.bold;
        self.italic |= other.italic;
        self.strikethrough |= other.strikethrough;
        self.underline |= other.underline;
        // Role: a more-specific role wins over `Body`; otherwise keep the
        // existing role. Roles are mutually distinct from each other in
        // practice (e.g. you don't get heading-and-code in the same span).
        if matches!(self.role, SpanRole::Body) {
            self.role = other.role;
        }
        // Font scale: take the maximum (a heading inside something else
        // should still scale up).
        if other.font_scale.0 > self.font_scale.0 {
            self.font_scale = other.font_scale;
        }
        self
    }
}

impl Default for SpanStyle {
    fn default() -> Self {
        Self::body()
    }
}

/// Font-size multiplier on the body font. Encoded as fixed-point so the
/// type is `Hash + Eq`.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct FontScale(pub u16);

impl FontScale {
    /// `1.0` (body size).
    pub(crate) const ONE: FontScale = FontScale(1000);
    /// Heading scale placeholder; the renderer picks the actual per-level
    /// scale from the theme. The display map only flags the run as
    /// heading-sized.
    pub(crate) const HEADING: FontScale = FontScale(1420);
    /// Superscript footnote-reference scale.
    pub(crate) const SUPERSCRIPT: FontScale = FontScale(700);

    /// Build a [`FontScale`] from a float.
    #[must_use]
    pub fn from_f32(v: f32) -> Self {
        let clamped = v.clamp(0.1, 10.0);
        Self((clamped * 1000.0).round() as u16)
    }

    /// Convert back to a float multiplier.
    #[must_use]
    pub fn as_f32(self) -> f32 {
        f32::from(self.0) / 1000.0
    }
}

impl Default for FontScale {
    fn default() -> Self {
        Self::ONE
    }
}

/// Theme role of a styled run. The renderer maps each role to a colour
/// drawn from the active theme; the display-map crate stays theme-agnostic.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub enum SpanRole {
    /// Plain body text.
    #[default]
    Body,
    /// Heading text. The `u8` is the heading level (1..=6).
    Heading(u8),
    /// A structural marker still rendered (e.g. revealed `## ` hash).
    Marker,
    /// A bullet glyph replacement (`•`).
    Bullet,
    /// Code span / fenced code block contents.
    Code,
    /// Hyperlink text.
    Link,
    /// Footnote reference / definition label.
    Footnote,
    /// Image-reference label.
    ImageLabel,
    /// Checkbox glyph (`☐` / `☑`).
    Checkbox,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_defaults_are_clean() {
        let s = SpanStyle::body();
        assert!(!s.bold);
        assert!(!s.italic);
        assert!(!s.strikethrough);
        assert!(!s.underline);
        assert_eq!(s.font_scale, FontScale::ONE);
        assert_eq!(s.role, SpanRole::Body);
    }

    #[test]
    fn merge_or_combines_attributes_and_promotes_role() {
        let a = SpanStyle::strong();
        let b = SpanStyle::link();
        let merged = a.merge(b);
        assert!(merged.bold);
        assert_eq!(merged.role, SpanRole::Link);
    }

    #[test]
    fn merge_keeps_existing_role_when_other_is_body() {
        let a = SpanStyle::heading(2);
        let b = SpanStyle::strong();
        let merged = a.merge(b);
        assert_eq!(merged.role, SpanRole::Heading(2));
        assert!(merged.bold);
        assert!(merged.font_scale >= FontScale::HEADING);
    }

    #[test]
    fn heading_revealed_keeps_role_without_scaling() {
        let s = SpanStyle::heading_revealed(2);
        assert_eq!(s.role, SpanRole::Heading(2));
        assert!(s.bold);
        assert_eq!(s.font_scale, FontScale::ONE);
    }

    #[test]
    fn footnote_style_is_smaller_and_role_tagged() {
        let s = SpanStyle::footnote();
        assert_eq!(s.role, SpanRole::Footnote);
        assert!(s.font_scale < FontScale::ONE);
    }

    #[test]
    fn font_scale_roundtrips_within_quantization() {
        let s = FontScale::from_f32(1.42);
        let diff = (s.as_f32() - 1.42).abs();
        assert!(diff < 0.001, "got {}", s.as_f32());
    }

    #[test]
    fn font_scale_clamps_extreme_values() {
        assert_eq!(FontScale::from_f32(-1.0).as_f32(), 0.1);
        assert_eq!(FontScale::from_f32(100.0).as_f32(), 10.0);
    }
}
