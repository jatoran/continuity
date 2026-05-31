//! Window-level motion timer, reduced-motion projection, and layer helpers.
//!
//! Thread ownership: all mutable state here is stored on [`crate::Window`]
//! and touched only from that window's UI thread.

use continuity_input::Modifiers;
use continuity_render::{JumpGlowDraw, OverlayDraw};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::motion::{MotionPolicy, MOTION_TIMER_MS};
use crate::surface_motion::MotionOverlayLayer;
use crate::window::Window;
use crate::window_timers::MOTION_TIMER_ID;

impl Window {
    /// Current resolved motion policy.
    #[must_use]
    pub(crate) fn motion_policy(&self) -> MotionPolicy {
        self.motion_policy
    }

    /// Apply `[ui].reduced_motion` and cancel active animations when enabled.
    pub(crate) fn apply_reduced_motion(&mut self, reduced_motion: bool) {
        if self.motion_policy.is_reduced_motion() == reduced_motion {
            return;
        }
        self.motion_policy.set_reduced_motion(reduced_motion);
        if reduced_motion {
            self.overlay_motion.clear();
            self.chord_hud_motion.clear();
            self.chrome_motion.clear();
            self.status_motion.evict_expired(u64::MAX);
            self.jump_glow = None;
            self.edit_pulse = None;
            self.caret_tween = None;
            self.stagger_scheduler.reset();
            self.stop_motion_timer();
            self.stop_scroll_anim(self.hwnd);
        }
    }

    /// Start the shared motion timer.
    pub(crate) fn start_motion_timer(&mut self) {
        if self.motion_timer_active || self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            let _ = SetTimer(Some(self.hwnd), MOTION_TIMER_ID, MOTION_TIMER_MS, None);
        }
        self.motion_timer_active = true;
    }

    /// Stop the shared motion timer.
    pub(crate) fn stop_motion_timer(&mut self) {
        if !self.motion_timer_active || self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), MOTION_TIMER_ID);
        }
        self.motion_timer_active = false;
    }

    /// `WM_TIMER` callback for motion invalidation and delayed chord-HUD dwell.
    pub(crate) fn on_motion_tick(&mut self, hwnd: HWND) {
        let now_ms = unsafe { GetTickCount64() };
        let mut needs_repaint = false;
        if self.poll_chord_hud(now_ms) {
            needs_repaint = true;
        }
        self.chrome_motion.evict_expired(now_ms);
        self.status_motion.evict_expired(now_ms);
        self.evict_expired_jump_glow();
        self.evict_expired_edit_pulse();
        self.evict_expired_caret_tween();
        // α.1 — fade the save-confirm chip out across its final
        // SAVE_NOTICE_FADE_MS window; eviction here ensures the chip
        // animates instead of snapping out when the file-io poll tick
        // races behind the lifetime.
        if crate::window_status_notice::retain_live_notices(&mut self.status_notices, now_ms) {
            needs_repaint = true;
        }
        if self.has_active_motion(now_ms) {
            needs_repaint = true;
        } else {
            self.stop_motion_timer();
        }
        if needs_repaint {
            self.invalidate_with_reason(hwnd, "motion_tick");
        }
    }

    /// Project the editor overlay/banner layer through the contract.
    pub(crate) fn project_overlay_layer(
        &mut self,
        draw: Option<OverlayDraw>,
        now_ms: u64,
    ) -> Option<MotionOverlayLayer> {
        let key = draw.as_ref().map(|_| self.overlay_motion_key());
        let layer = self.overlay_motion.project(
            key,
            draw,
            self.motion_policy,
            &mut self.stagger_scheduler,
            now_ms,
        );
        if self.overlay_motion.is_active(now_ms) {
            self.start_motion_timer();
        }
        layer
    }

    /// Project the chord-HUD overlay through the contract.
    pub(crate) fn project_chord_hud_layer(&mut self, now_ms: u64) -> Option<MotionOverlayLayer> {
        let draw = self.chord_hud.rows().and_then(|rows| {
            crate::chord_hud_render::build_chord_hud_overlay(
                rows,
                self.client_width_dip(),
                self.client_height_dip(),
            )
        });
        let key = draw.as_ref().map(|_| "chord-hud".to_string());
        let layer = self.chord_hud_motion.project(
            key,
            draw,
            self.motion_policy,
            &mut self.stagger_scheduler,
            now_ms,
        );
        if self.chord_hud_motion.is_active(now_ms) {
            self.start_motion_timer();
        }
        layer
    }

    /// Build the destination-row glow payload for this paint frame.
    pub(crate) fn jump_glow_draw(&self, now_ms: u64) -> Option<JumpGlowDraw> {
        let glow = self.jump_glow?;
        let alpha =
            crate::jump_glow::fade_alpha(glow, now_ms, u64::from(crate::motion::ACK_MOTION_MS))?;
        Some(JumpGlowDraw {
            display_line: glow.line,
            alpha,
            color: crate::window_theme::rgba_from_color(
                self.active_theme.current.editor_caret_jump_glow(),
            ),
        })
    }

    /// Chord-HUD modifier edge.
    pub(crate) fn on_chord_hud_modifier_edge(&mut self, modifiers: Modifiers) -> bool {
        let was_visible = self.chord_hud.is_visible();
        self.chord_hud
            .on_modifier_edge(modifiers, unsafe { GetTickCount64() });
        if !self.motion_policy.is_reduced_motion()
            && !matches!(self.chord_hud, crate::chord_hud::HudState::Idle)
        {
            self.start_motion_timer();
        }
        was_visible != self.chord_hud.is_visible()
    }

    /// Dismiss any pending/visible chord HUD after real typing.
    pub(crate) fn on_chord_hud_typed(&mut self) -> bool {
        let was_visible = self.chord_hud.is_visible();
        self.chord_hud.on_chord_typed();
        was_visible
    }

    fn poll_chord_hud(&mut self, now_ms: u64) -> bool {
        let mut hud = std::mem::take(&mut self.chord_hud);
        let changed = hud.poll(now_ms, &self.keymap, self).is_some();
        self.chord_hud = hud;
        changed
    }

    fn has_active_motion(&self, now_ms: u64) -> bool {
        // Tier 0 (doc-end / spectator-multiplier work): persistence
        // backlog is deliberately NOT a motion source. A large paste
        // leaves a big `unflushed_bytes()` queue, and treating that as
        // "active motion" kept the shared motion timer firing a
        // *full-window* `motion_tick` invalidate every tick until the
        // flush drained — which, in a 2x2 grid, repaints every spectator
        // pane each tick (the post-paste paint storm in
        // `trace_20260530-154920`). Flush progress is not a visual
        // animation; the saved/unsaved status updates opportunistically
        // on the next real paint (or a completion event) instead of
        // driving the grid.
        self.overlay_motion.is_active(now_ms)
            || self.chord_hud_motion.is_active(now_ms)
            || self.chrome_motion.is_active(now_ms)
            || self.status_motion.active_len(now_ms) > 0
            || self.jump_glow.is_some()
            || self.edit_pulse.is_some()
            || self.caret_tween.is_some()
            || !self.status_notices.is_empty()
            || matches!(self.chord_hud, crate::chord_hud::HudState::Pending { .. })
    }

    fn overlay_motion_key(&self) -> String {
        if self.overlays.is_active() {
            format!("overlay:{:?}", self.overlays.kind())
        } else if self
            .mouse_state
            .footnote_hover
            .as_ref()
            .is_some_and(|hover| hover.ready)
        {
            "footnote-hover".to_string()
        } else {
            "banner".to_string()
        }
    }
}
