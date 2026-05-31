//! Win32 popup-menu construction for `window_context_menu`.

use continuity_command::{
    MARKDOWN_TABLE_DELETE_COL, MARKDOWN_TABLE_DELETE_ROW, MARKDOWN_TABLE_DELETE_TABLE,
    MARKDOWN_TABLE_INSERT_COL_LEFT, MARKDOWN_TABLE_INSERT_COL_RIGHT,
    MARKDOWN_TABLE_INSERT_ROW_ABOVE, MARKDOWN_TABLE_INSERT_ROW_BELOW, PANE_SPLIT_HORIZONTAL,
    PANE_SPLIT_VERTICAL, TAB_CLOSE, TAB_NEW, WINDOW_NEW_WINDOW,
};
use continuity_keymap::Keymap;
use windows::core::HSTRING;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, TrackPopupMenu, MF_ENABLED, MF_SEPARATOR, MF_STRING,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN,
};

/// Menu-item IDs. Must be non-zero (Win32 treats `0` as "nothing chosen").
pub(super) const ID_TAB_NEW: usize = 1;
pub(super) const ID_WINDOW_NEW: usize = 2;
pub(super) const ID_TAB_CLOSE: usize = 3;
pub(super) const ID_PANE_SPLIT_VERTICAL: usize = 4;
pub(super) const ID_PANE_SPLIT_HORIZONTAL: usize = 5;
pub(super) const ID_CHROME_TOGGLE: usize = 20;
pub(super) const ID_TABLE_INSERT_ROW_ABOVE: usize = 30;
pub(super) const ID_TABLE_INSERT_ROW_BELOW: usize = 31;
pub(super) const ID_TABLE_INSERT_COL_LEFT: usize = 32;
pub(super) const ID_TABLE_INSERT_COL_RIGHT: usize = 33;
pub(super) const ID_TABLE_DELETE_ROW: usize = 34;
pub(super) const ID_TABLE_DELETE_COL: usize = 35;
pub(super) const ID_TABLE_DELETE_TABLE: usize = 36;
pub(super) const ID_TABLE_TOGGLE_WRAP: usize = 37;

pub(super) unsafe fn track_table_cell_menu(
    hwnd: HWND,
    screen_x: i32,
    screen_y: i32,
    keymap: &Keymap,
) -> usize {
    let menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return 0,
    };
    let insert_row_above = HSTRING::from(menu_label(
        "Insert Row Above",
        keymap,
        MARKDOWN_TABLE_INSERT_ROW_ABOVE.as_str(),
    ));
    let insert_row_below = HSTRING::from(menu_label(
        "Insert Row Below",
        keymap,
        MARKDOWN_TABLE_INSERT_ROW_BELOW.as_str(),
    ));
    let insert_col_left = HSTRING::from(menu_label(
        "Insert Column Left",
        keymap,
        MARKDOWN_TABLE_INSERT_COL_LEFT.as_str(),
    ));
    let insert_col_right = HSTRING::from(menu_label(
        "Insert Column Right",
        keymap,
        MARKDOWN_TABLE_INSERT_COL_RIGHT.as_str(),
    ));
    let delete_row = HSTRING::from(menu_label(
        "Delete Row",
        keymap,
        MARKDOWN_TABLE_DELETE_ROW.as_str(),
    ));
    let delete_col = HSTRING::from(menu_label(
        "Delete Column",
        keymap,
        MARKDOWN_TABLE_DELETE_COL.as_str(),
    ));
    let delete_table = HSTRING::from(menu_label(
        "Delete Table",
        keymap,
        MARKDOWN_TABLE_DELETE_TABLE.as_str(),
    ));
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_ENABLED,
        ID_TABLE_INSERT_ROW_ABOVE,
        &insert_row_above,
    );
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_ENABLED,
        ID_TABLE_INSERT_ROW_BELOW,
        &insert_row_below,
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_ENABLED,
        ID_TABLE_INSERT_COL_LEFT,
        &insert_col_left,
    );
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_ENABLED,
        ID_TABLE_INSERT_COL_RIGHT,
        &insert_col_right,
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_ENABLED,
        ID_TABLE_DELETE_ROW,
        &delete_row,
    );
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_ENABLED,
        ID_TABLE_DELETE_COL,
        &delete_col,
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_ENABLED,
        ID_TABLE_DELETE_TABLE,
        &delete_table,
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
    let toggle_wrap = HSTRING::from("Toggle Cell Wrap");
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_ENABLED,
        ID_TABLE_TOGGLE_WRAP,
        &toggle_wrap,
    );
    let chosen = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD,
        screen_x,
        screen_y,
        Some(0),
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    chosen.0 as usize
}

pub(super) unsafe fn track_chrome_toggle_menu(
    hwnd: HWND,
    screen_x: i32,
    screen_y: i32,
    keymap: &Keymap,
    label: &str,
    command_id: &str,
) -> usize {
    let menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return 0,
    };
    let toggle = HSTRING::from(menu_label(label, keymap, command_id));
    let _ = AppendMenuW(menu, MF_STRING | MF_ENABLED, ID_CHROME_TOGGLE, &toggle);
    let chosen = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD,
        screen_x,
        screen_y,
        Some(0),
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    chosen.0 as usize
}

/// Build the tab/pane/window popup menu and run `TrackPopupMenu`. Returns
/// the chosen item id (`0` when dismissed). Safety: caller must own
/// `hwnd`'s UI thread; `TrackPopupMenu` is thread-affine.
pub(super) unsafe fn track_menu(
    hwnd: HWND,
    screen_x: i32,
    screen_y: i32,
    keymap: &Keymap,
) -> usize {
    let menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return 0,
    };
    let new_tab = HSTRING::from(menu_label("New Tab", keymap, TAB_NEW.as_str()));
    let new_win = HSTRING::from(menu_label("New Window", keymap, WINDOW_NEW_WINDOW.as_str()));
    let close_tab = HSTRING::from(menu_label("Close Tab", keymap, TAB_CLOSE.as_str()));
    let split_v = HSTRING::from(menu_label(
        "Split Right",
        keymap,
        PANE_SPLIT_HORIZONTAL.as_str(),
    ));
    let split_h = HSTRING::from(menu_label(
        "Split Down",
        keymap,
        PANE_SPLIT_VERTICAL.as_str(),
    ));
    let _ = AppendMenuW(menu, MF_STRING | MF_ENABLED, ID_TAB_NEW, &new_tab);
    let _ = AppendMenuW(menu, MF_STRING | MF_ENABLED, ID_WINDOW_NEW, &new_win);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING | MF_ENABLED, ID_TAB_CLOSE, &close_tab);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_ENABLED,
        ID_PANE_SPLIT_VERTICAL,
        &split_v,
    );
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_ENABLED,
        ID_PANE_SPLIT_HORIZONTAL,
        &split_h,
    );
    let chosen = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD,
        screen_x,
        screen_y,
        Some(0),
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    chosen.0 as usize
}

/// Assemble a menu label `"Action<tab>Ctrl+T"` when a chord is bound.
fn menu_label(action: &str, keymap: &Keymap, command_id: &str) -> String {
    match keymap
        .first_binding_for_command(command_id)
        .and_then(|b| format_chord_sequence(&b.keys))
    {
        Some(hint) => format!("{action}\t{hint}"),
        None => action.to_string(),
    }
}

fn format_chord_sequence(keys: &[continuity_input::KeyChord]) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    let mut out = String::new();
    for (i, chord) in keys.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&pretty_chord(chord));
    }
    Some(out)
}

fn pretty_chord(chord: &continuity_input::KeyChord) -> String {
    let raw = chord.to_string();
    raw.split('+')
        .map(|seg| {
            if seg.len() == 1 {
                seg.to_ascii_uppercase()
            } else {
                let mut chars = seg.chars();
                match chars.next() {
                    Some(c) => {
                        let mut s = c.to_ascii_uppercase().to_string();
                        s.push_str(chars.as_str());
                        s
                    }
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_keymap::Keymap;

    #[test]
    fn menu_label_includes_chord_when_bound() {
        let km = Keymap::from_toml(
            r#"
[[binding]]
keys = ["ctrl+t"]
command = "tab.new"
"#,
        )
        .unwrap();
        assert_eq!(menu_label("New Tab", &km, "tab.new"), "New Tab\tCtrl+T");
    }

    #[test]
    fn menu_label_omits_hint_when_no_binding() {
        let km = Keymap::from_toml("").unwrap();
        assert_eq!(menu_label("New Tab", &km, "tab.new"), "New Tab");
    }

    #[test]
    fn menu_label_uses_overlay_binding_when_present() {
        let base = Keymap::from_toml(
            r#"
[[binding]]
keys = ["ctrl+t"]
command = "tab.new"
"#,
        )
        .unwrap();
        let overlay = Keymap::from_toml(
            r#"
[[binding]]
keys = ["ctrl+shift+t"]
command = "tab.new"
"#,
        )
        .unwrap();
        let km = Keymap::layered(base, overlay);
        assert_eq!(
            menu_label("New Tab", &km, "tab.new"),
            "New Tab\tCtrl+Shift+T"
        );
    }

    #[test]
    fn pretty_chord_renders_two_step_sequence() {
        let km = Keymap::from_toml(
            r#"
[[binding]]
keys = ["ctrl+k", "ctrl+s"]
command = "file.save_all"
"#,
        )
        .unwrap();
        assert_eq!(
            menu_label("Save All", &km, "file.save_all"),
            "Save All\tCtrl+K, Ctrl+S"
        );
    }
}
