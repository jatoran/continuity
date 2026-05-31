//! Motion adapter for overlay payloads.
//!
//! The UI schedules and reduces motion; this module applies the supplied
//! opacity/translation to the normal overlay draw tree before delegating to
//! [`crate::overlay::paint_overlay`].

use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};

use crate::motion::SurfaceMotion;
use crate::overlay::{paint_overlay, FocusField, FooterText, ListRow, OverlayDraw, PanelStyle};
use crate::{Error, Rgba};

/// Paint an overlay with an optional surface transform.
///
/// # Errors
///
/// Returns [`Error::Graphics`] on any underlying Win32 failure.
pub fn paint_overlay_with_motion(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    overlay: &OverlayDraw,
    motion: Option<SurfaceMotion>,
) -> Result<(), Error> {
    let motion = motion.unwrap_or_default();
    if motion.opacity <= f32::EPSILON {
        return Ok(());
    }
    if motion.is_identity() {
        return paint_overlay(ctx, dwrite, format, overlay);
    }
    let mut shifted = overlay.clone();
    apply_overlay_motion(&mut shifted, motion);
    paint_overlay(ctx, dwrite, format, &shifted)
}

fn apply_overlay_motion(overlay: &mut OverlayDraw, motion: SurfaceMotion) {
    apply_panel_motion(&mut overlay.panel, motion);
    if let Some(field) = overlay.focus_field.as_mut() {
        apply_field_motion(field, motion);
    }
    if let Some(field) = overlay.secondary_field.as_mut() {
        apply_field_motion(field, motion);
    }
    for row in &mut overlay.list_rows {
        apply_row_motion(row, motion);
    }
    if let Some(footer) = overlay.footer.as_mut() {
        apply_footer_motion(footer, motion);
    }
}

fn apply_panel_motion(panel: &mut PanelStyle, motion: SurfaceMotion) {
    panel.rect = panel.rect.translate(0.0, motion.translate_y_dip);
    panel.bg = fade(panel.bg, motion.opacity);
    panel.border = fade(panel.border, motion.opacity);
    panel.shadow = fade(panel.shadow, motion.opacity);
}

fn apply_field_motion(field: &mut FocusField, motion: SurfaceMotion) {
    field.rect = field.rect.translate(0.0, motion.translate_y_dip);
    field.fg = fade(field.fg, motion.opacity);
    field.selection_bg = fade(field.selection_bg, motion.opacity);
    field.placeholder_fg = fade(field.placeholder_fg, motion.opacity);
    field.caret_color = fade(field.caret_color, motion.opacity);
    field.focus_ring = fade(field.focus_ring, motion.opacity);
}

fn apply_row_motion(row: &mut ListRow, motion: SurfaceMotion) {
    row.rect = row.rect.translate(0.0, motion.translate_y_dip);
    row.fg = fade(row.fg, motion.opacity);
    row.secondary_fg = fade(row.secondary_fg, motion.opacity);
    row.bg = row.bg.map(|c| fade(c, motion.opacity));
}

fn apply_footer_motion(footer: &mut FooterText, motion: SurfaceMotion) {
    footer.rect = footer.rect.translate(0.0, motion.translate_y_dip);
    footer.fg = fade(footer.fg, motion.opacity);
}

fn fade(color: Rgba, opacity: f32) -> Rgba {
    Rgba {
        a: color.a * opacity.clamp(0.0, 1.0),
        ..color
    }
}
