//! Direct2D paint pass for the metrics-buffer surface.
//!
//! Sibling of [`crate::renderer`] - extracted so the renderer
//! orchestrator stays under the 600-line cap. The pure-data layout it
//! consumes lives in [`crate::metrics_panel`].
//!
//! The panel paints inside the focused pane's body rect. It does not
//! clear the back buffer; the caller is expected to have already
//! invoked `Renderer::draw_buffer_no_present` with an empty-rope body,
//! which paints the regular tab strip, status bar, and pane border
//! chrome. This pass overdraws only the body region.
//!
//! Thread ownership: a [`crate::renderer::Renderer`] is bound to one
//! HWND, so this function is implicitly UI-thread-only.

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_DRAW_TEXT_OPTIONS_CLIP,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteTextFormat, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::DXGI_PRESENT;

use crate::metrics_panel::{LabelDraw, MetricsPanelColors, MetricsPanelLayout, PanelRect};
use crate::renderer::Renderer;
use crate::Error;

/// Paint the metrics-buffer surface inside `layout.background_rect`
/// and present.
///
/// # Errors
///
/// Returns [`Error`] if any Direct2D / DXGI call fails.
pub fn paint_metrics_panel(
    renderer: &Renderer,
    layout: &MetricsPanelLayout,
    colors: MetricsPanelColors,
    text_format: &IDWriteTextFormat,
) -> Result<(), Error> {
    paint_metrics_panel_no_present(renderer, layout, colors, text_format)?;
    unsafe {
        renderer.swap_chain.Present(0, DXGI_PRESENT(0)).ok()?;
    }
    Ok(())
}

/// Paint pass without the final `Present`. Used by capture canaries so
/// the back buffer can be sampled before flip-discard presentation.
///
/// # Errors
///
/// Returns [`Error`] if any Direct2D call fails.
pub fn paint_metrics_panel_no_present(
    renderer: &Renderer,
    layout: &MetricsPanelLayout,
    colors: MetricsPanelColors,
    text_format: &IDWriteTextFormat,
) -> Result<(), Error> {
    let bg = argb_to_d2d(colors.background);
    let fg = argb_to_d2d(colors.foreground);
    let muted = argb_to_d2d(colors.muted_foreground);
    let quiet = argb_to_d2d(colors.heatmap_empty);
    let accent = argb_to_d2d(colors.heatmap_full);
    let activity = argb_to_d2d(colors.sparkline);
    let render_target: ID2D1RenderTarget = renderer.d2d_context.cast()?;
    unsafe {
        renderer.d2d_context.BeginDraw();

        let bg_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&bg, None)?;
        let fg_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&fg, None)?;
        let muted_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&muted, None)?;
        let quiet_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&quiet, None)?;
        let accent_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&accent, None)?;
        let activity_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&activity, None)?;

        let body = panel_rect_to_d2d(layout.background_rect);
        renderer.d2d_context.FillRectangle(&body, &bg_brush);

        draw_label_left(renderer, text_format, &layout.header, &fg_brush)?;
        if let Some(empty) = &layout.empty_state {
            draw_label_left(renderer, text_format, empty, &muted_brush)?;
        }

        for card in &layout.summary_cards {
            let rect = panel_rect_to_d2d(card.rect);
            renderer.d2d_context.FillRectangle(&rect, &quiet_brush);
            let accent_rect = D2D_RECT_F {
                left: card.rect.left,
                top: card.rect.top,
                right: card.rect.left + 2.0,
                bottom: card.rect.bottom,
            };
            renderer
                .d2d_context
                .FillRectangle(&accent_rect, &accent_brush);
            draw_label_left(renderer, text_format, &card.label, &muted_brush)?;
            draw_label_left(renderer, text_format, &card.value, &fg_brush)?;
            draw_label_left(renderer, text_format, &card.detail, &muted_brush)?;
        }

        draw_label_left(
            renderer,
            text_format,
            &layout.activity_heading,
            &muted_brush,
        )?;
        draw_label_left(
            renderer,
            text_format,
            &layout.activity_caption,
            &muted_brush,
        )?;
        for bar in &layout.activity_bars {
            let track = panel_rect_to_d2d(bar.track_rect);
            renderer.d2d_context.FillRectangle(&track, &quiet_brush);
            if bar.words > 0 {
                let fill = panel_rect_to_d2d(bar.fill_rect);
                renderer.d2d_context.FillRectangle(&fill, &activity_brush);
            }
        }
        for (idx, label) in layout.activity_axis_labels.iter().enumerate() {
            if idx + 1 == layout.activity_axis_labels.len() {
                draw_label_right(renderer, text_format, label, &muted_brush)?;
            } else {
                draw_label_left(renderer, text_format, label, &muted_brush)?;
            }
        }

        draw_label_left(
            renderer,
            text_format,
            &layout.top_buffers_heading,
            &muted_brush,
        )?;
        if let Some(empty) = &layout.top_buffers_empty {
            draw_label_left(renderer, text_format, empty, &muted_brush)?;
        }
        for row in &layout.top_buffers_rows {
            let track = panel_rect_to_d2d(row.bar_track);
            renderer.d2d_context.FillRectangle(&track, &quiet_brush);
            let fill = panel_rect_to_d2d(row.bar_fill);
            renderer.d2d_context.FillRectangle(&fill, &accent_brush);
            let sep = D2D_RECT_F {
                left: row.row_rect.left,
                right: row.row_rect.right,
                top: row.row_rect.bottom - 1.0,
                bottom: row.row_rect.bottom,
            };
            renderer.d2d_context.FillRectangle(&sep, &quiet_brush);
            draw_label_left(renderer, text_format, &row.title, &fg_brush)?;
            draw_label_right(renderer, text_format, &row.count, &muted_brush)?;
        }

        draw_label_left(renderer, text_format, &layout.heatmap_heading, &muted_brush)?;
        draw_label_left(renderer, text_format, &layout.heatmap_caption, &muted_brush)?;
        for dow in &layout.dow_labels {
            draw_label_centered(renderer, text_format, dow, &muted_brush)?;
        }
        for cell in &layout.heatmap {
            let color = argb_to_d2d(cell.color);
            let cell_brush: ID2D1SolidColorBrush =
                render_target.CreateSolidColorBrush(&color, None)?;
            let rect = panel_rect_to_d2d(cell.rect);
            renderer.d2d_context.FillRectangle(&rect, &cell_brush);
        }

        renderer.d2d_context.EndDraw(None, None)?;
    }
    Ok(())
}

unsafe fn draw_label_left(
    renderer: &Renderer,
    text_format: &IDWriteTextFormat,
    label: &LabelDraw,
    brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    draw_label(
        renderer,
        text_format,
        label,
        brush,
        DWRITE_TEXT_ALIGNMENT_LEADING,
    )
}

unsafe fn draw_label_right(
    renderer: &Renderer,
    text_format: &IDWriteTextFormat,
    label: &LabelDraw,
    brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    draw_label(
        renderer,
        text_format,
        label,
        brush,
        DWRITE_TEXT_ALIGNMENT_TRAILING,
    )
}

unsafe fn draw_label_centered(
    renderer: &Renderer,
    text_format: &IDWriteTextFormat,
    label: &LabelDraw,
    brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    draw_label(
        renderer,
        text_format,
        label,
        brush,
        DWRITE_TEXT_ALIGNMENT_CENTER,
    )
}

unsafe fn draw_label(
    renderer: &Renderer,
    text_format: &IDWriteTextFormat,
    label: &LabelDraw,
    brush: &ID2D1SolidColorBrush,
    alignment: DWRITE_TEXT_ALIGNMENT,
) -> Result<(), Error> {
    let utf16: Vec<u16> = label.text.encode_utf16().collect();
    let rect = panel_rect_to_d2d(label.rect);
    let layout = renderer.dwrite_factory.CreateTextLayout(
        &utf16,
        text_format,
        (rect.right - rect.left).max(1.0),
        (rect.bottom - rect.top).max(1.0),
    )?;
    layout.SetTextAlignment(alignment)?;
    layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
    layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
    renderer
        .d2d_context
        .PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_ALIASED);
    renderer.d2d_context.DrawTextLayout(
        D2D_POINT_2F {
            x: rect.left,
            y: rect.top,
        },
        &layout,
        brush,
        D2D1_DRAW_TEXT_OPTIONS_CLIP,
    );
    renderer.d2d_context.PopAxisAlignedClip();
    Ok(())
}

fn argb_to_d2d(argb: u32) -> D2D1_COLOR_F {
    let a = ((argb >> 24) & 0xff) as f32 / 255.0;
    let r = ((argb >> 16) & 0xff) as f32 / 255.0;
    let g = ((argb >> 8) & 0xff) as f32 / 255.0;
    let b = (argb & 0xff) as f32 / 255.0;
    D2D1_COLOR_F { r, g, b, a }
}

fn panel_rect_to_d2d(rect: PanelRect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}
