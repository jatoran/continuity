//! Tiny timing helper for `renderer_draw_main`.

use std::time::Instant;

pub(super) fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}
