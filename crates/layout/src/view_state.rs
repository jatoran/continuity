//! Per-pane runtime view state: scroll position (with subpixel offset and
//! optional animator), font zoom, and soft-wrap toggle.
//!
//! Persisted view rows live in `persist::view_states`; this struct is the
//! in-memory mirror that the renderer consults each frame.

/// Minimum allowed font-zoom multiplier.
pub const MIN_ZOOM: f32 = 0.5;
/// Maximum allowed font-zoom multiplier.
pub const MAX_ZOOM: f32 = 4.0;
/// Default scroll-animation duration (ms). The UI layer passes the
/// canonical α.0 motion-contract duration; this value remains as a
/// layout-crate fallback for direct callers.
pub const DEFAULT_SCROLL_ANIM_MS: u64 = 160;
/// Default zoom step per Ctrl+scroll notch (10 % per spec §5).
pub const DEFAULT_ZOOM_STEP: f32 = 0.10;

/// Viewport state for one (pane, buffer) pair. Coordinates are in
/// device-independent pixels (DIPs).
#[derive(Clone, Debug)]
pub struct ViewState {
    /// Top of viewport in DIPs from buffer top. May be fractional for
    /// subpixel scroll.
    pub scroll_y_dip: f32,
    /// Active animation target in DIPs, if any.
    scroll_target_dip: Option<f32>,
    /// `now_ms` value when the current animation started.
    anim_start_ms: u64,
    /// Animation duration (ms).
    anim_duration_ms: u64,
    /// Scroll value at animation start.
    anim_from_dip: f32,
    /// Per-pane font zoom; multiplies the configured base font size.
    pub font_size_scale: f32,
    /// Whether soft wrap is enabled.
    pub soft_wrap: bool,
    /// Viewport pixel width in DIPs (rebuilt by the window on resize).
    pub viewport_width_dip: f32,
    /// Viewport pixel height in DIPs.
    pub viewport_height_dip: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            scroll_y_dip: 0.0,
            scroll_target_dip: None,
            anim_start_ms: 0,
            anim_duration_ms: DEFAULT_SCROLL_ANIM_MS,
            anim_from_dip: 0.0,
            font_size_scale: 1.0,
            soft_wrap: true,
            viewport_width_dip: 0.0,
            viewport_height_dip: 0.0,
        }
    }
}

impl ViewState {
    /// Empty view state at zoom 1.0, scroll 0, with soft wrap on
    /// (matches the `[editor].word_wrap = true` settings default).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Convenience constructor for tests / harnesses that need a viewport
    /// of a specific size. Avoids exposing the animator-internal fields
    /// while still letting external callers construct a `ViewState`.
    #[must_use]
    pub fn with_viewport(width_dip: f32, height_dip: f32) -> Self {
        Self {
            viewport_width_dip: width_dip,
            viewport_height_dip: height_dip,
            ..Self::default()
        }
    }

    /// Width to feed into the layout-cache key. `0` when soft wrap is off so
    /// every cached layout shares the same key across viewport widths.
    #[must_use]
    pub fn wrap_width_key(&self) -> u32 {
        if self.soft_wrap {
            self.viewport_width_dip.max(0.0).round() as u32
        } else {
            0
        }
    }

    /// Pixel-locked instant scroll (mouse wheel, drag). Cancels any running
    /// animation.
    pub fn scroll_instant(&mut self, delta_dip: f32, content_height_dip: f32) {
        self.scroll_target_dip = None;
        self.scroll_y_dip = clamp_scroll(
            self.scroll_y_dip + delta_dip,
            content_height_dip,
            self.viewport_height_dip,
        );
    }

    /// Set scroll position immediately to an absolute value (used by goto-
    /// line, find, and similar focus-on-target commands).
    pub fn jump_to(&mut self, target_dip: f32, content_height_dip: f32) {
        self.scroll_target_dip = None;
        self.scroll_y_dip = clamp_scroll(target_dip, content_height_dip, self.viewport_height_dip);
    }

    /// Begin an animated easing scroll (Page Up/Down, Ctrl+Home/End).
    pub fn scroll_animated(
        &mut self,
        target_dip: f32,
        content_height_dip: f32,
        now_ms: u64,
        duration_ms: u64,
    ) {
        if duration_ms == 0 {
            self.jump_to(target_dip, content_height_dip);
            return;
        }
        self.anim_from_dip = self.scroll_y_dip;
        self.scroll_target_dip = Some(clamp_scroll(
            target_dip,
            content_height_dip,
            self.viewport_height_dip,
        ));
        self.anim_start_ms = now_ms;
        self.anim_duration_ms = duration_ms;
    }

    /// Step the animator forward. Returns `true` when the scroll position
    /// changed and a repaint is needed.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        let Some(target) = self.scroll_target_dip else {
            return false;
        };
        let dt = now_ms.saturating_sub(self.anim_start_ms);
        if dt >= self.anim_duration_ms {
            let moved = (self.scroll_y_dip - target).abs() > f32::EPSILON;
            self.scroll_y_dip = target;
            self.scroll_target_dip = None;
            return moved;
        }
        let t = dt as f32 / self.anim_duration_ms as f32;
        // Cubic ease-out: 1 - (1 - t)^3.
        let eased = 1.0 - (1.0 - t).powi(3);
        let prev = self.scroll_y_dip;
        self.scroll_y_dip = self.anim_from_dip + (target - self.anim_from_dip) * eased;
        (self.scroll_y_dip - prev).abs() > f32::EPSILON
    }

    /// Cancel any in-flight scroll animation while preserving the
    /// current fractional scroll position.
    pub fn cancel_scroll_animation(&mut self) {
        self.scroll_target_dip = None;
    }

    /// `true` while an animation is still in progress.
    #[must_use]
    pub fn animating(&self) -> bool {
        self.scroll_target_dip.is_some()
    }

    /// Adjust zoom by a multiplicative factor (e.g. `1.10` for one zoom-in
    /// notch at the default 10 % step).
    pub fn adjust_zoom(&mut self, factor: f32) {
        self.font_size_scale = (self.font_size_scale * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    }

    /// Reset zoom to 1.0.
    pub fn reset_zoom(&mut self) {
        self.font_size_scale = 1.0;
    }

    /// Toggle soft wrap. The caller is responsible for invalidating the
    /// layout cache for entries whose `wrap_width_dip` no longer matches
    /// (`LayoutCache::invalidate_other_wrap_widths`).
    pub fn toggle_soft_wrap(&mut self) {
        self.soft_wrap = !self.soft_wrap;
    }
}

fn clamp_scroll(scroll: f32, content_h: f32, viewport_h: f32) -> f32 {
    let max = (content_h - viewport_h).max(0.0);
    scroll.clamp(0.0, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state() {
        let s = ViewState::default();
        assert_eq!(s.scroll_y_dip, 0.0);
        assert_eq!(s.font_size_scale, 1.0);
        // Soft wrap is on by default — matches the `[editor].word_wrap`
        // settings default and avoids horizontal overflow on first paint.
        assert!(s.soft_wrap);
        assert!(!s.animating());
    }

    fn vp(width: f32, height: f32) -> ViewState {
        ViewState {
            viewport_width_dip: width,
            viewport_height_dip: height,
            ..ViewState::default()
        }
    }

    #[test]
    fn scroll_instant_clamps_in_both_directions() {
        let mut s = vp(0.0, 100.0);
        s.scroll_instant(50.0, 200.0); // content > viewport; max = 100.
        assert_eq!(s.scroll_y_dip, 50.0);
        s.scroll_instant(200.0, 200.0);
        assert_eq!(s.scroll_y_dip, 100.0);
        s.scroll_instant(-1000.0, 200.0);
        assert_eq!(s.scroll_y_dip, 0.0);
    }

    #[test]
    fn scroll_clamps_to_zero_when_content_fits() {
        let mut s = vp(0.0, 200.0);
        s.scroll_instant(50.0, 100.0); // content shorter than viewport.
        assert_eq!(s.scroll_y_dip, 0.0);
    }

    #[test]
    fn animated_scroll_progresses_and_settles() {
        let mut s = vp(0.0, 100.0);
        s.scroll_animated(80.0, 200.0, 0, 80);
        assert!(s.tick(40));
        assert!(s.scroll_y_dip > 0.0 && s.scroll_y_dip < 80.0);
        assert!(s.animating());
        let _ = s.tick(200);
        assert!((s.scroll_y_dip - 80.0).abs() < f32::EPSILON);
        assert!(!s.animating());
        // Further ticks are no-ops.
        assert!(!s.tick(300));
    }

    #[test]
    fn zero_duration_scroll_produces_no_animation_frames() {
        let mut s = vp(0.0, 100.0);
        s.scroll_animated(80.0, 200.0, 0, 0);
        assert_eq!(s.scroll_y_dip, 80.0);
        assert!(!s.animating());
        assert!(!s.tick(0));
        assert!(!s.tick(1));
    }

    #[test]
    fn zoom_clamps_within_min_max() {
        let mut s = ViewState::default();
        for _ in 0..200 {
            s.adjust_zoom(1.10);
        }
        assert!((s.font_size_scale - MAX_ZOOM).abs() < f32::EPSILON);
        s.reset_zoom();
        for _ in 0..200 {
            s.adjust_zoom(0.90);
        }
        assert!((s.font_size_scale - MIN_ZOOM).abs() < f32::EPSILON);
    }

    #[test]
    fn wrap_width_key_zero_when_disabled() {
        let mut s = vp(600.0, 0.0);
        s.soft_wrap = false;
        assert_eq!(s.wrap_width_key(), 0);
        s.soft_wrap = true;
        assert_eq!(s.wrap_width_key(), 600);
    }

    #[test]
    fn jump_to_clamps() {
        let mut s = vp(0.0, 100.0);
        s.jump_to(500.0, 250.0);
        assert_eq!(s.scroll_y_dip, 150.0);
    }
}
