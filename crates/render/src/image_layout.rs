//! Phase F5 Pass 2 — inline-image layout computation.
//!
//! Pure data transformation: given an image's native dimensions, the
//! optional `|<width>` hint from `parse_image_alt`, and the body
//! width of the pane the image will paint in, produce the rectangle
//! the painter should draw the bitmap into.
//!
//! Defaults (spec-delta §L#3, §F5 acceptance criteria 4 + 5):
//! * No hint, native ≤ pane → render native size.
//! * No hint, native > pane → scale down to pane width, preserving
//!   aspect ratio.
//! * Hint present → use the hint, capped at pane width (we deliberately
//!   never scale UP past the user-supplied number — that would be a
//!   layout surprise for an explicit override).
//!
//! Thread ownership: pure, callable from any thread.

use continuity_decorate::image_link::ImageLinkAttrs;

/// Layout output for a single inline image. Dimensions are in DIPs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageLayoutRect {
    /// Painted width in DIPs.
    pub width_dip: f32,
    /// Painted height in DIPs.
    pub height_dip: f32,
}

/// Compute the painted rectangle for one inline image. Preserves
/// aspect ratio in every branch.
///
/// `native_w` / `native_h` are the image's intrinsic pixel
/// dimensions. `pane_width_dip` is the body width available for the
/// image (the caller has already subtracted gutter / sidebar
/// reservations). Zero / negative inputs collapse to a zero
/// rectangle — the caller skips painting.
#[must_use]
pub fn compute_image_layout(
    attrs: &ImageLinkAttrs,
    native_w: u32,
    native_h: u32,
    pane_width_dip: f32,
) -> ImageLayoutRect {
    if native_w == 0 || native_h == 0 || pane_width_dip <= 0.0 {
        return ImageLayoutRect {
            width_dip: 0.0,
            height_dip: 0.0,
        };
    }
    let aspect = native_h as f32 / native_w as f32;
    let target_width_dip = match attrs.width {
        Some(hint) if hint > 0 => {
            // The user picked a specific DIP width. Honour it up to
            // the pane-width clamp.
            (hint as f32).min(pane_width_dip)
        }
        _ => {
            let native_w_dip = native_w as f32;
            if native_w_dip > pane_width_dip {
                pane_width_dip
            } else {
                native_w_dip
            }
        }
    };
    ImageLayoutRect {
        width_dip: target_width_dip,
        height_dip: target_width_dip * aspect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(width: Option<u32>) -> ImageLinkAttrs {
        ImageLinkAttrs {
            alt: String::new(),
            width,
        }
    }

    #[test]
    fn no_hint_native_fits_renders_native() {
        let rect = compute_image_layout(&attrs(None), 400, 200, 800.0);
        assert!((rect.width_dip - 400.0).abs() < f32::EPSILON);
        assert!((rect.height_dip - 200.0).abs() < f32::EPSILON);
    }

    #[test]
    fn no_hint_native_wider_than_pane_clamps_to_pane() {
        let rect = compute_image_layout(&attrs(None), 2000, 1000, 800.0);
        assert!((rect.width_dip - 800.0).abs() < f32::EPSILON);
        // 2:1 aspect ratio preserved.
        assert!((rect.height_dip - 400.0).abs() < f32::EPSILON);
    }

    #[test]
    fn explicit_hint_under_pane_width_is_honoured() {
        let rect = compute_image_layout(&attrs(Some(320)), 1000, 500, 800.0);
        assert!((rect.width_dip - 320.0).abs() < f32::EPSILON);
        assert!((rect.height_dip - 160.0).abs() < f32::EPSILON);
    }

    #[test]
    fn explicit_hint_over_pane_width_clamps_to_pane() {
        let rect = compute_image_layout(&attrs(Some(2000)), 1000, 500, 800.0);
        assert!((rect.width_dip - 800.0).abs() < f32::EPSILON);
        assert!((rect.height_dip - 400.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zero_dimensions_collapse_to_zero_rect() {
        let zw = compute_image_layout(&attrs(None), 0, 100, 800.0);
        assert_eq!(zw.width_dip, 0.0);
        assert_eq!(zw.height_dip, 0.0);
        let zh = compute_image_layout(&attrs(None), 100, 0, 800.0);
        assert_eq!(zh.width_dip, 0.0);
        let zp = compute_image_layout(&attrs(None), 100, 100, 0.0);
        assert_eq!(zp.width_dip, 0.0);
    }

    #[test]
    fn aspect_ratio_preserved_under_clamp() {
        // 16:9 native, narrow pane.
        let rect = compute_image_layout(&attrs(None), 1600, 900, 400.0);
        assert!((rect.width_dip - 400.0).abs() < f32::EPSILON);
        assert!((rect.height_dip - 225.0).abs() < f32::EPSILON);
    }
}
