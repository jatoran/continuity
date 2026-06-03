//! Global text-scale (zoom) command implementations.
//!
//! Zoom is a single global, durable multiplier sourced from
//! `[editor].text_scale`. It is *not* per-pane: a zoom in one window
//! applies to every pane in that window and — via the settings
//! write-back + registry config fan-out — to every other open window.
//!
//! Thread ownership: each window's UI thread is the sole writer of its
//! own `ViewState::font_size_scale`. The originating window applies the
//! new scale locally for instant feedback and writes `editor.text_scale`
//! back to `settings.toml`; the resulting watcher event is fanned to all
//! windows as `ConfigChanged`, and every *other* window re-projects the
//! scale through [`crate::Window::apply_settings`]. The originating
//! window swallows its own echo via `consume_writeback_echo`, so it does
//! not double-apply.
//!
//! δ.3 — the whole local apply is wrapped in `with_caret_line_anchored`
//! so the focused pane's caret line keeps its screen y across the zoom
//! reflow (char advance changes → wrap rows change → caret display-line
//! index shifts). Spectator panes have no tracked caret-y, so their
//! reflow is unanchored (acceptable).

use continuity_layout::{MAX_ZOOM, MIN_ZOOM};

use crate::{Error, Window};

impl Window {
    /// Adjust the global text scale by a multiplicative `factor` (e.g.
    /// `1.10` for one zoom-in notch at the default 10 % step). The new
    /// clamped scale is applied locally to every pane and persisted to
    /// `[editor].text_scale`, fanning out to every other window.
    pub(crate) fn view_adjust_zoom_impl(&mut self, factor: f32) -> Result<(), Error> {
        let new_scale = (self.view.font_size_scale * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.apply_global_text_scale(new_scale);
        Ok(())
    }

    /// Reset the global text scale to 1.0 and persist it.
    pub(crate) fn view_reset_zoom_impl(&mut self) -> Result<(), Error> {
        self.apply_global_text_scale(1.0);
        Ok(())
    }

    /// Apply a clamped global text scale to this window (focused mirror
    /// plus every entry in [`Window::panes`]), anchored on the focused
    /// caret line, then persist `[editor].text_scale` so the value
    /// survives relaunch and reaches every other window via the config
    /// fan-out. The settings write-back is idempotent, so a redundant
    /// scale (e.g. resetting when already at 1.0) touches neither disk
    /// nor the in-flight echo counter.
    ///
    /// Used by the zoom commands and the Ctrl+wheel zoom path so both
    /// routes funnel through one global, persisted mutator.
    pub(crate) fn apply_global_text_scale(&mut self, scale: f32) {
        let scale = scale.clamp(MIN_ZOOM, MAX_ZOOM);
        self.with_caret_line_anchored(|w| {
            w.view.font_size_scale = scale;
            for pane in w.panes.values_mut() {
                pane.view.font_size_scale = scale;
            }
            w.invalidate_font_state();
        });
        // Contract (C): commit the new global scale to settings.toml so
        // the zoom level survives relaunch and fans out to siblings.
        self.persist_float_or_log("editor", "text_scale", f64::from(scale));
    }
}
