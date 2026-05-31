//! Chord-HUD overlay layout.

use continuity_render::{FooterText, ListRow, OverlayDraw, PanelStyle, Rect, Rgba};

use crate::chord_hud::HudEntry;

const PANEL_W: f32 = 520.0;
const ROW_H: f32 = 24.0;
const PAD: f32 = 12.0;
const MAX_ROWS: usize = 10;
/// Soft cap on the rendered command label. The secondary text rect inside
/// each row is ~45% of the panel's interior width; cutting overlong labels
/// with an ellipsis keeps DirectWrite from word-wrapping the label into the
/// row below it. Tuned against the longest current command id
/// (`view.toggle_current_line_highlight` → "Toggle current line highlight",
/// 29 chars).
const LABEL_MAX_CHARS: usize = 32;

/// Build the passive chord HUD overlay. It never owns input focus.
#[must_use]
pub(crate) fn build_chord_hud_overlay(
    rows: &[HudEntry],
    client_w: f32,
    client_h: f32,
) -> Option<OverlayDraw> {
    if rows.is_empty() {
        return None;
    }
    let count = rows.len().min(MAX_ROWS);
    let panel_h = PAD * 2.0 + count as f32 * ROW_H;
    let x = (client_w - PANEL_W - 18.0).max(12.0);
    let y = 18.0_f32.min((client_h - panel_h).max(12.0));
    let list_rows = rows
        .iter()
        .take(MAX_ROWS)
        .enumerate()
        .map(|(idx, row)| ListRow {
            rect: Rect::new(
                x + PAD,
                y + PAD + idx as f32 * ROW_H,
                PANEL_W - PAD * 2.0,
                ROW_H,
            ),
            primary_text: row.chord_label.clone(),
            secondary_text: Some(humanize_command(&row.command)),
            keybinding: None,
            fg: Rgba {
                r: 0.92,
                g: 0.94,
                b: 0.98,
                a: 1.0,
            },
            secondary_fg: Rgba {
                r: 0.62,
                g: 0.68,
                b: 0.76,
                a: 1.0,
            },
            bg: None,
            disabled: false,
        })
        .collect();
    Some(OverlayDraw {
        panel: PanelStyle {
            rect: Rect::new(x, y, PANEL_W, panel_h),
            corner_radius: 6.0,
            bg: Rgba {
                r: 0.10,
                g: 0.11,
                b: 0.13,
                a: 0.94,
            },
            border: Rgba {
                r: 0.35,
                g: 0.39,
                b: 0.45,
                a: 1.0,
            },
            shadow: Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.30,
            },
            shadow_offset: 3.0,
        },
        input_focused: false,
        focus_field: None,
        secondary_field: None,
        list_rows,
        scrollbar: None,
        footer: Some(FooterText {
            rect: Rect::new(x + PAD, y + panel_h - 1.0, 1.0, 1.0),
            text: String::new(),
            fg: Rgba::TRANSPARENT,
        }),
    })
}

/// Render a command id as a short human label suitable for the HUD's
/// secondary column. Drops the namespace prefix (`editor.copy` → `copy`),
/// turns `_` into spaces, capitalizes the first character, and truncates
/// with an ellipsis past [`LABEL_MAX_CHARS`] so the row never word-wraps
/// onto the one below it.
fn humanize_command(command: &str) -> String {
    let tail = command.rsplit_once('.').map_or(command, |(_, t)| t);
    let mut out = String::with_capacity(tail.len());
    for (idx, ch) in tail.chars().enumerate() {
        if ch == '_' {
            out.push(' ');
        } else if idx == 0 {
            for up in ch.to_uppercase() {
                out.push(up);
            }
        } else {
            out.push(ch);
        }
    }
    if out.chars().count() > LABEL_MAX_CHARS {
        let truncated: String = out.chars().take(LABEL_MAX_CHARS - 1).collect();
        format!("{}…", truncated.trim_end())
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_drops_namespace_and_titlecases() {
        assert_eq!(humanize_command("editor.copy"), "Copy");
        assert_eq!(humanize_command("editor.cut"), "Cut");
        assert_eq!(humanize_command("editor.paste"), "Paste");
    }

    #[test]
    fn humanize_replaces_underscores_with_spaces() {
        assert_eq!(humanize_command("view.toggle_minimap"), "Toggle minimap");
        assert_eq!(humanize_command("markdown.toggle_bold"), "Toggle bold");
    }

    #[test]
    fn humanize_handles_namespaceless_command() {
        assert_eq!(humanize_command("undo"), "Undo");
    }

    #[test]
    fn humanize_truncates_past_cap() {
        let label =
            humanize_command("editor.this_is_a_deliberately_overlong_command_name_for_test");
        assert!(label.chars().count() <= LABEL_MAX_CHARS);
        assert!(label.ends_with('…'));
    }

    #[test]
    fn humanize_does_not_truncate_real_longest_command() {
        // The longest current command id, per the keymap, humanizes to
        // 29 chars — well under the cap.
        let label = humanize_command("view.toggle_current_line_highlight");
        assert_eq!(label, "Toggle current line highlight");
        assert!(!label.ends_with('…'));
    }
}
