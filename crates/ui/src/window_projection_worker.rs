//! ε.5b — projection worker UI-thread integration.
//!
//! Sibling of `window_paint.rs`. Owns the lazy spawn, the per-paint
//! stamp computation, the worker-result acceptance helper, and the
//! request submission. Pure helpers (`current_projection_stamp`,
//! `try_use_worker_result`, `build_projection_request`) are unit
//! tested without needing a real `Window` / DirectWrite.
//!
//! **Thread ownership**: UI thread of one window. The worker thread
//! itself is owned by [`crate::projection_worker::ProjectionWorker`]
//! and joined on drop.

mod miss_reason;
mod request_build;
mod result_accept;
mod stamp;

pub(crate) use miss_reason::WorkerMissReason;
pub(crate) use request_build::build_projection_request;
pub(crate) use result_accept::{try_use_worker_result_rich, WorkerOutcome};
pub(crate) use stamp::{current_projection_stamp, PaintProjectionInputs};

use continuity_render::DEFAULT_HEADING_SCALE;

use crate::projection_worker::{ProjectionWorker, SendCom, WorkerFontMetrics};
use crate::window::Window;

impl Window {
    /// Lazy-spawn the projection worker on the first paint that has
    /// a live `text_format`. No-op after the worker is spawned.
    /// Called from the top of `Window::on_paint`.
    ///
    /// The worker carries no baked font state; each request supplies
    /// the live font via [`Window::projection_font_metrics`], so the
    /// worker survives font-family / font-size changes without a
    /// respawn (RC1 stale-font fix). It only needs the renderer's
    /// text format to *exist* (renderer ready) before spawning.
    pub(crate) fn ensure_projection_worker(&mut self) {
        if self.projection_worker.is_some() {
            return;
        }
        if self.text_format.is_none() {
            return;
        }
        let mode = ProjectionWorker::direct_write_mode(
            self.dwrite.raw().clone(),
            std::sync::Arc::clone(&self.walker_run_cache),
        );
        self.projection_worker = Some(ProjectionWorker::spawn_with_caches(
            mode,
            std::sync::Arc::clone(&self.walker_wrap_cache),
            std::sync::Arc::clone(&self.walker_segment_cache),
        ));
        crate::paint_trace::log_event("projection_worker_spawn", "");
    }

    /// Snapshot the current DirectWrite font metrics for a worker
    /// request. The worker holds no baked font state (RC1 fix), so
    /// every request carries the live family + size + heading scale;
    /// a font change is reflected on the very next build. A `None`
    /// format (no live text format yet) selects the fixed-width
    /// fallback measurer.
    pub(crate) fn projection_font_metrics(&self) -> WorkerFontMetrics {
        let format = self.text_format.as_ref().map(|format| {
            // SAFETY: IDWriteTextFormat is documented thread-safe (see SendCom).
            unsafe { SendCom::new(format.clone()) }
        });
        WorkerFontMetrics {
            format,
            font_size_dip: self.scaled_font_size(),
            heading_scale: DEFAULT_HEADING_SCALE,
        }
    }

    /// Allocate the next worker request sequence number.
    pub(crate) fn next_projection_request_seq(&mut self) -> u64 {
        self.projection_request_seq = self.projection_request_seq.saturating_add(1);
        self.projection_request_seq
    }
}
