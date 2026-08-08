//! Direct2D paint for the left file-tree pane.

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_DRAW_TEXT_OPTIONS_CLIP,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, DWRITE_HIT_TEST_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
};

use crate::file_tree::{FileTreeDraw, FileTreeEntryKind, FileTreeRowDraw};
use crate::params::Rgba;
use crate::renderer::Renderer;
use crate::Error;

struct FileTreeRowPaint<'a> {
    ctx: &'a ID2D1DeviceContext,
    factory: &'a IDWriteFactory,
    text_format: &'a IDWriteTextFormat,
    fg: &'a ID2D1SolidColorBrush,
    muted: &'a ID2D1SolidColorBrush,
    folder: &'a ID2D1SolidColorBrush,
    selected: &'a ID2D1SolidColorBrush,
    background: &'a ID2D1SolidColorBrush,
    separator: &'a ID2D1SolidColorBrush,
}

/// Paint the file tree over the current back buffer without presenting.
///
/// The caller must have drawn the normal frame with
/// `Renderer::draw_buffer_no_present`; this pass fills only the file
/// tree's left pane and leaves the rest of the back buffer intact.
pub fn paint_file_tree_no_present(
    renderer: &Renderer,
    draw: &FileTreeDraw,
    text_format: &IDWriteTextFormat,
) -> Result<(), Error> {
    unsafe {
        renderer.d2d_context.BeginDraw();
        paint_file_tree(
            &renderer.d2d_context,
            &renderer.dwrite_factory,
            draw,
            text_format,
        )?;
        renderer.d2d_context.EndDraw(None, None)?;
    }
    Ok(())
}

pub(crate) fn paint_file_tree(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    draw: &FileTreeDraw,
    text_format: &IDWriteTextFormat,
) -> Result<(), Error> {
    let render_target: ID2D1RenderTarget = ctx.cast()?;
    let bg_brush = brush(&render_target, draw.colors.bg)?;
    let fg_brush = brush(&render_target, draw.colors.fg)?;
    let muted_brush = brush(&render_target, draw.colors.muted)?;
    let folder_brush = brush(&render_target, draw.colors.folder_fg)?;
    let selected_brush = brush(&render_target, draw.colors.selected_bg)?;
    let separator_brush = brush(&render_target, draw.colors.separator)?;

    unsafe {
        paint_shell(ctx, draw, &bg_brush, &separator_brush);
        paint_header(ctx, factory, draw, text_format, &fg_brush)?;
        let row_paint = FileTreeRowPaint {
            ctx,
            factory,
            text_format,
            fg: &fg_brush,
            muted: &muted_brush,
            folder: &folder_brush,
            selected: &selected_brush,
            background: &bg_brush,
            separator: &separator_brush,
        };
        paint_rows(&row_paint, draw)?;
        paint_drag_feedback(
            ctx,
            factory,
            draw,
            text_format,
            &fg_brush,
            &selected_brush,
            &separator_brush,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
unsafe fn paint_drag_feedback(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    draw: &FileTreeDraw,
    text_format: &IDWriteTextFormat,
    foreground: &ID2D1SolidColorBrush,
    fill: &ID2D1SolidColorBrush,
    border: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    let Some(drag) = draw.drag.as_ref() else {
        return Ok(());
    };
    if let Some(top) = drag.drop_target_top_dip {
        let target = D2D_RECT_F {
            left: draw.rect.0 + 1.0,
            top,
            right: draw.rect.0 + draw.rect.2 - 1.0,
            bottom: top + draw.row_height_dip,
        };
        ctx.FillRectangle(&target, fill);
        ctx.DrawRectangle(&target, border, 1.0, None);
    }
    let ghost_width = 180.0_f32.min((draw.rect.2 - 8.0).max(60.0));
    let left = (drag.cursor.0 + 12.0).clamp(4.0, (draw.rect.2 - ghost_width - 4.0).max(4.0));
    let top = (drag.cursor.1 + 12.0).clamp(4.0, (draw.rect.3 - draw.row_height_dip - 4.0).max(4.0));
    let ghost = D2D_RECT_F {
        left,
        top,
        right: left + ghost_width,
        bottom: top + draw.row_height_dip,
    };
    ctx.FillRectangle(&ghost, fill);
    ctx.DrawRectangle(&ghost, border, 1.0, None);
    draw_text(
        ctx,
        factory,
        text_format,
        &drag.label,
        D2D_RECT_F {
            left: ghost.left + 6.0,
            top: ghost.top,
            right: ghost.right - 6.0,
            bottom: ghost.bottom,
        },
        foreground,
    )
}

fn brush(target: &ID2D1RenderTarget, color: Rgba) -> Result<ID2D1SolidColorBrush, Error> {
    Ok(unsafe { target.CreateSolidColorBrush(&D2D1_COLOR_F::from(color), None)? })
}

unsafe fn paint_shell(
    ctx: &ID2D1DeviceContext,
    draw: &FileTreeDraw,
    bg: &ID2D1SolidColorBrush,
    separator: &ID2D1SolidColorBrush,
) {
    let (x, y, w, h) = draw.rect;
    let rect = D2D_RECT_F {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    ctx.FillRectangle(&rect, bg);
    let rule = D2D_RECT_F {
        left: x + w - 1.0,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    ctx.FillRectangle(&rule, separator);
}

unsafe fn paint_header(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    draw: &FileTreeDraw,
    text_format: &IDWriteTextFormat,
    brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    let (x, y, w, _) = draw.rect;
    draw_text(
        ctx,
        factory,
        text_format,
        &draw.title,
        D2D_RECT_F {
            left: x + 8.0,
            top: y,
            right: x + w - 8.0,
            bottom: y + draw.header_height_dip,
        },
        brush,
    )
}

unsafe fn paint_rows(row_paint: &FileTreeRowPaint<'_>, draw: &FileTreeDraw) -> Result<(), Error> {
    let (x, y, w, h) = draw.rect;
    let body_top = y + draw.header_height_dip;
    let clip = D2D_RECT_F {
        left: x,
        top: body_top,
        right: x + w,
        bottom: y + h,
    };
    row_paint
        .ctx
        .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
    let result = paint_row_texts(row_paint, draw);
    row_paint.ctx.PopAxisAlignedClip();
    result
}

unsafe fn paint_row_texts(
    row_paint: &FileTreeRowPaint<'_>,
    draw: &FileTreeDraw,
) -> Result<(), Error> {
    let (x, y, w, _) = draw.rect;
    let body_top = y + draw.header_height_dip;
    for (i, row) in draw.rows.iter().enumerate() {
        let absolute_index = draw.first_row_index as usize + i;
        let row_top =
            body_top + absolute_index as f32 * draw.row_height_dip - draw.scroll_offset_dip;
        let row_rect = D2D_RECT_F {
            left: x,
            top: row_top,
            right: x + w,
            bottom: row_top + draw.row_height_dip,
        };
        if row.selected {
            row_paint.ctx.FillRectangle(&row_rect, row_paint.selected);
        }
        let text = row_text(row);
        let indent = 8.0 + row.depth as f32 * 14.0;
        let text_rect = D2D_RECT_F {
            left: x + indent,
            top: row_top,
            right: x + w - 8.0,
            bottom: row_top + draw.row_height_dip,
        };
        let default_brush = match row.kind {
            FileTreeEntryKind::Directory => row_paint.folder,
            FileTreeEntryKind::File => row_paint.fg,
            FileTreeEntryKind::Notice => row_paint.muted,
        };
        let override_brush = row
            .color_override
            .map(|color| brush(&row_paint.ctx.cast()?, color))
            .transpose()?;
        let brush = override_brush.as_ref().unwrap_or(default_brush);
        if let Some(inline_edit) = row.inline_edit.as_ref() {
            let prefix = match row.kind {
                FileTreeEntryKind::Directory if row.expanded => "v ",
                FileTreeEntryKind::Directory => "> ",
                FileTreeEntryKind::File | FileTreeEntryKind::Notice => "  ",
            };
            draw_text(
                row_paint.ctx,
                row_paint.factory,
                row_paint.text_format,
                prefix,
                D2D_RECT_F {
                    left: text_rect.left,
                    top: text_rect.top,
                    right: text_rect.left + 16.0,
                    bottom: text_rect.bottom,
                },
                brush,
            )?;
            paint_inline_edit(
                row_paint,
                inline_edit,
                D2D_RECT_F {
                    left: text_rect.left + 15.0,
                    top: text_rect.top + 1.0,
                    right: text_rect.right,
                    bottom: text_rect.bottom - 1.0,
                },
                brush,
            )?;
            continue;
        }
        draw_text(
            row_paint.ctx,
            row_paint.factory,
            row_paint.text_format,
            &text,
            text_rect,
            brush,
        )?;
    }
    Ok(())
}

unsafe fn paint_inline_edit(
    row_paint: &FileTreeRowPaint<'_>,
    edit: &crate::file_tree::FileTreeInlineEditDraw,
    rect: D2D_RECT_F,
    text_brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    row_paint.ctx.FillRectangle(&rect, row_paint.background);
    row_paint
        .ctx
        .DrawRectangle(&rect, row_paint.separator, 1.0, None);
    let inner = D2D_RECT_F {
        left: rect.left + 3.0,
        top: rect.top,
        right: rect.right - 3.0,
        bottom: rect.bottom,
    };
    let wide: Vec<u16> = edit.text.encode_utf16().collect();
    let layout = row_paint.factory.CreateTextLayout(
        &wide,
        row_paint.text_format,
        (inner.right - inner.left).max(1.0),
        (inner.bottom - inner.top).max(1.0),
    )?;
    layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
    if let Some((start, end)) = edit.selection_range {
        let left = text_offset(&layout, &edit.text, start);
        let right = text_offset(&layout, &edit.text, end);
        if right > left {
            row_paint.ctx.FillRectangle(
                &D2D_RECT_F {
                    left: inner.left + left,
                    top: inner.top,
                    right: inner.left + right,
                    bottom: inner.bottom,
                },
                row_paint.selected,
            );
        }
    }
    row_paint
        .ctx
        .PushAxisAlignedClip(&inner, D2D1_ANTIALIAS_MODE_ALIASED);
    row_paint.ctx.DrawTextLayout(
        D2D_POINT_2F {
            x: inner.left,
            y: inner.top,
        },
        &layout,
        text_brush,
        D2D1_DRAW_TEXT_OPTIONS_CLIP,
    );
    let caret_x = inner.left + text_offset(&layout, &edit.text, edit.caret_byte);
    row_paint.ctx.FillRectangle(
        &D2D_RECT_F {
            left: caret_x,
            top: inner.top + 2.0,
            right: caret_x + 1.5,
            bottom: inner.bottom - 2.0,
        },
        text_brush,
    );
    row_paint.ctx.PopAxisAlignedClip();
    Ok(())
}

unsafe fn text_offset(
    layout: &windows::Win32::Graphics::DirectWrite::IDWriteTextLayout,
    text: &str,
    byte: usize,
) -> f32 {
    let utf16_index = text[..previous_char_boundary(text, byte.min(text.len()))]
        .encode_utf16()
        .count();
    let mut x = 0.0;
    let mut y = 0.0;
    let mut metrics = DWRITE_HIT_TEST_METRICS::default();
    let _ = layout.HitTestTextPosition(
        u32::try_from(utf16_index).unwrap_or(u32::MAX),
        false,
        &mut x,
        &mut y,
        &mut metrics,
    );
    x
}

fn previous_char_boundary(text: &str, byte: usize) -> usize {
    let mut boundary = byte;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn row_text(row: &FileTreeRowDraw) -> String {
    match row.kind {
        FileTreeEntryKind::Directory if row.loading => format!("> {} ...", row.label),
        FileTreeEntryKind::Directory if row.expanded => format!("v {}", row.label),
        FileTreeEntryKind::Directory => format!("> {}", row.label),
        FileTreeEntryKind::File => format!("  {}", row.label),
        FileTreeEntryKind::Notice => format!("  {}", row.label),
    }
}

unsafe fn draw_text(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    rect: D2D_RECT_F,
    brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    let width = (rect.right - rect.left).max(1.0);
    let height = (rect.bottom - rect.top).max(1.0);
    let utf16: Vec<u16> = text.encode_utf16().collect();
    let layout = factory.CreateTextLayout(&utf16, format, width, height)?;
    layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
    ctx.PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_ALIASED);
    ctx.DrawTextLayout(
        D2D_POINT_2F {
            x: rect.left,
            y: rect.top,
        },
        &layout,
        brush,
        D2D1_DRAW_TEXT_OPTIONS_CLIP,
    );
    ctx.PopAxisAlignedClip();
    Ok(())
}
