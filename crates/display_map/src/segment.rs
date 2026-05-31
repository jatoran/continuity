//! The atomic unit of a display line.
//!
//! Every byte of a source line falls into exactly one segment, in source
//! order. `Visible` segments contribute their literal source bytes to the
//! display string. `Hidden` segments contribute nothing (markers, fence
//! ticks, fold-collapsed ranges). `Replace` segments contribute the
//! pre-baked `display` string and remember their original source range for
//! caret round-tripping.

use std::ops::Range;

use crate::id::SourceByte;
use crate::style::SpanStyle;

/// Hit-test metadata carried by a `Replace` segment.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub enum SegmentHit {
    /// No interactive hit-target on this segment.
    #[default]
    None,
    /// Checkbox: clicking the segment toggles the source byte at `toggle`.
    Checkbox {
        /// The single source byte to flip between `' '` (unchecked) and
        /// `'x'` (checked).
        toggle: SourceByte,
        /// Current checked state.
        checked: bool,
    },
    /// Link: ctrl/cmd-clicking the segment opens `url` (the source byte
    /// range covering the URL inside `[…](url)`).
    Link {
        /// Source byte range covering the link's URL.
        url: Range<SourceByte>,
    },
    /// Footnote reference: Ctrl+click jumps to the matching definition.
    FootnoteReference {
        /// Footnote label without delimiters.
        label: String,
        /// Definition label source range, when one exists.
        definition: Option<Range<SourceByte>>,
    },
    /// Footnote definition label: Ctrl+click jumps to the first reference.
    FootnoteDefinition {
        /// Footnote label without delimiters.
        label: String,
        /// First body reference source range, when one exists.
        first_reference: Option<Range<SourceByte>>,
    },
}

/// One atomic piece of a display line.
#[derive(Clone, Debug)]
pub enum DisplaySegment {
    /// Source bytes are visible verbatim, styled per `style`.
    Visible {
        /// Source byte range contributing to display.
        source: Range<SourceByte>,
        /// Style applied at layout-build time.
        style: SpanStyle,
        /// Hit-test metadata; usually `None`, but a revealed link's text
        /// run may carry `Link`.
        hit: SegmentHit,
    },
    /// Source bytes are hidden from the layout — the renderer never sees
    /// them. The `source` range is preserved so caret motion can step
    /// over the hidden bytes (the caret cannot land *inside* a Hidden
    /// segment except at its start or end).
    Hidden {
        /// Hidden source byte range.
        source: Range<SourceByte>,
    },
    /// A pre-baked display string replaces the source bytes.
    Replace {
        /// Source byte range whose visible projection is `display`.
        source: Range<SourceByte>,
        /// The display string contributing to the layout.
        display: Box<str>,
        /// Style applied to the replacement glyphs.
        style: SpanStyle,
        /// Hit-test metadata, if any.
        hit: SegmentHit,
    },
}

impl DisplaySegment {
    /// Source range this segment covers.
    #[must_use]
    pub fn source_range(&self) -> Range<SourceByte> {
        match self {
            DisplaySegment::Visible { source, .. }
            | DisplaySegment::Hidden { source }
            | DisplaySegment::Replace { source, .. } => source.clone(),
        }
    }

    /// Number of source bytes covered.
    #[must_use]
    pub fn source_len(&self) -> u32 {
        let r = self.source_range();
        r.end.raw().saturating_sub(r.start.raw())
    }

    /// Bytes this segment contributes to the display string.
    #[must_use]
    pub(crate) fn display_bytes<'a>(
        &'a self,
        source_line: &'a str,
        line_start_source: SourceByte,
    ) -> &'a str {
        match self {
            DisplaySegment::Visible { source, .. } => {
                let start = source.start.raw().saturating_sub(line_start_source.raw()) as usize;
                let end = source.end.raw().saturating_sub(line_start_source.raw()) as usize;
                let end = end.min(source_line.len());
                let start = start.min(end);
                &source_line[start..end]
            }
            DisplaySegment::Hidden { .. } => "",
            DisplaySegment::Replace { display, .. } => display.as_ref(),
        }
    }

    /// Style of the segment, if any. `Hidden` returns `None`.
    #[must_use]
    pub fn style(&self) -> Option<&SpanStyle> {
        match self {
            DisplaySegment::Visible { style, .. } | DisplaySegment::Replace { style, .. } => {
                Some(style)
            }
            DisplaySegment::Hidden { .. } => None,
        }
    }

    /// Hit-test metadata of the segment, if any.
    #[must_use]
    pub fn hit(&self) -> Option<&SegmentHit> {
        match self {
            DisplaySegment::Visible { hit, .. } | DisplaySegment::Replace { hit, .. } => Some(hit),
            DisplaySegment::Hidden { .. } => None,
        }
    }

    /// `true` if the segment contributes zero bytes to the display string.
    #[must_use]
    pub fn is_zero_width(&self) -> bool {
        match self {
            DisplaySegment::Visible { source, .. } => source.start == source.end,
            DisplaySegment::Hidden { .. } => true,
            DisplaySegment::Replace { display, .. } => display.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::SpanStyle;

    fn visible(src: Range<u32>, style: SpanStyle) -> DisplaySegment {
        DisplaySegment::Visible {
            source: SourceByte(src.start)..SourceByte(src.end),
            style,
            hit: SegmentHit::None,
        }
    }

    #[test]
    fn visible_segment_yields_source_substring() {
        let line = "hello world";
        let s = visible(0..5, SpanStyle::body());
        assert_eq!(s.display_bytes(line, SourceByte(0)), "hello");
        assert_eq!(s.source_len(), 5);
    }

    #[test]
    fn visible_segment_handles_line_offset() {
        let line = "world";
        let s = visible(6..11, SpanStyle::body());
        assert_eq!(s.display_bytes(line, SourceByte(6)), "world");
    }

    #[test]
    fn hidden_segment_yields_empty_string() {
        let s = DisplaySegment::Hidden {
            source: SourceByte(0)..SourceByte(2),
        };
        assert_eq!(s.display_bytes("", SourceByte(0)), "");
        assert!(s.is_zero_width());
        assert!(s.style().is_none());
    }

    #[test]
    fn replace_segment_yields_pre_baked_display() {
        let s = DisplaySegment::Replace {
            source: SourceByte(0)..SourceByte(2),
            display: "• ".into(),
            style: SpanStyle::bullet(),
            hit: SegmentHit::None,
        };
        assert_eq!(s.display_bytes("- ", SourceByte(0)), "• ");
        assert_eq!(s.source_len(), 2);
        assert!(!s.is_zero_width());
    }

    #[test]
    fn checkbox_hit_metadata_carries_toggle_byte() {
        let s = DisplaySegment::Replace {
            source: SourceByte(0)..SourceByte(4),
            display: "☐ ".into(),
            style: SpanStyle::checkbox(),
            hit: SegmentHit::Checkbox {
                toggle: SourceByte(1),
                checked: false,
            },
        };
        match s.hit() {
            Some(SegmentHit::Checkbox { toggle, checked }) => {
                assert_eq!(toggle.raw(), 1);
                assert!(!checked);
            }
            other => panic!("unexpected hit: {other:?}"),
        }
    }

    #[test]
    fn footnote_hit_metadata_carries_target() {
        let hit = SegmentHit::FootnoteReference {
            label: "1".into(),
            definition: Some(SourceByte(10)..SourceByte(14)),
        };
        match hit {
            SegmentHit::FootnoteReference { label, definition } => {
                assert_eq!(label, "1");
                assert_eq!(definition.unwrap().start.raw(), 10);
            }
            other => panic!("unexpected hit: {other:?}"),
        }
    }
}
