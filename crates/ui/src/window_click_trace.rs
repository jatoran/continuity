use std::time::Instant;

use crate::paint_trace::EventScope;

const CLICK_STAGE_COUNT: usize = 19;

#[derive(Copy, Clone)]
#[repr(usize)]
pub(crate) enum ClickStage {
    Overlay = 0,
    BufferHistory,
    TimeMachine,
    StatusBar,
    TabStrip,
    Splitter,
    SearchMinimap,
    Minimap,
    Outline,
    FoldTriangle,
    Scrollbar,
    ImageHit,
    CloseArm,
    CodeCopy,
    PaneFocus,
    SegmentHit,
    BufferPosition,
    ClickState,
    CaretPlacement,
}

pub(crate) struct ClickLeftDownTrace {
    scope: Option<EventScope>,
    started: Option<Instant>,
    stage_us: [u64; CLICK_STAGE_COUNT],
    x: i32,
    y: i32,
    claimed: &'static str,
}

impl ClickLeftDownTrace {
    pub(crate) fn new(x: i32, y: i32) -> Self {
        let is_enabled = crate::paint_trace::is_trace_enabled();
        Self {
            scope: is_enabled
                .then(|| EventScope::with_detail("click_on_left_button_down", String::new())),
            started: is_enabled.then(Instant::now),
            stage_us: [0; CLICK_STAGE_COUNT],
            x,
            y,
            claimed: "none",
        }
    }

    pub(crate) fn claim(&mut self, claimed: &'static str) {
        if self.scope.is_some() {
            self.claimed = claimed;
        }
    }

    pub(crate) fn measure<T>(&mut self, stage: ClickStage, work: impl FnOnce() -> T) -> T {
        if self.scope.is_none() {
            return work();
        }
        let started = Instant::now();
        let result = work();
        let elapsed = started.elapsed().as_micros();
        let elapsed = u64::try_from(elapsed).unwrap_or(u64::MAX);
        self.stage_us[stage as usize] = self.stage_us[stage as usize].saturating_add(elapsed);
        result
    }
}

impl Drop for ClickLeftDownTrace {
    fn drop(&mut self) {
        let Some(started) = self.started.take() else {
            return;
        };
        let total_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let detail = build_click_detail(self.x, self.y, self.claimed, total_us, &self.stage_us);
        if let Some(scope) = self.scope.as_mut() {
            scope.set_detail(detail);
        }
    }
}

fn compute_stage_sum_us(stage_us: &[u64; CLICK_STAGE_COUNT]) -> u64 {
    stage_us
        .iter()
        .copied()
        .fold(0u64, |sum, value| sum.saturating_add(value))
}

fn compute_unattributed_us(total_us: u64, stage_sum_us: u64) -> u64 {
    total_us.saturating_sub(stage_sum_us)
}

fn compute_sum_us(total_us: u64, stage_sum_us: u64) -> u64 {
    total_us.max(stage_sum_us)
}

fn build_click_detail(
    x: i32,
    y: i32,
    claimed: &'static str,
    total_us: u64,
    stage_us: &[u64; CLICK_STAGE_COUNT],
) -> String {
    let stage_sum_us = compute_stage_sum_us(stage_us);
    let unattributed_us = compute_unattributed_us(total_us, stage_sum_us);
    let sum_us = compute_sum_us(total_us, stage_sum_us);
    format!(
        "x={x} y={y} claimed={claimed} \
         overlay_us={} buffer_history_us={} time_machine_us={} status_bar_us={} \
         tab_strip_us={} splitter_us={} search_minimap_us={} minimap_us={} \
         outline_us={} fold_triangle_us={} scrollbar_us={} image_hit_us={} \
         close_arm_us={} code_copy_us={} pane_focus_us={} segment_hit_us={} \
         buffer_position_us={} click_state_us={} caret_placement_us={} \
         unattributed_us={unattributed_us} sum_us={sum_us}",
        stage_us[ClickStage::Overlay as usize],
        stage_us[ClickStage::BufferHistory as usize],
        stage_us[ClickStage::TimeMachine as usize],
        stage_us[ClickStage::StatusBar as usize],
        stage_us[ClickStage::TabStrip as usize],
        stage_us[ClickStage::Splitter as usize],
        stage_us[ClickStage::SearchMinimap as usize],
        stage_us[ClickStage::Minimap as usize],
        stage_us[ClickStage::Outline as usize],
        stage_us[ClickStage::FoldTriangle as usize],
        stage_us[ClickStage::Scrollbar as usize],
        stage_us[ClickStage::ImageHit as usize],
        stage_us[ClickStage::CloseArm as usize],
        stage_us[ClickStage::CodeCopy as usize],
        stage_us[ClickStage::PaneFocus as usize],
        stage_us[ClickStage::SegmentHit as usize],
        stage_us[ClickStage::BufferPosition as usize],
        stage_us[ClickStage::ClickState as usize],
        stage_us[ClickStage::CaretPlacement as usize],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_within_accounting_tolerance(total_us: u64, sum_us: u64) -> bool {
        let delta = total_us.abs_diff(sum_us);
        delta <= (total_us / 20).max(1)
    }

    #[test]
    fn sub_stage_accounting_matches_total_within_tolerance() {
        assert_eq!(ClickStage::CaretPlacement as usize + 1, CLICK_STAGE_COUNT);
        let mut stage_us = [0; CLICK_STAGE_COUNT];
        stage_us[ClickStage::Overlay as usize] = 10;
        stage_us[ClickStage::PaneFocus as usize] = 880;
        stage_us[ClickStage::CaretPlacement as usize] = 50;
        let stage_sum_us = compute_stage_sum_us(&stage_us);
        let sum_us = compute_sum_us(1_000, stage_sum_us);
        assert!(is_within_accounting_tolerance(1_000, sum_us));
        assert_eq!(compute_unattributed_us(1_000, stage_sum_us), 60);
    }

    #[test]
    fn click_detail_contains_all_sub_stage_fields() {
        let mut stage_us = [0; CLICK_STAGE_COUNT];
        stage_us[ClickStage::CodeCopy as usize] = 7;
        let detail = build_click_detail(12, 34, "code_copy", 10, &stage_us);
        assert!(detail.contains("overlay_us=0"));
        assert!(detail.contains("buffer_history_us=0"));
        assert!(detail.contains("time_machine_us=0"));
        assert!(detail.contains("status_bar_us=0"));
        assert!(detail.contains("tab_strip_us=0"));
        assert!(detail.contains("splitter_us=0"));
        assert!(detail.contains("search_minimap_us=0"));
        assert!(detail.contains("minimap_us=0"));
        assert!(detail.contains("outline_us=0"));
        assert!(detail.contains("fold_triangle_us=0"));
        assert!(detail.contains("scrollbar_us=0"));
        assert!(detail.contains("image_hit_us=0"));
        assert!(detail.contains("close_arm_us=0"));
        assert!(detail.contains("code_copy_us=7"));
        assert!(detail.contains("pane_focus_us=0"));
        assert!(detail.contains("segment_hit_us=0"));
        assert!(detail.contains("buffer_position_us=0"));
        assert!(detail.contains("click_state_us=0"));
        assert!(detail.contains("caret_placement_us=0"));
        assert!(detail.contains("unattributed_us=3"));
        assert!(detail.contains("sum_us=10"));
    }
}
