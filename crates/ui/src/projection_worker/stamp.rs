// ε.5 ships the worker foundation only; until the integration slice
// wires `Window::on_paint` to dispatch + validate worker results,
// these types read "never used".
#![allow(dead_code)]
//! Native font-state specialization of the portable display-map stamp.

use continuity_layout::FontStateId;

pub(crate) type ProjectionStamp = continuity_display_map::ProjectionStamp<FontStateId>;
pub(crate) use continuity_display_map::StampMismatchField;
