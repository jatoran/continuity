//! Per-buffer right-edge chrome visibility and hit-testing.
//!
//! Minimap and outline visibility are view chrome. The live map is
//! keyed by `BufferId` and owned by the window UI thread.

use crate::pane_layout::{metrics, Rect};
use crate::pane_tree::PaneId;
use crate::window::Window;

/// Non-text chrome surfaces that expose a right-click toggle menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChromeContextTarget {
    /// Left folder-browser pane.
    FileTree,
    /// Right scaled-text minimap strip.
    Minimap {
        /// Pane whose active buffer owns the strip.
        pane_id: PaneId,
    },
    /// Right markdown outline strip.
    Outline {
        /// Pane whose active buffer owns the strip.
        pane_id: PaneId,
    },
}

/// Right-edge chrome bits that should follow one buffer view.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RightEdgeChromeState {
    /// Minimap strip visible.
    pub(crate) minimap: bool,
    /// Outline strip visible.
    pub(crate) outline: bool,
}

impl RightEdgeChromeState {
    pub(crate) fn new(minimap: bool, outline: bool) -> Self {
        Self { minimap, outline }
    }
}

impl Window {
    pub(crate) fn chrome_context_target_at(
        &self,
        x_dip: f32,
        y_dip: f32,
    ) -> Option<ChromeContextTarget> {
        if self.cursor_over_file_tree(x_dip, y_dip) {
            return Some(ChromeContextTarget::FileTree);
        }
        self.right_edge_chrome_target_at(x_dip, y_dip)
    }

    pub(crate) fn cursor_over_non_text_chrome(&self, x_dip: f32, y_dip: f32) -> bool {
        self.chrome_context_target_at(x_dip, y_dip).is_some()
    }

    pub(crate) fn current_right_edge_chrome_state(&self) -> RightEdgeChromeState {
        RightEdgeChromeState::new(
            self.view_options.minimap,
            self.view_options.show_outline_sidebar,
        )
    }

    pub(crate) fn save_current_right_edge_chrome_state(&mut self) {
        let key = self.current_right_edge_chrome_key();
        let state = self.current_right_edge_chrome_state();
        if state == self.right_edge_chrome_defaults {
            self.right_edge_chrome_by_view.remove(&key);
        } else {
            self.right_edge_chrome_by_view.insert(key, state);
        }
    }

    pub(crate) fn remember_current_right_edge_chrome_state(&mut self) {
        self.save_current_right_edge_chrome_state();
    }

    pub(crate) fn apply_right_edge_chrome_for_current_view(&mut self) {
        let state = self.right_edge_chrome_state_for_buffer(self.current_right_edge_chrome_key());
        self.apply_right_edge_chrome_state(state);
    }

    pub(crate) fn right_edge_chrome_state_for_buffer(
        &self,
        buffer_id: continuity_buffer::BufferId,
    ) -> RightEdgeChromeState {
        self.right_edge_chrome_by_view
            .get(&buffer_id)
            .copied()
            .unwrap_or(self.right_edge_chrome_defaults)
    }

    pub(crate) fn set_right_edge_chrome_defaults(&mut self, state: RightEdgeChromeState) {
        let current_has_override = self
            .right_edge_chrome_by_view
            .contains_key(&self.current_right_edge_chrome_key());
        self.right_edge_chrome_defaults = state;
        if !current_has_override {
            self.apply_right_edge_chrome_state(state);
        }
    }

    pub(crate) fn apply_right_edge_chrome_state(&mut self, state: RightEdgeChromeState) {
        if self.view_options.minimap == state.minimap
            && self.view_options.show_outline_sidebar == state.outline
        {
            return;
        }
        self.view_options.minimap = state.minimap;
        self.view_options.show_outline_sidebar = state.outline;
        self.clear_right_edge_layout_caches();
        self.outline_entries_cache
            .borrow_mut()
            .clear_for_buffer(self.buffer_id);
    }

    fn current_right_edge_chrome_key(&self) -> continuity_buffer::BufferId {
        self.buffer_id
    }

    fn cursor_over_file_tree(&self, x_dip: f32, y_dip: f32) -> bool {
        self.file_tree.is_visible()
            && x_dip >= 0.0
            && x_dip < self.file_tree.visible_width_dip()
            && y_dip >= 0.0
            && y_dip < self.client_height_dip()
    }

    fn right_edge_chrome_target_at(&self, x_dip: f32, y_dip: f32) -> Option<ChromeContextTarget> {
        for (pane_id, outer) in self.pane_outer_rects() {
            let body = body_rect_from_outer(outer);
            if !body.contains(x_dip, y_dip) {
                continue;
            }
            let Some(buffer_id) = self.active_buffer_id_for_pane(pane_id) else {
                continue;
            };
            let state = if pane_id == self.tree.focused {
                self.current_right_edge_chrome_state()
            } else {
                self.right_edge_chrome_state_for_buffer(buffer_id)
            };
            if self.cursor_over_outline_sidebar_for_pane(x_dip, y_dip, pane_id, body, state) {
                return Some(ChromeContextTarget::Outline { pane_id });
            }
            if self.cursor_over_minimap_for_pane(x_dip, y_dip, pane_id, body, state) {
                return Some(ChromeContextTarget::Minimap { pane_id });
            }
        }
        None
    }

    fn active_buffer_id_for_pane(&self, pane_id: PaneId) -> Option<continuity_buffer::BufferId> {
        let group = self.tree.groups.get(&pane_id)?;
        let tab = self.tree.tabs.get(&group.active)?;
        Some(tab.buffer_id)
    }

    fn cursor_over_outline_sidebar_for_pane(
        &self,
        x_dip: f32,
        y_dip: f32,
        pane_id: PaneId,
        body: Rect,
        state: RightEdgeChromeState,
    ) -> bool {
        if !state.outline {
            return false;
        }
        if pane_id == self.tree.focused {
            if let Some(layout) = self.view_options.outline_layout.as_ref() {
                return point_in_rect(x_dip, y_dip, layout.rect);
            }
        }
        let width = self
            .view_options
            .outline_sidebar_width_dip
            .max(0.0)
            .min(body.w);
        width > 0.0
            && x_dip >= body.x + body.w - width
            && x_dip <= body.x + body.w
            && y_dip >= body.y
            && y_dip <= body.y + body.h
    }

    fn cursor_over_minimap_for_pane(
        &self,
        x_dip: f32,
        y_dip: f32,
        pane_id: PaneId,
        body: Rect,
        state: RightEdgeChromeState,
    ) -> bool {
        if !state.minimap {
            return false;
        }
        if y_dip < body.y || y_dip > body.y + body.h {
            return false;
        }
        let pane_x = x_dip - body.x;
        let pane_y = y_dip - body.y;
        if pane_id == self.tree.focused {
            if let Some(layout) = self.view_options.minimap_layout.as_ref() {
                return point_in_rect(pane_x, pane_y, layout.rect);
            }
        }
        let outline_inset = if state.outline {
            self.view_options.outline_sidebar_width_dip.max(0.0)
        } else {
            0.0
        };
        let width = continuity_render::MINIMAP_WIDTH_DIP.min((body.w - outline_inset).max(0.0));
        let x = (body.w - outline_inset - width).max(0.0);
        width > 0.0 && pane_x >= x && pane_x <= x + width && pane_y >= 0.0 && pane_y <= body.h
    }
}

fn body_rect_from_outer(outer: Rect) -> Rect {
    let strip = metrics::TAB_STRIP_HEIGHT_DIP.min(outer.h);
    Rect::new(
        outer.x,
        outer.y + strip,
        outer.w,
        (outer.h - strip).max(1.0),
    )
}

fn point_in_rect(x: f32, y: f32, rect: (f32, f32, f32, f32)) -> bool {
    let (rx, ry, rw, rh) = rect;
    rw > 0.0 && rh > 0.0 && x >= rx && x <= rx + rw && y >= ry && y <= ry + rh
}
