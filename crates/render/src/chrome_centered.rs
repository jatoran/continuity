//! Phase H2 — body-column cap + centering helper for distraction-free
//! mode. Lives in its own file so `chrome.rs` stays under the 600-line
//! cap; the function is otherwise a natural method on
//! [`ContentMargins`].

use crate::chrome::{
    resolve_body_left_margin_for_line_count_dip, resolve_body_right_margin_dip, ContentMargins,
};
use crate::params::ViewOptionsDraw;

/// Body text-column width in DIPs — the width the renderer paints into
/// and the width the display-map projects soft-wrap against. Both paths
/// must use this so wrap fires at the actual visible right edge.
///
/// `distraction_free` mirrors the renderer's centering: the cap reduces
/// the body width to `distraction_free_max_width_dip`; the renderer then
/// splits the leftover space evenly between left and right margins for
/// centering, which leaves the body-width itself at the cap.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn resolve_body_text_width_dip(
    viewport_w: f32,
    font_size_dip: f32,
    line_numbers: bool,
    minimap: bool,
    search_minimap_active: bool,
    show_outline_sidebar: bool,
    outline_sidebar_width_dip: f32,
    distraction_free: bool,
    distraction_free_max_width_dip: f32,
) -> f32 {
    resolve_body_text_width_for_line_count_dip(
        viewport_w,
        font_size_dip,
        99,
        line_numbers,
        minimap,
        search_minimap_active,
        show_outline_sidebar,
        outline_sidebar_width_dip,
        distraction_free,
        distraction_free_max_width_dip,
    )
}

/// Body text-column width in DIPs for a specific buffer line count.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn resolve_body_text_width_for_line_count_dip(
    viewport_w: f32,
    font_size_dip: f32,
    source_line_count: usize,
    line_numbers: bool,
    minimap: bool,
    search_minimap_active: bool,
    show_outline_sidebar: bool,
    outline_sidebar_width_dip: f32,
    distraction_free: bool,
    distraction_free_max_width_dip: f32,
) -> f32 {
    let left =
        resolve_body_left_margin_for_line_count_dip(line_numbers, font_size_dip, source_line_count);
    let right = resolve_body_right_margin_dip(
        minimap,
        search_minimap_active,
        show_outline_sidebar,
        outline_sidebar_width_dip,
    );
    let base = (viewport_w.max(1.0) - left - right).max(0.0);
    if distraction_free {
        base.min(distraction_free_max_width_dip.max(1.0))
    } else {
        base
    }
}

/// Resolve margins using a per-buffer line count for the gutter.
#[must_use]
pub(crate) fn resolve_margins_for_line_count(
    opts: &ViewOptionsDraw<'_>,
    viewport_w: f32,
    font_size_dip: f32,
    source_line_count: usize,
) -> ContentMargins {
    let base =
        ContentMargins::from_view_options_for_line_count(opts, font_size_dip, source_line_count);
    if opts.distraction_free {
        with_centered_body(base, viewport_w, opts.distraction_free_max_width_dip)
    } else {
        base
    }
}

/// Cap the body width to `max_width_dip` and center it inside
/// `viewport_w` by splitting the leftover horizontal space evenly
/// between left and right margins. Passes through unchanged when the
/// base body is already under the cap.
#[must_use]
pub(crate) fn with_centered_body(
    base: ContentMargins,
    viewport_w: f32,
    max_width_dip: f32,
) -> ContentMargins {
    let base_body = (viewport_w.max(1.0) - base.left - base.right).max(0.0);
    let max_w = max_width_dip.max(1.0);
    if base_body <= max_w {
        return base;
    }
    let pad = (base_body - max_w) * 0.5;
    ContentMargins {
        left: base.left + pad,
        right: base.right + pad,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_and_passes_through() {
        let base = ContentMargins {
            left: 10.0,
            right: 10.0,
        };
        // body=780, cap=400, extra=380 → +190 each side
        let c = with_centered_body(base, 800.0, 400.0);
        assert!((c.left - 200.0).abs() < 0.1 && (c.right - 200.0).abs() < 0.1);
        // body under cap → unchanged
        let p = with_centered_body(base, 300.0, 400.0);
        assert_eq!((p.left, p.right), (10.0, 10.0));
    }

    /// `resolve_body_text_width_dip` must equal
    /// `viewport_w - margins.left - margins.right` from `resolve_margins_for_line_count`
    /// for every toggle combo. Any divergence means the display-map's
    /// soft-wrap projection drifts from the renderer's painted right edge.
    ///
    /// End-to-end: build a FrameDisplay with the user's default setup
    /// (Cascadia Mono 14 DIP, line_numbers=true, no sidebars,
    /// viewport=1200) against a long prose line and verify each wrap row's
    /// text fits in the painted body width.
    #[test]
    fn long_line_wrap_rows_fit_painted_body_width() {
        use continuity_buffer::{Revision, RopeSnapshot};
        use continuity_decorate::Decorations;
        use continuity_display_map::wrap::FixedCharWidth;
        use continuity_display_map::{DisplayMapBuilder, SourceByte, WrapConfig};
        use ropey::Rope;
        use std::sync::Arc;

        let viewport = 1200.0_f32;
        let font = 14.0_f32;
        let char_w = font * 0.55;
        let wrap_width =
            resolve_body_text_width_dip(viewport, font, true, false, false, false, 0.0, false, 0.0);
        // Body-text column width must match the painter's editor_w.
        let opts = ViewOptionsDraw {
            line_numbers: true,
            ..ViewOptionsDraw::default()
        };
        let margins = resolve_margins_for_line_count(&opts, viewport, font, 99);
        let painted = viewport - margins.left - margins.right;
        assert!((wrap_width - painted).abs() < 0.01);

        // 300-char prose line.
        let line: String = "the quick brown fox jumps over the lazy dog ".repeat(7);
        let rope = Rope::from_str(&line);
        let snap = RopeSnapshot::new(Arc::new(rope.clone()), Revision(1));
        let decos = Decorations::empty(1);
        let carets: Vec<SourceByte> = vec![];
        let folds: Vec<continuity_display_map::FoldRange> = vec![];
        let wrap = WrapConfig::new(wrap_width.round().max(0.0) as u32);
        let mut measure = FixedCharWidth::new(char_w);
        let map = DisplayMapBuilder::new(&snap, &decos, &carets, &folds, wrap)
            .build(&mut measure)
            .expect("build ok");

        // Word wrapping can carry the final glyph of a word just past
        // the ideal width; it must stay within one measured glyph.
        let mut max_row_width = 0.0_f32;
        for i in 0..map.display_line_count() {
            let spec = map
                .display_line(continuity_display_map::DisplayLine::from_usize(i as usize))
                .unwrap();
            let w = spec.display_text().chars().count() as f32 * char_w;
            max_row_width = max_row_width.max(w);
            assert!(
                w <= wrap_width + char_w,
                "row {i} width {w} exceeds wrap_width {wrap_width}",
            );
        }
        // Sanity: at least one row should use most of the available width
        // (≥ 80% of wrap_width). If wrap was firing "long before edge",
        // every row would be much shorter than this.
        let utilization = max_row_width / wrap_width;
        assert!(
            utilization >= 0.80,
            "max wrap row utilization {utilization} (max={max_row_width}, wrap={wrap_width}) — wrap firing too early",
        );
    }

    /// User's default scenario: Cascadia Mono 14 DIP, line_numbers=true,
    /// no sidebars, viewport=1000. The wrap width should be exactly the
    /// painted text-column width.
    #[test]
    fn user_default_scenario_wrap_width_is_visible_body() {
        let viewport_w = 1000.0;
        let font = 14.0;
        let opts = ViewOptionsDraw {
            line_numbers: true,
            ..ViewOptionsDraw::default()
        };
        let margins = resolve_margins_for_line_count(&opts, viewport_w, font, 99);
        let painted = viewport_w - margins.left - margins.right;
        let wrap = resolve_body_text_width_dip(
            viewport_w, font, true, false, false, false, 0.0, false, 0.0,
        );
        let expected = viewport_w
            - (crate::chrome::gutter_width_for_line_count(font, 99)
                + crate::chrome::GUTTER_BODY_GAP_DIP)
            - crate::chrome::BODY_RIGHT_PADDING_DIP;
        assert!((wrap - expected).abs() < 0.1, "wrap={wrap}");
        assert!(
            (painted - wrap).abs() < 0.01,
            "painted={painted} wrap={wrap}"
        );
    }

    #[test]
    fn resolve_body_text_width_dip_matches_resolve_margins() {
        let cases: Vec<(bool, bool, bool, bool, f32, bool, f32)> = vec![
            // line_numbers, minimap, search_minimap, outline, outline_w, df, df_max
            (true, false, false, false, 0.0, false, 0.0),
            (false, false, false, false, 0.0, false, 0.0),
            (true, true, false, false, 0.0, false, 0.0),
            (true, false, true, false, 0.0, false, 0.0),
            (true, false, false, true, 220.0, false, 0.0),
            (true, true, false, true, 220.0, false, 0.0),
            (false, false, false, false, 0.0, true, 600.0),
            (true, true, false, true, 220.0, true, 600.0),
        ];
        let viewport_w = 1200.0;
        let font = 13.0;
        for (ln, mm, smm, ol, ow, df, dfw) in cases {
            let opts = ViewOptionsDraw {
                line_numbers: ln,
                minimap: mm,
                search_minimap_active: smm,
                show_outline_sidebar: ol,
                outline_sidebar_width_dip: ow,
                distraction_free: df,
                distraction_free_max_width_dip: dfw,
                ..ViewOptionsDraw::default()
            };
            let margins = resolve_margins_for_line_count(&opts, viewport_w, font, 99);
            let painted_body = (viewport_w - margins.left - margins.right).max(0.0);
            let helper =
                resolve_body_text_width_dip(viewport_w, font, ln, mm, smm, ol, ow, df, dfw);
            assert!(
                (helper - painted_body).abs() < 0.01,
                "diverged for case ln={ln} mm={mm} smm={smm} ol={ol} ow={ow} df={df} dfw={dfw}: \
                 helper={helper} painted={painted_body}",
            );
        }
    }
}
