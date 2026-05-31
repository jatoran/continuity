//! Phase-I1 time-machine slider HUD paint pass.
//!
//! Paints the floating slider strip (band background + track line +
//! named/edit-only ticks + draggable thumb) on top of the buffer body
//! when the time-machine overlay is open. Owned by the render crate
//! because Direct2D / brush handling lives here; the geometry is
//! computed UI-side and handed in via [`TimeMachineHudDraw`] so this
//! module has no knowledge of `SliderGeometry`'s internals or of the
//! UI-thread state that produced it.
//!
//! Call site: [`crate::Renderer::draw_buffer_no_present`] after the
//! pane-chrome pass, before the overlay pass — matches the I1 wire-up
//! doc's "below modal overlays, above buffer body" layering.

use windows::core::{Interface, HSTRING};
use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
};

use crate::overlay::BrushCache;
use crate::params::Rgba;
use crate::Error;

/// One tick on the slider strip (resolved geometry + kind).
#[derive(Debug, Clone, Copy)]
pub struct TimeMachineHudTick {
    /// Center x-coordinate of the tick in client DIPs.
    pub x_dip: f32,
    /// `true` when the tick points at a labelled snapshot — drawn
    /// taller / accented; `false` for edit-only snapshots.
    pub is_named: bool,
}

/// Render-side payload for one frame of the time-machine HUD. Built by
/// the UI layer from its `SliderGeometry` + theme; the renderer just
/// paints it.
///
/// Coordinates are in client DIPs; the renderer assumes an identity
/// transform on entry (matches the chrome / overlay paint passes).
#[derive(Debug, Clone)]
pub struct TimeMachineHudDraw {
    /// Top edge of the HUD band (background fill).
    pub band_top_dip: f32,
    /// Bottom edge of the HUD band.
    pub band_bottom_dip: f32,
    /// Left edge of the strip / track line.
    pub strip_left_dip: f32,
    /// Right edge of the strip / track line.
    pub strip_right_dip: f32,
    /// Vertical center of the strip — where the track line sits and
    /// where ticks / the thumb are anchored.
    pub strip_center_y_dip: f32,
    /// X-position of the thumb on the strip.
    pub thumb_x_dip: f32,
    /// Resolved tick layout (ascending by `x_dip`).
    pub ticks: Vec<TimeMachineHudTick>,
    /// Background fill for the band (typically the editor bg with a
    /// touch of opacity bump so the slider reads against the buffer).
    pub band_color: Rgba,
    /// Track line color.
    pub track_color: Rgba,
    /// Tick color (edit-only).
    pub tick_edit_only_color: Rgba,
    /// Tick color (named snapshot — accented).
    pub tick_named_color: Rgba,
    /// Thumb fill color.
    pub thumb_color: Rgba,
    /// Muted text color used for the temporal labels (date / time
    /// strings flanking the strip). Pick the theme's secondary text /
    /// gutter color so it reads as caption-weight.
    pub text_color: Rgba,
    /// Timestamp / date text painted at the left edge of the strip
    /// (typically the earliest persisted snapshot's date). Empty
    /// string skips the paint.
    pub left_label: String,
    /// Timestamp / date text painted at the right edge of the strip
    /// (typically the head revision's date / time). Empty string
    /// skips the paint.
    pub right_label: String,
    /// Timestamp text painted above the thumb (the previewed
    /// revision's wall-clock timestamp). Empty string skips the
    /// paint — the slider parked at head omits the floating label
    /// since the live header already shows that time.
    pub thumb_label: String,
}

/// Track stroke thickness (DIPs).
const TRACK_STROKE_DIP: f32 = 2.0;

/// Half-width of the thumb fill rectangle (DIPs).
const THUMB_HALF_WIDTH_DIP: f32 = 6.0;

/// Half-height of the thumb fill rectangle (DIPs).
const THUMB_HALF_HEIGHT_DIP: f32 = 10.0;

/// Half-width of every tick mark (DIPs).
const TICK_HALF_WIDTH_DIP: f32 = 1.0;

/// Half-height of an edit-only tick (DIPs).
const TICK_EDIT_ONLY_HALF_HEIGHT_DIP: f32 = 4.0;

/// Half-height of a named-snapshot tick (DIPs).
const TICK_NAMED_HALF_HEIGHT_DIP: f32 = 7.0;

/// Font size (DIPs) for the temporal labels — small caption type.
const LABEL_FONT_SIZE_DIP: f32 = 11.0;

/// Pixel gap between the strip and the flanking date labels.
const LABEL_STRIP_GAP_DIP: f32 = 6.0;

/// Vertical room reserved for one row of label text.
const LABEL_ROW_HEIGHT_DIP: f32 = 14.0;

/// Paint the time-machine HUD onto `ctx`. Must be called inside an
/// active `BeginDraw`/`EndDraw` bracket with an identity transform.
///
/// # Errors
///
/// Returns [`Error::Graphics`] on any underlying Win32 failure.
pub fn paint_time_machine_hud(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    hud: &TimeMachineHudDraw,
) -> Result<(), Error> {
    let render_target: ID2D1RenderTarget = ctx.cast()?;
    let mut brushes = BrushCache::new(&render_target)?;
    // Band background.
    let band_brush = brushes.solid(hud.band_color)?;
    let band_rect = D2D_RECT_F {
        left: hud.strip_left_dip - 12.0,
        top: hud.band_top_dip,
        right: hud.strip_right_dip + 12.0,
        bottom: hud.band_bottom_dip,
    };
    unsafe {
        ctx.FillRectangle(&band_rect, &band_brush);
    }

    // Track line.
    let track_brush = brushes.solid(hud.track_color)?;
    let track_start = D2D_POINT_2F {
        x: hud.strip_left_dip,
        y: hud.strip_center_y_dip,
    };
    let track_end = D2D_POINT_2F {
        x: hud.strip_right_dip,
        y: hud.strip_center_y_dip,
    };
    unsafe {
        ctx.DrawLine(track_start, track_end, &track_brush, TRACK_STROKE_DIP, None);
    }

    // Ticks.
    let edit_only_brush = brushes.solid(hud.tick_edit_only_color)?;
    let named_brush = brushes.solid(hud.tick_named_color)?;
    for tick in &hud.ticks {
        let half_h = if tick.is_named {
            TICK_NAMED_HALF_HEIGHT_DIP
        } else {
            TICK_EDIT_ONLY_HALF_HEIGHT_DIP
        };
        let rect = D2D_RECT_F {
            left: tick.x_dip - TICK_HALF_WIDTH_DIP,
            top: hud.strip_center_y_dip - half_h,
            right: tick.x_dip + TICK_HALF_WIDTH_DIP,
            bottom: hud.strip_center_y_dip + half_h,
        };
        let brush = if tick.is_named {
            &named_brush
        } else {
            &edit_only_brush
        };
        unsafe {
            ctx.FillRectangle(&rect, brush);
        }
    }

    // Thumb.
    let thumb_brush = brushes.solid(hud.thumb_color)?;
    let thumb_rect = D2D_RECT_F {
        left: hud.thumb_x_dip - THUMB_HALF_WIDTH_DIP,
        top: hud.strip_center_y_dip - THUMB_HALF_HEIGHT_DIP,
        right: hud.thumb_x_dip + THUMB_HALF_WIDTH_DIP,
        bottom: hud.strip_center_y_dip + THUMB_HALF_HEIGHT_DIP,
    };
    unsafe {
        ctx.FillRectangle(&thumb_rect, &thumb_brush);
    }

    // Temporal labels: caption-weight strings flanking the strip + a
    // floating timestamp above the thumb. Created on demand from the
    // dwrite factory so the renderer doesn't carry a per-frame text
    // format.
    paint_temporal_labels(ctx, dwrite, &mut brushes, hud)?;

    Ok(())
}

fn paint_temporal_labels(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    brushes: &mut BrushCache,
    hud: &TimeMachineHudDraw,
) -> Result<(), Error> {
    if hud.left_label.is_empty() && hud.right_label.is_empty() && hud.thumb_label.is_empty() {
        return Ok(());
    }
    let text_brush = brushes.solid(hud.text_color)?;
    let label_y = hud.strip_center_y_dip + (TICK_NAMED_HALF_HEIGHT_DIP + LABEL_STRIP_GAP_DIP);
    let label_h = LABEL_ROW_HEIGHT_DIP;

    if !hud.left_label.is_empty() {
        let format = make_label_format(dwrite, DWRITE_TEXT_ALIGNMENT_LEADING)?;
        let rect = D2D_RECT_F {
            left: hud.strip_left_dip,
            top: label_y,
            right: hud.strip_left_dip + 160.0,
            bottom: label_y + label_h,
        };
        let wide: Vec<u16> = hud.left_label.encode_utf16().collect();
        unsafe {
            ctx.DrawText(
                &wide,
                &format,
                &rect,
                &text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
    if !hud.right_label.is_empty() {
        let format = make_label_format(dwrite, DWRITE_TEXT_ALIGNMENT_TRAILING)?;
        let rect = D2D_RECT_F {
            left: hud.strip_right_dip - 160.0,
            top: label_y,
            right: hud.strip_right_dip,
            bottom: label_y + label_h,
        };
        let wide: Vec<u16> = hud.right_label.encode_utf16().collect();
        unsafe {
            ctx.DrawText(
                &wide,
                &format,
                &rect,
                &text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
    if !hud.thumb_label.is_empty() {
        // Floating label above the thumb — centered horizontally so
        // it tracks the thumb position; clamped inside the band so it
        // doesn't fly off the strip near the endpoints.
        let format = make_label_format(dwrite, DWRITE_TEXT_ALIGNMENT_CENTER)?;
        let half_w = 80.0;
        let mut left = hud.thumb_x_dip - half_w;
        let mut right = hud.thumb_x_dip + half_w;
        if left < hud.strip_left_dip {
            let shift = hud.strip_left_dip - left;
            left += shift;
            right += shift;
        }
        if right > hud.strip_right_dip {
            let shift = right - hud.strip_right_dip;
            left -= shift;
            right -= shift;
        }
        let top = hud.strip_center_y_dip - THUMB_HALF_HEIGHT_DIP - LABEL_ROW_HEIGHT_DIP;
        let rect = D2D_RECT_F {
            left,
            top,
            right,
            bottom: top + label_h,
        };
        let wide: Vec<u16> = hud.thumb_label.encode_utf16().collect();
        unsafe {
            ctx.DrawText(
                &wide,
                &format,
                &rect,
                &text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
    Ok(())
}

fn make_label_format(
    dwrite: &IDWriteFactory,
    alignment: windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT,
) -> Result<windows::Win32::Graphics::DirectWrite::IDWriteTextFormat, Error> {
    let family = HSTRING::from("Segoe UI");
    let locale = HSTRING::from("en-us");
    let format = unsafe {
        dwrite.CreateTextFormat(
            &family,
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            LABEL_FONT_SIZE_DIP,
            &locale,
        )?
    };
    unsafe {
        format.SetTextAlignment(alignment)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
    }
    Ok(format)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_hud() -> TimeMachineHudDraw {
        TimeMachineHudDraw {
            band_top_dip: 100.0,
            band_bottom_dip: 140.0,
            strip_left_dip: 24.0,
            strip_right_dip: 376.0,
            strip_center_y_dip: 122.0,
            thumb_x_dip: 200.0,
            ticks: vec![
                TimeMachineHudTick {
                    x_dip: 24.0,
                    is_named: false,
                },
                TimeMachineHudTick {
                    x_dip: 200.0,
                    is_named: true,
                },
                TimeMachineHudTick {
                    x_dip: 376.0,
                    is_named: false,
                },
            ],
            band_color: Rgba {
                r: 0.1,
                g: 0.1,
                b: 0.1,
                a: 0.9,
            },
            track_color: Rgba {
                r: 0.5,
                g: 0.5,
                b: 0.5,
                a: 1.0,
            },
            tick_edit_only_color: Rgba {
                r: 0.6,
                g: 0.6,
                b: 0.6,
                a: 1.0,
            },
            tick_named_color: Rgba {
                r: 1.0,
                g: 0.7,
                b: 0.2,
                a: 1.0,
            },
            thumb_color: Rgba {
                r: 0.9,
                g: 0.9,
                b: 0.9,
                a: 1.0,
            },
            text_color: Rgba {
                r: 0.55,
                g: 0.55,
                b: 0.6,
                a: 1.0,
            },
            left_label: "May 12 14:30".into(),
            right_label: "May 13 09:12".into(),
            thumb_label: "May 12 18:44".into(),
        }
    }

    #[test]
    fn payload_carries_three_ticks_in_ascending_x_order() {
        let h = sample_hud();
        assert_eq!(h.ticks.len(), 3);
        assert!(h.ticks[0].x_dip < h.ticks[1].x_dip);
        assert!(h.ticks[1].x_dip < h.ticks[2].x_dip);
    }

    #[test]
    fn named_tick_is_marked() {
        let h = sample_hud();
        assert!(h.ticks[1].is_named);
        assert!(!h.ticks[0].is_named);
        assert!(!h.ticks[2].is_named);
    }

    #[test]
    fn track_stroke_constant_is_2_dip() {
        assert!((TRACK_STROKE_DIP - 2.0).abs() < f32::EPSILON);
    }
}
