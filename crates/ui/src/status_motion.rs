//! Status-bar value-change transients.
//!
//! Owned by `Window` on the UI thread. The tracker compares the current
//! status payload with the previous frame and emits localized draw
//! transients for changed C1/C2/C3 values.

use continuity_render::{
    StatusBarSegmentDraw, StatusBarSegmentKind, StatusTransientDraw, StatusTransientGroup,
};

/// Status-bar segment kinds whose values update so frequently
/// (per keystroke / per caret move / per selection-drag tick) that
/// the 180 ms acknowledgement transient — a second copy of the
/// text painted at a sliding `−3 DIP` offset with fading alpha —
/// is perceived as **ghost-offset blur** rather than a tactile
/// bump. The other kinds (encoding label, line-ending detection,
/// language tag, chips) change rarely enough that the motion reads
/// the way it was designed to. See the
/// `paint_transients`/`transient_alpha_and_offset` code for the
/// underlying double-draw shape.
#[must_use]
fn is_motion_eligible_kind(kind: StatusBarSegmentKind) -> bool {
    match kind {
        // High-frequency live counters — suppress.
        StatusBarSegmentKind::Position
        | StatusBarSegmentKind::Chars
        | StatusBarSegmentKind::Words
        | StatusBarSegmentKind::Lines
        | StatusBarSegmentKind::Selection
        | StatusBarSegmentKind::NumericSum => false,
        // Rare state changes + chips — keep the motion cue.
        StatusBarSegmentKind::Encoding
        | StatusBarSegmentKind::LineEndings
        | StatusBarSegmentKind::Language
        | StatusBarSegmentKind::IdleStale
        | StatusBarSegmentKind::Chip
        | StatusBarSegmentKind::NoticeChip
        | StatusBarSegmentKind::PersistQueueChip => true,
    }
}

use crate::motion::{
    transient_alpha_and_offset, MotionPolicy, MotionSpan, StaggerScheduler, ACK_MOTION_MS,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct SegmentValue {
    group: StatusTransientGroup,
    index: usize,
    kind: continuity_render::StatusBarSegmentKind,
    text: String,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct ActiveTransient {
    group: StatusTransientGroup,
    index: usize,
    span: MotionSpan,
}

/// UI-thread tracker for status-bar retinal transients.
#[derive(Clone, Debug, Default)]
pub(crate) struct StatusMotionState {
    previous: Vec<SegmentValue>,
    active: Vec<ActiveTransient>,
}

impl StatusMotionState {
    /// Update from the current status payload and return per-frame draw data.
    pub(crate) fn update(
        &mut self,
        segments: &[StatusBarSegmentDraw],
        chips: &[StatusBarSegmentDraw],
        policy: MotionPolicy,
        stagger: &mut StaggerScheduler,
        now_ms: u64,
    ) -> Vec<StatusTransientDraw> {
        let current = collect_values(segments, chips);
        if policy.is_reduced_motion() {
            self.previous = current;
            self.active.clear();
            return Vec::new();
        }
        if self.previous.is_empty() {
            self.previous = current;
            return Vec::new();
        }
        for value in &current {
            let changed = match self
                .previous
                .iter()
                .find(|old| old.group == value.group && old.index == value.index)
            {
                Some(old) => old.kind != value.kind || old.text != value.text,
                None => true,
            };
            if changed {
                self.active
                    .retain(|t| !(t.group == value.group && t.index == value.index));
                // Skip the slide-and-fade acknowledgement for kinds
                // that re-render every keystroke / every caret move
                // — the doubled draw shows up as ghost-offset blur
                // on live counters. See `is_motion_eligible_kind`.
                if !is_motion_eligible_kind(value.kind) {
                    continue;
                }
                if let Some(span) = stagger.schedule(policy, now_ms, ACK_MOTION_MS) {
                    self.active.push(ActiveTransient {
                        group: value.group,
                        index: value.index,
                        span,
                    });
                }
            }
        }
        self.previous = current;
        self.draw(now_ms)
    }

    /// Current active transient count, used by timer gating/tests.
    #[must_use]
    pub(crate) fn active_len(&self, now_ms: u64) -> usize {
        self.active
            .iter()
            .filter(|t| t.span.is_alive(now_ms))
            .count()
    }

    /// Evict elapsed spans.
    pub(crate) fn evict_expired(&mut self, now_ms: u64) {
        self.active.retain(|t| t.span.is_alive(now_ms));
    }

    fn draw(&mut self, now_ms: u64) -> Vec<StatusTransientDraw> {
        let mut out = Vec::with_capacity(self.active.len());
        self.active.retain(|active| {
            let Some(progress) = active.span.progress(now_ms) else {
                return false;
            };
            let (alpha, translate_y_dip) = transient_alpha_and_offset(progress);
            out.push(StatusTransientDraw {
                group: active.group,
                index: active.index,
                alpha,
                translate_y_dip,
            });
            true
        });
        out
    }
}

fn collect_values(
    segments: &[StatusBarSegmentDraw],
    chips: &[StatusBarSegmentDraw],
) -> Vec<SegmentValue> {
    let mut values = Vec::with_capacity(segments.len() + chips.len());
    values.extend(
        segments
            .iter()
            .enumerate()
            .map(|(index, segment)| SegmentValue {
                group: StatusTransientGroup::Segment,
                index,
                kind: segment.kind,
                text: segment.text.clone(),
            }),
    );
    values.extend(chips.iter().enumerate().map(|(index, chip)| SegmentValue {
        group: StatusTransientGroup::Chip,
        index,
        kind: chip.kind,
        text: chip.text.clone(),
    }));
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_render::StatusBarSegmentKind;

    fn seg(text: &str) -> StatusBarSegmentDraw {
        seg_of_kind(text, StatusBarSegmentKind::Position)
    }

    fn seg_of_kind(text: &str, kind: StatusBarSegmentKind) -> StatusBarSegmentDraw {
        StatusBarSegmentDraw {
            text: text.to_string(),
            kind,
            hover: None,
            alpha: 1.0,
        }
    }

    #[test]
    fn first_frame_is_baseline_only() {
        let mut state = StatusMotionState::default();
        let mut stagger = StaggerScheduler::default();
        let draw = state.update(
            &[seg("Ln 1, Col 1")],
            &[],
            MotionPolicy::default(),
            &mut stagger,
            100,
        );
        assert!(draw.is_empty());
    }

    #[test]
    fn changed_motion_eligible_segment_emits_transient() {
        // Encoding flips are rare (file load / reload-with-encoding)
        // and read well as a tactile "settled to UTF-8" bump.
        let mut state = StatusMotionState::default();
        let mut stagger = StaggerScheduler::default();
        let policy = MotionPolicy::default();
        let _ = state.update(
            &[seg_of_kind("UTF-8", StatusBarSegmentKind::Encoding)],
            &[],
            policy,
            &mut stagger,
            100,
        );
        let draw = state.update(
            &[seg_of_kind("CP1252", StatusBarSegmentKind::Encoding)],
            &[],
            policy,
            &mut stagger,
            100,
        );
        assert_eq!(draw.len(), 1);
        assert_eq!(draw[0].group, StatusTransientGroup::Segment);
        assert_eq!(draw[0].index, 0);
    }

    #[test]
    fn high_frequency_counter_change_does_not_emit_transient() {
        // Regression: the 180 ms slide-and-fade transient over
        // live counter text produced a "ghost-offset blur" while
        // typing — the new text was drawn at the steady Y AND a
        // second copy at `top + (−3..0 DIP)` with fading alpha.
        // Suppressed for Position/Chars/Words/Lines/Selection/
        // NumericSum.
        let mut stagger = StaggerScheduler::default();
        let policy = MotionPolicy::default();
        for kind in [
            StatusBarSegmentKind::Position,
            StatusBarSegmentKind::Chars,
            StatusBarSegmentKind::Words,
            StatusBarSegmentKind::Lines,
            StatusBarSegmentKind::Selection,
            StatusBarSegmentKind::NumericSum,
        ] {
            // Fresh state per kind so they assert independently.
            let mut state = StatusMotionState::default();
            let _ = state.update(&[seg_of_kind("old", kind)], &[], policy, &mut stagger, 100);
            let draw = state.update(&[seg_of_kind("new", kind)], &[], policy, &mut stagger, 100);
            assert!(
                draw.is_empty(),
                "kind={kind:?} must not emit a transient, but draw={draw:?}",
            );
            assert_eq!(state.active_len(100), 0, "kind={kind:?}");
        }
    }

    #[test]
    fn reduced_motion_suppresses_status_transients() {
        let mut state = StatusMotionState::default();
        let mut stagger = StaggerScheduler::default();
        let _ = state.update(&[seg("A")], &[], MotionPolicy::new(true), &mut stagger, 100);
        let draw = state.update(&[seg("B")], &[], MotionPolicy::new(true), &mut stagger, 100);
        assert!(draw.is_empty());
        assert_eq!(state.active_len(100), 0);
    }

    #[test]
    fn new_chip_emits_transient_after_baseline() {
        let mut state = StatusMotionState::default();
        let mut stagger = StaggerScheduler::default();
        let policy = MotionPolicy::default();
        let _ = state.update(&[seg("Ln 1")], &[], policy, &mut stagger, 100);
        let draw = state.update(
            &[seg("Ln 1")],
            &[seg_of_kind("sync", StatusBarSegmentKind::Chip)],
            policy,
            &mut stagger,
            100,
        );
        assert_eq!(draw.len(), 1);
        assert_eq!(draw[0].group, StatusTransientGroup::Chip);
        assert_eq!(draw[0].index, 0);
    }
}
