//! Tab-drag resolution + cancellation siblings of [`crate::window_mouse`].
//!
//! Holds the pure four-case drop resolver shared by the in-flight
//! preview (`WM_MOUSEMOVE`) and the commit (`WM_LBUTTONUP`), plus the
//! ESC / capture-loss cancel paths and the cross-window drag-hover
//! broadcast wiring.
//!
//! The resolver was lifted out of `on_left_button_up` so the painted
//! affordance and the actual drop decision can never diverge — both
//! call [`Window::compute_tab_drop_resolution`] with the same `(x, y)`
//! and read the same answer.
//!
//! Thread ownership: each helper runs on the owning `Window`'s UI thread.

use windows::core::HSTRING;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowRect, IsWindowVisible, PostMessageW, RegisterWindowMessageW,
};

use std::sync::OnceLock;

use crate::mouse::{ForeignTabDragHover, TabDrag, TabDropResolution};
use crate::pane_layout::metrics;
use crate::window_mouse_hover::wall_clock_ms;
use crate::Window;

/// Hysteresis radius around the press point — releases inside this
/// circle are treated as a pure click rather than a drag.
pub(crate) const TAB_DRAG_HYSTERESIS_PX: i32 = 6;

/// Lazily registered Win32 message id used by the source window to
/// broadcast "my drag is hovering over your window" to sibling
/// Continuity windows so they can paint their own drop affordance.
///
/// `wparam == 1` ⇒ hover present (`lparam` encodes the cursor in this
/// window's client DIPs as `(y_i16 << 16) | x_i16`); `wparam == 0` ⇒
/// hover cleared. The source HWND is stamped into the message via
/// `Continuity.TabDragHover.Source` so the receiver knows who to
/// blame when the source closes mid-drag.
pub(crate) fn tab_drag_hover_message_id() -> u32 {
    static ID: OnceLock<u32> = OnceLock::new();
    *ID.get_or_init(|| {
        let name = HSTRING::from("Continuity.TabDragHover");
        unsafe { RegisterWindowMessageW(&name) }
    })
}

impl Window {
    /// Pure four-case drop resolver shared by drag-in-flight preview
    /// and `WM_LBUTTONUP` commit.
    pub(crate) fn compute_tab_drop_resolution(
        &self,
        drag: &TabDrag,
        x: i32,
        y: i32,
    ) -> TabDropResolution {
        let dx = (x - drag.start_x).abs();
        let dy = (y - drag.start_y).abs();
        if dx < TAB_DRAG_HYSTERESIS_PX && dy < TAB_DRAG_HYSTERESIS_PX {
            return TabDropResolution::Cancel;
        }
        if let Some(indicator) = self.compute_tab_drop_indicator(x, y) {
            return TabDropResolution::SourceStrip(indicator);
        }
        let xf = x as f32;
        let yf = y as f32;
        let root = self.pane_root_rect();
        let inside_self =
            xf >= root.x && xf < root.x + root.w && yf >= root.y && yf < root.y + root.h;
        if inside_self {
            if let Some((pane, rect)) = crate::pane_layout::pane_at_point(&self.tree, root, xf, yf)
            {
                // Pane bodies that the cursor is sitting in (strip case
                // already handled above by `compute_tab_drop_indicator`).
                if yf >= rect.y + metrics::TAB_STRIP_HEIGHT_DIP {
                    return TabDropResolution::PaneBody {
                        pane,
                        rect: (rect.x, rect.y, rect.w, rect.h),
                    };
                }
            }
        }
        if let Some(hwnd_raw) = self.find_sibling_window_at_client_point(x, y) {
            return TabDropResolution::ForeignWindow { hwnd_raw };
        }
        TabDropResolution::TearOff
    }

    /// Find a sibling Continuity window whose screen rect contains the
    /// cursor at this window's client `(x, y)`. Mirrors the candidate
    /// enumeration in [`Window::try_cross_window_tab_drop`] so the
    /// preview affordance points at the same window the commit will
    /// adopt into.
    fn find_sibling_window_at_client_point(&self, x: i32, y: i32) -> Option<isize> {
        if self.hwnd.0 as isize == 0 {
            return None;
        }
        let mut pt = POINT { x, y };
        let translated = unsafe { ClientToScreen(self.hwnd, &mut pt) }.as_bool();
        if !translated {
            let mut cp = POINT::default();
            if unsafe { GetCursorPos(&mut cp) }.is_err() {
                return None;
            }
            pt = cp;
        }
        let candidates = crate::window_registry::snapshot_others(self.hwnd);
        let vd_manager = continuity_win::VirtualDesktopManager::new().ok();
        let my_vd = vd_manager
            .as_ref()
            .and_then(|m| m.desktop_id_of_window(self.hwnd));
        for hwnd in candidates {
            if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
                continue;
            }
            if let (Some(mgr), Some(my)) = (vd_manager.as_ref(), my_vd) {
                if let Some(theirs) = mgr.desktop_id_of_window(hwnd) {
                    if theirs != my {
                        continue;
                    }
                }
            }
            let mut rect = RECT::default();
            if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
                continue;
            }
            if pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom {
                return Some(hwnd.0 as isize);
            }
        }
        None
    }

    /// Convert a client DIP point from the tab-drag mouse path into
    /// physical screen pixels for `CreateWindowExW` placement.
    pub(crate) fn client_dip_point_to_screen(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        if self.hwnd.0 as isize == 0 {
            return None;
        }
        let scale = self.dpi_scale();
        let mut pt = POINT {
            x: dip_client_to_physical(x, scale),
            y: dip_client_to_physical(y, scale),
        };
        if unsafe { ClientToScreen(self.hwnd, &mut pt) }.as_bool() {
            Some((pt.x, pt.y))
        } else {
            let mut fallback = POINT::default();
            unsafe { GetCursorPos(&mut fallback) }
                .map(|()| (fallback.x, fallback.y))
                .ok()
        }
    }

    /// Update the live resolution + indicator on the in-flight tab
    /// drag from a fresh `(x, y)`. Returns the new resolution so the
    /// caller can decide whether to broadcast / log / invalidate.
    pub(crate) fn refresh_tab_drag_resolution(
        &mut self,
        x: i32,
        y: i32,
    ) -> Option<ResolutionDelta> {
        let drag_clone = self.mouse_state.tab_drag.as_ref().cloned()?;
        let resolution = self.compute_tab_drop_resolution(&drag_clone, x, y);
        let indicator = match resolution {
            TabDropResolution::SourceStrip(i) => Some(i),
            _ => None,
        };
        let drag_mut = self.mouse_state.tab_drag.as_mut()?;
        let prev_resolution = drag_mut.resolution;
        let prev_indicator = drag_mut.drop_indicator;
        let prev_variant = std::mem::discriminant(&prev_resolution);
        let next_variant = std::mem::discriminant(&resolution);
        let variant_changed = prev_variant != next_variant
            || matches!(
                (&prev_resolution, &resolution),
                (
                    TabDropResolution::ForeignWindow { hwnd_raw: a },
                    TabDropResolution::ForeignWindow { hwnd_raw: b },
                ) if a != b,
            )
            || matches!(
                (&prev_resolution, &resolution),
                (
                    TabDropResolution::PaneBody { pane: a, .. },
                    TabDropResolution::PaneBody { pane: b, .. },
                ) if a != b,
            )
            || matches!(
                (&prev_resolution, &resolution),
                (
                    TabDropResolution::SourceStrip(a),
                    TabDropResolution::SourceStrip(b),
                ) if a.pane != b.pane,
            );
        let indicator_changed = prev_indicator != indicator;
        drag_mut.resolution = resolution;
        drag_mut.drop_indicator = indicator;
        Some(ResolutionDelta {
            previous: prev_resolution,
            current: resolution,
            variant_changed,
            indicator_changed,
        })
    }

    /// Cancel an in-flight tab drag without committing a drop. Used by
    /// ESC during the drag and by `WM_CAPTURECHANGED` (lost capture).
    /// Returns `true` if a drag was actually cleared.
    pub(crate) fn cancel_tab_drag(&mut self) -> bool {
        let Some(drag) = self.mouse_state.tab_drag.take() else {
            return false;
        };
        if self.hwnd.0 as isize != 0 {
            // Release capture if we still hold it (ESC path) and tell
            // any sibling we may have been broadcasting hover to that
            // the drag is over.
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        self.clear_tab_drag_ghost();
        self.broadcast_tab_drag_hover_leave(&drag);
        self.mouse_state.dragging = false;
        let elapsed = wall_clock_ms().saturating_sub(drag.start_ms);
        crate::paint_trace::log_event(
            "tab_drag",
            &format!(
                "state=cancel target=cancel slot=-1 foreign_hwnd=0 elapsed_ms_since_start={elapsed}",
            ),
        );
        true
    }

    /// Send a "hover" or "leave" message to a sibling Continuity
    /// window. `client_x_dip` / `client_y_dip` come from the source
    /// window's DIP-space mouse handlers; this function converts them
    /// back to physical pixels for `ClientToScreen` (which does not
    /// know about per-window DPI) so a HiDPI source talking to a HiDPI
    /// receiver does not double-scale.
    ///
    /// Idempotent — repeated hovers to the same target are fine; the
    /// receiver short-circuits on identical state.
    pub(crate) fn send_tab_drag_hover(
        &self,
        target_hwnd_raw: isize,
        client_x_dip: i32,
        client_y_dip: i32,
    ) {
        if target_hwnd_raw == 0 || self.hwnd.0 as isize == 0 {
            return;
        }
        let msg = tab_drag_hover_message_id();
        if msg == 0 {
            return;
        }
        let scale = self.dpi_scale();
        let physical_x = dip_client_to_physical(client_x_dip, scale);
        let physical_y = dip_client_to_physical(client_y_dip, scale);
        // Translate this window's client (physical px) to screen
        // coordinates so the receiver can convert into *its* client
        // space without needing the sender's HWND or DPI.
        let mut pt = POINT {
            x: physical_x,
            y: physical_y,
        };
        let translated = unsafe { ClientToScreen(self.hwnd, &mut pt) }.as_bool();
        if !translated {
            return;
        }
        let target_hwnd = HWND(target_hwnd_raw as *mut std::ffi::c_void);
        let wparam = WPARAM(1);
        let lparam = LPARAM(pack_screen_point(pt.x, pt.y));
        let _ = unsafe { PostMessageW(Some(target_hwnd), msg, wparam, lparam) };
    }

    /// Clear any cross-window hover affordance the source previously
    /// painted on a sibling. Posts a `wparam == 0` "leave" message.
    pub(crate) fn send_tab_drag_leave(&self, target_hwnd_raw: isize) {
        if target_hwnd_raw == 0 || self.hwnd.0 as isize == 0 {
            return;
        }
        let msg = tab_drag_hover_message_id();
        if msg == 0 {
            return;
        }
        let target_hwnd = HWND(target_hwnd_raw as *mut std::ffi::c_void);
        let _ = unsafe { PostMessageW(Some(target_hwnd), msg, WPARAM(0), LPARAM(0)) };
    }

    /// Walk every sibling we have previously hovered and clear it. Used
    /// at drag end / cancel so a leftover indicator never lingers on a
    /// foreign window.
    pub(crate) fn broadcast_tab_drag_hover_leave(&self, drag: &TabDrag) {
        if let TabDropResolution::ForeignWindow { hwnd_raw } = drag.resolution {
            self.send_tab_drag_leave(hwnd_raw);
        }
    }

    /// Receiver-side handler for `Continuity.TabDragHover`. Stores or
    /// clears the cross-window hover state and invalidates so the
    /// drop affordance paints. Returns `true` if the paint should be
    /// invalidated.
    pub(crate) fn on_foreign_tab_drag_hover(&mut self, wparam: WPARAM, lparam: LPARAM) -> bool {
        if wparam.0 == 0 {
            return self.mouse_state.foreign_tab_drag_hover.take().is_some();
        }
        let (screen_x, screen_y) = unpack_screen_point(lparam.0);
        let mut pt = POINT {
            x: screen_x,
            y: screen_y,
        };
        let translated =
            unsafe { windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut pt).as_bool() };
        if !translated {
            return false;
        }
        let (x_dip, y_dip) = self.physical_point_to_dip(pt.x, pt.y);
        let next = ForeignTabDragHover {
            source_hwnd_raw: 0,
            cursor_x_dip: x_dip as f32,
            cursor_y_dip: y_dip as f32,
        };
        let prev = self.mouse_state.foreign_tab_drag_hover;
        self.mouse_state.foreign_tab_drag_hover = Some(next);
        prev != Some(next)
    }
}

/// Pure helper — should the source-window broadcast a hover message
/// to the foreign target on this `WM_MOUSEMOVE` tick?
///
/// Returns `true` for any move where the current resolution is
/// `ForeignWindow`, regardless of variant-change / indicator-change.
/// The receiver short-circuits on identical state, so spamming is
/// cheap and necessary — the source-window's
/// `compute_tab_drop_indicator` is `None` whenever the cursor is
/// outside its own strips, so the indicator-change gate alone never
/// fires while moving *within* a foreign window.
#[must_use]
pub(crate) fn should_broadcast_foreign_hover(current: TabDropResolution) -> bool {
    matches!(current, TabDropResolution::ForeignWindow { .. })
}

/// Delta returned by [`Window::refresh_tab_drag_resolution`] so the
/// caller can decide what side-effects to fire.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolutionDelta {
    /// Resolution before this move.
    pub previous: TabDropResolution,
    /// Resolution at the current cursor position.
    pub current: TabDropResolution,
    /// `true` when the variant or its key payload (target pane,
    /// foreign hwnd) changed. Drives trace + cross-window broadcast.
    pub variant_changed: bool,
    /// `true` when the source-strip indicator slot changed even though
    /// the variant did not (e.g. user slides across tabs on the source
    /// strip). Drives repaint without firing a trace.
    pub indicator_changed: bool,
}

/// Pack `(screen_x, screen_y)` into a `LPARAM`-sized isize. Used so the
/// cross-window broadcast carries cursor coordinates with the message
/// instead of a separate IPC channel.
fn pack_screen_point(x: i32, y: i32) -> isize {
    let lo = (x as u16) as u32;
    let hi = (y as u16) as u32;
    ((hi << 16) | lo) as isize
}

/// Inverse of [`pack_screen_point`]. Sign-extends through `i16` so a
/// negative cursor coordinate (e.g. above the primary monitor's top
/// edge in a multi-monitor setup) round-trips correctly.
fn unpack_screen_point(packed: isize) -> (i32, i32) {
    let raw = packed as i32;
    let x = (raw & 0xFFFF) as i16 as i32;
    let y = ((raw >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

/// Convert a DIP client coordinate to physical pixels at a given DPI
/// scale. Mirrors the inline math in [`Window::send_tab_drag_hover`];
/// extracted so the round-trip with [`Window::physical_point_to_dip`]
/// is testable without needing a real `Window`.
#[must_use]
pub(crate) fn dip_client_to_physical(value_dip: i32, scale: f32) -> i32 {
    (value_dip as f32 * scale.max(0.01)).round() as i32
}

/// Inverse of [`dip_client_to_physical`] — what the receiver's
/// [`Window::physical_point_to_dip`] does after `ScreenToClient` returns
/// physical pixels.
#[cfg(test)]
#[must_use]
pub(crate) fn physical_to_dip_client(value_physical: i32, scale: f32) -> i32 {
    (value_physical as f32 / scale.max(0.01)).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mouse::DropIndicator;
    use crate::pane_tree::PaneId;

    #[test]
    fn screen_point_round_trips_through_lparam_pack() {
        for &(x, y) in &[(0, 0), (100, 200), (-1, -1), (-32_000, 31_000)] {
            let packed = pack_screen_point(x, y);
            let (rx, ry) = unpack_screen_point(packed);
            assert_eq!((x, y), (rx, ry));
        }
    }

    #[test]
    fn dip_to_physical_to_dip_round_trips_at_100_percent_scale() {
        for v in [-100, -1, 0, 1, 50, 500] {
            let physical = dip_client_to_physical(v, 1.0);
            let back = physical_to_dip_client(physical, 1.0);
            assert_eq!(v, back, "DPI 96 round trip failed for {v}");
        }
    }

    #[test]
    fn dip_to_physical_to_dip_round_trips_at_200_percent_scale() {
        // At 2.0× scale, a DIP value v becomes 2v physical, which
        // converts back to v. Without the fix to `send_tab_drag_hover`,
        // the receiver would see v/2 — half the actual position.
        for v in [0, 25, 50, 200, 1000] {
            let physical = dip_client_to_physical(v, 2.0);
            assert_eq!(physical, v * 2);
            let back = physical_to_dip_client(physical, 2.0);
            assert_eq!(v, back, "DPI 192 round trip failed for {v}");
        }
    }

    #[test]
    fn dip_to_physical_to_dip_round_trips_at_125_percent_scale() {
        // 1.25× is the most common Windows HiDPI scale. Check it
        // separately because rounding can drift by a half-DIP if the
        // helper used floor instead of round.
        for v in [40, 80, 120, 240] {
            let physical = dip_client_to_physical(v, 1.25);
            let back = physical_to_dip_client(physical, 1.25);
            assert_eq!(v, back, "DPI 120 round trip failed for {v}");
        }
    }

    #[test]
    fn broadcast_predicate_fires_for_foreign_only() {
        assert!(should_broadcast_foreign_hover(
            TabDropResolution::ForeignWindow { hwnd_raw: 1 }
        ));
        // Crucially, the predicate must not depend on indicator-
        // change or variant-change — every move over a foreign
        // window broadcasts. This guards against a regression of
        // the cross-window stick-at-entry-position bug.
        assert!(should_broadcast_foreign_hover(
            TabDropResolution::ForeignWindow { hwnd_raw: 9999 }
        ));
        assert!(!should_broadcast_foreign_hover(TabDropResolution::Cancel));
        assert!(!should_broadcast_foreign_hover(TabDropResolution::TearOff));
        assert!(!should_broadcast_foreign_hover(
            TabDropResolution::SourceStrip(DropIndicator {
                pane: PaneId(1),
                slot: 0,
            })
        ));
        assert!(!should_broadcast_foreign_hover(
            TabDropResolution::PaneBody {
                pane: PaneId(2),
                rect: (0.0, 0.0, 0.0, 0.0),
            }
        ));
    }

    #[test]
    fn trace_str_per_variant_is_stable() {
        assert_eq!(TabDropResolution::Cancel.as_trace_str(), "cancel");
        assert_eq!(TabDropResolution::TearOff.as_trace_str(), "tear_off");
        assert_eq!(
            TabDropResolution::SourceStrip(DropIndicator {
                pane: PaneId(1),
                slot: 0,
            })
            .as_trace_str(),
            "source_strip",
        );
        assert_eq!(
            TabDropResolution::PaneBody {
                pane: PaneId(1),
                rect: (0.0, 0.0, 0.0, 0.0),
            }
            .as_trace_str(),
            "pane_body",
        );
        assert_eq!(
            TabDropResolution::ForeignWindow { hwnd_raw: 1234 }.as_trace_str(),
            "foreign_window",
        );
    }
}
