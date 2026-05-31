//! Deferred font-change swap: commit-without-overflow for `view.pick_font`,
//! `view.set_font_size`, and the corresponding `[editor]` settings.toml
//! reload branches.
//!
//! The problem: changing the active font triggers a wrap recomputation
//! against the new glyph metrics, but the display map rebuild runs on
//! the projection worker thread. Until the worker delivers, paint
//! draws *new glyphs* against *old wrap break points* — visually,
//! soft-wrapped lines overflow the right edge until the next worker
//! frame lands (~17 ms warm, up to ~480 ms cold on a 10k-line buffer
//! per `perf-snapshots/trace_20260528-113328.report.md`).
//!
//! The fix: split the commit into a *request* phase and a *swap*
//! phase. [`Window::request_font_change`] stores the desired family /
//! size in [`Window::pending_font_change`] and forces subsequent
//! projection stamps to use the *new* font_state — but leaves the
//! live `prose_font_family`/`font_size_dip_override` pair *and* the
//! text_format untouched. Paint keeps rendering the previous font
//! against the previous display map; no overflow. The worker rebuilds
//! in the background. When [`Window::try_apply_pending_font_swap`]
//! (called at the top of every paint via
//! `window_paint::on_paint`) sees that the worker has a result for
//! the pending font_state, it atomically swaps the live state inside
//! `with_caret_line_anchored` — caret screen-y is preserved, the same
//! paint then accepts the worker result, and the next frame renders
//! the new font against the matching display map.
//!
//! Preview (arrow-step in the picker overlay) is left on its
//! instant-swap path (`Window::set_font_family`) — it's exploratory
//! and 17 ms of typical-warm cache lag per row would make
//! preview-scroll unusable on large buffers. Commit (Enter,
//! ChooseFontW fallback, settings.toml save) is what routes through
//! here.
//!
//! Thread ownership: every entry point on [`Window`] is called from
//! the UI thread.

use std::time::{Duration, Instant};

use continuity_layout::FontStateId;

use crate::pane_tree::PaneId;
use crate::window::Window;

/// Watchdog window for the spectator-settle nudge loop. Two seconds
/// is the longest a real-world cold rebuild on a 10 k-line buffer
/// took in `perf-snapshots/trace_20260528-113328.report.md` (~480 ms),
/// padded ~4× to absorb pathological multi-pane / slow-disk cases.
/// If a pane is still lagging at the deadline, the loop gives up
/// rather than spin-paint forever — the next user interaction
/// (mouse, focus, scroll) will trigger the usual drain.
const FONT_SWAP_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);

/// Inert record of a pending font commit. Constructed by
/// [`Window::request_font_change`], read by stamp builders via
/// [`Window::effective_font_state`], and consumed by
/// [`Window::try_apply_pending_font_swap`] on delivery.
#[derive(Clone, Debug)]
pub(crate) struct PendingFontChange {
    /// Family the user committed to. Becomes the live
    /// `prose_font_family` at swap time.
    pub(crate) target_family: String,
    /// Size (DIPs) the user committed to. Becomes the live
    /// `font_size_dip_override` at swap time.
    pub(crate) target_size_dip: f32,
    /// [`FontStateId`] computed from `target_family` + `target_size_dip`
    /// + the current locale/DPI. Stamp builders use this in place of
    ///   `Window::font_state` while the pending change is in flight, so
    ///   the worker rebuilds for the new font.
    pub(crate) target_font_state: FontStateId,
}

impl Window {
    /// Stamp-side accessor: the font_state projection requests should
    /// use right now. Equals `self.font_state` outside of a pending
    /// change, and `pending.target_font_state` while one is in flight.
    /// Wire every `PaintProjectionInputs.font_state` call site through
    /// this helper.
    #[must_use]
    pub(crate) fn effective_font_state(&self) -> FontStateId {
        match self.pending_font_change.as_ref() {
            Some(pending) => pending.target_font_state,
            None => self.font_state,
        }
    }

    /// Request a deferred font change. `family` and `size_dip` are
    /// independent — pass `None` for the axis that should keep its
    /// current value. Idempotent against the active font_state: a
    /// request that resolves to the current font is dropped, so
    /// `request_font_change(Some("Consolas"), None)` after Consolas
    /// already landed is a no-op.
    ///
    /// On a real change the pending record is stored, the layout
    /// cache is invalidated for every font_state other than the
    /// target (so the new bucket can fill without the LRU evicting
    /// useful neighbours), and the HWND is invalidated so the next
    /// paint goes through [`Self::try_apply_pending_font_swap`].
    ///
    /// Does *not* mutate `prose_font_family`, `font_size_dip_override`,
    /// `font_state`, or `text_format` — those swap atomically when
    /// the worker delivers a matching display map.
    pub(crate) fn request_font_change(&mut self, family: Option<String>, size_dip: Option<f32>) {
        let target_family = family
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| self.prose_font_family.clone());
        let target_size_dip = size_dip.map(|s| s.clamp(6.0, 96.0)).unwrap_or_else(|| {
            self.font_size_dip_override
                .unwrap_or(crate::window::FONT_SIZE_DIP)
        });
        let target_font_state = FontStateId::from_parts(
            &target_family,
            target_size_dip * self.view.font_size_scale,
            crate::window::FONT_LOCALE,
            self.dpi_scale(),
        );
        // Idempotency: silently drop a request that resolves to the
        // already-active font_state. Caller is then free to invoke
        // this on every keystroke / settings reload without churning
        // the projection worker.
        if target_font_state == self.font_state
            && self
                .pending_font_change
                .as_ref()
                .is_none_or(|p| p.target_font_state == target_font_state)
        {
            return;
        }
        let next_pending = PendingFontChange {
            target_family,
            target_size_dip,
            target_font_state,
        };
        // Same target font_state as the previous pending? Keep the
        // older request — its in-flight worker stamp is the one that
        // will land. Replace only when the target itself changed.
        if let Some(existing) = self.pending_font_change.as_ref() {
            if existing.target_font_state == next_pending.target_font_state {
                return;
            }
        }
        self.cache.invalidate_other_font_states(target_font_state);
        self.pending_font_change = Some(next_pending);
        crate::window_helpers::invalidate_hwnd(self.hwnd);
    }

    /// Check the projection worker for a result built against the
    /// pending font_state. If one exists, atomically swap the live
    /// font state (family + size + font_state + text_format) inside
    /// `with_caret_line_anchored`, then clear the pending record.
    ///
    /// The peek does not consume — the regular per-paint
    /// `take_latest_result_for_target` call later in the same paint
    /// drains it normally, now hitting the freshly-swapped paint
    /// stamp.
    ///
    /// Returns `true` when a swap was performed. Callers don't need
    /// to react to the bool; the caret-anchor wrap already handles
    /// the y-preservation, and the HWND is already pumping a paint.
    pub(crate) fn try_apply_pending_font_swap(&mut self, focused_pane: PaneId) -> bool {
        let pending_target = match self.pending_font_change.as_ref() {
            Some(pending) => pending.target_font_state,
            None => return false,
        };
        let Some(worker) = self.projection_worker.as_ref() else {
            return false;
        };
        let result_font_state = match worker.peek_latest_result_font_state_for_target(focused_pane)
        {
            Some(font_state) => font_state,
            None => return false,
        };
        if result_font_state != pending_target {
            return false;
        }
        // The match is firm — perform the swap. `with_caret_line_anchored`
        // captures the pre-mutation caret line's screen-y so a wrap-on
        // delta across a large family/size change doesn't drift the
        // user's eye. Pre-mutation capture is mandatory: wrapping
        // inside `invalidate_font_state` would capture post-swap state
        // and be a no-op.
        self.with_caret_line_anchored(|w| {
            let pending = w.pending_font_change.take().expect("pending checked above");
            w.prose_font_family = pending.target_family;
            w.font_size_dip_override = Some(pending.target_size_dip);
            // `invalidate_font_state` re-reads `prose_font_family` /
            // size, recomputes the FontStateId, updates the layout
            // cache invalidation set, and drops `text_format` so the
            // next `ensure_renderer` call rebuilds it against the
            // new family. After this call, `w.font_state` lags by one
            // `ensure_renderer` — that's already how the rest of the
            // file picks up `set_font_family` mutations.
            w.invalidate_font_state();
        });
        // Arm the spectator-settle watchdog. The focused pane is now
        // on the new font_state, but spectator panes have their own
        // worker results still in flight; without further nudges, the
        // message pump may go quiet before
        // `drain_spectator_projection_worker_results` ever sees those
        // results. See [`Self::nudge_font_swap_settle`].
        self.font_swap_settle_deadline = Some(Instant::now() + FONT_SWAP_SETTLE_TIMEOUT);
        true
    }

    /// Spectator settle nudge — called at the end of every `on_paint`.
    ///
    /// While [`Window::font_swap_settle_deadline`] is `Some` and
    /// unexpired, this checks whether any spectator's
    /// [`crate::window_spectator_cache::SpectatorFrameCache`] entry
    /// still carries a `font_state` other than `self.font_state`.
    /// If so, schedules another paint via `invalidate_hwnd` so the
    /// next `drain_spectator_projection_worker_results` can pick up
    /// any worker results that have landed since the focused-pane
    /// swap. The existing `drain → populated → invalidate_with_reason`
    /// chain takes over once the first spectator result drains.
    ///
    /// Clears the deadline when (a) every spectator cache entry
    /// matches `self.font_state`, or (b) the deadline expires — the
    /// latter caps any infinite-spin risk in case a spectator pane
    /// is somehow stuck (worker hung, ungrowable cache, …). The
    /// next user interaction will fire the normal drain regardless.
    pub(crate) fn nudge_font_swap_settle(&mut self) {
        let Some(deadline) = self.font_swap_settle_deadline else {
            return;
        };
        if Instant::now() >= deadline {
            self.font_swap_settle_deadline = None;
            return;
        }
        let current = self.font_state;
        let any_lagging = self
            .spectator_frame_cache
            .borrow()
            .any_entry_lags_font_state(current);
        if !any_lagging {
            self.font_swap_settle_deadline = None;
            return;
        }
        crate::window_helpers::invalidate_hwnd(self.hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_font_change_carries_target_triple() {
        let pending = PendingFontChange {
            target_family: "Consolas".into(),
            target_size_dip: 14.0,
            target_font_state: FontStateId::from_parts("Consolas", 14.0, "en-us", 1.0),
        };
        assert_eq!(pending.target_family, "Consolas");
        assert!((pending.target_size_dip - 14.0).abs() < f32::EPSILON);
        assert_ne!(pending.target_font_state, FontStateId::default());
    }
}
