//! Picker-overlay layout (font + theme) split out of `overlay_render.rs`
//! to keep that file under the 600-line cap.
//!
//! Pure layout / theming, no Win32, no editor mutation. Both
//! [`layout_font_picker`] and [`layout_theme_picker`] consume `&` state
//! and return an [`OverlayDraw`] for the renderer.

use continuity_render::{FocusField, FooterText, ListRow, OverlayDraw, Rect};

use crate::font_picker::FontPicker;
use crate::overlay_render::{
    make_panel, CARET_COLOR, FOCUS_RING, INPUT_SELECTION_BG, PLACEHOLDER_FG, PRIMARY_FG,
    ROW_HEIGHT, ROW_SELECTED_BG, SECONDARY_FG,
};
use crate::previous_buffer_browser::PreviousBufferBrowser;
use crate::slash_palette::SlashPalette;
use crate::tab_switcher::TabSwitcher;
use crate::theme_picker::ThemePicker;

/// §E4 — theme picker overlay layout.
pub(crate) fn layout_theme_picker(
    tp: &ThemePicker,
    panel_x: f32,
    panel_w: f32,
    height: f32,
    input_focused: bool,
) -> OverlayDraw {
    let panel_y = 8.0;
    let max_rows = ((height - 60.0) / ROW_HEIGHT).floor().max(4.0) as usize;
    let visible = tp.filtered.len().min(max_rows);
    let panel_h = 56.0 + ROW_HEIGHT * visible as f32;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let mut rows: Vec<ListRow> = Vec::with_capacity(visible);
    for (row_idx, &i) in tp.filtered.iter().take(visible).enumerate() {
        let entry = &tp.all[i];
        let y = panel_y + 44.0 + row_idx as f32 * ROW_HEIGHT;
        let hint = match &entry.source {
            crate::theme_picker::ThemeSource::Bundled => "bundled",
            crate::theme_picker::ThemeSource::UserFile(_) => "user",
        };
        rows.push(ListRow {
            rect: Rect::new(panel_x + 6.0, y, panel_w - 12.0, ROW_HEIGHT),
            primary_text: entry.name.clone(),
            secondary_text: None,
            keybinding: Some(hint.into()),
            fg: PRIMARY_FG,
            secondary_fg: SECONDARY_FG,
            bg: if row_idx == tp.selected {
                Some(ROW_SELECTED_BG)
            } else {
                None
            },
            disabled: false,
        });
    }
    // δ.5 — footer hint differs by row source: bundled rows advertise
    // Enter + Ctrl+D; custom rows additionally advertise Ctrl+E and
    // Ctrl+Backspace. Falls back to the count summary when no row is
    // selected (e.g. the filter eliminated every row).
    let footer_text = tp
        .filtered
        .get(tp.selected)
        .and_then(|i| tp.all.get(*i))
        .map(|entry| {
            crate::window_theme_manage::theme_picker_footer_hint(&entry.source).to_string()
        })
        .unwrap_or_else(|| format!("{} of {}", visible, tp.all.len()));
    OverlayDraw {
        panel,
        input_focused,
        focus_field: Some(FocusField {
            rect: Rect::new(panel_x + 12.0, panel_y + 12.0, panel_w - 24.0, 24.0),
            text: tp.input.text.clone(),
            placeholder: Some("Pick a theme…".into()),
            caret_byte: tp.input.caret,
            selection_range: tp.input.selection_range(),
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

/// §E3 — font picker overlay layout.
pub(crate) fn layout_font_picker(
    fp: &FontPicker,
    panel_x: f32,
    panel_w: f32,
    height: f32,
    input_focused: bool,
) -> OverlayDraw {
    let panel_y = 8.0;
    let max_rows = ((height - 60.0) / ROW_HEIGHT).floor().max(4.0) as usize;
    let visible = fp.filtered.len().min(max_rows);
    let panel_h = 56.0 + ROW_HEIGHT * visible as f32;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let mut rows: Vec<ListRow> = Vec::with_capacity(visible);
    for (row_idx, &i) in fp.filtered.iter().take(visible).enumerate() {
        let name = &fp.all[i];
        let y = panel_y + 44.0 + row_idx as f32 * ROW_HEIGHT;
        rows.push(ListRow {
            rect: Rect::new(panel_x + 6.0, y, panel_w - 12.0, ROW_HEIGHT),
            primary_text: name.clone(),
            secondary_text: None,
            keybinding: if name == &fp.original_family {
                Some("current".into())
            } else {
                None
            },
            fg: PRIMARY_FG,
            secondary_fg: SECONDARY_FG,
            bg: if row_idx == fp.selected {
                Some(ROW_SELECTED_BG)
            } else {
                None
            },
            disabled: false,
        });
    }
    let footer_text = format!("{} of {}", visible, fp.all.len());
    OverlayDraw {
        panel,
        input_focused,
        focus_field: Some(FocusField {
            rect: Rect::new(panel_x + 12.0, panel_y + 12.0, panel_w - 24.0, 24.0),
            text: fp.input.text.clone(),
            placeholder: Some("Pick a font…".into()),
            caret_byte: fp.input.caret,
            selection_range: fp.input.selection_range(),
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

/// §H5 — slash-command palette layout. Sized like the quick-open
/// HUD; rows show the derived display label, optional one-line
/// description as a secondary string, and a keybinding hint when
/// known. The `anchor_line` on the underlying state is consumed by
/// the surrounding window-paint layer for caret-docked geometry —
/// this function just sizes the popup against the supplied panel
/// rect.
pub(crate) fn layout_slash_palette(
    sp: &SlashPalette,
    panel_x: f32,
    panel_w: f32,
    height: f32,
    input_focused: bool,
) -> OverlayDraw {
    let panel_y = 8.0;
    let max_rows = ((height - 60.0) / ROW_HEIGHT).floor().max(4.0) as usize;
    let visible = sp.filtered.len().min(max_rows);
    let panel_h = 56.0 + ROW_HEIGHT * visible as f32;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let mut rows: Vec<ListRow> = Vec::with_capacity(visible);
    for (row_idx, &i) in sp.filtered.iter().take(visible).enumerate() {
        let entry = &sp.all[i];
        let y = panel_y + 44.0 + row_idx as f32 * ROW_HEIGHT;
        rows.push(ListRow {
            rect: Rect::new(panel_x + 6.0, y, panel_w - 12.0, ROW_HEIGHT),
            primary_text: entry.label.clone(),
            secondary_text: entry.description.clone(),
            keybinding: entry.keybinding.clone(),
            fg: PRIMARY_FG,
            secondary_fg: SECONDARY_FG,
            bg: if row_idx == sp.selected {
                Some(ROW_SELECTED_BG)
            } else {
                None
            },
            disabled: !entry.applicable,
        });
    }
    let footer_text = format!("{} of {}", visible, sp.all.len());
    OverlayDraw {
        panel,
        input_focused,
        focus_field: Some(FocusField {
            rect: Rect::new(panel_x + 12.0, panel_y + 12.0, panel_w - 24.0, 24.0),
            text: sp.input.text.clone(),
            placeholder: Some("/command…".into()),
            caret_byte: sp.input.caret,
            selection_range: sp.input.selection_range(),
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

/// §H6 — Ctrl+Tab transient tab switcher overlay. Lists tabs in
/// positional order; the highlighted row is the tab that becomes
/// active on Ctrl release / Enter. Sized like the quick-open HUD.
pub(crate) fn layout_tab_switcher(
    ts: &TabSwitcher,
    panel_x: f32,
    panel_w: f32,
    height: f32,
) -> OverlayDraw {
    let panel_y = 8.0;
    let max_rows = ((height - 60.0) / ROW_HEIGHT).floor().max(4.0) as usize;
    let visible = ts.rows.len().min(max_rows);
    // No focus field — this overlay has no text input. Just the
    // header band + the list.
    let header_h: f32 = 32.0;
    let panel_h = header_h + 24.0 + ROW_HEIGHT * visible as f32;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let mut rows: Vec<ListRow> = Vec::with_capacity(visible);
    for (row_idx, row) in ts.rows.iter().take(visible).enumerate() {
        let y = panel_y + header_h + row_idx as f32 * ROW_HEIGHT;
        let primary = if row.dirty {
            format!("● {}", row.title)
        } else {
            row.title.clone()
        };
        let secondary = if row.subtitle.is_empty() {
            None
        } else {
            Some(row.subtitle.clone())
        };
        rows.push(ListRow {
            rect: Rect::new(panel_x + 6.0, y, panel_w - 12.0, ROW_HEIGHT),
            primary_text: primary,
            secondary_text: secondary,
            // 1-indexed positional hint (`Ctrl+N` chord parity).
            keybinding: Some(format!("{}", row_idx + 1)),
            fg: PRIMARY_FG,
            secondary_fg: SECONDARY_FG,
            bg: if row_idx == ts.selected {
                Some(ROW_SELECTED_BG)
            } else {
                None
            },
            disabled: false,
        });
    }
    let footer_text = if ts.rows.is_empty() {
        "no tabs".into()
    } else {
        format!("{} of {}", ts.selected + 1, ts.rows.len())
    };
    OverlayDraw {
        panel,
        input_focused: false,
        focus_field: None,
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

/// δ.4 — previous-buffer browser overlay layout. Mirrors the
/// quick-open / theme-picker shape: focus field on top, scrollable
/// row list below, footer carrying "<filter> N of M" plus chord hints.
pub(crate) fn layout_previous_buffer_browser(
    b: &PreviousBufferBrowser,
    panel_x: f32,
    panel_w: f32,
    height: f32,
    input_focused: bool,
) -> OverlayDraw {
    let panel_y = 8.0;
    let max_rows = ((height - 60.0) / ROW_HEIGHT).floor().max(4.0) as usize;
    let visible = b.filtered.len().min(max_rows);
    let panel_h = 56.0 + ROW_HEIGHT * visible as f32;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let mut rows: Vec<ListRow> = Vec::with_capacity(visible);
    for (row_idx, &i) in b.filtered.iter().take(visible).enumerate() {
        let entry = &b.all[i];
        let y = panel_y + 44.0 + row_idx as f32 * ROW_HEIGHT;
        let primary = if entry.is_trashed {
            format!("[trash] {}", entry.title)
        } else {
            entry.title.clone()
        };
        rows.push(ListRow {
            rect: Rect::new(panel_x + 6.0, y, panel_w - 12.0, ROW_HEIGHT),
            primary_text: primary,
            secondary_text: Some(entry.subtitle.clone()),
            keybinding: None,
            fg: PRIMARY_FG,
            secondary_fg: SECONDARY_FG,
            bg: if row_idx == b.selected {
                Some(ROW_SELECTED_BG)
            } else {
                None
            },
            disabled: false,
        });
    }
    let filter_label = match b.filter {
        continuity_persist::BufferListFilter::ActiveOnly => "Active",
        continuity_persist::BufferListFilter::All => "All",
        continuity_persist::BufferListFilter::TrashedOnly => "Trash",
    };
    let footer_text = format!(
        "{filter_label} {} of {}  ·  Ctrl+T cycle filter  ·  Ctrl+R timeline",
        visible,
        b.all.len()
    );
    OverlayDraw {
        panel,
        input_focused,
        focus_field: Some(FocusField {
            rect: Rect::new(panel_x + 12.0, panel_y + 12.0, panel_w - 24.0, 24.0),
            text: b.input.text.clone(),
            placeholder: Some("Filter buffers…".into()),
            caret_byte: b.input.caret,
            selection_range: b.input.selection_range(),
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
