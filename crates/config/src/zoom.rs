//! Shared text-scale (zoom) bounds.
//!
//! These bounds are the single config-side home for the global
//! `[editor].text_scale` multiplier validated in [`crate::validate`].
//! They must stay numerically identical to the runtime clamp range in
//! `continuity_layout::view_state` (`MIN_ZOOM` / `MAX_ZOOM`): config
//! validates a value into this range and layout clamps zoom into the
//! same range, so a value that passes validation must never be
//! re-clamped on apply. The two crates are siblings in the layer graph
//! (neither depends on the other), so the constants are duplicated here
//! rather than imported; keep them in sync.

/// Minimum allowed global text-scale multiplier.
///
/// Mirrors `continuity_layout::view_state::MIN_ZOOM`.
pub const MIN_ZOOM: f32 = 0.5;

/// Maximum allowed global text-scale multiplier.
///
/// Mirrors `continuity_layout::view_state::MAX_ZOOM`.
pub const MAX_ZOOM: f32 = 4.0;
