//! UI-thread surface enter/exit motion for overlay-like layers.

use continuity_render::{OverlayDraw, SurfaceMotion};

use crate::motion::{
    enter_motion, exit_motion, MotionPolicy, MotionSpan, StaggerScheduler, STRUCTURAL_MOTION_MS,
};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum SurfaceDirection {
    Enter,
    Exit,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct SurfaceTransition {
    span: MotionSpan,
    direction: SurfaceDirection,
}

/// Projected overlay layer for one paint frame.
#[derive(Clone, Debug)]
pub(crate) struct MotionOverlayLayer {
    /// Overlay draw payload.
    pub(crate) draw: OverlayDraw,
    /// Per-frame motion projection.
    pub(crate) motion: SurfaceMotion,
}

/// Tracks enter/exit motion and retains the previous payload long enough
/// for a dismiss animation.
#[derive(Clone, Debug, Default)]
pub(crate) struct SurfaceMotionState {
    visible_key: Option<String>,
    retained: Option<OverlayDraw>,
    transition: Option<SurfaceTransition>,
}

impl SurfaceMotionState {
    /// Project the current payload to a drawable layer.
    pub(crate) fn project(
        &mut self,
        key: Option<String>,
        draw: Option<OverlayDraw>,
        policy: MotionPolicy,
        stagger: &mut StaggerScheduler,
        now_ms: u64,
    ) -> Option<MotionOverlayLayer> {
        if policy.is_reduced_motion() {
            self.visible_key = key;
            self.retained = draw.clone();
            self.transition = None;
            return draw.map(|draw| MotionOverlayLayer {
                draw,
                motion: SurfaceMotion::IDENTITY,
            });
        }

        match (key, draw) {
            (Some(next_key), Some(next_draw)) => {
                if self.visible_key.as_deref() != Some(next_key.as_str()) {
                    self.transition =
                        stagger
                            .schedule(policy, now_ms, STRUCTURAL_MOTION_MS)
                            .map(|span| SurfaceTransition {
                                span,
                                direction: SurfaceDirection::Enter,
                            });
                }
                self.visible_key = Some(next_key);
                self.retained = Some(next_draw.clone());
                Some(MotionOverlayLayer {
                    draw: next_draw,
                    motion: self.current_motion(now_ms).unwrap_or_default(),
                })
            }
            (None, None) => {
                if self.visible_key.is_some() && self.retained.is_some() {
                    self.transition =
                        stagger
                            .schedule(policy, now_ms, STRUCTURAL_MOTION_MS)
                            .map(|span| SurfaceTransition {
                                span,
                                direction: SurfaceDirection::Exit,
                            });
                    self.visible_key = None;
                }
                let motion = self.current_motion(now_ms)?;
                let draw = self.retained.clone()?;
                if self.transition.is_none() {
                    self.retained = None;
                    return None;
                }
                Some(MotionOverlayLayer { draw, motion })
            }
            _ => None,
        }
    }

    /// `true` while a transition still needs frame ticks.
    #[must_use]
    pub(crate) fn is_active(&self, now_ms: u64) -> bool {
        self.transition
            .is_some_and(|transition| transition.span.is_alive(now_ms))
    }

    /// Cancel all retained animation state.
    pub(crate) fn clear(&mut self) {
        self.visible_key = None;
        self.retained = None;
        self.transition = None;
    }

    fn current_motion(&mut self, now_ms: u64) -> Option<SurfaceMotion> {
        let transition = self.transition?;
        let Some(progress) = transition.span.progress(now_ms) else {
            self.transition = None;
            if transition.direction == SurfaceDirection::Exit {
                self.retained = None;
            }
            return None;
        };
        Some(match transition.direction {
            SurfaceDirection::Enter => enter_motion(progress),
            SurfaceDirection::Exit => exit_motion(progress),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_render::{PanelStyle, Rect, Rgba};

    fn overlay() -> OverlayDraw {
        OverlayDraw {
            panel: PanelStyle {
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                corner_radius: 0.0,
                bg: Rgba::BLACK,
                border: Rgba::TRANSPARENT,
                shadow: Rgba::TRANSPARENT,
                shadow_offset: 0.0,
            },
            input_focused: false,
            focus_field: None,
            secondary_field: None,
            list_rows: Vec::new(),
            scrollbar: None,
            footer: None,
        }
    }

    #[test]
    fn reduced_motion_projects_identity() {
        let mut state = SurfaceMotionState::default();
        let mut stagger = StaggerScheduler::default();
        let layer = state
            .project(
                Some("overlay".into()),
                Some(overlay()),
                MotionPolicy::new(true),
                &mut stagger,
                100,
            )
            .expect("layer");
        assert_eq!(layer.motion, SurfaceMotion::IDENTITY);
        assert!(!state.is_active(100));
    }

    #[test]
    fn reduced_motion_produces_zero_banner_and_chord_frames() {
        let mut banner = SurfaceMotionState::default();
        let mut chord = SurfaceMotionState::default();
        let mut stagger = StaggerScheduler::default();
        let policy = MotionPolicy::new(true);
        let _ = banner.project(
            Some("banner".into()),
            Some(overlay()),
            policy,
            &mut stagger,
            0,
        );
        let _ = chord.project(
            Some("chord-hud".into()),
            Some(overlay()),
            policy,
            &mut stagger,
            0,
        );
        assert!(!banner.is_active(0));
        assert!(!chord.is_active(0));
    }

    #[test]
    fn enter_transition_is_active() {
        let mut state = SurfaceMotionState::default();
        let mut stagger = StaggerScheduler::default();
        let layer = state
            .project(
                Some("overlay".into()),
                Some(overlay()),
                MotionPolicy::default(),
                &mut stagger,
                100,
            )
            .expect("layer");
        assert!(layer.motion.opacity < 0.05);
        assert!(state.is_active(100));
    }
}
