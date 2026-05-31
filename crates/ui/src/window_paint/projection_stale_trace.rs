//! Trace formatting for rejected projection-worker results.

use crate::projection_worker::{ProjectionStamp, StampMismatchField};
use crate::window_projection_worker::WorkerOutcome;

/// Emit the stale-result breadcrumb before the outcome is converted to
/// the legacy worker-miss shape.
pub(crate) fn log_projection_worker_stale_result(
    outcome: &WorkerOutcome,
    current_projection_stamp: &ProjectionStamp,
) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    let (field, stale) = match outcome {
        WorkerOutcome::StaleResult { field, stale } => (*field, stale),
        WorkerOutcome::CrossBufferResult { stale } => (StampMismatchField::Document, stale),
        WorkerOutcome::Hit(_) | WorkerOutcome::NoResult(_) => return,
    };
    crate::paint_trace::log_event(
        "event:projection_worker_stale_result",
        &format!(
            "seq={} field={} build_dur_us={} coalesced_dropped={} \
             stale_rope_rev={} paint_rope_rev={} stale_decoration_rev={:?} \
             paint_decoration_rev={:?} result_buffer_id={} paint_buffer_id={}",
            stale.seq,
            field.as_str(),
            stale.build_dur_us,
            stale.coalesced_dropped,
            stale.stamp.rope_revision,
            current_projection_stamp.rope_revision,
            stale.stamp.decoration_revision,
            current_projection_stamp.decoration_revision,
            uuid::Uuid::from_u128(stale.stamp.document),
            uuid::Uuid::from_u128(current_projection_stamp.document),
        ),
    );
}
