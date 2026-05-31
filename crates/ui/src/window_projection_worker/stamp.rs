//! Per-paint projection stamp computation.
//!
//! [`PaintProjectionInputs`] is the snapshot of every input that feeds
//! both the worker-result acceptance check and the worker request;
//! [`current_projection_stamp`] derives the matching [`ProjectionStamp`]
//! from it. Pure — does not touch the worker.

use std::ops::Range;

use continuity_buffer::BufferId;
use continuity_display_map::{FoldRange, ImageRowReservation};
use continuity_layout::FontStateId;

use crate::projection_worker::ProjectionStamp;

/// Snapshot of all paint inputs needed to construct a
/// [`ProjectionStamp`] and a matching [`ProjectionRequest`]. Built
/// once per paint by the caller and threaded into both the
/// worker-result acceptance check and the post-paint dispatch.
pub(crate) struct PaintProjectionInputs<'a> {
    pub buffer_id: BufferId,
    pub rope_revision: u64,
    pub decoration_revision: Option<u64>,
    pub decoration_parse_revision: Option<u64>,
    pub caret_bytes: &'a [usize],
    pub folds: &'a [FoldRange],
    pub image_reservations: &'a [ImageRowReservation],
    pub wrap_width_dip: u32,
    pub font_state: FontStateId,
    pub viewport_rows: Range<u32>,
    pub overscan: u32,
}

/// Compute the [`ProjectionStamp`] for the current paint inputs.
/// Pure — does not touch the worker.
#[must_use]
pub(crate) fn current_projection_stamp(inputs: &PaintProjectionInputs<'_>) -> ProjectionStamp {
    ProjectionStamp {
        document: inputs.buffer_id.as_uuid().as_u128(),
        rope_revision: inputs.rope_revision,
        decoration_revision: inputs.decoration_revision,
        decoration_parse_revision: inputs.decoration_parse_revision,
        caret_signature: ProjectionStamp::caret_signature(inputs.caret_bytes),
        fold_signature: ProjectionStamp::fold_signature(inputs.folds),
        image_reservations_signature: ProjectionStamp::image_reservations_signature(
            inputs.image_reservations,
        ),
        wrap_width_dip: inputs.wrap_width_dip,
        font_state: inputs.font_state,
        viewport_rows: inputs.viewport_rows.clone(),
        overscan: inputs.overscan,
    }
}

#[cfg(test)]
pub(super) fn test_inputs<'a>(
    buffer_id: BufferId,
    rope_revision: u64,
    decoration_revision: Option<u64>,
    caret_bytes: &'a [usize],
    folds: &'a [FoldRange],
    image_reservations: &'a [ImageRowReservation],
    viewport_rows: Range<u32>,
) -> PaintProjectionInputs<'a> {
    PaintProjectionInputs {
        buffer_id,
        rope_revision,
        decoration_revision,
        decoration_parse_revision: decoration_revision,
        caret_bytes,
        folds,
        image_reservations,
        wrap_width_dip: 0,
        font_state: FontStateId::default(),
        viewport_rows,
        overscan: 20,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_is_pure_function_of_inputs() {
        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        let stamp_a = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..40,
        ));
        let stamp_b = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..40,
        ));
        assert_eq!(stamp_a, stamp_b);
    }

    #[test]
    fn stamp_differs_when_caret_moves() {
        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        let stamp_a = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..40,
        ));
        let stamp_b = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[42],
            &folds,
            &reservations,
            0..40,
        ));
        assert_ne!(stamp_a, stamp_b);
        assert_ne!(stamp_a.caret_signature, stamp_b.caret_signature);
    }

    #[test]
    fn stamp_differs_when_viewport_scrolls() {
        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        let stamp_a = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..40,
        ));
        let stamp_b = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            100..140,
        ));
        assert_ne!(stamp_a, stamp_b);
    }
}
