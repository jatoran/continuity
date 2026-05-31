//! §H1 — focus-mode dim overlay painter.
//!
//! When `PaneModesState.focus_mode != Off`, the caret's source-byte
//! span (line / sentence / paragraph) stays at full contrast and every
//! other visible source byte is dimmed by overlaying a semi-transparent
//! foreground-dim rect over its display row.
//!
//! This module ships two pure helpers + a D2D paint entry:
//!
//! - [`compute_focus_span`] dispatches a `mode_str` (one of
//!   `"line" | "sentence" | "paragraph"`) to the right
//!   [`continuity_decorate::focus_span`] function. `"off"` and unknown
//!   modes return `None`.
//! - [`compute_dim_rows`] turns a [`FocusSpan`] into a list of
//!   `(display_row_index, y_top_dip, height_dip)` triples for every
//!   visible display row whose source line lies *outside* the focused
//!   span. The caller paints a translucent rect over each row.
//! - [`paint_focus_dim`] performs the actual D2D fill given a brush.
//!
//! Sibling to `chrome_fold.rs`. Currently uncalled — the wiring to
//! forward `pane_modes.focus_mode` + dim-color through `DrawParams` is
//! tracked in `.docs/development/wire_H1_focus_mode.md`. The module
//! ships behind `#[allow(dead_code)]` in `lib.rs` until that lands.
//!
//! **Thread ownership**: UI thread.

use continuity_decorate::{line_span, paragraph_span, sentence_span, FocusSpan};
use continuity_display_map::DisplayMap;
use ropey::Rope;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};

use crate::Error;

/// A single dim-overlay rectangle in body-content space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DimRow {
    /// Top-edge y in DIPs (already scroll-adjusted).
    pub y_top_dip: f32,
    /// Row height in DIPs.
    pub height_dip: f32,
}

/// Dispatch `mode_str` to the matching [`continuity_decorate::focus_span`]
/// function. Returns `None` when the mode is `"off"` or unknown so the
/// caller can short-circuit the paint pass.
///
/// `caret_byte` is the primary caret's source byte. `source` is the
/// rope's contents rendered to a `&str`; callers that already have a
/// rope should hand in the `to_string()` form once per frame.
#[must_use]
pub(crate) fn compute_focus_span(
    source: &str,
    caret_byte: usize,
    mode_str: &str,
) -> Option<FocusSpan> {
    match mode_str {
        "line" => Some(line_span(source, caret_byte)),
        "sentence" => Some(sentence_span(source, caret_byte)),
        "paragraph" => Some(paragraph_span(source, caret_byte)),
        _ => None,
    }
}

/// Identify every visible display row whose source line falls **outside**
/// the focused span. Each such row needs a dim overlay.
///
/// Inclusion model: a display row is "outside" when its source line
/// index is strictly less than the focus span's start line, or strictly
/// greater than the focus span's end line (inclusive). The focused
/// line range itself stays undimmed — even when the focus mode is
/// `Sentence`, sub-line dimming is a follow-up; the MVP dims by source
/// line, which is the spec's "decoration pass" minimum.
#[must_use]
pub(crate) fn compute_dim_rows(
    rope: &Rope,
    map: &DisplayMap,
    focus: FocusSpan,
    line_height_dip: f32,
    scroll_y_dip: f32,
    first_visible_row: u32,
    last_visible_row: u32,
) -> Vec<DimRow> {
    if line_height_dip <= 0.0 {
        return Vec::new();
    }
    let focus_start_line = byte_to_source_line(rope, focus.start);
    let focus_end_line = byte_to_source_line(rope, focus.end.saturating_sub(1).max(focus.start));
    let display_total = map.display_line_count();
    let last = last_visible_row.min(display_total);
    let mut out = Vec::new();
    for row in first_visible_row..last {
        let spec = match map.display_line(continuity_display_map::DisplayLine(row)) {
            Some(s) => s,
            None => continue,
        };
        let src_line = spec.source_line.raw();
        let in_focus = src_line >= focus_start_line && src_line <= focus_end_line;
        if in_focus {
            continue;
        }
        let y_top_dip = row as f32 * line_height_dip - scroll_y_dip;
        out.push(DimRow {
            y_top_dip,
            height_dip: line_height_dip,
        });
    }
    out
}

/// Source line containing `byte`. Clamped to the rope's last line for
/// EOF inputs.
fn byte_to_source_line(rope: &Rope, byte: usize) -> u32 {
    let clamped = byte.min(rope.len_bytes());
    let line = rope.byte_to_line(clamped);
    u32::try_from(line).unwrap_or(u32::MAX)
}

/// Paint the dim overlay rectangles. `x_left_dip` / `width_dip` cover
/// the body content band (gutter excluded). `brush` is a pre-built
/// translucent solid-color brush — the caller supplies the alpha via the
/// brush's color (effective alpha = `editor.focus_dim_alpha`, falling
/// back to `[focus].dim_alpha` when non-zero per `focus.rs:9-11`).
///
/// # Errors
///
/// Returns `Ok(())` always — D2D `FillRectangle` is infallible. Returns
/// `Result` for symmetry with sibling painters and to leave room for
/// future allocation paths.
pub(crate) fn paint_focus_dim(
    ctx: &ID2D1DeviceContext,
    rows: &[DimRow],
    x_left_dip: f32,
    width_dip: f32,
    brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    for row in rows {
        let rect = D2D_RECT_F {
            left: x_left_dip,
            top: row.y_top_dip,
            right: x_left_dip + width_dip,
            bottom: row.y_top_dip + row.height_dip,
        };
        unsafe { ctx.FillRectangle(&rect, brush) };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    #[test]
    fn compute_focus_span_off_returns_none() {
        assert!(compute_focus_span("hello", 2, "off").is_none());
        assert!(compute_focus_span("hello", 2, "unknown").is_none());
    }

    #[test]
    fn compute_focus_span_line_matches_decorate() {
        let span = compute_focus_span("foo\nbar\n", 5, "line").unwrap();
        // Caret at byte 5 (inside "bar"). Line span excludes \n.
        assert_eq!(span.start, 4);
        assert_eq!(span.end, 7);
    }

    #[test]
    fn compute_focus_span_paragraph_matches_decorate() {
        let span = compute_focus_span("p1 l1\np1 l2\n\np2\n", 8, "paragraph").unwrap();
        // Caret inside paragraph 1.
        assert_eq!(span.start, 0);
        assert_eq!(span.end, 11);
    }

    #[test]
    fn compute_focus_span_sentence_matches_decorate() {
        let s = "First sentence. Second sentence!";
        let span = compute_focus_span(s, 20, "sentence").unwrap();
        assert!(span.contains(20));
    }

    #[test]
    fn byte_to_source_line_clamps_past_eof() {
        let rope = Rope::from_str("a\nb\nc\n");
        assert_eq!(byte_to_source_line(&rope, 999), 3);
    }

    #[test]
    fn byte_to_source_line_zero_returns_zero() {
        let rope = Rope::from_str("abc\n");
        assert_eq!(byte_to_source_line(&rope, 0), 0);
    }

    #[test]
    fn compute_dim_rows_skips_rows_with_zero_line_height() {
        let rope = Rope::from_str("a\nb\nc\n");
        let snap = continuity_buffer::RopeSnapshot::new(
            std::sync::Arc::new(rope.clone()),
            continuity_buffer::Revision(1),
        );
        let decos = continuity_decorate::Decorations::empty(1);
        let mut measure = continuity_display_map::wrap::FixedCharWidth::new(8.0);
        let map = continuity_display_map::DisplayMapBuilder::new(
            &snap,
            &decos,
            &[],
            &[],
            continuity_display_map::WrapConfig::NONE,
        )
        .build(&mut measure)
        .unwrap();
        let focus = FocusSpan { start: 0, end: 1 };
        let rows = compute_dim_rows(&rope, &map, focus, 0.0, 0.0, 0, 3);
        assert!(rows.is_empty());
    }

    #[test]
    fn compute_dim_rows_dims_only_lines_outside_focus() {
        // Three single-line source rows, no wrap. Focus span covers
        // byte 0 (source line 0 only). Lines 1 and 2 must dim.
        let rope = Rope::from_str("aaa\nbbb\nccc\n");
        let snap = continuity_buffer::RopeSnapshot::new(
            std::sync::Arc::new(rope.clone()),
            continuity_buffer::Revision(1),
        );
        let decos = continuity_decorate::Decorations::empty(1);
        let mut measure = continuity_display_map::wrap::FixedCharWidth::new(8.0);
        let map = continuity_display_map::DisplayMapBuilder::new(
            &snap,
            &decos,
            &[],
            &[],
            continuity_display_map::WrapConfig::NONE,
        )
        .build(&mut measure)
        .unwrap();
        let focus = FocusSpan { start: 0, end: 3 };
        let line_h = 16.0;
        let rows = compute_dim_rows(&rope, &map, focus, line_h, 0.0, 0, 3);
        assert_eq!(rows.len(), 2);
        assert!((rows[0].y_top_dip - line_h).abs() < 1e-6);
        assert!((rows[1].y_top_dip - 2.0 * line_h).abs() < 1e-6);
        assert!((rows[0].height_dip - line_h).abs() < 1e-6);
    }

    #[test]
    fn compute_dim_rows_dims_nothing_when_all_visible_in_focus() {
        let rope = Rope::from_str("only line\n");
        let snap = continuity_buffer::RopeSnapshot::new(
            std::sync::Arc::new(rope.clone()),
            continuity_buffer::Revision(1),
        );
        let decos = continuity_decorate::Decorations::empty(1);
        let mut measure = continuity_display_map::wrap::FixedCharWidth::new(8.0);
        let map = continuity_display_map::DisplayMapBuilder::new(
            &snap,
            &decos,
            &[],
            &[],
            continuity_display_map::WrapConfig::NONE,
        )
        .build(&mut measure)
        .unwrap();
        let focus = FocusSpan {
            start: 0,
            end: rope.len_bytes(),
        };
        let rows = compute_dim_rows(&rope, &map, focus, 16.0, 0.0, 0, 1);
        assert!(rows.is_empty());
    }

    #[test]
    fn compute_dim_rows_respects_scroll_offset() {
        let rope = Rope::from_str("a\nb\n");
        let snap = continuity_buffer::RopeSnapshot::new(
            std::sync::Arc::new(rope.clone()),
            continuity_buffer::Revision(1),
        );
        let decos = continuity_decorate::Decorations::empty(1);
        let mut measure = continuity_display_map::wrap::FixedCharWidth::new(8.0);
        let map = continuity_display_map::DisplayMapBuilder::new(
            &snap,
            &decos,
            &[],
            &[],
            continuity_display_map::WrapConfig::NONE,
        )
        .build(&mut measure)
        .unwrap();
        // Focus line 0; line 1 should be dimmed at y = 16 - 4 = 12.
        let focus = FocusSpan { start: 0, end: 1 };
        let rows = compute_dim_rows(&rope, &map, focus, 16.0, 4.0, 0, 2);
        assert_eq!(rows.len(), 1);
        assert!((rows[0].y_top_dip - 12.0).abs() < 1e-6);
    }
}
