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

use crate::projection_worker::ProjectionWorker;
use crate::window::Window;

impl Window {
    /// Lazy-spawn the projection worker on the first paint that has
    /// a live `text_format`. No-op after the worker is spawned.
    /// Called from the top of `Window::on_paint`.
    pub(crate) fn ensure_projection_worker(&mut self) {
        if self.projection_worker.is_some() {
            return;
        }
        let Some(format) = self.text_format.as_ref() else {
            return;
        };
        let mode = ProjectionWorker::direct_write_mode(
            self.dwrite.raw().clone(),
            format.clone(),
            self.scaled_font_size(),
            DEFAULT_HEADING_SCALE,
            std::sync::Arc::clone(&self.walker_run_cache),
        );
        self.projection_worker = Some(ProjectionWorker::spawn_with_caches(
            mode,
            std::sync::Arc::clone(&self.walker_wrap_cache),
            std::sync::Arc::clone(&self.walker_segment_cache),
        ));
        crate::paint_trace::log_event("projection_worker_spawn", "");
    }

    /// Allocate the next worker request sequence number.
    pub(crate) fn next_projection_request_seq(&mut self) -> u64 {
        self.projection_request_seq = self.projection_request_seq.saturating_add(1);
        self.projection_request_seq
    }
}
