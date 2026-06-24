//! Clickable action buttons for the external-change conflict banner.
//!
//! The conflict banner (`reload / keep mine / show diff`) is a decision
//! surface, so its actions are real buttons the user can click — not just
//! text. The geometry lives here and is shared by both the paint path
//! ([`crate::Window::file_banner_overlay`]) and the mouse hit-test
//! ([`Window::try_file_banner_left_down`]) so the painted rects and the
//! clickable rects can never drift apart.
//!
//! Only the conflict banner (`FileBanner.pending` set) shows buttons;
//! transient/info banners ("Saved …", failures) render as a plain field.
//!
//! Thread ownership: UI thread of one window.

use continuity_render::{ListRow, Rect as DrawRect, Rgba}; // alias: `Rect` collides with `crate::pane_layout::Rect`

use crate::pane_layout::metrics::TAB_STRIP_HEIGHT_DIP;
use crate::window::Window;

const RIBBON_GAP_DIP: f32 = 6.0;
const PANEL_LEFT_DIP: f32 = 12.0;
const FIELD_INSET_X_DIP: f32 = 12.0;
pub(crate) const FIELD_HEIGHT_DIP: f32 = 26.0;
const FIELD_PAD_TOP_DIP: f32 = 8.0;
const FIELD_PAD_BOTTOM_DIP: f32 = 8.0;
const BUTTON_GAP_TOP_DIP: f32 = 6.0;
const BUTTON_HEIGHT_DIP: f32 = 24.0;
const BUTTON_PAD_BOTTOM_DIP: f32 = 8.0;
const BUTTON_INTER_GAP_DIP: f32 = 8.0;

/// Which conflict action a button invokes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BannerAction {
    Reload,
    KeepMine,
    ShowDiff,
}

/// One clickable conflict-banner button in DIP coordinates.
pub(crate) struct BannerButton {
    pub rect: DrawRect,
    pub label: &'static str,
    pub action: BannerAction,
}

/// Resolved banner geometry shared by paint and hit-test.
pub(crate) struct BannerGeometry {
    pub panel_rect: DrawRect,
    pub field_rect: DrawRect,
    /// Empty unless the banner is a conflict banner (`with_buttons`).
    pub buttons: Vec<BannerButton>,
}

impl Window {
    /// Compute the banner panel/field rects and, when `with_buttons`, the
    /// three action-button rects, from the client `width`. Pure geometry —
    /// delegates to [`compute_banner_geometry`] (no window state read).
    pub(crate) fn banner_geometry(&self, width: f32, with_buttons: bool) -> BannerGeometry {
        compute_banner_geometry(width, with_buttons)
    }

    /// Try to consume a left click at `(x, y)` (client DIP) as a conflict-
    /// banner button press. Returns `true` when a button was hit and its
    /// command dispatched.
    pub(crate) fn try_file_banner_left_down(&mut self, x: i32, y: i32) -> bool {
        let is_conflict = self
            .file_banner
            .as_ref()
            .is_some_and(|b| b.pending.is_some());
        if !is_conflict {
            return false;
        }
        let geo = compute_banner_geometry(self.client_width_dip(), true);
        let (px, py) = (x as f32, y as f32);
        let Some(action) = geo
            .buttons
            .iter()
            .find(|b| rect_contains(&b.rect, px, py))
            .map(|b| b.action)
        else {
            return false;
        };
        let _ = match action {
            BannerAction::Reload => self.file_reload_external_impl(),
            BannerAction::KeepMine => self.file_keep_mine_impl(),
            BannerAction::ShowDiff => self.file_show_diff_impl(),
        };
        true
    }
}

/// Resolve banner geometry from the client `width`. Free function so the
/// layout is unit-testable without constructing a [`Window`] (which would
/// spawn a real Win32 surface).
pub(crate) fn compute_banner_geometry(width: f32, with_buttons: bool) -> BannerGeometry {
    let panel_top = TAB_STRIP_HEIGHT_DIP + RIBBON_GAP_DIP;
    let panel_width = (width - 2.0 * PANEL_LEFT_DIP).clamp(240.0, 760.0);
    let field_left = PANEL_LEFT_DIP + FIELD_INSET_X_DIP;
    let field_width = (panel_width - 2.0 * FIELD_INSET_X_DIP).clamp(200.0, 720.0);
    let field_top = panel_top + FIELD_PAD_TOP_DIP;

    let panel_height = if with_buttons {
        FIELD_PAD_TOP_DIP
            + FIELD_HEIGHT_DIP
            + BUTTON_GAP_TOP_DIP
            + BUTTON_HEIGHT_DIP
            + BUTTON_PAD_BOTTOM_DIP
    } else {
        FIELD_PAD_TOP_DIP + FIELD_HEIGHT_DIP + FIELD_PAD_BOTTOM_DIP
    };

    let buttons = if with_buttons {
        let buttons_top = field_top + FIELD_HEIGHT_DIP + BUTTON_GAP_TOP_DIP;
        let button_width = (field_width - 2.0 * BUTTON_INTER_GAP_DIP) / 3.0;
        const LABELS: [(&str, BannerAction); 3] = [
            ("Reload", BannerAction::Reload),
            ("Keep mine", BannerAction::KeepMine),
            ("Show diff", BannerAction::ShowDiff),
        ];
        LABELS
            .iter()
            .enumerate()
            .map(|(i, (label, action))| {
                let x = field_left + (i as f32) * (button_width + BUTTON_INTER_GAP_DIP);
                BannerButton {
                    rect: DrawRect::new(x, buttons_top, button_width, BUTTON_HEIGHT_DIP),
                    label,
                    action: *action,
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    BannerGeometry {
        panel_rect: DrawRect::new(PANEL_LEFT_DIP, panel_top, panel_width, panel_height),
        field_rect: DrawRect::new(field_left, field_top, field_width, FIELD_HEIGHT_DIP),
        buttons,
    }
}

/// Build the painted [`ListRow`] for one conflict-banner button.
pub(crate) fn banner_button_row(button: &BannerButton) -> ListRow {
    ListRow {
        rect: button.rect,
        primary_text: button.label.to_string(),
        secondary_text: None,
        keybinding: None,
        fg: Rgba {
            r: 0.96,
            g: 0.97,
            b: 1.0,
            a: 1.0,
        },
        secondary_fg: Rgba::TRANSPARENT,
        // A slightly raised fill so each action reads as a button against
        // the darker banner panel.
        bg: Some(Rgba {
            r: 0.22,
            g: 0.25,
            b: 0.30,
            a: 1.0,
        }),
        disabled: false,
    }
}

fn rect_contains(rect: &DrawRect, px: f32, py: f32) -> bool {
    px >= rect.x && px <= rect.x + rect.w && py >= rect.y && py <= rect.y + rect.h
}

#[cfg(test)]
mod tests {
    use super::{compute_banner_geometry, rect_contains, BannerAction};

    #[test]
    fn info_banner_has_no_buttons() {
        let geo = compute_banner_geometry(1200.0, false);
        assert!(geo.buttons.is_empty());
    }

    #[test]
    fn conflict_banner_lays_out_three_ordered_buttons() {
        let geo = compute_banner_geometry(1200.0, true);
        assert_eq!(geo.buttons.len(), 3);
        assert_eq!(geo.buttons[0].action, BannerAction::Reload);
        assert_eq!(geo.buttons[1].action, BannerAction::KeepMine);
        assert_eq!(geo.buttons[2].action, BannerAction::ShowDiff);
        // Left-to-right, non-overlapping.
        let (a, b, c) = (&geo.buttons[0], &geo.buttons[1], &geo.buttons[2]);
        assert!(a.rect.x + a.rect.w <= b.rect.x + 0.01);
        assert!(b.rect.x + b.rect.w <= c.rect.x + 0.01);
        // Buttons sit inside the panel and below the message field.
        for btn in &geo.buttons {
            assert!(btn.rect.x >= geo.panel_rect.x);
            assert!(btn.rect.x + btn.rect.w <= geo.panel_rect.x + geo.panel_rect.w + 0.01);
            assert!(btn.rect.y >= geo.field_rect.y + geo.field_rect.h);
        }
        // The conflict panel is taller than the plain info panel.
        let info = compute_banner_geometry(1200.0, false);
        assert!(geo.panel_rect.h > info.panel_rect.h);
    }

    #[test]
    fn hit_test_matches_button_rects() {
        let geo = compute_banner_geometry(1200.0, true);
        let mid = &geo.buttons[1].rect;
        let (cx, cy) = (mid.x + mid.w / 2.0, mid.y + mid.h / 2.0);
        assert!(rect_contains(mid, cx, cy));
        // A point above the buttons (in the message field) misses them.
        assert!(geo
            .buttons
            .iter()
            .all(|b| !rect_contains(&b.rect, cx, geo.field_rect.y)));
    }
}
