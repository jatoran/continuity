//! Per-line, per-byte, and row-index queries against the realized
//! [`DisplayMap`]. Every method here is a thin projection over the
//! `Arc<DisplayMap>` carried by [`FrameDisplay`]; nothing here mutates
//! state or builds a new map.

use std::ops::Range;

use continuity_display_map::{
    DisplayByte, DisplayLine, DisplayLineSpec, DisplayMap, DisplayRowIndex, DisplayUtf16,
    SourceByte, SourceLine, SpanStyle,
};

use super::FrameDisplay;

impl FrameDisplay {
    /// Absolute display-row range the realized `DisplayLineSpec` vector
    /// covers. `0..display_line_count()` for a full-document build;
    /// narrower for a viewport build.
    #[must_use]
    pub fn realized_row_range(&self) -> Range<u32> {
        self.map.realized_row_range()
    }

    /// First `DisplayLineSpec` belonging to `source_line`, if any.
    ///
    /// Returns `None` for:
    /// - **Folded** source lines (row count == 0 in the index).
    /// - **Off-viewport** source lines under ε.2 viewport realization
    ///   (row count > 0 but no realized spec). Callers that need to
    ///   distinguish "folded" from "off-viewport" should query
    ///   [`Self::row_index`] for the row count; non-zero count + `None`
    ///   from this method ⇒ off-viewport.
    ///
    /// Practically every consumer of this method (paint loops, mouse
    /// hit-test, caret anchor, spell squiggles, selection rendering)
    /// only reads source lines whose display rows fall inside the
    /// renderer's first..last visible range — and the
    /// `build_viewport(visible_rows, overscan)` invariant guarantees
    /// those rows are realized — so the off-viewport path is exercised
    /// only on coding errors.
    #[must_use]
    pub fn line(&self, source_line: usize) -> Option<&DisplayLineSpec> {
        let n = self
            .map
            .display_line_count_for_source(SourceLine::from_usize(source_line));
        if n == 0 {
            return None;
        }
        let first = self
            .map
            .source_line_to_first_display(SourceLine::from_usize(source_line));
        self.map.display_line(first)
    }

    /// Display text for `source_line`, or `None` if folded.
    #[must_use]
    pub fn display_text(&self, source_line: usize) -> Option<&str> {
        self.line(source_line).map(|l| l.display_text())
    }

    /// Translate a source byte (relative to the source line's start) into
    /// a UTF-16 code-unit index inside the display layout. Falls back to
    /// `0` if the position has no preimage (e.g. lies inside a `Hidden`
    /// segment).
    #[must_use]
    pub(crate) fn source_byte_in_line_to_display_utf16(
        &self,
        source_line: usize,
        byte_in_source_line: usize,
    ) -> Option<u32> {
        let line = self.line(source_line)?;
        let abs_src = line.source_byte_start.raw() as usize + byte_in_source_line;
        let db = line
            .source_to_display(SourceByte::from_usize(abs_src))
            .or_else(|| {
                // Walk forward to the next visible source byte; clamp to
                // line end. This keeps carets / selections from landing
                // at zero whenever they touch a hidden marker boundary.
                let line_end_src = line.source_byte_end.raw() as usize;
                let mut probe = abs_src;
                while probe <= line_end_src {
                    if let Some(d) = line.source_to_display(SourceByte::from_usize(probe)) {
                        return Some(d);
                    }
                    probe += 1;
                }
                None
            })
            .unwrap_or(DisplayByte(line.display_len()));
        line.display_byte_to_utf16(db).map(|u| u.raw())
    }

    /// Convert a *display* byte (the unit DirectWrite hands back from
    /// `HitTestPoint`) into a source byte *relative to the source line's
    /// start* so the editor can update selection / cursor state.
    #[must_use]
    pub fn display_byte_to_source_byte_in_line(
        &self,
        source_line: usize,
        display_byte: usize,
    ) -> Option<usize> {
        let line = self.line(source_line)?;
        let sb = line.display_to_source(DisplayByte::from_usize(display_byte))?;
        Some(sb.raw().saturating_sub(line.source_byte_start.raw()) as usize)
    }

    /// Style runs (display UTF-16 ranges + `SpanStyle`) for `source_line`.
    #[must_use]
    pub fn style_runs(&self, source_line: usize) -> Vec<(Range<DisplayUtf16>, SpanStyle)> {
        self.line(source_line)
            .map(|l| l.style_runs().collect())
            .unwrap_or_default()
    }

    /// Borrow the underlying map (mostly for tests / instrumentation).
    #[must_use]
    pub fn map(&self) -> &DisplayMap {
        &self.map
    }

    /// Borrow the row-count index. Offscreen consumers should query
    /// the index directly rather than walking the realized spec
    /// vector — ε.2 will narrow the spec vector to viewport, but the
    /// index always covers the whole document.
    #[must_use]
    pub fn row_index(&self) -> &DisplayRowIndex {
        self.map.row_index()
    }

    /// Cheap clone of the row-index `Arc` backing this frame. Used
    /// by the UI's row-index cache to reuse the index across cold
    /// viewport builds in different panes / tabs / layouts (any case
    /// where the buffer geometry stays the same but the per-pane
    /// `FrameDisplay` would otherwise be rebuilt from scratch).
    #[must_use]
    pub fn row_index_arc(&self) -> std::sync::Arc<DisplayRowIndex> {
        self.map.row_index_arc()
    }

    /// Total number of *visible* display lines (post-reveal, post-wrap,
    /// post-fold) — the unit the renderer's per-frame y-grid uses when
    /// soft wrap is active.
    #[must_use]
    pub fn display_line_count(&self) -> u32 {
        self.map.display_line_count()
    }

    /// Look up a display line by absolute index.
    #[must_use]
    pub fn display_line_by_index(&self, idx: u32) -> Option<&DisplayLineSpec> {
        self.map.display_line(DisplayLine(idx))
    }

    /// Display-line index of the *first* visible row for `source_line`.
    /// Folded source lines return the index of the next visible row.
    #[must_use]
    pub fn first_display_line_index_for_source(&self, source_line: usize) -> u32 {
        self.map
            .source_line_to_first_display(SourceLine::from_usize(source_line))
            .raw()
    }

    /// Number of display rows used by `source_line` (0 if folded).
    #[must_use]
    pub fn display_line_count_for_source(&self, source_line: usize) -> u32 {
        self.map
            .display_line_count_for_source(SourceLine::from_usize(source_line))
    }

    /// Index of the display line containing `(source_line, byte_in_line)`.
    /// When the source position lies inside a soft-wrap continuation, this
    /// returns the continuation's display line; the first display line
    /// for a source line otherwise.
    #[must_use]
    pub fn display_line_index_for_source_pos(
        &self,
        source_line: usize,
        byte_in_source_line: usize,
    ) -> Option<u32> {
        let n = self.display_line_count_for_source(source_line);
        if n == 0 {
            return None;
        }
        let first = self.first_display_line_index_for_source(source_line);
        let target = self
            .map
            .display_line(DisplayLine(first))
            .map(|spec| spec.source_byte_start.raw() as usize + byte_in_source_line);
        let target = target?;
        // Walk display lines for this source line; return the last one
        // whose source range starts at or before `target`.
        let mut chosen = first;
        for i in 0..n {
            let idx = first + i;
            if let Some(spec) = self.map.display_line(DisplayLine(idx)) {
                if (spec.source_byte_start.raw() as usize) <= target {
                    chosen = idx;
                } else {
                    break;
                }
            }
        }
        Some(chosen)
    }
}
