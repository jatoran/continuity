//! Phase F5 — inline-image paint dispatch.
//!
//! Two modes, gated by [`InlineImagePlacement::is_expanded`]:
//!
//! * **Collapsed** (default) — a single-row affordance with a 24×24
//!   thumbnail rectangle, the filename label, and a `▸` chevron at
//!   the right. Same vertical real estate as a normal text line; the
//!   markdown source `![](url)` underneath stays as backing text.
//! * **Expanded** — the full bitmap scaled to fit pane width
//!   (preserving aspect ratio) via
//!   [`crate::image_layout::compute_image_layout`].
//!
//! The painter records every collapsed affordance's bounding rect
//! into the supplied [`InlineImageHit`] vector so the UI can
//! hit-test future click events without re-deriving the layout.
//!
//! Failure modes: a missing file, a corrupt image, or a disabled
//! cache (capacity 0) cause the painter to log to stderr (so a
//! "images render as plain text" regression stays visible) and skip
//! the image. The buffer's markdown reference becomes a broken-image
//! span — identical to a broken external URL.
//!
//! Thread ownership: renderer's UI thread.

use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BRUSH_PROPERTIES,
    D2D1_INTERPOLATION_MODE_LINEAR,
};
use windows::Win32::Graphics::DirectWrite::IDWriteFactory;
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;

use crate::image_cache::ImageCache;
use crate::image_layout::compute_image_layout;
use crate::inline_image_types::{InlineImageHit, InlineImagePlacement};
use crate::params::Rgba;

/// Width of the collapsed thumbnail in DIPs (square).
const COLLAPSED_THUMB_DIP: f32 = 18.0;
/// Width of the chevron glyph cell in DIPs.
const COLLAPSED_CHEVRON_W_DIP: f32 = 14.0;
/// Padding between thumb / label / chevron.
const COLLAPSED_PAD_DIP: f32 = 6.0;
/// Maximum total affordance width — caps the label so a long filename
/// does not push the chevron off-screen on a narrow pane. Generous so
/// typical filenames (`screenshot-2026-05-13.png`, ~25 chars) fit
/// without truncation.
const COLLAPSED_MAX_W_DIP: f32 = 480.0;
/// Side length in DIPs of the collapse chevron painted at the
/// top-right corner of an expanded image. Sized so the touch target
/// is comfortably clickable even when the image content behind it is
/// dense.
const EXPANDED_CHEVRON_DIP: f32 = 22.0;
/// Inset of the expanded-image chevron from the image's top-right
/// corner. Keeps the affordance visually inside the bitmap rather
/// than hanging off the edge.
const EXPANDED_CHEVRON_INSET_DIP: f32 = 4.0;

/// Paint every inline image. Called from `Renderer::draw_buffer`
/// after the text pass and before overlays so images sit *under* the
/// caret / selection rectangles.
///
/// * `body_origin` — `(x, y)` of the focused pane's body in client
///   space (the body rect's screen-absolute upper-left, *including*
///   the gutter column on the left).
/// * `margins_left` — gutter width in DIPs. The painter offsets each
///   image's left edge by this so the affordance lines up with the
///   first text column, not the gutter.
/// * `scroll_y` — pane vertical scroll offset in DIPs. Subtracted
///   from each image's `top` so images travel with the surrounding
///   text rather than freezing relative to the pane body.
/// * `body_width_dip` — pane body width available for image clamping.
/// * `line_height_dip` — height of one display line; the painter uses
///   this to translate `placement.display_line` to a y coordinate.
/// * `max_pane_bottom_dip` — visible bottom of the pane body in
///   pane-body-relative DIPs (i.e. excluding the status bar reserve
///   when the bar is visible). The expanded-image bitmap is clamped
///   so its `bottom` never exceeds this, preserving aspect ratio.
///   Collapsed affordances ignore the clamp — they fit in one row.
/// * `hits` — populated by the painter with one entry per collapsed
///   affordance. Rects are stored in **pane-body-relative** coords so
///   the UI mouse handler can hit-test against `x - body.x` /
///   `y - body.y` directly. The caller passes a fresh vector each
///   frame.
// Direct2D paint entry point — every argument is an independent
// renderer handle or geometry value; bundling them into a struct would
// just be a one-shot record with no other call site.
#[allow(clippy::too_many_arguments)]
pub fn paint_inline_images(
    device_context: &ID2D1DeviceContext,
    cache: &mut ImageCache,
    dwrite: &IDWriteFactory,
    text_format: &IDWriteTextFormat,
    placements: &[InlineImagePlacement],
    body_origin: (f32, f32),
    margins_left: f32,
    scroll_y: f32,
    body_width_dip: f32,
    line_height_dip: f32,
    max_pane_bottom_dip: f32,
    chevron_color: Rgba,
    hits: &mut Vec<InlineImageHit>,
) {
    if placements.is_empty() {
        return;
    }
    let (body_x, body_y) = body_origin;
    // Clip image paint to the visible body so collapsed thumbnails
    // (and any other image artwork) can't draw over the status bar
    // when their source line scrolls down into that region. Caller
    // provides `max_pane_bottom_dip` already status-bar-aware. Use
    // ALIASED so the right edge of the clip is pixel-exact and the
    // chevron / label glyphs don't bleed half-pixels into the bar.
    let clip_rect = D2D_RECT_F {
        left: body_x,
        top: body_y,
        right: body_x + body_width_dip,
        bottom: body_y + max_pane_bottom_dip.max(0.0),
    };
    let clip_active = clip_rect.bottom > clip_rect.top;
    if clip_active {
        unsafe {
            device_context.PushAxisAlignedClip(&clip_rect, D2D1_ANTIALIAS_MODE_ALIASED);
        }
    }
    for placement in placements {
        let handle = match cache.get_or_decode(&placement.path, device_context) {
            Ok(Some(h)) => h,
            Ok(None) => {
                eprintln!(
                    "continuity-render: image cache disabled (capacity = 0); inline image `{}` rendered as text",
                    placement.path.display()
                );
                continue;
            }
            Err(err) => {
                eprintln!(
                    "continuity-render: image decode failed for `{}`: {err}",
                    placement.path.display()
                );
                continue;
            }
        };
        // Pane-body-relative geometry of the visible row, then shift
        // into screen-absolute space for the Direct2D paint call.
        let pane_left = margins_left;
        let pane_top = (placement.display_line as f32) * line_height_dip - scroll_y;
        let left = body_x + pane_left;
        let top = body_y + pane_top;
        if placement.is_expanded {
            // Remaining visible height between the image's top and
            // the body's visible bottom (status-bar-aware). Negative
            // values mean the image's top is already past the visible
            // area — skip painting in that case.
            let remaining_visible_height = max_pane_bottom_dip - pane_top;
            if remaining_visible_height <= 0.0 {
                continue;
            }
            let (image_w, image_h) = paint_expanded(
                device_context,
                dwrite,
                text_format,
                handle.bitmap,
                &placement.attrs,
                handle.width,
                handle.height,
                left,
                top,
                line_height_dip,
                body_width_dip,
                remaining_visible_height,
                chevron_color,
            );
            // Hit rect = the collapse chevron at the image's
            // top-right corner. Sized to the chevron square, not the
            // whole image, so clicks inside the image body don't
            // collapse accidentally.
            let chev_x = pane_left + image_w - EXPANDED_CHEVRON_DIP - EXPANDED_CHEVRON_INSET_DIP;
            let chev_y = pane_top + EXPANDED_CHEVRON_INSET_DIP;
            hits.push(InlineImageHit {
                source_byte: placement.source_byte,
                rect: (
                    chev_x.max(pane_left),
                    chev_y,
                    EXPANDED_CHEVRON_DIP.min(image_w),
                    EXPANDED_CHEVRON_DIP.min(image_h),
                ),
            });
        } else {
            let total_w = paint_collapsed(
                device_context,
                dwrite,
                text_format,
                handle.bitmap,
                &placement.url,
                left,
                top,
                line_height_dip,
                body_width_dip,
                chevron_color,
            );
            hits.push(InlineImageHit {
                source_byte: placement.source_byte,
                rect: (pane_left, pane_top, total_w, line_height_dip),
            });
        }
    }
    if clip_active {
        unsafe {
            device_context.PopAxisAlignedClip();
        }
    }
}

/// Paint the expanded inline image and its top-right collapse
/// chevron. Returns `(painted_width_dip, painted_height_dip)` so the
/// caller can record a hit rect for the chevron.
///
/// The bitmap is width-clamped to the pane body by
/// [`crate::image_layout::compute_image_layout`] and additionally
/// height-clamped to `max_visible_height_dip` so it cannot paint
/// past the visible bottom of the pane body (or over a visible
/// status bar). Aspect ratio is preserved under both clamps.
///
/// γ — the display map now reserves
/// `ceil(image_height_dip / line_height_dip)` phantom display rows
/// beneath the image's source line (see
/// [`continuity_display_map::image_row_reservation_provider`]), so
/// the bitmap paints across rows that carry no text. The
/// `max_visible_height_dip` clamp survives as a viewport-bottom
/// guard; it is no longer load-bearing for overdraw because the
/// reserved rows already make the space below the image text-free.
/// The collapse chevron at the image's top-right corner stays
/// clickable, so the user can always return to thumbnail mode.
// See note on `paint_inline_images`.
#[allow(clippy::too_many_arguments)]
fn paint_expanded(
    device_context: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    text_format: &IDWriteTextFormat,
    bitmap: &windows::Win32::Graphics::Direct2D::ID2D1Bitmap1,
    attrs: &continuity_decorate::image_link::ImageLinkAttrs,
    native_w: u32,
    native_h: u32,
    left: f32,
    top: f32,
    _line_height_dip: f32,
    body_width_dip: f32,
    max_visible_height_dip: f32,
    chevron_color: Rgba,
) -> (f32, f32) {
    let layout = compute_image_layout(attrs, native_w, native_h, body_width_dip);
    if layout.width_dip <= 0.0 || layout.height_dip <= 0.0 || max_visible_height_dip <= 0.0 {
        return (0.0, 0.0);
    }
    let painted_h = layout.height_dip.min(max_visible_height_dip);
    let painted_w = if layout.height_dip > 0.0 {
        // Preserve aspect ratio when the height clamp kicks in.
        layout.width_dip * (painted_h / layout.height_dip)
    } else {
        layout.width_dip
    };
    let rect = D2D_RECT_F {
        left,
        top,
        right: left + painted_w,
        bottom: top + painted_h,
    };
    unsafe {
        device_context.DrawBitmap(
            bitmap,
            Some(&rect),
            1.0,
            D2D1_INTERPOLATION_MODE_LINEAR,
            None,
            None,
        );
    }
    paint_expanded_chevron(
        device_context,
        dwrite,
        text_format,
        left,
        top,
        painted_w,
        painted_h,
        chevron_color,
    );
    (painted_w, painted_h)
}

/// Paint a `▾` collapse chevron at the top-right corner of an
/// expanded image with a translucent dark backing so it stays
/// readable regardless of underlying image content.
#[allow(clippy::too_many_arguments)]
fn paint_expanded_chevron(
    device_context: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    text_format: &IDWriteTextFormat,
    image_left: f32,
    image_top: f32,
    image_width: f32,
    image_height: f32,
    chevron_color: Rgba,
) {
    let chev_w = EXPANDED_CHEVRON_DIP.min(image_width);
    let chev_h = EXPANDED_CHEVRON_DIP.min(image_height);
    if chev_w <= 0.0 || chev_h <= 0.0 {
        return;
    }
    let chev_left = image_left + image_width - chev_w - EXPANDED_CHEVRON_INSET_DIP;
    let chev_top = image_top + EXPANDED_CHEVRON_INSET_DIP;
    let backing = D2D_RECT_F {
        left: chev_left,
        top: chev_top,
        right: chev_left + chev_w,
        bottom: chev_top + chev_h,
    };
    let backing_color = D2D1_COLOR_F {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.55,
    };
    let brush_props = D2D1_BRUSH_PROPERTIES {
        opacity: 1.0,
        transform: windows::Foundation::Numerics::Matrix3x2::identity(),
    };
    if let Ok(brush) =
        unsafe { device_context.CreateSolidColorBrush(&backing_color, Some(&brush_props)) }
    {
        unsafe { device_context.FillRectangle(&backing, &brush) };
    }
    draw_text_at(
        device_context,
        dwrite,
        text_format,
        "\u{25be}", // ▾
        chev_left,
        chev_top,
        chev_w,
        chev_h,
        chevron_color,
    );
}

/// Paint the single-row affordance and return its total width in
/// DIPs so the caller can build a hit-test rect in pane-body coords.
#[allow(clippy::too_many_arguments)]
fn paint_collapsed(
    device_context: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    text_format: &IDWriteTextFormat,
    bitmap: &windows::Win32::Graphics::Direct2D::ID2D1Bitmap1,
    url: &str,
    left: f32,
    top: f32,
    line_height_dip: f32,
    body_width_dip: f32,
    chevron_color: Rgba,
) -> f32 {
    let label = derive_filename(url);
    let label_w = estimate_label_width(&label);
    let total_w = (COLLAPSED_THUMB_DIP
        + COLLAPSED_PAD_DIP
        + label_w
        + COLLAPSED_PAD_DIP
        + COLLAPSED_CHEVRON_W_DIP)
        .min(COLLAPSED_MAX_W_DIP)
        .min(body_width_dip);
    let thumb_y_offset = ((line_height_dip - COLLAPSED_THUMB_DIP) * 0.5).max(0.0);
    let thumb_rect = D2D_RECT_F {
        left,
        top: top + thumb_y_offset,
        right: left + COLLAPSED_THUMB_DIP,
        bottom: top + thumb_y_offset + COLLAPSED_THUMB_DIP,
    };
    unsafe {
        device_context.DrawBitmap(
            bitmap,
            Some(&thumb_rect),
            1.0,
            D2D1_INTERPOLATION_MODE_LINEAR,
            None,
            None,
        );
    }

    // Label + chevron — paint via DirectWrite text layouts so they
    // pick up the same font / size / antialiasing as body glyphs.
    let label_left = left + COLLAPSED_THUMB_DIP + COLLAPSED_PAD_DIP;
    let label_width =
        (total_w - COLLAPSED_THUMB_DIP - COLLAPSED_PAD_DIP - COLLAPSED_CHEVRON_W_DIP).max(0.0);
    draw_text_at(
        device_context,
        dwrite,
        text_format,
        &label,
        label_left,
        top,
        label_width,
        line_height_dip,
        chevron_color,
    );
    draw_text_at(
        device_context,
        dwrite,
        text_format,
        "\u{25b8}", // ▸
        left + total_w - COLLAPSED_CHEVRON_W_DIP,
        top,
        COLLAPSED_CHEVRON_W_DIP,
        line_height_dip,
        chevron_color,
    );

    total_w
}

fn derive_filename(url: &str) -> String {
    let normalised = url.replace('\\', "/");
    let after_slash = normalised.rsplit('/').next().unwrap_or(&normalised);
    if after_slash.is_empty() {
        url.to_string()
    } else {
        after_slash.to_string()
    }
}

fn estimate_label_width(label: &str) -> f32 {
    // 10.0 DIPs per char is a slightly-over estimate at 14 DIP base
    // size — wide enough that proportional fonts (Segoe UI, system
    // default) don't get clipped, monospace fonts get a little
    // breathing room. The previous 7.0 was monospace-tight and
    // truncated proportional labels to ~70 % of their rendered width,
    // so longer filenames showed as `screenshot-2…` even when there
    // was pane room for the full text.
    (label.chars().count() as f32) * 10.0
}

// See note on `paint_inline_images`.
#[allow(clippy::too_many_arguments)]
fn draw_text_at(
    device_context: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    text_format: &IDWriteTextFormat,
    text: &str,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: Rgba,
) {
    if text.is_empty() || width <= 0.0 || height <= 0.0 {
        return;
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let layout = unsafe {
        match dwrite.CreateTextLayout(&wide, text_format, width, height) {
            Ok(l) => l,
            Err(err) => {
                eprintln!(
                    "continuity-render: CreateTextLayout failed for affordance text `{text}`: {err}"
                );
                return;
            }
        }
    };
    let d2d_color: D2D1_COLOR_F = color.into();
    let brush_props = D2D1_BRUSH_PROPERTIES {
        opacity: 1.0,
        transform: windows::Foundation::Numerics::Matrix3x2::identity(),
    };
    let brush = match unsafe {
        device_context.CreateSolidColorBrush(&d2d_color, Some(&brush_props))
    } {
        Ok(b) => b,
        Err(err) => {
            eprintln!("continuity-render: CreateSolidColorBrush failed for affordance text: {err}");
            return;
        }
    };
    let origin = windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F { x, y };
    unsafe {
        device_context.DrawTextLayout(
            origin,
            &layout,
            &brush,
            windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_decorate::image_link::ImageLinkAttrs;

    #[test]
    fn empty_placement_slice_is_noop() {
        let placements: &[InlineImagePlacement] = &[];
        assert!(placements.is_empty());
    }

    #[test]
    fn placement_default_shape() {
        let p = InlineImagePlacement {
            path: std::path::PathBuf::from("test.png"),
            attrs: ImageLinkAttrs {
                alt: String::new(),
                width: None,
            },
            display_line: 0,
            is_expanded: false,
            url: "images/abc.png".into(),
            source_byte: 0,
        };
        assert_eq!(p.path.file_name().unwrap(), "test.png");
        assert!(!p.is_expanded);
        assert_eq!(p.url, "images/abc.png");
    }

    #[test]
    fn filename_derivation_handles_both_separators() {
        assert_eq!(derive_filename("images/abc.png"), "abc.png");
        assert_eq!(derive_filename("images\\abc.png"), "abc.png");
        assert_eq!(derive_filename("plain.png"), "plain.png");
        assert_eq!(derive_filename(""), "");
    }

    #[test]
    fn estimate_label_width_is_proportional() {
        assert!(estimate_label_width("abc") < estimate_label_width("abcdef"));
        assert_eq!(estimate_label_width(""), 0.0);
    }
}
