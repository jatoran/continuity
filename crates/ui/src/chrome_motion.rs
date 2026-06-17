//! Pane chrome focus/tab activation motion.
//!
//! The tracker is owned by `Window` on the UI thread. It compares the
//! current per-frame chrome payload with the previous one and annotates
//! both the newly-focused and previously-focused panes with a paired
//! focus-crossfade transient, plus the changing pane's tab strip with a
//! slide source/destination pair for the active-tab underline.
//!
//! Pane focus and tab activation both ride the shared 160 ms
//! [`STRUCTURAL_MOTION_MS`] ease-out-cubic. Reduced motion clears all
//! transient state and emits zero frames.

use continuity_render::{PaneChromeDraw, SurfaceMotion};

use crate::motion::{MotionPolicy, MotionSpan, StaggerScheduler, STRUCTURAL_MOTION_MS};

/// One pane-focus crossfade — the same span drives the in-pane (rising
/// active alpha) and out-pane (falling active alpha) annotations.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct FocusTransient {
    in_pane: Option<usize>,
    out_pane: Option<usize>,
    span: MotionSpan,
}

/// One tab-activation slide on a specific pane.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct TabTransient {
    pane_index: usize,
    previous_active: usize,
    span: MotionSpan,
}

/// UI-thread tracker for pane chrome transitions.
#[derive(Clone, Debug, Default)]
pub(crate) struct ChromeMotionState {
    previous_focused: Option<usize>,
    previous_active_tabs: Vec<usize>,
    focus: Option<FocusTransient>,
    tab: Option<TabTransient>,
}

impl ChromeMotionState {
    /// Annotate `chrome` with active focus/tab motion for this frame.
    pub(crate) fn update(
        &mut self,
        chrome: &mut PaneChromeDraw,
        policy: MotionPolicy,
        stagger: &mut StaggerScheduler,
        now_ms: u64,
    ) {
        for pane in &mut chrome.panes {
            pane.focus_motion = None;
            pane.active_tab_motion = None;
            pane.previous_active_tab_index = None;
        }
        if policy.is_reduced_motion() {
            self.capture_baseline(chrome);
            self.focus = None;
            self.tab = None;
            return;
        }
        let focused = chrome.panes.iter().position(|pane| pane.focused);
        if let Some(prev) = self.previous_focused {
            if focused != Some(prev) {
                if let Some(span) = stagger.schedule(policy, now_ms, STRUCTURAL_MOTION_MS) {
                    self.focus = Some(FocusTransient {
                        in_pane: focused,
                        out_pane: Some(prev),
                        span,
                    });
                }
            }
        }
        for (idx, pane) in chrome.panes.iter().enumerate() {
            let previous = self.previous_active_tabs.get(idx).copied();
            if let Some(prev) = previous {
                if prev != pane.active_index {
                    if let Some(span) = stagger.schedule(policy, now_ms, STRUCTURAL_MOTION_MS) {
                        self.tab = Some(TabTransient {
                            pane_index: idx,
                            previous_active: prev,
                            span,
                        });
                    }
                    break;
                }
            }
        }
        self.capture_baseline(chrome);
        self.apply_draw(chrome, now_ms);
    }

    /// `true` while focus/tab chrome has active frames.
    #[must_use]
    pub(crate) fn is_active(&self, now_ms: u64) -> bool {
        self.focus.is_some_and(|t| t.span.is_alive(now_ms))
            || self.tab.is_some_and(|t| t.span.is_alive(now_ms))
    }

    /// Evict elapsed spans.
    pub(crate) fn evict_expired(&mut self, now_ms: u64) {
        if self.focus.is_some_and(|t| !t.span.is_alive(now_ms)) {
            self.focus = None;
        }
        if self.tab.is_some_and(|t| !t.span.is_alive(now_ms)) {
            self.tab = None;
        }
    }

    /// Clear all animation state.
    pub(crate) fn clear(&mut self) {
        self.focus = None;
        self.tab = None;
    }

    fn capture_baseline(&mut self, chrome: &PaneChromeDraw) {
        self.previous_focused = chrome.panes.iter().position(|pane| pane.focused);
        self.previous_active_tabs = chrome.panes.iter().map(|pane| pane.active_index).collect();
    }

    fn apply_draw(&mut self, chrome: &mut PaneChromeDraw, now_ms: u64) {
        if let Some(active) = self.focus {
            match active.span.progress(now_ms) {
                Some(progress) => {
                    let rising = SurfaceMotion::new(progress, 0.0);
                    let falling = SurfaceMotion::new(1.0 - progress, 0.0);
                    if let Some(in_idx) = active.in_pane {
                        if let Some(pane) = chrome.panes.get_mut(in_idx) {
                            pane.focus_motion = Some(rising);
                        }
                    }
                    if let Some(out_idx) = active.out_pane {
                        if let Some(pane) = chrome.panes.get_mut(out_idx) {
                            pane.focus_motion = Some(falling);
                        }
                    }
                }
                None => self.focus = None,
            }
        }
        if let Some(active) = self.tab {
            match active.span.progress(now_ms) {
                Some(progress) => {
                    if let Some(pane) = chrome.panes.get_mut(active.pane_index) {
                        pane.active_tab_motion = Some(SurfaceMotion::new(progress, 0.0));
                        pane.previous_active_tab_index = Some(active.previous_active);
                    }
                }
                None => self.tab = None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_render::{PaneStripDraw, PanelColors};

    fn chrome(focused: usize, active: usize) -> PaneChromeDraw {
        PaneChromeDraw {
            panes: vec![
                PaneStripDraw {
                    outer: (0.0, 0.0, 100.0, 100.0),
                    focused: focused == 0,
                    tabs: Vec::new(),
                    active_index: active,
                    focus_motion: None,
                    active_tab_motion: None,
                    previous_active_tab_index: None,
                    tab_scroll_offset_dip: 0.0,
                },
                PaneStripDraw {
                    outer: (100.0, 0.0, 100.0, 100.0),
                    focused: focused == 1,
                    tabs: Vec::new(),
                    active_index: 0,
                    focus_motion: None,
                    active_tab_motion: None,
                    previous_active_tab_index: None,
                    tab_scroll_offset_dip: 0.0,
                },
            ],
            colors: PanelColors::default(),
            strip_height: 24.0,
            tab_drag: None,
        }
    }

    fn chrome_with_active(active_left: usize, active_right: usize) -> PaneChromeDraw {
        let mut c = chrome(0, active_left);
        c.panes[1].active_index = active_right;
        c
    }

    #[test]
    fn reduced_motion_produces_zero_chrome_frames() {
        let mut state = ChromeMotionState::default();
        let mut stagger = StaggerScheduler::default();
        let mut first = chrome(0, 0);
        state.update(&mut first, MotionPolicy::new(true), &mut stagger, 100);
        let mut second = chrome(1, 1);
        state.update(&mut second, MotionPolicy::new(true), &mut stagger, 100);
        assert!(second.panes.iter().all(|p| p.focus_motion.is_none()));
        assert!(second.panes.iter().all(|p| p.active_tab_motion.is_none()));
        assert!(second
            .panes
            .iter()
            .all(|p| p.previous_active_tab_index.is_none()));
        assert!(!state.is_active(100));
    }

    #[test]
    fn focus_change_marks_new_focused_pane_with_rising_alpha() {
        let mut state = ChromeMotionState::default();
        let mut stagger = StaggerScheduler::default();
        // Baseline frame, focus change frame, then a follow-up frame at
        // mid-span so progress is non-edge.
        state.update(
            &mut chrome(0, 0),
            MotionPolicy::default(),
            &mut stagger,
            100,
        );
        state.update(
            &mut chrome(1, 0),
            MotionPolicy::default(),
            &mut stagger,
            100,
        );
        let mut mid = chrome(1, 0);
        state.update(&mut mid, MotionPolicy::default(), &mut stagger, 180);
        let rising = mid.panes[1].focus_motion.expect("focus-in motion");
        assert!(rising.opacity > 0.0 && rising.opacity < 1.0);
    }

    #[test]
    fn focus_change_paired_panes_alpha_sums_to_one() {
        let mut state = ChromeMotionState::default();
        let mut stagger = StaggerScheduler::default();
        state.update(
            &mut chrome(0, 0),
            MotionPolicy::default(),
            &mut stagger,
            100,
        );
        state.update(
            &mut chrome(1, 0),
            MotionPolicy::default(),
            &mut stagger,
            100,
        );
        let mut mid = chrome(1, 0);
        state.update(&mut mid, MotionPolicy::default(), &mut stagger, 180);
        let rising = mid.panes[1].focus_motion.expect("focus-in").opacity;
        let falling = mid.panes[0].focus_motion.expect("focus-out").opacity;
        // The two panes ride the same span — focus-in α + focus-out α
        // must always equal 1.0 so the visual is a true crossfade.
        assert!((rising + falling - 1.0).abs() < 1e-3);
    }

    #[test]
    fn tab_change_records_previous_index_for_slide() {
        let mut state = ChromeMotionState::default();
        let mut stagger = StaggerScheduler::default();
        let mut first = chrome_with_active(0, 0);
        first.panes[0].tabs = vec![tab("a"), tab("b"), tab("c")];
        state.update(&mut first, MotionPolicy::default(), &mut stagger, 100);
        let mut second = chrome_with_active(2, 0);
        second.panes[0].tabs = vec![tab("a"), tab("b"), tab("c")];
        state.update(&mut second, MotionPolicy::default(), &mut stagger, 180);
        assert_eq!(second.panes[0].previous_active_tab_index, Some(0));
        assert!(second.panes[0].active_tab_motion.is_some());
    }

    #[test]
    fn span_expiry_clears_focus_state() {
        let mut state = ChromeMotionState::default();
        let mut stagger = StaggerScheduler::default();
        state.update(
            &mut chrome(0, 0),
            MotionPolicy::default(),
            &mut stagger,
            100,
        );
        state.update(
            &mut chrome(1, 0),
            MotionPolicy::default(),
            &mut stagger,
            100,
        );
        assert!(state.is_active(100));
        state.evict_expired(100 + u64::from(STRUCTURAL_MOTION_MS) + 1);
        assert!(!state.is_active(100 + u64::from(STRUCTURAL_MOTION_MS) + 1));
    }

    fn tab(label: &str) -> continuity_render::TabLabel {
        continuity_render::TabLabel {
            text: label.to_string(),
            dirty: false,
            show_close: false,
        }
    }
}
