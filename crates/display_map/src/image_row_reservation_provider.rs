//! γ — phantom-row reservation for expanded inline images.
//!
//! Today an `![](url)` source line projects to a single display row
//! even when the user has expanded the image; the bitmap paints over
//! that row's natural height and overdraws text below it. This
//! provider produces the per-source-line phantom-row counts the
//! [`crate::builder::DisplayMapBuilder`] needs to make text *below* an
//! expanded image flow below it rather than be overdrawn.
//!
//! Modeled after [`crate::backslash_escape_provider`] and
//! [`crate::table_hide_provider`]: a pure function that emits
//! directives the builder consumes. It owes nothing to the renderer —
//! the input is plain data the UI fills from its per-frame inline-
//! image placement list plus a per-window native-dimensions cache.
//!
//! ## Contract
//!
//! For every expanded image with known native dimensions:
//!
//! ```text
//! reserved_display_rows = ceil(image_display_height_dip / line_height_dip).max(1)
//! ```
//!
//! `image_display_height_dip` is derived from the same pane-width
//! clamp the renderer uses ([`continuity_render::image_layout::compute_image_layout`]);
//! the formula is reproduced here verbatim so this crate stays free
//! of a render-layer dependency. Collapsed images and expanded images
//! whose native dimensions are not yet cached emit no directive — the
//! source line keeps its single display row (existing behaviour).
//!
//! ## Thread ownership
//!
//! Pure data, callable from the display-map worker thread or the UI
//! thread. No I/O, no statics.

use crate::id::SourceLine;

/// One reservation directive: source line `source_line` should occupy
/// `reserved_display_rows` display rows total (1 = no extra rows, the
/// default; >1 = inject `reserved_display_rows - 1` phantom rows after
/// the source line's natural display row).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ImageRowReservation {
    /// Source line index the expanded image lives on.
    pub source_line: SourceLine,
    /// Total display rows the line should occupy. Always `>= 1` once
    /// emitted; collapsed / unknown-dimension images are filtered out
    /// before this value is computed.
    pub reserved_display_rows: u32,
}

/// Plain-data input to [`compute_image_row_reservations`].
///
/// One entry per inline image reference in the buffer. The UI fills
/// these from its [`continuity_render::InlineImagePlacement`] list plus
/// the per-window image-dimensions cache; the provider stays free of
/// any render-layer type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageRowReservationInput {
    /// Source line index the image's `![](url)` reference lives on.
    pub source_line: SourceLine,
    /// `true` when the user has expanded this image (the renderer paints
    /// the full bitmap rather than the single-row collapsed
    /// affordance).
    pub is_expanded: bool,
    /// Native pixel dimensions of the underlying image. `None` when the
    /// renderer has not yet decoded the image — the provider emits no
    /// reservation in that case so the very first paint after expand
    /// degrades to the pre-reservation overdraw behaviour for one
    /// frame; the next frame, once the cache is warm, reservation
    /// kicks in.
    pub native_dimensions: Option<(u32, u32)>,
    /// The `|<width>` hint from `parse_image_alt`, if any. Honoured up
    /// to the pane-width clamp.
    pub width_hint: Option<u32>,
}

/// Compute the per-source-line phantom-row reservations for every
/// expanded inline image in `inputs`.
///
/// `line_height_dip` is the height of one display row in DIPs (the
/// renderer's body line height). `pane_width_dip` is the pane body
/// width the image will be clamped to — same value the painter feeds
/// [`continuity_render::image_layout::compute_image_layout`].
///
/// Collapsed images and expanded images with unknown native
/// dimensions emit no entry. The output is sorted by `source_line`
/// ascending so the builder can scan it in a single pass alongside
/// its line loop.
#[must_use]
pub fn compute_image_row_reservations(
    inputs: &[ImageRowReservationInput],
    line_height_dip: f32,
    pane_width_dip: f32,
) -> Vec<ImageRowReservation> {
    if line_height_dip <= 0.0 || pane_width_dip <= 0.0 {
        return Vec::new();
    }
    let mut out: Vec<ImageRowReservation> = Vec::new();
    for input in inputs {
        if !input.is_expanded {
            continue;
        }
        let Some((native_w, native_h)) = input.native_dimensions else {
            continue;
        };
        let Some(rows) = compute_reserved_rows(
            native_w,
            native_h,
            input.width_hint,
            line_height_dip,
            pane_width_dip,
        ) else {
            continue;
        };
        out.push(ImageRowReservation {
            source_line: input.source_line,
            reserved_display_rows: rows,
        });
    }
    out.sort_by_key(|r| r.source_line.raw());
    // Coalesce: if two `ImageRef`s share a source line (rare but
    // legal), the line should reserve max(rows) so the taller image
    // fits. The painter still paints both at their own coords.
    coalesce_by_source_line(out)
}

fn compute_reserved_rows(
    native_w: u32,
    native_h: u32,
    width_hint: Option<u32>,
    line_height_dip: f32,
    pane_width_dip: f32,
) -> Option<u32> {
    if native_w == 0 || native_h == 0 {
        return None;
    }
    let aspect = native_h as f32 / native_w as f32;
    let target_width_dip = match width_hint {
        Some(hint) if hint > 0 => (hint as f32).min(pane_width_dip),
        _ => {
            let nw = native_w as f32;
            if nw > pane_width_dip {
                pane_width_dip
            } else {
                nw
            }
        }
    };
    let height_dip = target_width_dip * aspect;
    if !height_dip.is_finite() || height_dip <= 0.0 {
        return None;
    }
    let raw = (height_dip / line_height_dip).ceil();
    // Sanity bound — protects against pathological 50_000-pixel images.
    // Generous enough to never clip a real screenshot at typical line
    // heights (14–18 DIP × 1000 rows = 14 000–18 000 DIP).
    const MAX_RESERVED_ROWS: u32 = 1000;
    let rows = (raw as u32).clamp(1, MAX_RESERVED_ROWS);
    if rows <= 1 {
        // Image fits inside one line — no phantom rows needed.
        return None;
    }
    Some(rows)
}

fn coalesce_by_source_line(mut sorted: Vec<ImageRowReservation>) -> Vec<ImageRowReservation> {
    if sorted.len() <= 1 {
        return sorted;
    }
    let mut out: Vec<ImageRowReservation> = Vec::with_capacity(sorted.len());
    for r in sorted.drain(..) {
        match out.last_mut() {
            Some(last) if last.source_line == r.source_line => {
                if r.reserved_display_rows > last.reserved_display_rows {
                    last.reserved_display_rows = r.reserved_display_rows;
                }
            }
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        source_line: u32,
        is_expanded: bool,
        native: Option<(u32, u32)>,
        width_hint: Option<u32>,
    ) -> ImageRowReservationInput {
        ImageRowReservationInput {
            source_line: SourceLine(source_line),
            is_expanded,
            native_dimensions: native,
            width_hint,
        }
    }

    #[test]
    fn empty_input_yields_nothing() {
        let out = compute_image_row_reservations(&[], 20.0, 800.0);
        assert!(out.is_empty());
    }

    #[test]
    fn collapsed_images_yield_no_reservation() {
        let inputs = vec![
            input(3, false, Some((400, 600)), None),
            input(7, false, Some((1200, 800)), None),
        ];
        let out = compute_image_row_reservations(&inputs, 20.0, 800.0);
        assert!(out.is_empty());
    }

    #[test]
    fn expanded_without_dimensions_yields_no_reservation() {
        let inputs = vec![input(2, true, None, None)];
        let out = compute_image_row_reservations(&inputs, 20.0, 800.0);
        assert!(out.is_empty());
    }

    #[test]
    fn image_fitting_within_one_line_emits_nothing() {
        // 100×10 image at 20 DIP line height → 10 DIP tall → 1 row.
        let inputs = vec![input(0, true, Some((100, 10)), None)];
        let out = compute_image_row_reservations(&inputs, 20.0, 800.0);
        assert!(out.is_empty());
    }

    #[test]
    fn expanded_image_reserves_ceiling_of_height_over_line_height() {
        // 800×400 native, pane=800 → 800×400 DIP. Line height 20 → 20 rows.
        let inputs = vec![input(5, true, Some((800, 400)), None)];
        let out = compute_image_row_reservations(&inputs, 20.0, 800.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source_line.raw(), 5);
        assert_eq!(out[0].reserved_display_rows, 20);
    }

    #[test]
    fn pane_width_clamp_kicks_in_for_wide_images() {
        // 1600×800 native, pane=400 → 400×200 DIP. Line height 20 → 10 rows.
        let inputs = vec![input(0, true, Some((1600, 800)), None)];
        let out = compute_image_row_reservations(&inputs, 20.0, 400.0);
        assert_eq!(out[0].reserved_display_rows, 10);
    }

    #[test]
    fn width_hint_honoured_up_to_pane_width() {
        // 1000×500 native, hint=320, pane=800 → 320×160 DIP. Line 20 → 8 rows.
        let inputs = vec![input(0, true, Some((1000, 500)), Some(320))];
        let out = compute_image_row_reservations(&inputs, 20.0, 800.0);
        assert_eq!(out[0].reserved_display_rows, 8);
    }

    #[test]
    fn width_hint_capped_by_pane_width() {
        // hint=2000, pane=800 → 800×400 DIP. Line 20 → 20 rows.
        let inputs = vec![input(0, true, Some((1000, 500)), Some(2000))];
        let out = compute_image_row_reservations(&inputs, 20.0, 800.0);
        assert_eq!(out[0].reserved_display_rows, 20);
    }

    #[test]
    fn ceiling_rounds_partial_row_up() {
        // Image 805×400, pane=800 → 800×397.5 DIP. Line 20 → 19.875 → 20 rows.
        let inputs = vec![input(0, true, Some((805, 400)), None)];
        let out = compute_image_row_reservations(&inputs, 20.0, 800.0);
        assert_eq!(out[0].reserved_display_rows, 20);
    }

    #[test]
    fn output_is_sorted_by_source_line() {
        let inputs = vec![
            input(10, true, Some((800, 400)), None),
            input(3, true, Some((800, 200)), None),
            input(7, true, Some((800, 600)), None),
        ];
        let out = compute_image_row_reservations(&inputs, 20.0, 800.0);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].source_line.raw(), 3);
        assert_eq!(out[1].source_line.raw(), 7);
        assert_eq!(out[2].source_line.raw(), 10);
    }

    #[test]
    fn duplicate_source_lines_coalesce_to_max_rows() {
        // Two images on the same source line — line reserves the taller.
        let inputs = vec![
            input(4, true, Some((800, 200)), None),
            input(4, true, Some((800, 800)), None),
        ];
        let out = compute_image_row_reservations(&inputs, 20.0, 800.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source_line.raw(), 4);
        // The 800-tall image wins: 800 DIP / 20 = 40 rows.
        assert_eq!(out[0].reserved_display_rows, 40);
    }

    #[test]
    fn zero_line_height_yields_nothing_defensively() {
        let inputs = vec![input(0, true, Some((800, 400)), None)];
        let out = compute_image_row_reservations(&inputs, 0.0, 800.0);
        assert!(out.is_empty());
    }

    #[test]
    fn zero_pane_width_yields_nothing_defensively() {
        let inputs = vec![input(0, true, Some((800, 400)), None)];
        let out = compute_image_row_reservations(&inputs, 20.0, 0.0);
        assert!(out.is_empty());
    }

    #[test]
    fn pathological_height_clamped_to_sanity_bound() {
        // 1×50_000 native at 800 pane → 800×40_000_000 DIP. Clamped to
        // the 1000-row sanity bound.
        let inputs = vec![input(0, true, Some((1, 50_000)), None)];
        let out = compute_image_row_reservations(&inputs, 20.0, 800.0);
        assert_eq!(out[0].reserved_display_rows, 1000);
    }
}
