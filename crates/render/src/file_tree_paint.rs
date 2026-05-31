//! Direct2D paint for the left file-tree pane.

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_DRAW_TEXT_OPTIONS_CLIP,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, DWRITE_WORD_WRAPPING_NO_WRAP,
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
        };
        paint_rows(&row_paint, draw)?;
    }
    Ok(())
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
        let brush = match row.kind {
            FileTreeEntryKind::Directory => row_paint.folder,
            FileTreeEntryKind::File => row_paint.fg,
            FileTreeEntryKind::Notice => row_paint.muted,
        };
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
