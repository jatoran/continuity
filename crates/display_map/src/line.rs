//! One visible line of the display map.
//!
//! A [`DisplayLineSpec`] is a sorted list of [`DisplaySegment`]s plus
//! pre-computed byte tables that map between source and display offsets in
//! O(log n) per query. The renderer builds *exactly one*
//! `IDWriteTextLayout` per `DisplayLineSpec`; styles and hit metadata are
//! baked into the layout once at build time.

use std::ops::Range;

use crate::id::{DisplayByte, DisplayUtf16, SourceByte, SourceLine};
use crate::segment::DisplaySegment;
use crate::style::SpanStyle;

mod tables;

use tables::{
    assert_sorted_and_contiguous, build_display_byte_to_utf16, build_display_text,
    build_display_to_source, build_source_to_display, compute_content_stamp,
};

/// One visible line of the display map.
#[derive(Clone, Debug)]
pub struct DisplayLineSpec {
    /// Source line index this display line belongs to. When soft-wrap
    /// fires, multiple display lines share a `source_line`. When a fold
    /// collapses many source lines into one display line, this is the
    /// *first* source line of the collapsed range.
    pub source_line: SourceLine,
    /// First source byte represented by this display line. For
    /// soft-wrapped lines this is the wrap-break point inside the source
    /// line. For folded lines this is the start of the fold.
    pub source_byte_start: SourceByte,
    /// Last source byte (exclusive) represented by this display line.
    pub source_byte_end: SourceByte,
    /// `true` if this is a wrap-continuation of the preceding display
    /// line (no caret leading indent needed by the renderer).
    pub is_wrap_continuation: bool,
    /// Sorted, non-overlapping segments. Concatenating their display
    /// strings yields the line's display text.
    pub segments: Vec<DisplaySegment>,
    /// Pre-built display string. Empty for blank lines.
    pub display_text: Box<str>,
    /// `source_byte → display_byte` table. Length = source span + 1; the
    /// last entry is `display_text.len()`.
    source_to_display: Vec<u32>,
    /// `display_byte → source_byte` table. Length = display_text.len() + 1.
    display_to_source: Vec<u32>,
    /// `display_byte → display_utf16` prefix table. Length = display_text.len() + 1.
    display_byte_to_utf16: Vec<u32>,
    /// Stamp of the display text — keyed by the renderer's layout cache.
    content_stamp: u64,
}

impl DisplayLineSpec {
    /// Build a `DisplayLineSpec` from a list of segments.
    ///
    /// # Panics
    ///
    /// Panics if `segments` is not sorted by source range or if the source
    /// ranges overlap.
    #[must_use]
    pub fn new(
        source_line: SourceLine,
        source_byte_start: SourceByte,
        source_byte_end: SourceByte,
        is_wrap_continuation: bool,
        segments: Vec<DisplaySegment>,
        source_line_text: &str,
    ) -> Self {
        assert_sorted_and_contiguous(&segments, source_byte_start, source_byte_end);
        let display_text = build_display_text(&segments, source_line_text, source_byte_start);
        let source_to_display = build_source_to_display(
            &segments,
            source_byte_start,
            source_byte_end,
            source_line_text,
        );
        let display_to_source =
            build_display_to_source(&segments, source_byte_start, source_line_text);
        let display_byte_to_utf16 = build_display_byte_to_utf16(&display_text);
        let content_stamp = compute_content_stamp(&display_text, &segments);
        Self {
            source_line,
            source_byte_start,
            source_byte_end,
            is_wrap_continuation,
            segments,
            display_text: display_text.into_boxed_str(),
            source_to_display,
            display_to_source,
            display_byte_to_utf16,
            content_stamp,
        }
    }

    /// Pre-built display text for this line.
    #[must_use]
    pub fn display_text(&self) -> &str {
        &self.display_text
    }

    /// Iterator over `(display utf-16 range, style)` runs — pass directly
    /// to `IDWriteTextLayout` attribute setters.
    pub fn style_runs(&self) -> impl Iterator<Item = (Range<DisplayUtf16>, SpanStyle)> + '_ {
        let table = &self.display_byte_to_utf16;
        let last_idx = table.len().saturating_sub(1) as u32;
        let mut display_cursor: u32 = 0;
        self.segments.iter().filter_map(move |seg| {
            let len_bytes = match seg {
                DisplaySegment::Visible { source, .. } => source.end.raw() - source.start.raw(),
                DisplaySegment::Hidden { .. } => 0,
                DisplaySegment::Replace { display, .. } => display.len() as u32,
            };
            let start_b = display_cursor;
            let end_b = start_b + len_bytes;
            display_cursor = end_b;
            if len_bytes == 0 {
                return None;
            }
            let style = match seg {
                DisplaySegment::Visible { style, .. } | DisplaySegment::Replace { style, .. } => {
                    *style
                }
                DisplaySegment::Hidden { .. } => return None,
            };
            // Defensive clamp — `style_runs` is called from the paint
            // path, where stale decorations during undo replay can
            // make segment metadata briefly inconsistent with the
            // baked display string. Saturate at the last valid index
            // rather than panicking; the next paint pass after the
            // decoration worker catches up will be coherent.
            let start_b = start_b.min(last_idx);
            let end_b = end_b.min(last_idx);
            let utf16_start = table[start_b as usize];
            let utf16_end = table[end_b as usize];
            Some((DisplayUtf16(utf16_start)..DisplayUtf16(utf16_end), style))
        })
    }

    /// Translate a `source` byte to its `display` byte on this line.
    /// Returns `None` if the source byte falls outside this display line's
    /// covered source range, or maps into a `Hidden` segment.
    #[must_use]
    pub fn source_to_display(&self, source: SourceByte) -> Option<DisplayByte> {
        let span_start = self.source_byte_start.raw();
        let span_end = self.source_byte_end.raw();
        if source.raw() < span_start || source.raw() > span_end {
            return None;
        }
        let idx = (source.raw() - span_start) as usize;
        let d = *self.source_to_display.get(idx)?;
        if d == u32::MAX {
            None
        } else {
            Some(DisplayByte(d))
        }
    }

    /// Translate a `display` byte back to a source byte. Always succeeds
    /// for valid display offsets (0..=display_text.len()); returned source
    /// byte is the *first* source byte mapped to that display position.
    #[must_use]
    pub fn display_to_source(&self, display: DisplayByte) -> Option<SourceByte> {
        let idx = display.raw() as usize;
        let s = *self.display_to_source.get(idx)?;
        Some(SourceByte(s))
    }

    /// UTF-16 prefix length of `display`'s byte offset on this line.
    #[must_use]
    pub fn display_byte_to_utf16(&self, display: DisplayByte) -> Option<DisplayUtf16> {
        self.display_byte_to_utf16
            .get(display.raw() as usize)
            .map(|n| DisplayUtf16(*n))
    }

    /// Stable hash of the display content; the renderer's layout cache
    /// keys on this so caret-only frames don't rebuild any layout.
    #[must_use]
    pub fn content_stamp(&self) -> u64 {
        self.content_stamp
    }

    /// Source byte range this display line covers (start inclusive,
    /// end exclusive).
    #[must_use]
    pub fn source_range(&self) -> Range<SourceByte> {
        self.source_byte_start..self.source_byte_end
    }

    /// Display text length in bytes.
    #[must_use]
    pub fn display_len(&self) -> u32 {
        self.display_text.len() as u32
    }

    /// Shift every absolute source-byte reference inside this spec by
    /// `delta` bytes.
    ///
    /// `DisplayMapBuilder::rebuild_dirty` calls this after a within-
    /// line edit that left a source line's *content* clean but shifted
    /// its *absolute byte position* (every source line that comes
    /// after the edit point). The display string, line-relative
    /// `source_to_display` table, display-relative
    /// `display_byte_to_utf16` table, and `content_stamp` are all
    /// invariant under such a shift; only the fields whose values are
    /// absolute source-byte coordinates need to be rebased.
    ///
    /// `delta` is added to:
    /// - [`Self::source_byte_start`] / [`Self::source_byte_end`]
    /// - the `source` byte range of every [`DisplaySegment`]
    /// - every entry in the `display_byte → source_byte` lookup table.
    ///
    /// A `delta` of `0` returns immediately; callers can pass it
    /// unconditionally without paying for a no-op walk. Underflow into
    /// negative source coordinates is a logic bug — debug builds
    /// panic; release builds clamp at `0`.
    pub(crate) fn shift_source_bytes(&mut self, delta: i64) {
        if delta == 0 {
            return;
        }
        let shifted = |raw: u32| -> u32 {
            let new = raw as i64 + delta;
            debug_assert!(
                new >= 0,
                "shift_source_bytes underflow: raw={raw} delta={delta}",
            );
            new.max(0) as u32
        };
        self.source_byte_start = SourceByte(shifted(self.source_byte_start.raw()));
        self.source_byte_end = SourceByte(shifted(self.source_byte_end.raw()));
        for seg in &mut self.segments {
            match seg {
                DisplaySegment::Visible { source, .. }
                | DisplaySegment::Hidden { source }
                | DisplaySegment::Replace { source, .. } => {
                    source.start = SourceByte(shifted(source.start.raw()));
                    source.end = SourceByte(shifted(source.end.raw()));
                }
            }
        }
        for entry in &mut self.display_to_source {
            *entry = shifted(*entry);
        }
        // `source_to_display` is keyed by index *within* the source
        // line span (0..span_len) and its values are display-relative
        // offsets — neither side carries an absolute source byte, so
        // it stays correct under a shift.
        // `display_byte_to_utf16` is purely display-keyed.
        // `content_stamp` is derived from the display text and segment
        // style only — invariant under a position-only shift.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::SegmentHit;

    fn visible(src: Range<u32>) -> DisplaySegment {
        DisplaySegment::Visible {
            source: SourceByte(src.start)..SourceByte(src.end),
            style: SpanStyle::body(),
            hit: SegmentHit::None,
        }
    }

    fn hidden(src: Range<u32>) -> DisplaySegment {
        DisplaySegment::Hidden {
            source: SourceByte(src.start)..SourceByte(src.end),
        }
    }

    fn replace(src: Range<u32>, display: &str) -> DisplaySegment {
        DisplaySegment::Replace {
            source: SourceByte(src.start)..SourceByte(src.end),
            display: display.into(),
            style: SpanStyle::bullet(),
            hit: SegmentHit::None,
        }
    }

    #[test]
    fn plain_line_round_trips_byte_for_byte() {
        let line = "hello";
        let spec = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(0),
            SourceByte(5),
            false,
            vec![visible(0..5)],
            line,
        );
        assert_eq!(spec.display_text(), "hello");
        for i in 0..=5u32 {
            assert_eq!(
                spec.source_to_display(SourceByte(i)),
                Some(DisplayByte(i)),
                "i={i}"
            );
            assert_eq!(
                spec.display_to_source(DisplayByte(i)),
                Some(SourceByte(i)),
                "i={i}"
            );
        }
    }

    #[test]
    fn hidden_segment_removes_bytes_from_display_and_loses_preimage() {
        // source: `**hi**`, display: `hi`
        let line = "**hi**";
        let spec = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(0),
            SourceByte(6),
            false,
            vec![hidden(0..2), visible(2..4), hidden(4..6)],
            line,
        );
        assert_eq!(spec.display_text(), "hi");
        // Source bytes 0,1,4,5 are hidden → no display.
        assert_eq!(spec.source_to_display(SourceByte(0)), None);
        assert_eq!(spec.source_to_display(SourceByte(1)), None);
        assert_eq!(spec.source_to_display(SourceByte(4)), None);
        assert_eq!(spec.source_to_display(SourceByte(5)), None);
        // Source byte 2 (first visible) → display 0.
        assert_eq!(spec.source_to_display(SourceByte(2)), Some(DisplayByte(0)));
        assert_eq!(spec.source_to_display(SourceByte(3)), Some(DisplayByte(1)));
        assert_eq!(spec.source_to_display(SourceByte(6)), Some(DisplayByte(2)));
        // Display → source.
        assert_eq!(spec.display_to_source(DisplayByte(0)), Some(SourceByte(2)));
        assert_eq!(spec.display_to_source(DisplayByte(1)), Some(SourceByte(3)));
    }

    #[test]
    fn display_to_source_snaps_visible_multibyte_bytes_to_char_start() {
        let line = "# café";
        let spec = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(0),
            SourceByte(7),
            false,
            vec![hidden(0..2), visible(2..7)],
            line,
        );
        assert_eq!(spec.display_text(), "café");
        assert_eq!(spec.display_to_source(DisplayByte(3)), Some(SourceByte(5)));
        assert_eq!(spec.display_to_source(DisplayByte(4)), Some(SourceByte(5)));
        assert_eq!(spec.display_to_source(DisplayByte(5)), Some(SourceByte(7)));
    }

    #[test]
    fn replace_segment_yields_baked_display_string() {
        // source: `- item`, display: `• item`
        let line = "- item";
        let spec = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(0),
            SourceByte(6),
            false,
            vec![replace(0..2, "• "), visible(2..6)],
            line,
        );
        assert_eq!(spec.display_text(), "• item");
        // Replace: source 0 -> display 0; source 2 -> display 4 (bullet glyph utf-8 = 3 bytes + space).
        assert_eq!(spec.source_to_display(SourceByte(0)), Some(DisplayByte(0)));
        // Replace eats a 2-byte source range; midpoint loses preimage.
        assert_eq!(spec.source_to_display(SourceByte(1)), None);
        assert_eq!(spec.source_to_display(SourceByte(2)), Some(DisplayByte(4)));
        assert_eq!(spec.source_to_display(SourceByte(6)), Some(DisplayByte(8)));
    }

    #[test]
    fn style_runs_skip_hidden_segments() {
        let line = "**hi**";
        let spec = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(0),
            SourceByte(6),
            false,
            vec![hidden(0..2), visible(2..4), hidden(4..6)],
            line,
        );
        let runs: Vec<_> = spec.style_runs().collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0.start.raw(), 0);
        assert_eq!(runs[0].0.end.raw(), 2);
    }

    #[test]
    fn display_byte_to_utf16_counts_units() {
        let line = "• hi";
        // `•` = 3 bytes utf-8, 1 utf-16; ` hi` = 3 bytes, 3 utf-16. Total
        // line = 6 bytes / 4 utf-16. Source bytes pass through verbatim.
        let spec = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(0),
            SourceByte(6),
            false,
            vec![visible(0..6)],
            line,
        );
        assert_eq!(spec.display_byte_to_utf16(DisplayByte(0)).unwrap().raw(), 0);
        assert_eq!(spec.display_byte_to_utf16(DisplayByte(3)).unwrap().raw(), 1);
        assert_eq!(spec.display_byte_to_utf16(DisplayByte(6)).unwrap().raw(), 4);
    }

    #[test]
    fn content_stamp_changes_when_style_changes() {
        let line = "hi";
        let plain = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(0),
            SourceByte(2),
            false,
            vec![visible(0..2)],
            line,
        );
        let bold = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(0),
            SourceByte(2),
            false,
            vec![DisplaySegment::Visible {
                source: SourceByte(0)..SourceByte(2),
                style: SpanStyle::strong(),
                hit: SegmentHit::None,
            }],
            line,
        );
        assert_ne!(plain.content_stamp(), bold.content_stamp());
    }

    #[test]
    fn shift_source_bytes_rebases_absolute_fields() {
        let line = "abcdef";
        let mut spec = DisplayLineSpec::new(
            SourceLine(2),
            SourceByte(100),
            SourceByte(106),
            false,
            vec![visible(100..106)],
            line,
        );
        spec.shift_source_bytes(7);
        assert_eq!(spec.source_byte_start.raw(), 107);
        assert_eq!(spec.source_byte_end.raw(), 113);
        match &spec.segments[0] {
            DisplaySegment::Visible { source, .. } => {
                assert_eq!(source.start.raw(), 107);
                assert_eq!(source.end.raw(), 113);
            }
            _ => panic!("expected Visible"),
        }
        // display→source table is absolute and must rebase.
        assert_eq!(
            spec.display_to_source(DisplayByte(0)),
            Some(SourceByte(107))
        );
        assert_eq!(
            spec.display_to_source(DisplayByte(6)),
            Some(SourceByte(113))
        );
        // display text and length unchanged.
        assert_eq!(spec.display_text(), "abcdef");
        assert_eq!(spec.display_len(), 6);
    }

    #[test]
    fn shift_source_bytes_handles_negative_delta() {
        let line = "abcdef";
        let mut spec = DisplayLineSpec::new(
            SourceLine(2),
            SourceByte(100),
            SourceByte(106),
            false,
            vec![visible(100..106)],
            line,
        );
        spec.shift_source_bytes(-50);
        assert_eq!(spec.source_byte_start.raw(), 50);
        assert_eq!(spec.source_byte_end.raw(), 56);
        assert_eq!(spec.display_to_source(DisplayByte(0)), Some(SourceByte(50)));
    }

    #[test]
    fn shift_source_bytes_zero_is_noop() {
        let line = "abc";
        let mut spec = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(0),
            SourceByte(3),
            false,
            vec![visible(0..3)],
            line,
        );
        let before_stamp = spec.content_stamp();
        spec.shift_source_bytes(0);
        assert_eq!(spec.source_byte_start.raw(), 0);
        assert_eq!(spec.source_byte_end.raw(), 3);
        assert_eq!(spec.content_stamp(), before_stamp);
    }

    #[test]
    fn shift_source_bytes_walks_every_segment_kind() {
        let line = "**hi**";
        let mut spec = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(20),
            SourceByte(26),
            false,
            vec![hidden(20..22), visible(22..24), replace(24..26, "XX")],
            line,
        );
        spec.shift_source_bytes(3);
        assert_eq!(spec.source_byte_start.raw(), 23);
        assert_eq!(spec.source_byte_end.raw(), 29);
        let ranges: Vec<(u32, u32)> = spec
            .segments
            .iter()
            .map(|s| {
                let r = s.source_range();
                (r.start.raw(), r.end.raw())
            })
            .collect();
        assert_eq!(ranges, vec![(23, 25), (25, 27), (27, 29)]);
    }

    #[test]
    fn content_stamp_stable_for_same_input() {
        let line = "hi";
        let a = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(0),
            SourceByte(2),
            false,
            vec![visible(0..2)],
            line,
        );
        let b = DisplayLineSpec::new(
            SourceLine(0),
            SourceByte(0),
            SourceByte(2),
            false,
            vec![visible(0..2)],
            line,
        );
        assert_eq!(a.content_stamp(), b.content_stamp());
    }
}
