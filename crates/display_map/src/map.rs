//! Immutable display-map snapshot consumed by the renderer + UI thread.

use std::ops::Range;
use std::sync::Arc;

use crate::id::{DisplayByte, DisplayLine, SourceByte, SourceLine};
use crate::line::DisplayLineSpec;
use crate::row_index::{DisplayRowIndex, IndexStamps};

/// An immutable, ref-counted snapshot describing every display line of a
/// buffer at one source revision + caret + fold + wrap-width tuple.
///
/// Built on a worker thread, consumed on the UI thread as `Arc<DisplayMap>`.
///
/// Source ↔ display offscreen queries (scrollbar content height, EOF
/// visibility, caret anchoring, hit-test, fold/wrap/image-reservation
/// math) flow through the embedded [`DisplayRowIndex`] rather than the
/// realized `lines` vector. ε.2 will narrow `lines` to a viewport
/// window; the index keeps every offscreen consumer honest in the
/// meantime.
#[derive(Clone, Debug)]
pub struct DisplayMap {
    /// Source revision the map was built against.
    revision: u64,
    /// Soft-wrap width in DIPs at build time (0 = no wrap).
    wrap_width_dip: u32,
    /// Per-source-line row-count index. Sole source of truth for
    /// offscreen source↔display queries; the realized `lines` vector
    /// supplies in-viewport specs.
    row_index: Arc<DisplayRowIndex>,
    /// ε.2 — absolute display-row index of `lines[0]`. Anything
    /// outside `realized_row_start..realized_row_start + lines.len()`
    /// has no realized spec; callers reach the row index for
    /// offscreen answers. `realized_row_start == 0` and
    /// `lines.len() == row_index.display_row_count()` means the map
    /// realizes the whole document (the legacy `build` path).
    realized_row_start: u32,
    /// Realized display-line specs covering
    /// `realized_row_start..realized_row_start + lines.len()`.
    lines: Arc<[DisplayLineSpec]>,
}

impl DisplayMap {
    /// Build a `DisplayMap` from already-prepared inputs. Callers should
    /// normally go through [`crate::DisplayMapBuilder`] instead.
    ///
    /// The row-count index is derived from the supplied `lines` by
    /// tallying how many display rows each source line contributed. ε.2
    /// will lift this construction into the builder so the index can be
    /// computed without materializing the spec vector.
    #[must_use]
    pub fn new(
        revision: u64,
        source_line_count: u32,
        wrap_width_dip: u32,
        lines: Vec<DisplayLineSpec>,
    ) -> Self {
        let row_counts = derive_row_counts(source_line_count, &lines);
        let stamps = IndexStamps {
            rope_revision: revision,
            decoration_revision: revision,
            wrap_width_dip,
            font_state: 0,
            fold_signature: 0,
        };
        let row_index = Arc::new(DisplayRowIndex::from_row_counts(row_counts, stamps));
        Self {
            revision,
            wrap_width_dip,
            row_index,
            realized_row_start: 0,
            lines: lines.into(),
        }
    }

    /// Build a `DisplayMap` from a pre-computed [`DisplayRowIndex`] and
    /// a full-document realized spec vector. The realized vector is
    /// assumed to cover display rows `[0, lines.len())`.
    #[must_use]
    pub fn from_parts(
        revision: u64,
        wrap_width_dip: u32,
        row_index: Arc<DisplayRowIndex>,
        lines: Vec<DisplayLineSpec>,
    ) -> Self {
        Self::from_parts_viewport(revision, wrap_width_dip, row_index, lines, 0)
    }

    /// ε.2 — build a `DisplayMap` whose realized `lines` vector covers
    /// only a viewport window. The realized rows are
    /// `[realized_row_start, realized_row_start + lines.len())`; queries
    /// outside that range return `None` for the spec (consumers
    /// fall through to the row index for source↔display answers).
    #[must_use]
    pub fn from_parts_viewport(
        revision: u64,
        wrap_width_dip: u32,
        row_index: Arc<DisplayRowIndex>,
        lines: Vec<DisplayLineSpec>,
        realized_row_start: u32,
    ) -> Self {
        Self {
            revision,
            wrap_width_dip,
            row_index,
            realized_row_start,
            lines: lines.into(),
        }
    }

    /// Source revision this snapshot was built against.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Number of source lines in the underlying rope at build time.
    #[must_use]
    pub fn source_line_count(&self) -> u32 {
        self.row_index.source_line_count()
    }

    /// Number of *display* lines after reveal/replace/wrap/fold across
    /// the whole document — sourced from the row index so the answer is
    /// valid even when `lines` is narrowed to a viewport window.
    #[must_use]
    pub fn display_line_count(&self) -> u32 {
        self.row_index.display_row_count()
    }

    /// Soft-wrap width at build time.
    #[must_use]
    pub fn wrap_width_dip(&self) -> u32 {
        self.wrap_width_dip
    }

    /// Borrow the row index backing this map. Offscreen consumers
    /// (scrollbar, EOF probing, source→display lookups for unrealized
    /// rows) should query the index directly.
    #[must_use]
    pub fn row_index(&self) -> &DisplayRowIndex {
        &self.row_index
    }

    /// Cheap clone of the row-index `Arc`. Used by the UI-side
    /// row-index cache to keep the same allocation alive across paint
    /// frames so a later viewport build can reuse it via
    /// [`crate::builder::DisplayMapBuilder::build_viewport_with_row_index`].
    #[must_use]
    pub fn row_index_arc(&self) -> Arc<DisplayRowIndex> {
        self.row_index.clone()
    }

    /// Half-open absolute display-row range the realized `lines`
    /// vector covers. `0..display_line_count()` for a full-document
    /// build; a narrower span for a viewport build.
    #[must_use]
    pub fn realized_row_range(&self) -> Range<u32> {
        self.realized_row_start..self.realized_row_start + self.lines.len() as u32
    }

    /// Lookup a display line by absolute display-row index. Returns
    /// `None` when the row lies outside the realized window (ε.2
    /// onward) or when the row index is past the document end.
    #[must_use]
    pub fn display_line(&self, idx: DisplayLine) -> Option<&DisplayLineSpec> {
        let absolute = idx.raw();
        if absolute < self.realized_row_start {
            return None;
        }
        let offset = (absolute - self.realized_row_start) as usize;
        self.lines.get(offset)
    }

    /// Iterate every realized display line in order, paired with its
    /// absolute display-row index.
    pub fn realized_display_lines(&self) -> impl Iterator<Item = (DisplayLine, &DisplayLineSpec)> {
        let start = self.realized_row_start;
        self.lines
            .iter()
            .enumerate()
            .map(move |(offset, spec)| (DisplayLine(start + offset as u32), spec))
    }

    /// ε.3 — slice of realized specs for `source_line`, or `None` if
    /// the source line's display rows don't fall inside the realized
    /// window (off-viewport or folded). Used by
    /// `DisplayMapBuilder::rebuild_dirty` to reuse prev's specs for
    /// clean source lines.
    ///
    /// Returns `None` (never panics) for out-of-range source lines —
    /// a debug assertion catches the miscalculation in non-release
    /// builds so a future caller bug doesn't silently look like
    /// "folded" when it's actually "off-by-one".
    #[must_use]
    pub fn realized_lines_for_source(&self, source_line: SourceLine) -> Option<&[DisplayLineSpec]> {
        debug_assert!(
            source_line.as_usize() < self.row_index.source_line_count() as usize,
            "DisplayMap::realized_lines_for_source called with out-of-range source_line={}; index has {} source lines",
            source_line.as_usize(),
            self.row_index.source_line_count(),
        );
        let count = self.row_index.display_row_count_for_source(source_line);
        if count == 0 {
            return None;
        }
        let first = self
            .row_index
            .first_display_row_of_source_line(source_line)
            .raw();
        if first < self.realized_row_start {
            return None;
        }
        let offset = (first - self.realized_row_start) as usize;
        let end = offset + count as usize;
        if end > self.lines.len() {
            return None;
        }
        Some(&self.lines[offset..end])
    }

    /// Iterate every realized display line in order.
    pub fn display_lines(&self) -> impl Iterator<Item = &DisplayLineSpec> {
        self.lines.iter()
    }

    /// Index of the *first* display line for `source_line`.
    /// For folded-out source lines this is the next visible display
    /// line (or `display_line_count` if past EOF).
    #[must_use]
    pub fn source_line_to_first_display(&self, source_line: SourceLine) -> DisplayLine {
        self.row_index.first_display_row_of_source_line(source_line)
    }

    /// Number of display lines used by `source_line` (0 if folded).
    #[must_use]
    pub fn display_line_count_for_source(&self, source_line: SourceLine) -> u32 {
        self.row_index.display_row_count_for_source(source_line)
    }

    /// Intersect a source byte range with each *realized* display line
    /// it touches, returning `(display_line_index, display-byte-range)`
    /// pairs. The returned indices are absolute display-row indices
    /// (offset by `realized_row_start`). Used for selection rendering,
    /// spell-squiggle painting, etc. Folded regions and rows outside
    /// the realized window naturally produce no entries.
    #[must_use]
    pub fn range_intersect_display(
        &self,
        range: Range<SourceByte>,
    ) -> Vec<(DisplayLine, Range<DisplayByte>)> {
        let mut out = Vec::new();
        if range.end.raw() < range.start.raw() {
            return out;
        }
        let base = self.realized_row_start;
        for (offset, line) in self.lines.iter().enumerate() {
            let line_start = line.source_byte_start;
            let line_end = line.source_byte_end;
            if line_end.raw() <= range.start.raw() {
                continue;
            }
            if line_start.raw() >= range.end.raw() {
                break;
            }
            let s = if range.start.raw() < line_start.raw() {
                line.source_to_display(line_start)
            } else {
                line.source_to_display(range.start)
            };
            let e = if range.end.raw() > line_end.raw() {
                line.source_to_display(line_end)
            } else {
                line.source_to_display(range.end)
            };
            // If either end fell inside a Hidden segment, fall back to
            // the nearest visible boundary on that side.
            let s = s.unwrap_or_else(|| nearest_visible_display_after(line, range.start));
            let e = e.unwrap_or_else(|| nearest_visible_display_before(line, range.end));
            let absolute = DisplayLine(base + offset as u32);
            if s.raw() < e.raw() {
                out.push((absolute, s..e));
            } else if range.start.raw() <= line_start.raw() && line_end.raw() <= range.end.raw() {
                // Pure-Hidden line whose whole source falls inside the
                // range — still emit a zero-width entry so callers can
                // mark it visited.
                out.push((absolute, s..e));
            }
        }
        out
    }
}

fn nearest_visible_display_after(line: &DisplayLineSpec, _src: SourceByte) -> DisplayByte {
    // Walk forward from the line's start; the first non-Hidden segment's
    // display start is the answer. If none, return display_len.
    let mut display_cursor: u32 = 0;
    for seg in &line.segments {
        match seg {
            crate::segment::DisplaySegment::Hidden { .. } => continue,
            crate::segment::DisplaySegment::Visible { source, .. } => {
                let len = source.end.raw() - source.start.raw();
                if len > 0 {
                    return DisplayByte(display_cursor);
                }
            }
            crate::segment::DisplaySegment::Replace { display, .. } => {
                if !display.is_empty() {
                    return DisplayByte(display_cursor);
                }
            }
        }
        match seg {
            crate::segment::DisplaySegment::Visible { source, .. } => {
                display_cursor += source.end.raw() - source.start.raw();
            }
            crate::segment::DisplaySegment::Replace { display, .. } => {
                display_cursor += display.len() as u32;
            }
            crate::segment::DisplaySegment::Hidden { .. } => {}
        }
    }
    DisplayByte(line.display_len())
}

fn nearest_visible_display_before(line: &DisplayLineSpec, _src: SourceByte) -> DisplayByte {
    DisplayByte(line.display_len())
}

/// Tally how many realized display rows each source line contributed.
/// Used by [`DisplayMap::new`] to derive the row-count vector when
/// callers haven't constructed a [`DisplayRowIndex`] themselves. The
/// builder threads the row counts through directly to avoid this
/// second pass over `lines`.
fn derive_row_counts(source_line_count: u32, lines: &[DisplayLineSpec]) -> Vec<u16> {
    let mut counts = vec![0u16; source_line_count as usize];
    for line in lines {
        let src = line.source_line.raw() as usize;
        if src < counts.len() {
            counts[src] = counts[src].saturating_add(1);
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::{DisplaySegment, SegmentHit};
    use crate::style::SpanStyle;

    fn line(idx: u32, start: u32, end: u32, source: &str) -> DisplayLineSpec {
        DisplayLineSpec::new(
            SourceLine(idx),
            SourceByte(start),
            SourceByte(end),
            false,
            vec![DisplaySegment::Visible {
                source: SourceByte(start)..SourceByte(end),
                style: SpanStyle::body(),
                hit: SegmentHit::None,
            }],
            source,
        )
    }

    #[test]
    fn display_line_count_matches_input() {
        let map = DisplayMap::new(
            0,
            2,
            0,
            vec![line(0, 0, 5, "hello"), line(1, 6, 11, "world")],
        );
        assert_eq!(map.display_line_count(), 2);
        assert_eq!(map.source_line_count(), 2);
    }

    #[test]
    fn source_to_display_line_maps_one_to_one_by_default() {
        let map = DisplayMap::new(
            0,
            2,
            0,
            vec![line(0, 0, 5, "hello"), line(1, 6, 11, "world")],
        );
        assert_eq!(map.source_line_to_first_display(SourceLine(0)).raw(), 0);
        assert_eq!(map.source_line_to_first_display(SourceLine(1)).raw(), 1);
        assert_eq!(map.display_line_count_for_source(SourceLine(0)), 1);
    }

    #[test]
    fn intersect_returns_per_line_display_ranges() {
        let map = DisplayMap::new(
            0,
            2,
            0,
            vec![line(0, 0, 5, "hello"), line(1, 6, 11, "world")],
        );
        let hits = map.range_intersect_display(SourceByte(2)..SourceByte(9));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0.raw(), 0);
        assert_eq!(hits[0].1.start.raw(), 2);
        assert_eq!(hits[0].1.end.raw(), 5);
        assert_eq!(hits[1].0.raw(), 1);
        assert_eq!(hits[1].1.start.raw(), 0);
        assert_eq!(hits[1].1.end.raw(), 3);
    }

    #[test]
    fn folded_source_lines_show_zero_display_lines() {
        // Two source lines but only the first emits display lines.
        let map = DisplayMap::new(0, 2, 0, vec![line(0, 0, 5, "hello")]);
        assert_eq!(map.display_line_count_for_source(SourceLine(0)), 1);
        assert_eq!(map.display_line_count_for_source(SourceLine(1)), 0);
        // Folded source line 1 should point at end-of-document.
        assert_eq!(map.source_line_to_first_display(SourceLine(1)).raw(), 1);
    }
}
