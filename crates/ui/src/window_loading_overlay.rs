//! Transient "building view" overlay state machine.
//!
//! Owned by the UI thread of one [`crate::window::Window`]. As of
//! P18.10 paint never blocks on the worker, so no production call site
//! arms this overlay — every dispatch path has a sub-50 ms inline
//! fallback and the user never sees a stall worth annotating. The
//! state machine is retained so a future surface (e.g., an explicit
//! "loading a multi-megabyte file" banner) can re-arm it without
//! re-introducing the overlay plumbing.
//!
//! The rope is canonical — this is chrome over an unchanged buffer; no
//! display-map projection touches it. If a future feature wants to
//! delete the overlay state, the editor degrades to "frozen until the
//! cold build finishes" but remains correct.

use std::time::Instant;

use continuity_render::{EditorColors, LoadingOverlayDraw, Rgba, SurfaceMotion};

use crate::window::Window;

/// Fade-in duration in milliseconds. Matches the
/// `.docs/design/motion.md` shared contract (120–240 ms band) and
/// resolves to zero when reduced-motion is active.
pub(crate) const LOADING_OVERLAY_FADE_IN_MS: u64 = 160;

/// Neutral label per `principles.md`'s "trust the writer" — no
/// moralizing language ("Please wait", "Almost there", …).
pub(crate) const LOADING_OVERLAY_LABEL: &str = "Loading view";

/// One window's loading-overlay state. `None` when the overlay is
/// dismissed; `Some(since)` while armed.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct LoadingOverlayState {
    armed_at: Option<Instant>,
}

impl LoadingOverlayState {
    /// Construct in the dismissed state.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self { armed_at: None }
    }

    /// Arm the overlay. Idempotent — re-arming preserves the original
    /// arming instant so the fade-in progress survives subsequent
    /// paints. Returns `true` when this call transitioned from
    /// dismissed to armed (a `event:paint_loading_overlay state=show`
    /// trace should follow).
    ///
    /// Currently unreferenced: P18.10 removed the only caller (the
    /// bounded-wait helper). Retained because the state machine is
    /// the obvious surface for a future "loading huge file" banner.
    #[allow(dead_code)]
    pub(crate) fn arm(&mut self, now: Instant) -> bool {
        if self.armed_at.is_some() {
            false
        } else {
            self.armed_at = Some(now);
            true
        }
    }

    /// Dismiss the overlay. Returns `true` when this call transitioned
    /// from armed to dismissed (a `event:paint_loading_overlay
    /// state=hide` trace should follow).
    ///
    /// Currently unreferenced: see [`Self::arm`] for context.
    #[allow(dead_code)]
    pub(crate) fn dismiss(&mut self) -> bool {
        self.armed_at.take().is_some()
    }

    /// `true` when the overlay should paint this frame.
    #[must_use]
    pub(crate) fn is_armed(&self) -> bool {
        self.armed_at.is_some()
    }

    /// Microseconds since the overlay was armed. `0` when dismissed.
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn elapsed_us(&self, now: Instant) -> u64 {
        self.armed_at
            .map(|since| now.duration_since(since).as_micros())
            .unwrap_or(0)
            .min(u128::from(u64::MAX)) as u64
    }

    /// Per-frame motion projection. Reduced motion snaps to identity;
    /// otherwise opacity ramps over [`LOADING_OVERLAY_FADE_IN_MS`].
    #[must_use]
    pub(crate) fn motion(&self, now: Instant, reduced_motion: bool) -> SurfaceMotion {
        let Some(since) = self.armed_at else {
            return SurfaceMotion::IDENTITY;
        };
        if reduced_motion {
            return SurfaceMotion::IDENTITY;
        }
        let elapsed_ms = now.duration_since(since).as_millis() as u64;
        if elapsed_ms >= LOADING_OVERLAY_FADE_IN_MS {
            return SurfaceMotion::IDENTITY;
        }
        let t = elapsed_ms as f32 / LOADING_OVERLAY_FADE_IN_MS as f32;
        let eased = ease_out_cubic(t);
        SurfaceMotion::new(eased, 0.0)
    }
}

/// Project the current state into a renderer payload. `None` when the
/// overlay is dismissed.
#[must_use]
pub(crate) fn build_loading_overlay_draw(
    state: &LoadingOverlayState,
    pane_body_width_dip: f32,
    bg: Rgba,
    fg: Rgba,
    border: Rgba,
) -> Option<LoadingOverlayDraw> {
    if !state.is_armed() {
        return None;
    }
    Some(LoadingOverlayDraw::centered(
        pane_body_width_dip,
        bg,
        fg,
        border,
        LOADING_OVERLAY_LABEL,
    ))
}

/// Build the `(draw, motion)` pair for the current frame. `None` /
/// `None` when the overlay is dismissed; otherwise the draw payload
/// plus the motion projection appropriate to the elapsed arm time and
/// the active reduced-motion preference.
#[must_use]
pub(crate) fn build_loading_overlay_frame(
    state: &LoadingOverlayState,
    pane_body_width_dip: f32,
    bg: Rgba,
    fg: Rgba,
    border: Rgba,
    now: Instant,
    reduced_motion: bool,
) -> (Option<LoadingOverlayDraw>, Option<SurfaceMotion>) {
    let draw = build_loading_overlay_draw(state, pane_body_width_dip, bg, fg, border);
    let motion = draw.as_ref().map(|_| state.motion(now, reduced_motion));
    (draw, motion)
}

impl Window {
    /// Project the loading-overlay state into a renderer payload for
    /// the current paint. Bundles geometry, color, motion, and
    /// reduced-motion handling so the paint orchestrator's call site
    /// stays one line.
    #[must_use]
    pub(crate) fn build_loading_overlay_frame_for_paint(
        &self,
        editor_colors: &EditorColors,
    ) -> (Option<LoadingOverlayDraw>, Option<SurfaceMotion>) {
        build_loading_overlay_frame(
            &self.loading_overlay_state,
            self.view.viewport_width_dip.max(0.0),
            editor_colors.loading_overlay_bg,
            editor_colors.loading_overlay_fg,
            editor_colors.loading_overlay_border,
            Instant::now(),
            self.motion_policy.is_reduced_motion(),
        )
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn arm_is_idempotent() {
        let mut state = LoadingOverlayState::new();
        let now = Instant::now();
        assert!(state.arm(now));
        assert!(!state.arm(now + Duration::from_millis(50)));
        assert!(state.is_armed());
    }

    #[test]
    fn dismiss_returns_true_only_when_armed() {
        let mut state = LoadingOverlayState::new();
        assert!(!state.dismiss());
        state.arm(Instant::now());
        assert!(state.dismiss());
        assert!(!state.is_armed());
    }

    #[test]
    fn reduced_motion_collapses_to_identity() {
        let mut state = LoadingOverlayState::new();
        let now = Instant::now();
        state.arm(now);
        let motion = state.motion(now, /* reduced_motion = */ true);
        assert!(motion.is_identity());
    }

    #[test]
    fn motion_fades_in_over_band() {
        let mut state = LoadingOverlayState::new();
        let started = Instant::now();
        state.arm(started);
        let halfway = started + Duration::from_millis(LOADING_OVERLAY_FADE_IN_MS / 2);
        let motion = state.motion(halfway, /* reduced_motion = */ false);
        assert!(motion.opacity > 0.0);
        assert!(motion.opacity < 1.0);
        let after = started + Duration::from_millis(LOADING_OVERLAY_FADE_IN_MS + 16);
        let motion = state.motion(after, /* reduced_motion = */ false);
        assert!(motion.is_identity());
    }

    #[test]
    fn build_returns_none_when_dismissed() {
        let state = LoadingOverlayState::new();
        let draw = build_loading_overlay_draw(
            &state,
            600.0,
            Rgba::default(),
            Rgba::default(),
            Rgba::default(),
        );
        assert!(draw.is_none());
    }

    #[test]
    fn build_returns_centered_when_armed() {
        let mut state = LoadingOverlayState::new();
        state.arm(Instant::now());
        let draw = build_loading_overlay_draw(
            &state,
            600.0,
            Rgba::default(),
            Rgba::default(),
            Rgba::default(),
        );
        let draw = draw.expect("armed state must build a draw payload");
        assert_eq!(draw.label, LOADING_OVERLAY_LABEL);
    }
}
