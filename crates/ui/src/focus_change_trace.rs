//! `event:buffer_focus_change` emission for pane focus transitions.
//!
//! Emits one trace line per UI focus switch with cache-hit predictions
//! for the destination buffer — feeds Block 0.5 of the memory
//! optimization plan ("correlate buffer-switch timing with cache hit /
//! miss to validate eviction is acceptable").
//!
//! The two cache-hit fields use the state *at the moment the focus
//! switch is initiated*, before any per-switch warmup runs.

use continuity_buffer::BufferId;

use crate::pane_tree::PaneId;
use crate::window::Window;

/// Emit `event:buffer_focus_change` with `from_buffer`, `to_buffer`,
/// `from_pane`, `to_pane`, and predicted cache-hit / tree-cache-hit
/// flags for the destination buffer.
pub(crate) fn emit_buffer_focus_change(
    window: &Window,
    from_buffer: BufferId,
    to_buffer: BufferId,
    from_pane: PaneId,
    to_pane: PaneId,
) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    let decoration_cache_hit = window
        .decoration_cache
        .get(to_buffer.as_uuid().as_u128())
        .is_some();
    let tree_cache_hit = window
        .decorate_pool
        .as_ref()
        .map(|pool| pool.has_cached_tree(to_buffer.as_uuid().as_u128()))
        .unwrap_or(false);
    let detail = format!(
        "from_buffer={} to_buffer={} from_pane={:?} to_pane={:?} \
         decoration_cache_hit={decoration_cache_hit} tree_cache_hit={tree_cache_hit}",
        from_buffer.as_uuid(),
        to_buffer.as_uuid(),
        from_pane,
        to_pane,
    );
    crate::paint_trace::log_event("buffer_focus_change", &detail);
}
