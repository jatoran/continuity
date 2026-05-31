//! Goto-line and goto-heading overlay layouts.
//!
//! Sibling to [`crate::overlay_render`]; owns only the pure layout
//! translation for goto overlays so the parent module stays under the
//! conventions line cap.

use continuity_render::{FocusField, FooterText, ListRow, OverlayDraw, Rect};

use crate::goto_overlay::{GotoHeading, GotoLine};
use crate::overlay_render::{
    make_panel, CARET_COLOR, FOCUS_RING, INPUT_SELECTION_BG, PLACEHOLDER_FG, PRIMARY_FG,
    ROW_HEIGHT, ROW_SELECTED_BG, SECONDARY_FG,
};

pub(crate) fn layout_goto_line(
    g: &GotoLine,
    panel_x: f32,
    panel_w: f32,
    input_focused: bool,
) -> OverlayDraw {
    let panel_y = 8.0;
    let panel_h = 56.0;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let footer_text = g
        .target()
        .map(|(line, column)| format!("-> line {}, col {}", line + 1, column + 1))
        .unwrap_or_else(|| "Enter <line> or <line>:<col>".into());
    OverlayDraw {
        panel,
        input_focused,
        focus_field: Some(FocusField {
            rect: Rect::new(panel_x + 12.0, panel_y + 12.0, panel_w - 24.0, 24.0),
            text: g.input.text.clone(),
            placeholder: Some("Goto line...".into()),
            caret_byte: g.input.caret,
            selection_range: g.input.selection_range(),
            fg: PRIMARY_FG,
            selection_bg: INPUT_SELECTION_BG,
            placeholder_fg: PLACEHOLDER_FG,
            caret_color: CARET_COLOR,
            focus_ring: FOCUS_RING,
        }),
        secondary_field: None,
        list_rows: Vec::new(),
        scrollbar: None,
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

pub(crate) fn layout_goto_heading(
    g: &GotoHeading,
    panel_x: f32,
    panel_w: f32,
    height: f32,
    input_focused: bool,
) -> OverlayDraw {
    let panel_y = 8.0;
    let max_rows = ((height - 60.0) / ROW_HEIGHT).floor().max(4.0) as usize;
    let visible = g.filtered.len().min(max_rows);
    let panel_h = 56.0 + ROW_HEIGHT * visible as f32;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let mut rows: Vec<ListRow> = Vec::with_capacity(visible);
    for (row_idx, &i) in g.filtered.iter().take(visible).enumerate() {
        let entry = &g.all[i];
        let y = panel_y + 44.0 + row_idx as f32 * ROW_HEIGHT;
        let indent = "  ".repeat(entry.level.saturating_sub(1) as usize);
        rows.push(ListRow {
            rect: Rect::new(panel_x + 6.0, y, panel_w - 12.0, ROW_HEIGHT),
            primary_text: format!("{}{}", indent, entry.text),
            secondary_text: None,
            keybinding: Some(format!("L{}", entry.line + 1)),
            fg: PRIMARY_FG,
            secondary_fg: SECONDARY_FG,
            bg: if row_idx == g.selected {
                Some(ROW_SELECTED_BG)
            } else {
                None
            },
            disabled: false,
        });
    }
    let footer_text = format!("{} of {}", visible, g.all.len());
    OverlayDraw {
        panel,
        input_focused,
        focus_field: Some(FocusField {
            rect: Rect::new(panel_x + 12.0, panel_y + 12.0, panel_w - 24.0, 24.0),
            text: g.input.text.clone(),
            placeholder: Some("Goto heading...".into()),
            caret_byte: g.input.caret,
            selection_range: g.input.selection_range(),
            fg: PRIMARY_FG,
            selection_bg: INPUT_SELECTION_BG,
            placeholder_fg: PLACEHOLDER_FG,
            caret_color: CARET_COLOR,
            focus_ring: FOCUS_RING,
        }),
        secondary_field: None,
        list_rows: rows,
        scrollbar: None,
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
