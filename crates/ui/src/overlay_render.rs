//! Translate the active overlay state into a [`continuity_render::OverlayDraw`].
//!
//! Pure layout / theme decisions; no Win32, no editor mutation. Returns
//! `None` when no overlay is active. Returned strings are owned by the
//! `OverlayDraw`.

use continuity_command::Registry;
use continuity_keymap::Keymap;
use continuity_render::{FocusField, FooterText, ListRow, OverlayDraw, PanelStyle, Rect, Rgba};

use crate::overlays::Overlays;
use crate::quick_open::QuickOpen;

pub(crate) const ROW_HEIGHT: f32 = 22.0;
pub(crate) const PANEL_BG: Rgba = Rgba {
    r: 0.13,
    g: 0.14,
    b: 0.16,
    a: 0.96,
};
pub(crate) const PANEL_BORDER: Rgba = Rgba {
    r: 0.28,
    g: 0.30,
    b: 0.34,
    a: 1.0,
};
pub(crate) const PANEL_SHADOW: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.45,
};
pub(crate) const PRIMARY_FG: Rgba = Rgba {
    r: 0.92,
    g: 0.92,
    b: 0.94,
    a: 1.0,
};
pub(crate) const SECONDARY_FG: Rgba = Rgba {
    r: 0.62,
    g: 0.64,
    b: 0.70,
    a: 1.0,
};
pub(crate) const PLACEHOLDER_FG: Rgba = Rgba {
    r: 0.50,
    g: 0.52,
    b: 0.58,
    a: 1.0,
};
pub(crate) const FOCUS_RING: Rgba = Rgba {
    r: 0.45,
    g: 0.65,
    b: 1.0,
    a: 1.0,
};
pub(crate) const ROW_SELECTED_BG: Rgba = Rgba {
    r: 0.22,
    g: 0.32,
    b: 0.55,
    a: 0.9,
};
pub(crate) const INPUT_SELECTION_BG: Rgba = Rgba {
    r: 0.27,
    g: 0.43,
    b: 0.78,
    a: 0.82,
};
pub(crate) const CARET_COLOR: Rgba = Rgba {
    r: 1.0,
    g: 0.55,
    b: 0.0,
    a: 1.0,
};

/// Build the overlay draw payload, if any. The returned [`OverlayDraw`] owns
/// every string it carries.
#[must_use]
pub(crate) fn build_overlay_draw(
    overlays: &Overlays,
    keymap: &Keymap,
    registry: &Registry,
    width: f32,
    height: f32,
    input_focused: bool,
) -> Option<OverlayDraw> {
    let _ = keymap;
    if !overlays.is_active() {
        return None;
    }
    let panel_w = (width * 0.6).clamp(360.0, 720.0);
    let panel_x = (width - panel_w) / 2.0;
    match overlays {
        Overlays::Idle => None,
        Overlays::Find(fb) => Some(layout_find_bar(fb, panel_x, panel_w, height, input_focused)),
        Overlays::FindInAll(fia) => Some(layout_find_in_all(
            fia,
            panel_x,
            panel_w,
            height,
            input_focused,
        )),
        Overlays::Palette(p) => Some(layout_palette(
            p,
            registry,
            panel_x,
            panel_w,
            height,
            input_focused,
        )),
        Overlays::QuickOpen(q) => Some(layout_quick_open(
            q,
            panel_x,
            panel_w,
            height,
            input_focused,
        )),
        Overlays::GotoLine(g) => Some(layout_goto_line(g, panel_x, panel_w, input_focused)),
        Overlays::GotoHeading(g) => Some(layout_goto_heading(
            g,
            panel_x,
            panel_w,
            height,
            input_focused,
        )),
        Overlays::FontPicker(fp) => Some(layout_font_picker(
            fp,
            panel_x,
            panel_w,
            height,
            input_focused,
        )),
        Overlays::ThemePicker(tp) => Some(layout_theme_picker(
            tp,
            panel_x,
            panel_w,
            height,
            input_focused,
        )),
        Overlays::TabSwitcher(ts) => Some(layout_tab_switcher(ts, panel_x, panel_w, height)),
        Overlays::SlashPalette(sp) => Some(layout_slash_palette(
            sp,
            panel_x,
            panel_w,
            height,
            input_focused,
        )),
        Overlays::HexPicker(hp) => Some(layout_hex_picker(hp, panel_x, panel_w, input_focused)),
        Overlays::PreviousBufferBrowser(b) => Some(layout_previous_buffer_browser(
            b,
            panel_x,
            panel_w,
            height,
            input_focused,
        )),
    }
}

fn layout_hex_picker(
    hp: &crate::hex_picker::HexPicker,
    panel_x: f32,
    panel_w: f32,
    input_focused: bool,
) -> OverlayDraw {
    let panel_y = 8.0;
    let panel_h = 56.0;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let footer_text = match hp.digit_count() {
        0 => "rgb · rgba · rrggbb · rrggbbaa".to_string(),
        3 | 4 | 6 | 8 => "Enter to apply".to_string(),
        n => format!("{n} digits — need 3 / 4 / 6 / 8"),
    };
    OverlayDraw {
        panel,
        input_focused,
        focus_field: Some(FocusField {
            rect: Rect::new(panel_x + 12.0, panel_y + 12.0, panel_w - 24.0, 24.0),
            text: hp.digits().to_string(),
            placeholder: Some("hex…".into()),
            caret_byte: hp.input.caret,
            selection_range: hp.input.selection_range(),
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

// `layout_font_picker` / `layout_theme_picker` / `layout_tab_switcher`
// / `layout_slash_palette` live in `overlay_render_pickers.rs` to
// keep this file under the 600-line cap.
pub(crate) use crate::overlay_render_pickers::{
    layout_font_picker, layout_previous_buffer_browser, layout_slash_palette, layout_tab_switcher,
    layout_theme_picker,
};

pub(crate) fn make_panel(rect: Rect) -> PanelStyle {
    PanelStyle {
        rect,
        corner_radius: 8.0,
        bg: PANEL_BG,
        border: PANEL_BORDER,
        shadow: PANEL_SHADOW,
        shadow_offset: 4.0,
    }
}

// `layout_find_bar` / `layout_find_in_all` live in
// `overlay_render_find.rs` so this file stays under the cap.
pub(crate) use crate::overlay_render_find::{layout_find_bar, layout_find_in_all};
pub(crate) use crate::overlay_render_goto::{layout_goto_heading, layout_goto_line};
pub(crate) use crate::overlay_render_palette::layout_palette;

fn layout_quick_open(
    q: &QuickOpen,
    panel_x: f32,
    panel_w: f32,
    height: f32,
    input_focused: bool,
) -> OverlayDraw {
    let panel_y = 8.0;
    let max_rows = ((height - 60.0) / ROW_HEIGHT).floor().max(4.0) as usize;
    let visible = q.filtered.len().min(max_rows);
    let panel_h = 56.0 + ROW_HEIGHT * visible as f32;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let mut rows: Vec<ListRow> = Vec::with_capacity(visible);
    for (row_idx, &i) in q.filtered.iter().take(visible).enumerate() {
        let entry = &q.all[i];
        let y = panel_y + 44.0 + row_idx as f32 * ROW_HEIGHT;
        rows.push(ListRow {
            rect: Rect::new(panel_x + 6.0, y, panel_w - 12.0, ROW_HEIGHT),
            primary_text: entry.title.clone(),
            secondary_text: Some(entry.first_line.clone()),
            keybinding: None,
            fg: PRIMARY_FG,
            secondary_fg: SECONDARY_FG,
            bg: if row_idx == q.selected {
                Some(ROW_SELECTED_BG)
            } else {
                None
            },
            disabled: false,
        });
    }
    let footer_text = format!("{} of {}", visible, q.all.len());
    OverlayDraw {
        panel,
        input_focused,
        focus_field: Some(FocusField {
            rect: Rect::new(panel_x + 12.0, panel_y + 12.0, panel_w - 24.0, 24.0),
            text: q.input.text.clone(),
            placeholder: Some("Open buffer…".into()),
            caret_byte: q.input.caret,
            selection_range: q.input.selection_range(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::PaletteEntry;
    use crate::quick_open::QuickOpenEntry;

    fn empty_keymap() -> Keymap {
        Keymap::default()
    }

    #[test]
    fn idle_returns_none() {
        let o = Overlays::idle();
        let r = build_overlay_draw(&o, &empty_keymap(), &Registry::new(), 1200.0, 800.0, true);
        assert!(r.is_none());
    }

    #[test]
    fn find_overlay_renders_with_panel() {
        let mut o = Overlays::idle();
        o.open(crate::overlays::OverlayKind::Find);
        if let Some(fb) = o.find_bar_mut() {
            fb.query_input.set_text("hello");
        }
        let r = build_overlay_draw(&o, &empty_keymap(), &Registry::new(), 1200.0, 800.0, true);
        let draw = r.unwrap();
        assert!(draw.panel.rect.w > 0.0);
        assert!(draw.focus_field.is_some());
    }

    #[test]
    fn slash_palette_renders_safelist_rows() {
        use crate::slash_palette::{SlashPaletteEntry, SlashTrigger};
        let entries = vec![
            SlashPaletteEntry {
                command: "markdown.insert_toc".into(),
                label: "Insert TOC".into(),
                description: Some("table of contents".into()),
                keybinding: Some("Ctrl+T".into()),
                applicable: true,
            },
            SlashPaletteEntry {
                command: "markdown.insert_table".into(),
                label: "Insert table".into(),
                description: None,
                keybinding: None,
                applicable: true,
            },
        ];
        let mut o = Overlays::idle();
        o.open_slash_palette(entries, 0, SlashTrigger::TypedSlash);
        let r = build_overlay_draw(&o, &empty_keymap(), &Registry::new(), 1200.0, 800.0, true);
        let draw = r.unwrap();
        assert!(draw.focus_field.is_some());
        assert_eq!(draw.list_rows.len(), 2);
        assert_eq!(draw.list_rows[0].primary_text, "Insert TOC");
        assert_eq!(
            draw.list_rows[0].secondary_text.as_deref(),
            Some("table of contents")
        );
        assert_eq!(draw.list_rows[0].keybinding.as_deref(), Some("Ctrl+T"));
        // Row 0 is selected by default.
        assert!(draw.list_rows[0].bg.is_some());
        assert!(draw.list_rows[1].bg.is_none());
        let footer = draw.footer.unwrap();
        assert_eq!(footer.text, "2 of 2");
    }

    fn palette_entry(name: String) -> PaletteEntry {
        PaletteEntry {
            command: name,
            keybinding: None,
            description: None,
            applicable: true,
        }
    }

    #[test]
    fn command_palette_caps_results_at_ten_rows() {
        let mut o = Overlays::idle();
        o.open(crate::overlays::OverlayKind::Palette);
        o.palette_mut().unwrap().set_candidates(
            (0..12)
                .map(|i| palette_entry(format!("cmd.{i:02}")))
                .collect(),
        );
        let r = build_overlay_draw(&o, &empty_keymap(), &Registry::new(), 1200.0, 800.0, true);
        let draw = r.unwrap();
        assert_eq!(draw.list_rows.len(), 10);
        assert!(draw.scrollbar.is_some());
        assert_eq!(draw.footer.unwrap().text, "1-10 of 12");
    }

    #[test]
    fn command_palette_layout_uses_scrolled_visible_rows() {
        let mut o = Overlays::idle();
        o.open(crate::overlays::OverlayKind::Palette);
        let palette = o.palette_mut().unwrap();
        palette.set_candidates(
            (0..12)
                .map(|i| palette_entry(format!("cmd.{i:02}")))
                .collect(),
        );
        palette.step(11);
        let r = build_overlay_draw(&o, &empty_keymap(), &Registry::new(), 1200.0, 800.0, true);
        let draw = r.unwrap();
        assert_eq!(draw.list_rows[0].primary_text, "cmd.02");
        assert_eq!(draw.list_rows[9].primary_text, "cmd.11");
        assert!(draw.list_rows[9].bg.is_some());
        assert_eq!(draw.footer.unwrap().text, "3-12 of 12");
    }

    #[test]
    fn tab_switcher_renders_positional_rows() {
        use crate::pane_tree::TabId;
        use crate::tab_switcher::TabSwitcherRow;
        let a = TabId::fresh();
        let b = TabId::fresh();
        let c = TabId::fresh();
        let rows = vec![
            TabSwitcherRow {
                tab_id: a,
                buffer_id: continuity_buffer::BufferId::new(),
                title: "alpha".into(),
                subtitle: String::new(),
                dirty: false,
            },
            TabSwitcherRow {
                tab_id: b,
                buffer_id: continuity_buffer::BufferId::new(),
                title: "beta".into(),
                subtitle: "path/beta.md".into(),
                dirty: true,
            },
            TabSwitcherRow {
                tab_id: c,
                buffer_id: continuity_buffer::BufferId::new(),
                title: "gamma".into(),
                subtitle: String::new(),
                dirty: false,
            },
        ];
        let mut o = Overlays::idle();
        o.open_tab_switcher(rows, a, 1);
        let r = build_overlay_draw(&o, &empty_keymap(), &Registry::new(), 1200.0, 800.0, true);
        let draw = r.unwrap();
        // No text input → no focus field.
        assert!(draw.focus_field.is_none());
        // Three positional rows, cursor pre-advanced to row 1 (`beta`).
        assert_eq!(draw.list_rows.len(), 3);
        assert!(draw.list_rows[1].bg.is_some());
        // Dirty marker is prepended on row 1.
        assert!(draw.list_rows[1].primary_text.starts_with('●'));
        // 1-indexed positional keybinding hints.
        assert_eq!(draw.list_rows[0].keybinding.as_deref(), Some("1"));
        assert_eq!(draw.list_rows[2].keybinding.as_deref(), Some("3"));
        // Footer shows "selected of total".
        let footer = draw.footer.unwrap();
        assert_eq!(footer.text, "2 of 3");
    }

    #[test]
    fn quick_open_renders_rows() {
        let mut o = Overlays::idle();
        o.open(crate::overlays::OverlayKind::QuickOpen);
        if let Some(q) = o.quick_open_mut() {
            q.set_candidates(vec![
                QuickOpenEntry {
                    id: continuity_buffer::BufferId::new(),
                    title: "alpha".into(),
                    first_line: "first line".into(),
                },
                QuickOpenEntry {
                    id: continuity_buffer::BufferId::new(),
                    title: "beta".into(),
                    first_line: "another".into(),
                },
            ]);
        }
        let r = build_overlay_draw(&o, &empty_keymap(), &Registry::new(), 1200.0, 800.0, true);
        let draw = r.unwrap();
        assert_eq!(draw.list_rows.len(), 2);
    }
}
