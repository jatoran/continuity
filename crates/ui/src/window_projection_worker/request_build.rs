//! Worker request assembly.
//!
//! [`build_projection_request`] packages a per-paint stamp, the
//! rope/decoration/caret/fold snapshot, and the worker plan into a
//! [`ProjectionRequest`]. The rope is `Arc`-wrapped from a cheap
//! `Rope::clone` (ropey clones share internal nodes); the
//! caret/fold/reservation slices become `Arc<[…]>` so the worker can
//! move them between threads without re-allocating per submit.

use std::sync::Arc;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation};
use ropey::Rope;

use crate::pane_tree::PaneId;
use crate::projection_worker::{ProjectionPlan, ProjectionRequest, ProjectionStamp};

/// Assemble a [`ProjectionRequest`] from the paint inputs + stamp +
/// chosen plan.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_projection_request(
    seq: u64,
    target_pane: PaneId,
    stamp: ProjectionStamp,
    rope: &Rope,
    decorations: Option<Arc<Decorations>>,
    caret_bytes: &[usize],
    folds: &[FoldRange],
    image_reservations: &[ImageRowReservation],
    suppressed_table_blocks: &[std::ops::Range<usize>],
    fallback_char_width_dip: f32,
    plan: ProjectionPlan,
) -> ProjectionRequest {
    ProjectionRequest {
        seq,
        target_pane,
        stamp,
        rope: Arc::new(rope.clone()),
        decorations,
        caret_bytes: Arc::from(caret_bytes.to_vec()),
        folds: Arc::from(folds.to_vec()),
        image_reservations: Arc::from(image_reservations.to_vec()),
        suppressed_table_blocks: Arc::from(suppressed_table_blocks.to_vec()),
        fallback_char_width_dip,
        plan,
    }
}
