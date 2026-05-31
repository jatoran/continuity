//! Caret-style to renderer-shape mapping for paint view options.

use continuity_render::CaretShape;

use crate::window_view_options::CaretStyle;

#[must_use]
pub(crate) fn caret_shape_for(style: CaretStyle) -> CaretShape {
    match style {
        CaretStyle::Bar => CaretShape::Bar,
        CaretStyle::Block => CaretShape::Block,
        CaretStyle::Underline => CaretShape::Underline,
    }
}
