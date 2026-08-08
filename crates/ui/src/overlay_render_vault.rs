//! Draw projection for the compact vault launcher.

use continuity_render::{FocusField, FooterText, ListRow, OverlayDraw, Rect};

use crate::overlay_render::{
    make_panel, CARET_COLOR, FOCUS_RING, INPUT_SELECTION_BG, PLACEHOLDER_FG, PRIMARY_FG,
    ROW_HEIGHT, ROW_SELECTED_BG, SECONDARY_FG,
};
use crate::vault_launcher::VaultLauncher;

pub(crate) fn layout_vault_launcher(
    launcher: &VaultLauncher,
    panel_x: f32,
    panel_w: f32,
    height: f32,
    input_focused: bool,
) -> OverlayDraw {
    let panel_y = 8.0;
    let max_rows = ((height - 82.0) / ROW_HEIGHT).floor().max(4.0) as usize;
    let visible_vaults = launcher.filtered.len().min(max_rows.saturating_sub(2));
    let visible = visible_vaults + 2;
    let panel_h = 78.0 + ROW_HEIGHT * visible as f32;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let mut rows = Vec::with_capacity(visible);
    for (row_index, &vault_index) in launcher.filtered.iter().take(visible_vaults).enumerate() {
        let vault = &launcher.all[vault_index];
        let y = panel_y + 44.0 + row_index as f32 * ROW_HEIGHT;
        rows.push(ListRow {
            rect: Rect::new(panel_x + 6.0, y, panel_w - 12.0, ROW_HEIGHT),
            primary_text: if vault.pinned {
                format!("◆ {}", vault.display_name)
            } else {
                vault.display_name.clone()
            },
            secondary_text: Some(vault.root_path.display().to_string()),
            keybinding: None,
            fg: PRIMARY_FG,
            secondary_fg: SECONDARY_FG,
            bg: (row_index == launcher.selected).then_some(ROW_SELECTED_BG),
            disabled: false,
        });
    }
    for (action_offset, (label, detail)) in [
        ("Browse for Folder…", "Open a folder or existing vault"),
        (
            "Initialize Vault…",
            "Create .continuity configuration in a folder",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let row_index = visible_vaults + action_offset;
        let y = panel_y + 44.0 + row_index as f32 * ROW_HEIGHT;
        rows.push(ListRow {
            rect: Rect::new(panel_x + 6.0, y, panel_w - 12.0, ROW_HEIGHT),
            primary_text: label.into(),
            secondary_text: Some(detail.into()),
            keybinding: None,
            fg: PRIMARY_FG,
            secondary_fg: SECONDARY_FG,
            bg: (launcher.selected == launcher.filtered.len() + action_offset)
                .then_some(ROW_SELECTED_BG),
            disabled: false,
        });
    }
    OverlayDraw {
        panel,
        input_focused,
        focus_field: Some(FocusField {
            rect: Rect::new(panel_x + 12.0, panel_y + 12.0, panel_w - 24.0, 24.0),
            text: launcher.input.text.clone(),
            placeholder: Some("Open a vault…".into()),
            caret_byte: launcher.input.caret,
            selection_range: launcher.input.selection_range(),
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
            text: "Enter open · Ctrl+Enter here · Alt+P pin · Alt+S shortcut · Ctrl+Del forget"
                .into(),
            fg: SECONDARY_FG,
        }),
    }
}
