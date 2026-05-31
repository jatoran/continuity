//! Painter for the fenced-code-block copy button overlay.
//!
//! Paints a small rounded rect with a "Copy" / "Copied" / "Failed"
//! label at client-DIP coordinates supplied by the UI. The UI is
//! responsible for ensuring the rect overlaps the painted block (the
//! same hover state drives both paint and hit-test, so the click
//! target is exact).
//!
//! Thread ownership: UI thread (sole owner of the `ID2D1DeviceContext`).

use windows::core::Interface;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteTextFormat, DWRITE_MEASURING_MODE_NATURAL};

use crate::params::colors::MarkdownColors;
use crate::{Error, Rgba};

/// Visible state of the fenced-block copy button at paint time.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CodeCopyButtonFeedback {
    /// Idle / cursor-hover paint.
    None,
    /// Brief "Copied" confirmation — accent background, success label.
    Copied,
    /// Brief "Failed" notice — error-tinted background, error label.
    Failed,
}

/// Per-frame payload for the fenced-block copy button overlay.
///
/// `rect_client` is the button rect in **client DIPs** (the window's
/// own coordinate space — *not* body-relative). The painter resets the
/// D2D transform to identity before drawing so the rect lands at the
/// exact pixels the UI hit-tests on a click.
#[derive(Copy, Clone, Debug)]
pub struct CodeCopyButtonDraw {
    /// Button rect `(x, y, w, h)` in client DIPs.
    pub rect_client: (f32, f32, f32, f32),
    /// `true` when the cursor is currently over the button (a deeper
    /// hover tint paints over the idle surface).
    pub hovered: bool,
    /// Feedback state, which overrides idle/hover colors when active.
    pub feedback: CodeCopyButtonFeedback,
}

/// Corner radius matches the fenced-block panel chrome elsewhere in
/// the renderer (loading overlay uses 6, status chip uses 4 — the
/// button is small so 4 reads as crisper).
const BUTTON_CORNER_RADIUS_DIP: f32 = 4.0;

/// Paint the copy button at its client-DIP rect.
///
/// The caller must already be inside an active `BeginDraw`/`EndDraw`
/// bracket. The function captures the current D2D transform, paints
/// in identity coords (so `rect_client` is interpreted as raw client
/// DIPs), and restores the prior transform before returning.
///
/// `info_brush` is reused if the caller wants to amortize allocation
/// across multiple buttons in one frame; the current renderer paints
/// at most one button per frame so the function builds its own
/// brushes inline.
///
/// # Errors
///
/// Returns [`Error::Graphics`] if D2D brush creation or text layout
/// fails for any reason.
pub fn paint_code_copy_button(
    ctx: &ID2D1DeviceContext,
    format: &IDWriteTextFormat,
    draw: &CodeCopyButtonDraw,
    markdown_colors: &MarkdownColors,
    accent: Rgba,
) -> Result<(), Error> {
    let (label, bg, fg) = colors_for(draw, markdown_colors, accent);
    let (x, y, w, h) = draw.rect_client;
    if w <= 0.0 || h <= 0.0 {
        return Ok(());
    }
    let previous_transform = unsafe {
        let mut t = Matrix3x2::default();
        ctx.GetTransform(&mut t);
        t
    };
    unsafe {
        ctx.SetTransform(&Matrix3x2::identity());
    }
    let render_target: ID2D1RenderTarget = ctx.cast()?;
    let panel_rect = D2D_RECT_F {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    let rounded = D2D1_ROUNDED_RECT {
        rect: panel_rect,
        radiusX: BUTTON_CORNER_RADIUS_DIP,
        radiusY: BUTTON_CORNER_RADIUS_DIP,
    };
    let bg_color: D2D1_COLOR_F = bg.into();
    let bg_brush = unsafe { render_target.CreateSolidColorBrush(&bg_color, None)? };
    unsafe {
        ctx.FillRoundedRectangle(&rounded, &bg_brush);
    }
    // Border uses the block border color at higher alpha for definition.
    let border = Rgba {
        a: (markdown_colors.code_block_border.a + 0.4).min(1.0),
        ..markdown_colors.code_block_border
    };
    let border_color: D2D1_COLOR_F = border.into();
    let border_brush = unsafe { render_target.CreateSolidColorBrush(&border_color, None)? };
    unsafe {
        ctx.DrawRoundedRectangle(&rounded, &border_brush, 1.0, None);
    }
    if !label.is_empty() {
        let fg_color: D2D1_COLOR_F = fg.into();
        let fg_brush = unsafe { render_target.CreateSolidColorBrush(&fg_color, None)? };
        let wide: Vec<u16> = label.encode_utf16().collect();
        let text_rect = D2D_RECT_F {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        };
        unsafe {
            ctx.DrawText(
                &wide,
                format,
                &text_rect,
                &fg_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
    unsafe {
        ctx.SetTransform(&previous_transform);
    }
    Ok(())
}

/// Resolve glyph + background + foreground for the button's current
/// state. Split out for unit testing — the choice of glyph is part
/// of the public contract with the UI's hit-test path.
///
/// Glyph choices:
///
/// - Idle / hovered: `⎘` (U+2398 NEXT PAGE) — the most widely-
///   supported monochrome "copy / duplicate page" glyph; the
///   Windows default symbol fallback (Segoe UI Symbol) carries it.
/// - Copied: `✓` (U+2713 CHECK MARK) — used elsewhere in the
///   codebase for confirmed state.
/// - Failed: `✕` (U+2715 MULTIPLICATION X) — visually distinct
///   from the check without re-using any error-emoji codepoint.
fn colors_for(
    draw: &CodeCopyButtonDraw,
    markdown_colors: &MarkdownColors,
    accent: Rgba,
) -> (&'static str, Rgba, Rgba) {
    match draw.feedback {
        CodeCopyButtonFeedback::Copied => {
            // Confirmed-copy chip: solid accent fill so the eye lands
            // on it immediately (the previous "blend toward fg"
            // landed on a muddy gray and read as janky against the
            // surrounding code block). Foreground stays at a high-
            // contrast neutral so the checkmark pops over the accent.
            let bg = with_alpha(accent, 0.92);
            let fg = pick_high_contrast_foreground(bg);
            ("\u{2713}", bg, fg)
        }
        CodeCopyButtonFeedback::Failed => {
            // Failed-copy chip: same accent treatment but desaturated
            // by mixing toward the block border so it reads
            // separately from a success. No new theme key needed.
            let bg = blend(accent, markdown_colors.code_block_border, 0.55, 0.92);
            let fg = pick_high_contrast_foreground(bg);
            ("\u{2715}", bg, fg)
        }
        CodeCopyButtonFeedback::None => {
            let glyph = "\u{2398}";
            let alpha = if draw.hovered { 0.95 } else { 0.72 };
            let bg = Rgba {
                a: alpha,
                ..markdown_colors.code_block_bg
            };
            (glyph, bg, markdown_colors.code_fg)
        }
    }
}

fn with_alpha(c: Rgba, a: f32) -> Rgba {
    Rgba {
        a: a.clamp(0.0, 1.0),
        ..c
    }
}

/// Choose black or white text for legibility over a coloured chip
/// background, using the perceptual-luma threshold.
fn pick_high_contrast_foreground(bg: Rgba) -> Rgba {
    // ITU-R BT.601 luma — cheap, good enough at button scale.
    let luma = 0.299 * bg.r + 0.587 * bg.g + 0.114 * bg.b;
    if luma > 0.55 {
        Rgba {
            r: 0.05,
            g: 0.05,
            b: 0.05,
            a: 1.0,
        }
    } else {
        Rgba {
            r: 0.97,
            g: 0.97,
            b: 0.97,
            a: 1.0,
        }
    }
}

fn blend(base: Rgba, accent: Rgba, t: f32, alpha: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: base.r * (1.0 - t) + accent.r * t,
        g: base.g * (1.0 - t) + accent.g * t,
        b: base.b * (1.0 - t) + accent.b * t,
        a: alpha.clamp(0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_colors() -> MarkdownColors {
        // Construct one with known sentinel colors so the blend math
        // is easy to verify. Other fields default to TRANSPARENT.
        let mut c = MarkdownColors {
            heading: [Rgba::TRANSPARENT; 6],
            bold: Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            italic: Rgba::TRANSPARENT,
            strikethrough: Rgba::TRANSPARENT,
            code_fg: Rgba {
                r: 0.0,
                g: 1.0,
                b: 0.0,
                a: 1.0,
            },
            code_bg: Rgba::TRANSPARENT,
            code_block_bg: Rgba {
                r: 0.0,
                g: 0.0,
                b: 1.0,
                a: 0.5,
            },
            code_block_border: Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.5,
                a: 0.3,
            },
            blockquote_fg: Rgba::TRANSPARENT,
            blockquote_bar: Rgba::TRANSPARENT,
            link: Rgba::TRANSPARENT,
            footnote: Rgba::TRANSPARENT,
            url: Rgba::TRANSPARENT,
            image_alt: Rgba::TRANSPARENT,
            list_marker: Rgba::TRANSPARENT,
            checkbox_checked: Rgba::TRANSPARENT,
            checkbox_unchecked: Rgba::TRANSPARENT,
            hr: Rgba::TRANSPARENT,
            table_border: Rgba::TRANSPARENT,
            table_header_bg: Rgba::TRANSPARENT,
            table_alignment_bg: Rgba::TRANSPARENT,
            table_active_cell_outline: Rgba::TRANSPARENT,
            inline_highlight_fg: Rgba::TRANSPARENT,
            inline_highlight_bg: Rgba::TRANSPARENT,
            formula_value: Rgba::TRANSPARENT,
            formula_error: Rgba::TRANSPARENT,
        };
        // Avoid "field never read" warnings; markdown colors evolve.
        let _ = &mut c;
        c
    }

    #[test]
    fn idle_glyph_is_copy_icon() {
        let draw = CodeCopyButtonDraw {
            rect_client: (0.0, 0.0, 22.0, 18.0),
            hovered: false,
            feedback: CodeCopyButtonFeedback::None,
        };
        let (glyph, _, _) = colors_for(&draw, &sample_colors(), accent_sample());
        assert_eq!(glyph, "\u{2398}");
    }

    fn accent_sample() -> Rgba {
        // Continuity's caret-orange — what the renderer threads
        // through `EditorColors::caret` at runtime.
        Rgba {
            r: 1.0,
            g: 0.55,
            b: 0.0,
            a: 1.0,
        }
    }

    #[test]
    fn hovered_brightens_background_alpha() {
        let colors = sample_colors();
        let accent = accent_sample();
        let idle = colors_for(
            &CodeCopyButtonDraw {
                rect_client: (0.0, 0.0, 22.0, 18.0),
                hovered: false,
                feedback: CodeCopyButtonFeedback::None,
            },
            &colors,
            accent,
        );
        let hovered = colors_for(
            &CodeCopyButtonDraw {
                rect_client: (0.0, 0.0, 22.0, 18.0),
                hovered: true,
                feedback: CodeCopyButtonFeedback::None,
            },
            &colors,
            accent,
        );
        assert!(
            hovered.1.a > idle.1.a,
            "hovered alpha {} should exceed idle {}",
            hovered.1.a,
            idle.1.a
        );
    }

    #[test]
    fn copied_glyph_replaces_copy_icon() {
        let draw = CodeCopyButtonDraw {
            rect_client: (0.0, 0.0, 22.0, 18.0),
            hovered: false,
            feedback: CodeCopyButtonFeedback::Copied,
        };
        let (glyph, _, _) = colors_for(&draw, &sample_colors(), accent_sample());
        assert_eq!(glyph, "\u{2713}");
    }

    #[test]
    fn failed_glyph_replaces_copy_icon() {
        let draw = CodeCopyButtonDraw {
            rect_client: (0.0, 0.0, 22.0, 18.0),
            hovered: false,
            feedback: CodeCopyButtonFeedback::Failed,
        };
        let (glyph, _, _) = colors_for(&draw, &sample_colors(), accent_sample());
        assert_eq!(glyph, "\u{2715}");
    }

    #[test]
    fn copied_state_uses_accent_background() {
        let colors = sample_colors();
        let accent = accent_sample();
        let copied = colors_for(
            &CodeCopyButtonDraw {
                rect_client: (0.0, 0.0, 22.0, 18.0),
                hovered: false,
                feedback: CodeCopyButtonFeedback::Copied,
            },
            &colors,
            accent,
        );
        // The Copied background should land on the accent hue, not
        // on a muddy mid-blend of code_block_bg + code_fg as the
        // pre-polish version did.
        assert!(
            copied.1.r > 0.7 && copied.1.g > 0.4 && copied.1.b < 0.2,
            "expected accent-colored Copied bg, got rgba({:.2}, {:.2}, {:.2}, {:.2})",
            copied.1.r,
            copied.1.g,
            copied.1.b,
            copied.1.a,
        );
    }

    #[test]
    fn blend_midpoint_averages_components() {
        let base = Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let accent = Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        let out = blend(base, accent, 0.5, 1.0);
        assert!((out.r - 0.5).abs() < 1e-6);
        assert!((out.g - 0.5).abs() < 1e-6);
        assert!((out.b - 0.5).abs() < 1e-6);
        assert!((out.a - 1.0).abs() < 1e-6);
    }
}
