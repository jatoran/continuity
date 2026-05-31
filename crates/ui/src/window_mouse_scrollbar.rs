//! Vertical-scrollbar mouse interaction for the focused pane.
//!
//! Three behaviours, layered on top of the paint-only thumb in
//! [`continuity_render::scrollbar`]:
//!
//! * **Hover** — `on_set_cursor` consults [`Window::cursor_over_scrollbar`]
//!   and overrides the body's I-beam with `IDC_ARROW` so the user can
//!   tell the affordance is grabbable.
//! * **Thumb drag** — `WM_LBUTTONDOWN` on the thumb captures the mouse,
//!   stashes a [`ScrollbarDrag`] in `mouse_state`, and routes
//!   subsequent `WM_MOUSEMOVE` samples into absolute scroll positions.
//!   `WM_LBUTTONUP` releases capture.
//! * **Track click** — `WM_LBUTTONDOWN` on the track outside the thumb
//!   pages up/down by one viewport height, matching the Win32 standard
//!   scrollbar idiom.
//!
//! The hit geometry is derived on demand from the focused pane's body
//! rect, the active scroll state, and the rope's estimated content
//! height — the same inputs the painter uses — so the hit target and
//! the painted thumb can't drift apart.
//!
//! **Thread ownership**: UI thread (same as the rest of `window_mouse*`).

use continuity_render::chrome::resolve_body_right_margin_dip;
use continuity_render::scrollbar::{
    compute_scrollbar_layout, scroll_y_for_thumb_top, ScrollbarLayout,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetCapture;

use crate::mouse::ScrollbarDrag;
use crate::Window;

fn compute_scrollbar_drag_target_scroll(
    layout: &ScrollbarLayout,
    drag: ScrollbarDrag,
    mouse_y_dip: f32,
) -> f32 {
    scroll_y_for_thumb_top(layout, mouse_y_dip - drag.thumb_grab_offset_dip)
}

fn should_trace_scrollbar_drag_move(move_count: u32) -> bool {
    move_count == 1 || (move_count > 0 && move_count.is_multiple_of(8))
}

/// Emit scrollbar drag trace rows.
///
/// Start and end are emitted once per drag. Move rows are emitted for
/// the first processed move and then every eighth processed move so
/// traces expose drift without logging every mouse sample.
fn trace_scrollbar_drag(
    state: &str,
    mouse_y_dip: f32,
    scroll_y_dip: f32,
    layout: Option<&ScrollbarLayout>,
) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    let (scroll_max, travel_dip, thumb_h_dip) = layout
        .map(|layout| {
            (
                layout.scroll_max(),
                (layout.track_h() - layout.thumb_h()).max(0.0),
                layout.thumb_h(),
            )
        })
        .unwrap_or((0.0, 0.0, 0.0));
    crate::paint_trace::log_event(
        "scrollbar_drag",
        &format!(
            "state={state} mouse_y_dip={mouse_y_dip:.1} scroll_y_dip={scroll_y_dip:.1} \
             scroll_max={scroll_max:.1} travel_dip={travel_dip:.1} \
             thumb_h_dip={thumb_h_dip:.1}"
        ),
    );
}

impl Window {
    /// Compute the focused pane's scrollbar layout for the current
    /// scroll / content / viewport state. Returns `None` when the
    /// content fits inside the viewport (nothing to scroll).
    ///
    /// The right edge mirrors the renderer's
    /// `body_origin.x + margins.left + editor_w` for the non-
    /// distraction-free case by routing through
    /// [`resolve_body_right_margin_dip`], the same helper
    /// `ContentMargins::from_view_options` uses. Toggling the minimap,
    /// outline sidebar, or search strip moves the painted thumb
    /// leftward; this keeps the hit target in sync so the user can
    /// still grab it. Distraction-free's centered cap moves the
    /// painted thumb slightly leftward; the [`HIT_LEFT_SLOP_DIP`]-
    /// expanded hit rect still covers most of that offset.
    ///
    /// [`HIT_LEFT_SLOP_DIP`]: continuity_render::scrollbar::HIT_LEFT_SLOP_DIP
    pub(crate) fn focused_scrollbar_layout(&self) -> Option<ScrollbarLayout> {
        let body = self.focused_body_rect();
        let viewport_h = body.h.max(0.0);
        if viewport_h <= 0.0 {
            return None;
        }
        let content_h = self.estimated_content_height();
        let right_margin = resolve_body_right_margin_dip(
            self.view_options.minimap,
            self.is_search_minimap_active(),
            self.view_options.show_outline_sidebar,
            self.view_options.outline_sidebar_width_dip,
        );
        let right_edge_x = body.x + body.w - right_margin;
        compute_scrollbar_layout(
            right_edge_x,
            body.y,
            self.view.scroll_y_dip,
            viewport_h,
            content_h,
        )
    }

    /// True while the find bar is open and the rope has at least one
    /// match — the moment the renderer's
    /// `ViewOptionsDraw::search_minimap_active` flag is set, so the
    /// scrollbar hit-test reserves the same 12-DIP strip the painter
    /// does. Mirrors the inline computation in `window_paint::on_paint`.
    fn is_search_minimap_active(&self) -> bool {
        self.overlays
            .find_bar()
            .is_some_and(|fb| !fb.matches.is_empty())
    }

    /// `WM_SETCURSOR` helper: true when the pointer is over the
    /// scrollbar's hit region (thumb or track) on the focused pane.
    pub(crate) fn cursor_over_scrollbar(&self, x_dip: f32, y_dip: f32) -> bool {
        let Some(layout) = self.focused_scrollbar_layout() else {
            return false;
        };
        layout.hit_test_thumb(x_dip, y_dip) || layout.hit_test_track_outside_thumb(x_dip, y_dip)
    }

    /// `WM_LBUTTONDOWN` handler: thumb-grab starts a drag, track click
    /// pages by one viewport height. Returns `true` when the click was
    /// claimed by the scrollbar (caller should skip caret placement).
    pub(crate) fn try_scrollbar_left_down(&mut self, x: i32, y: i32) -> bool {
        let xf = x as f32;
        let yf = y as f32;
        let Some(layout) = self.focused_scrollbar_layout() else {
            return false;
        };
        if layout.hit_test_thumb(xf, yf) {
            self.cancel_scroll_inertia();
            self.mouse_state.scrollbar_drag = Some(ScrollbarDrag {
                thumb_grab_offset_dip: yf - layout.thumb_top,
                last_mouse_y_dip: yf,
                move_count: 0,
            });
            trace_scrollbar_drag("start", yf, self.view.scroll_y_dip, Some(&layout));
            // `MouseState::register_click` is the canonical "begin drag"
            // signal in the splitter / tab paths — but it also rolls the
            // triple-click line counter, which is meaningless for a
            // scrollbar grab. Set `dragging` directly so the drag flag
            // is set without disturbing click state.
            self.mouse_state.dragging = true;
            unsafe {
                let _ = SetCapture(self.hwnd);
            }
            return true;
        }
        if layout.hit_test_track_outside_thumb(xf, yf) {
            self.cancel_scroll_inertia();
            // Page-up when the click landed above the thumb, page-down
            // when below. `scroll_instant` clamps so a click near either
            // end can't overshoot.
            let page = layout.viewport_h;
            let dy = if yf < layout.thumb_top { -page } else { page };
            let before = self.view.scroll_y_dip;
            self.view.scroll_instant(dy, layout.content_h);
            // Always return true (consume the click) even when the
            // viewport was already pinned — clicking the track is an
            // unambiguous scrollbar interaction, not a caret-placement
            // intent.
            let _ = before;
            return true;
        }
        false
    }

    /// `WM_MOUSEMOVE` handler during a thumb drag. Returns `true` when
    /// the scroll offset moved (caller invalidates the client area).
    pub(crate) fn try_scrollbar_drag_mouse_move(&mut self, _x: i32, y: i32) -> bool {
        let Some(drag) = self.mouse_state.scrollbar_drag else {
            return false;
        };
        let mouse_y_dip = y as f32;
        let Some(layout) = self.focused_scrollbar_layout() else {
            return false;
        };
        let target_scroll = compute_scrollbar_drag_target_scroll(&layout, drag, mouse_y_dip);
        let before = self.view.scroll_y_dip;
        let move_count = if let Some(active_drag) = self.mouse_state.scrollbar_drag.as_mut() {
            active_drag.last_mouse_y_dip = mouse_y_dip;
            active_drag.move_count = active_drag.move_count.saturating_add(1);
            active_drag.move_count
        } else {
            0
        };
        if (target_scroll - before).abs() < f32::EPSILON {
            if should_trace_scrollbar_drag_move(move_count) {
                trace_scrollbar_drag("move", mouse_y_dip, self.view.scroll_y_dip, Some(&layout));
            }
            return false;
        }
        self.view.jump_to(target_scroll, layout.content_h);
        let moved = (self.view.scroll_y_dip - before).abs() > f32::EPSILON;
        if moved && should_trace_scrollbar_drag_move(move_count) {
            trace_scrollbar_drag("move", mouse_y_dip, self.view.scroll_y_dip, Some(&layout));
        }
        moved
    }

    /// `WM_LBUTTONUP` handler. Releases capture and clears drag state
    /// when a scrollbar drag was in flight. Returns `true` when it
    /// owned the up-click.
    pub(crate) fn try_scrollbar_left_up(&mut self) -> bool {
        let Some(drag) = self.mouse_state.scrollbar_drag.take() else {
            return false;
        };
        let layout = self.focused_scrollbar_layout();
        trace_scrollbar_drag(
            "end",
            drag.last_mouse_y_dip,
            self.view.scroll_y_dip,
            layout.as_ref(),
        );
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
        // The drag mirrored scroll_y into `view.scroll_y_dip` on every
        // move; nothing else to commit. Persist so the session state
        // captures the new offset.
        self.request_state_save();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_for(scroll_y_dip: f32) -> ScrollbarLayout {
        compute_scrollbar_layout(1000.0, 0.0, scroll_y_dip, 400.0, 2000.0)
            .expect("content overflows in this fixture")
    }

    fn drag_for(layout: &ScrollbarLayout, mouse_y_dip: f32) -> ScrollbarDrag {
        ScrollbarDrag {
            thumb_grab_offset_dip: mouse_y_dip - layout.thumb_top,
            last_mouse_y_dip: mouse_y_dip,
            move_count: 0,
        }
    }

    fn assert_near(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.001,
            "actual={actual}, expected={expected}"
        );
    }

    #[test]
    fn drag_move_maps_mouse_delta_through_current_layout() {
        let layout = layout_for(0.0);
        let start_y_dip = 100.0;
        let drag = drag_for(&layout, start_y_dip);
        let target = compute_scrollbar_drag_target_scroll(&layout, drag, 200.0);
        let travel = layout.track_h() - layout.thumb_h();
        let expected = 100.0 * (layout.scroll_max() / travel);

        assert_near(target, expected);
    }

    #[test]
    fn drag_move_uses_dip_delta_after_dpi_conversion() {
        let layout = layout_for(0.0);
        let dpi_scale = 1.25_f32;
        let physical_start_y = 125.0_f32;
        let physical_move_y = 250.0_f32;
        let start_y_dip = (physical_start_y / dpi_scale).round();
        let move_y_dip = (physical_move_y / dpi_scale).round();
        let drag = drag_for(&layout, start_y_dip);
        let target = compute_scrollbar_drag_target_scroll(&layout, drag, move_y_dip);
        let travel = layout.track_h() - layout.thumb_h();
        let expected = 100.0 * (layout.scroll_max() / travel);

        assert_near(target, expected);
    }

    #[test]
    fn drag_bottom_lands_on_scroll_max() {
        let layout = layout_for(0.0);
        let drag = drag_for(&layout, layout.thumb_top);
        let bottom_mouse_y = layout.track_top + layout.track_h() - layout.thumb_h();
        let target = compute_scrollbar_drag_target_scroll(&layout, drag, bottom_mouse_y);

        assert_near(target, layout.scroll_max());
    }
}
