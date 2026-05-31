//! Command-palette overlay layout.
//!
//! Sibling to [`crate::overlay_render`]; owns the capped result-window
//! projection and scrollbar geometry for the command palette.

use continuity_command::Registry;
use continuity_render::{
    FocusField, FooterText, ListRow, OverlayDraw, OverlayScrollbar, Rect, Rgba,
};

use crate::overlay_render::{
    make_panel, CARET_COLOR, FOCUS_RING, INPUT_SELECTION_BG, PLACEHOLDER_FG, PRIMARY_FG,
    ROW_HEIGHT, ROW_SELECTED_BG, SECONDARY_FG,
};
use crate::palette::Palette;

const PALETTE_SCROLLBAR_TRACK: Rgba = Rgba {
    r: 0.45,
    g: 0.48,
    b: 0.54,
    a: 0.18,
};
const PALETTE_SCROLLBAR_THUMB: Rgba = Rgba {
    r: 0.72,
    g: 0.76,
    b: 0.84,
    a: 0.58,
};

pub(crate) fn layout_palette(
    p: &Palette,
    registry: &Registry,
    panel_x: f32,
    panel_w: f32,
    _height: f32,
    input_focused: bool,
) -> OverlayDraw {
    let panel_y = 8.0;
    let total_rows = p.row_count();
    let visible = p.visible_row_count();
    let visible_range = p.visible_row_range();
    let has_scrollbar = total_rows > visible;
    let panel_h = 56.0 + ROW_HEIGHT * visible as f32;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let mut rows: Vec<ListRow> = Vec::with_capacity(visible);
    let row_w = if has_scrollbar {
        panel_w - 24.0
    } else {
        panel_w - 12.0
    };
    for (visible_idx, row_idx) in visible_range.enumerate() {
        let y = panel_y + 44.0 + visible_idx as f32 * ROW_HEIGHT;
        let rect = Rect::new(panel_x + 6.0, y, row_w, ROW_HEIGHT);
        if row_idx == 0 && p.math_preview.is_some() {
            append_math_row(p, rect, row_idx, &mut rows);
        } else {
            append_command_row(p, registry, rect, row_idx, &mut rows);
        }
    }
    let scrollbar = palette_scrollbar(p, panel_x, panel_w, panel_y, visible);
    let footer_text = palette_footer_text(p.first_visible(), visible, total_rows);
    OverlayDraw {
        panel,
        input_focused,
        focus_field: Some(FocusField {
            rect: Rect::new(panel_x + 12.0, panel_y + 12.0, panel_w - 24.0, 24.0),
            text: p.input.text.clone(),
            placeholder: Some("Run command…".into()),
            caret_byte: p.input.caret,
            selection_range: p.input.selection_range(),
            fg: PRIMARY_FG,
            selection_bg: INPUT_SELECTION_BG,
            placeholder_fg: PLACEHOLDER_FG,
            caret_color: CARET_COLOR,
            focus_ring: FOCUS_RING,
        }),
        secondary_field: None,
        list_rows: rows,
        scrollbar,
        footer: Some(FooterText {
            rect: Rect::new(
                panel_x + 12.0,
                panel_y + panel_h - 22.0,
                panel_w - 24.0,
                18.0,
            ),
            text: footer_text,
            fg: SECONDARY_FG,
        }),
    }
}

fn append_math_row(p: &Palette, rect: Rect, row_idx: usize, rows: &mut Vec<ListRow>) {
    let Some(math) = p.math_preview.as_ref() else {
        return;
    };
    let value_str = crate::palette_math::format_value(math.value);
    rows.push(ListRow {
        rect,
        primary_text: format!("{} = {}", math.expr, value_str),
        secondary_text: None,
        keybinding: Some("Enter / Ctrl+C".into()),
        fg: PRIMARY_FG,
        secondary_fg: SECONDARY_FG,
        bg: (row_idx == p.selected).then_some(ROW_SELECTED_BG),
        disabled: false,
    });
}

fn append_command_row(
    p: &Palette,
    registry: &Registry,
    rect: Rect,
    row_idx: usize,
    rows: &mut Vec<ListRow>,
) {
    let Some(i) = p.command_index_for_row(row_idx) else {
        return;
    };
    let entry = &p.all[i];
    rows.push(ListRow {
        rect,
        primary_text: entry.command.clone(),
        secondary_text: entry
            .description
            .clone()
            .or_else(|| registry.description(&entry.command).map(str::to_owned)),
        keybinding: entry.keybinding.clone(),
        fg: PRIMARY_FG,
        secondary_fg: SECONDARY_FG,
        bg: (row_idx == p.selected).then_some(ROW_SELECTED_BG),
        disabled: !entry.applicable,
    });
}

fn palette_scrollbar(
    p: &Palette,
    panel_x: f32,
    panel_w: f32,
    panel_y: f32,
    visible_rows: usize,
) -> Option<OverlayScrollbar> {
    let total_rows = p.row_count();
    if visible_rows == 0 || total_rows <= visible_rows {
        return None;
    }
    let track_h = ROW_HEIGHT * visible_rows as f32;
    let track = Rect::new(panel_x + panel_w - 10.0, panel_y + 44.0, 4.0, track_h);
    let thumb_h = (track_h * visible_rows as f32 / total_rows as f32)
        .max(18.0)
        .min(track_h);
    let max_first_visible = total_rows.saturating_sub(visible_rows).max(1);
    let scroll_ratio = p.first_visible() as f32 / max_first_visible as f32;
    let thumb_y = track.y + (track_h - thumb_h) * scroll_ratio;
    Some(OverlayScrollbar {
        track,
        thumb: Rect::new(track.x, thumb_y, track.w, thumb_h),
        track_color: PALETTE_SCROLLBAR_TRACK,
        thumb_color: PALETTE_SCROLLBAR_THUMB,
    })
}

fn palette_footer_text(first_visible: usize, visible_rows: usize, total_rows: usize) -> String {
    if total_rows == 0 {
        return "0 of 0".into();
    }
    if total_rows <= visible_rows {
        return format!("{} of {}", total_rows, total_rows);
    }
    let last_visible = (first_visible + visible_rows).min(total_rows);
    format!("{}-{} of {}", first_visible + 1, last_visible, total_rows)
}
