//! Idle-tick trigger detection for the display-map prewarm subsystem.
//!
//! Owns the UI-thread timer, idle-detection predicates, and MRU-
//! adjacent buffer selection. Decides *when* prewarm runs; the
//! per-stage frame build runs in [`super::stage`].
//!
//! Same single-writer thread invariants as the parent module: every
//! method here executes on the [`crate::Window`]-owning UI thread.

use std::time::{Duration, Instant};

use continuity_buffer::BufferId;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::GetUpdateRect;
use windows::Win32::System::SystemInformation::GetTickCount64;

use crate::display_prewarm_cache::PREWARM_TARGET_BUFFERS;
use crate::window::Window;

const PREWARM_IDLE_GRACE_MS: u64 = 120;
const PREWARM_TICK_BUDGET: Duration = Duration::from_millis(2);
/// Buffers at or above this source-line count skip MRU prewarm —
/// each cold projection is too expensive to fit in a UI-thread
/// idle tick. The paint-time reuse cache covers same-buffer
/// steady-state caret motion; the only loss is a one-time cold
/// build on Ctrl+Tab to a never-prewarmed large buffer.
const PREWARM_BIG_BUFFER_LINE_CAP: usize = 2_000;

impl Window {
    /// Start the idle prewarm timer.
    pub(crate) fn start_display_prewarm_timer(&mut self, hwnd: HWND) {
        if self.display_prewarm_timer_active {
            return;
        }
        unsafe {
            let armed = windows::Win32::UI::WindowsAndMessaging::SetTimer(
                Some(hwnd),
                crate::window_timers::DISPLAY_PREWARM_TIMER_ID,
                crate::window_timers::DISPLAY_PREWARM_TIMER_MS,
                None,
            );
            self.display_prewarm_timer_active = armed != 0;
        }
    }

    /// Timer entry point. Does at most one stage of display-map work per
    /// idle tick so a user event waiting behind the timer is never starved.
    pub(crate) fn on_display_prewarm_tick(&mut self, hwnd: HWND) {
        let _scope = crate::paint_trace::EventScope::new("on_display_prewarm_tick");
        if !self.is_idle_for_display_prewarm(hwnd) {
            return;
        }
        // Activation-grace gate: refusing to prewarm for the first
        // second after the user came back to the window is what
        // keeps focus-return responsive at 6000 lines. Without this
        // the timer fires inside the grace, the budget check below
        // is checked *before* the work (no-op), and one
        // `process_one_display_prewarm_stage` cold-builds a full
        // 6000-line `FrameDisplay` on the UI thread.
        if self.in_activation_grace() {
            crate::paint_trace::log_event("on_display_prewarm_tick", "skipped=activation_grace");
            return;
        }
        // Spell recheck deferred from the paint path. UI-thread still
        // blocks while `ISpellChecker` runs, but the idle gate keeps
        // it out of the focus-return / first-paint window — a future
        // worker-thread variant removes the residual block entirely.
        {
            let _spell_scope = crate::paint_trace::EventScope::new("tick_spell_recheck");
            self.tick_spell_recheck();
        }
        let targets = self.focused_mru_adjacent_buffers();
        if targets.is_empty() {
            return;
        }
        // Skip MRU prewarm for very large buffers. Each stage cold-
        // builds a full `FrameDisplay` on the UI thread; for ~6000
        // line buffers that is 10–20 ms per stage × 3 stages × 2
        // targets. Idle ticks fire at the prewarm cadence, so the
        // total UI-thread block can stretch into the hundreds of
        // milliseconds and starve input. Above the threshold we
        // accept a cold cold-build on the eventual Ctrl+Tab; the
        // paint-time reuse cache covers the same-buffer steady
        // state.
        let big_buffer_targets: Vec<BufferId> = targets
            .iter()
            .copied()
            .filter(|id| !self.buffer_exceeds_prewarm_cap(*id))
            .collect();
        if big_buffer_targets.is_empty() {
            crate::paint_trace::log_event(
                "on_display_prewarm_tick",
                "skipped=all_targets_over_cap",
            );
            return;
        }
        self.display_map_prewarm
            .refresh_targets(&big_buffer_targets);
        // The previous budget check was a no-op (`start.elapsed()`
        // measured zero before the work). Drop it; the per-stage
        // cap is "one stage per tick" by construction.
        let _ = Instant::now();
        let _ = PREWARM_TICK_BUDGET;
        self.process_one_display_prewarm_stage();
    }

    /// True when `buffer_id` should not be MRU-prewarmed on the UI
    /// thread (`PREWARM_BIG_BUFFER_LINE_CAP` source lines or larger).
    /// Treats a missing snapshot as "small" so prewarm still fires
    /// against just-opened buffers; the next tick re-checks.
    #[must_use]
    pub(crate) fn buffer_exceeds_prewarm_cap(&self, buffer_id: BufferId) -> bool {
        match self.editor.snapshot(buffer_id) {
            Some(snap) => snap.rope_snapshot().rope().len_lines() >= PREWARM_BIG_BUFFER_LINE_CAP,
            None => false,
        }
    }

    fn is_idle_for_display_prewarm(&self, hwnd: HWND) -> bool {
        if self.is_window_minimized || self.scroll_anim_active || self.state_save_pending {
            return false;
        }
        if self
            .persist_client
            .as_ref()
            .is_some_and(|client| client.unflushed_bytes() > 0)
        {
            return false;
        }
        let now = unsafe { GetTickCount64() };
        if self.last_input_tick != 0
            && now.saturating_sub(self.last_input_tick) < PREWARM_IDLE_GRACE_MS
        {
            return false;
        }
        !has_pending_update_region(hwnd)
    }

    fn focused_mru_adjacent_buffers(&self) -> Vec<BufferId> {
        let Some(group) = self.tree.groups.get(&self.tree.focused) else {
            return Vec::new();
        };
        let active = group.active;
        let mut out = Vec::with_capacity(PREWARM_TARGET_BUFFERS);
        for tab_id in &group.mru {
            if *tab_id == active {
                continue;
            }
            let Some(tab) = self.tree.tabs.get(tab_id) else {
                continue;
            };
            if out.contains(&tab.buffer_id) {
                continue;
            }
            out.push(tab.buffer_id);
            if out.len() >= PREWARM_TARGET_BUFFERS {
                break;
            }
        }
        out
    }

    pub(super) fn is_focused_mru_target(&self, buffer_id: BufferId) -> bool {
        self.focused_mru_adjacent_buffers()
            .into_iter()
            .any(|candidate| candidate == buffer_id)
    }
}

fn has_pending_update_region(hwnd: HWND) -> bool {
    let mut rect = RECT::default();
    unsafe { GetUpdateRect(hwnd, Some(&mut rect), false).as_bool() }
}
