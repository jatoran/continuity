//! Find-bar and "find in all buffers" overlay layouts.
//!
//! Sibling to [`crate::overlay_render`]; pulled out so the parent stays
//! under the conventions file-length cap. Both painters consume the
//! shared theme constants and `make_panel` helper from the parent.

use continuity_render::{FocusField, FooterText, ListRow, OverlayDraw, Rect, Rgba};

use crate::find_bar::{FindBar, FindFocus, FindScope};
use crate::find_in_all::FindInAll;
use crate::find_regex_help::{FindControl, REGEX_SNIPPETS};
use crate::overlay_render::{
    make_panel, CARET_COLOR, FOCUS_RING, INPUT_SELECTION_BG, PLACEHOLDER_FG, PRIMARY_FG,
    ROW_HEIGHT, ROW_SELECTED_BG, SECONDARY_FG,
};

const FIND_TOGGLE_FG: Rgba = Rgba {
    r: 0.82,
    g: 0.88,
    b: 1.0,
    a: 1.0,
};
const PRESERVE_CASE_SECONDARY_FG: Rgba = Rgba {
    r: 0.55,
    g: 0.76,
    b: 1.0,
    a: 1.0,
};
const FIND_CONTROL_BG: Rgba = Rgba {
    r: 0.18,
    g: 0.20,
    b: 0.24,
    a: 0.96,
};
const FIND_CONTROL_HOVER_BG: Rgba = Rgba {
    r: 0.24,
    g: 0.27,
    b: 0.32,
    a: 0.98,
};
const FIND_CONTROL_ACTIVE_BG: Rgba = Rgba {
    r: 0.26,
    g: 0.36,
    b: 0.58,
    a: 0.98,
};
const REGEX_HELP_BG: Rgba = Rgba {
    r: 0.10,
    g: 0.11,
    b: 0.13,
    a: 0.98,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum FindBarHit {
    /// A chrome control was hit.
    Control(FindControl),
    /// A regex helper row was hit.
    RegexSnippet(usize),
}

#[derive(Copy, Clone)]
struct ControlRect {
    control: FindControl,
    rect: Rect,
}

pub(crate) fn layout_find_bar(
    fb: &FindBar,
    panel_x: f32,
    panel_w: f32,
    height: f32,
    input_focused: bool,
) -> OverlayDraw {
    // G4: anchor bottom (status-bar-adjacent), not top.
    let panel_h = if fb.replace_visible { 124.0 } else { 92.0 };
    let panel_y = (height - panel_h - continuity_render::STATUS_BAR_HEIGHT_DIP - 8.0).max(8.0);
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let mut rows = find_control_rows(fb, panel.rect);
    append_control_tooltip_row(fb, panel.rect, &mut rows);
    append_regex_help_rows(fb, panel.rect, &mut rows);
    let count_text = format!(
        "{}/{}",
        if fb.matches.is_empty() {
            0
        } else {
            fb.current + 1
        },
        fb.matches.len()
    );
    let footer_text = if fb.target_label.is_empty() {
        count_text
    } else {
        format!("{}  {}", count_text, fb.target_label)
    };
    let mk_field = |y, text: String, caret, selection_range, placeholder: &str| FocusField {
        rect: Rect::new(panel_x + 12.0, y, panel_w - 206.0, 24.0),
        text,
        placeholder: Some(placeholder.into()),
        caret_byte: caret,
        selection_range,
        fg: PRIMARY_FG,
        selection_bg: INPUT_SELECTION_BG,
        placeholder_fg: PLACEHOLDER_FG,
        caret_color: CARET_COLOR,
        focus_ring: FOCUS_RING,
    };
    let find_field = mk_field(
        panel_y + 12.0,
        fb.query().to_owned(),
        fb.query_caret(),
        fb.query_input.selection_range(),
        "Find…",
    );
    // G4-UX: dual inputs stacked when replace is visible — focused
    // field gets the caret + focus ring, the other is read-only-looking.
    let (focus_field, secondary_field) = if fb.replace_visible {
        let replace = mk_field(
            panel_y + 44.0,
            fb.replace().to_owned(),
            fb.replace_caret(),
            fb.replace_input.selection_range(),
            "Replace with…",
        );
        match fb.focus {
            FindFocus::Find => (Some(find_field), Some(replace)),
            FindFocus::Replace => (Some(replace), Some(find_field)),
        }
    } else {
        (Some(find_field), None)
    };
    OverlayDraw {
        panel,
        input_focused,
        focus_field,
        secondary_field,
        list_rows: rows,
        scrollbar: None,
        footer: Some(FooterText {
            rect: Rect::new(
                panel_x + 326.0,
                panel_y + panel_h - 29.0,
                (panel_w - 338.0).max(12.0),
                18.0,
            ),
            text: footer_text,
            fg: FIND_TOGGLE_FG,
        }),
    }
}

pub(crate) fn hit_test_find_bar(fb: &FindBar, panel: Rect, x: f32, y: f32) -> Option<FindBarHit> {
    for c in find_control_rects(fb, panel) {
        if contains(c.rect, x, y) {
            return Some(FindBarHit::Control(c.control));
        }
    }
    regex_snippet_index_at(fb, panel, x, y).map(FindBarHit::RegexSnippet)
}

/// Return the hovered find-bar control, preserving regex help hover.
pub(crate) fn hover_find_control(fb: &FindBar, panel: Rect, x: f32, y: f32) -> Option<FindControl> {
    for c in find_control_rects(fb, panel) {
        if contains(c.rect, x, y) {
            return Some(c.control);
        }
    }
    if is_inside_regex_help(fb, panel, x, y) {
        return Some(FindControl::Regex);
    }
    None
}

fn find_control_rows(fb: &FindBar, panel: Rect) -> Vec<ListRow> {
    find_control_rects(fb, panel)
        .into_iter()
        .map(|c| {
            let (label, active) = control_label(fb, c.control);
            control_row(fb, c.control, c.rect, label, active)
        })
        .collect()
}

fn find_control_rects(fb: &FindBar, panel: Rect) -> Vec<ControlRect> {
    let mut rects = Vec::with_capacity(10);
    let top_y = panel.y + 12.0;
    let right_x = panel.x + panel.w - 186.0;
    push_control(&mut rects, FindControl::Previous, right_x, top_y, 48.0);
    push_control(&mut rects, FindControl::Next, right_x + 54.0, top_y, 48.0);
    push_control(
        &mut rects,
        FindControl::Replace,
        right_x + 108.0,
        top_y,
        78.0,
    );
    if fb.replace_visible {
        let replace_y = panel.y + 44.0;
        push_control(
            &mut rects,
            FindControl::ReplaceOne,
            right_x + 54.0,
            replace_y,
            56.0,
        );
        push_control(
            &mut rects,
            FindControl::ReplaceAll,
            right_x + 116.0,
            replace_y,
            70.0,
        );
    }
    let toggle_y = panel.y + panel.h - 34.0;
    let mut x = panel.x + 12.0;
    for (control, width) in [
        (FindControl::Case, 38.0),
        (FindControl::Word, 42.0),
        (FindControl::Regex, 38.0),
        (FindControl::PreserveCase, 38.0),
        (FindControl::Scope, 44.0),
        (FindControl::Cursors, 52.0),
    ] {
        push_control(&mut rects, control, x, toggle_y, width);
        x += width + 6.0;
    }
    rects
}

fn push_control(rects: &mut Vec<ControlRect>, control: FindControl, x: f32, y: f32, w: f32) {
    rects.push(ControlRect {
        control,
        rect: Rect::new(x, y, w, 22.0),
    });
}

fn control_label(fb: &FindBar, control: FindControl) -> (&'static str, bool) {
    match control {
        FindControl::Case => ("Aa", fb.case_sensitive),
        FindControl::Word => ("|w|", fb.whole_word),
        FindControl::Regex => (".*", fb.regex),
        FindControl::PreserveCase => ("AB", fb.preserve_case),
        FindControl::Scope => (
            match fb.scope {
                FindScope::Buffer => "All",
                FindScope::Selection => "Sel",
            },
            matches!(fb.scope, FindScope::Selection),
        ),
        FindControl::Replace => ("Replace", fb.replace_visible),
        FindControl::ReplaceOne => ("One", false),
        FindControl::ReplaceAll => ("All", false),
        FindControl::Previous => ("Prev", false),
        FindControl::Next => ("Next", false),
        FindControl::Cursors => ("Cur", false),
    }
}

fn control_row(
    fb: &FindBar,
    control: FindControl,
    rect: Rect,
    label: &'static str,
    active: bool,
) -> ListRow {
    let hovered = fb.hovered_control == Some(control);
    let (primary_text, secondary_text, secondary_fg) = if control == FindControl::PreserveCase {
        (
            "A".to_string(),
            Some("B".to_string()),
            PRESERVE_CASE_SECONDARY_FG,
        )
    } else {
        (label.to_string(), None, SECONDARY_FG)
    };
    ListRow {
        rect,
        primary_text,
        secondary_text,
        keybinding: None,
        fg: PRIMARY_FG,
        secondary_fg,
        bg: Some(if active {
            FIND_CONTROL_ACTIVE_BG
        } else if hovered {
            FIND_CONTROL_HOVER_BG
        } else {
            FIND_CONTROL_BG
        }),
        disabled: false,
    }
}

fn append_control_tooltip_row(fb: &FindBar, panel: Rect, rows: &mut Vec<ListRow>) {
    let Some(control) = fb.hovered_control else {
        return;
    };
    if control == FindControl::Regex {
        return;
    }
    let Some(rect) = find_control_rects(fb, panel)
        .into_iter()
        .find(|c| c.control == control)
        .map(|c| c.rect)
    else {
        return;
    };
    let width = 218.0_f32.min((panel.w - 24.0).max(80.0));
    let x = rect
        .x
        .min(panel.x + panel.w - width - 12.0)
        .max(panel.x + 12.0);
    rows.push(ListRow {
        rect: Rect::new(x, panel.y - ROW_HEIGHT - 6.0, width, ROW_HEIGHT),
        primary_text: control_tooltip_label(control).into(),
        secondary_text: None,
        keybinding: Some(control_hotkey(control).into()),
        fg: FIND_TOGGLE_FG,
        secondary_fg: SECONDARY_FG,
        bg: Some(REGEX_HELP_BG),
        disabled: false,
    });
}

fn control_tooltip_label(control: FindControl) -> &'static str {
    match control {
        FindControl::Case => "Case sensitive",
        FindControl::Word => "Whole word",
        FindControl::PreserveCase => "Preserve case",
        FindControl::Scope => "Search scope",
        FindControl::Replace => "Show replace",
        FindControl::ReplaceOne => "Replace current",
        FindControl::ReplaceAll => "Replace all",
        FindControl::Previous => "Previous match",
        FindControl::Next => "Next match",
        FindControl::Cursors => "Matches to cursors",
        FindControl::Regex => "Regex",
    }
}

fn control_hotkey(control: FindControl) -> &'static str {
    match control {
        FindControl::Case => "Alt+C",
        FindControl::Word => "Alt+W",
        FindControl::Regex => "Alt+R",
        FindControl::PreserveCase => "Alt+P",
        FindControl::Scope => "Alt+S",
        FindControl::Replace => "Ctrl+H",
        FindControl::ReplaceOne => "Ctrl+Enter",
        FindControl::ReplaceAll => "Ctrl+Alt+Enter",
        FindControl::Previous => "Shift+F3",
        FindControl::Next => "F3",
        FindControl::Cursors => "Alt+Enter",
    }
}

fn append_regex_help_rows(fb: &FindBar, panel: Rect, rows: &mut Vec<ListRow>) {
    if fb.hovered_control != Some(FindControl::Regex) {
        return;
    }
    let mut y = regex_help_top(panel);
    rows.push(ListRow {
        rect: Rect::new(panel.x + 12.0, y, 340.0, ROW_HEIGHT),
        primary_text: "Regex quick insert".into(),
        secondary_text: Some("click a row".into()),
        keybinding: Some(control_hotkey(FindControl::Regex).into()),
        fg: FIND_TOGGLE_FG,
        secondary_fg: SECONDARY_FG,
        bg: Some(REGEX_HELP_BG),
        disabled: false,
    });
    y += ROW_HEIGHT;
    for snippet in REGEX_SNIPPETS {
        rows.push(ListRow {
            rect: Rect::new(panel.x + 12.0, y, 340.0, ROW_HEIGHT),
            primary_text: snippet.label.into(),
            secondary_text: Some(snippet.description.into()),
            keybinding: None,
            fg: PRIMARY_FG,
            secondary_fg: SECONDARY_FG,
            bg: Some(REGEX_HELP_BG),
            disabled: false,
        });
        y += ROW_HEIGHT;
    }
}

fn regex_snippet_index_at(fb: &FindBar, panel: Rect, x: f32, y: f32) -> Option<usize> {
    if !is_inside_regex_help(fb, panel, x, y) {
        return None;
    }
    let row = ((y - regex_help_top(panel)) / ROW_HEIGHT).floor() as isize;
    if row <= 0 {
        return None;
    }
    let index = (row - 1) as usize;
    if index < REGEX_SNIPPETS.len() {
        Some(index)
    } else {
        None
    }
}

fn is_inside_regex_help(fb: &FindBar, panel: Rect, x: f32, y: f32) -> bool {
    if fb.hovered_control != Some(FindControl::Regex) {
        return false;
    }
    let help_h = ROW_HEIGHT * (REGEX_SNIPPETS.len() as f32 + 1.0);
    contains(
        Rect::new(panel.x + 12.0, regex_help_top(panel), 340.0, help_h),
        x,
        y,
    )
}

fn regex_help_top(panel: Rect) -> f32 {
    let help_h = ROW_HEIGHT * (REGEX_SNIPPETS.len() as f32 + 1.0);
    (panel.y - help_h - 6.0).max(8.0)
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
}

pub(crate) fn layout_find_in_all(
    fia: &FindInAll,
    panel_x: f32,
    panel_w: f32,
    height: f32,
    input_focused: bool,
) -> OverlayDraw {
    let panel_y = 8.0;
    let max_rows = ((height - 60.0) / ROW_HEIGHT).floor().max(4.0) as usize;
    let visible = fia.flat_rows.len().min(max_rows);
    let panel_h = 56.0 + ROW_HEIGHT * visible as f32;
    let panel = make_panel(Rect::new(panel_x, panel_y, panel_w, panel_h));
    let mut rows: Vec<ListRow> = Vec::with_capacity(visible);
    for (row_idx, row) in fia.flat_rows.iter().take(visible).enumerate() {
        let y = panel_y + 44.0 + row_idx as f32 * ROW_HEIGHT;
        rows.push(ListRow {
            rect: Rect::new(panel_x + 6.0, y, panel_w - 12.0, ROW_HEIGHT),
            primary_text: format!("{}: {}", row.buffer_title, row.line_text),
            secondary_text: None,
            keybinding: Some(format!("L{}", row.line)),
            fg: PRIMARY_FG,
            secondary_fg: SECONDARY_FG,
            bg: if row_idx == fia.selected {
                Some(ROW_SELECTED_BG)
            } else {
                None
            },
            disabled: false,
        });
    }
    let footer_text = format!(
        "{}{}{}{} matches",
        if fia.case_sensitive { "Aa  " } else { "" },
        if fia.whole_word { "\\b  " } else { "" },
        if fia.regex { ".*  " } else { "" },
        fia.flat_rows.len()
    );
    OverlayDraw {
        panel,
        input_focused,
        focus_field: Some(FocusField {
            rect: Rect::new(panel_x + 12.0, panel_y + 12.0, panel_w - 24.0, 24.0),
            text: fia.input.text.clone(),
            placeholder: Some("Find in all buffers…".into()),
            caret_byte: fia.input.caret,
            selection_range: fia.input.selection_range(),
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
