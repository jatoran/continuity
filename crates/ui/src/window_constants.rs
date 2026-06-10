pub(crate) const FONT_FAMILY: &str = "Cascadia Mono";
pub(crate) const FONT_LOCALE: &str = "en-us";
pub(crate) const FONT_SIZE_DIP: f32 = 14.0;
pub(crate) const LINE_HEIGHT_DIP: f32 = 20.0;
/// Functional bottom inset used by caret/doc-end reveals so the final
/// display row is not painted exactly on the viewport clip edge.
pub(crate) const END_OF_BUFFER_BOTTOM_PADDING_DIP: f32 = LINE_HEIGHT_DIP;
/// Default soft-cap for cached layouts. ~10x a typical 50-line viewport per
/// spec section 5.
pub(crate) const LAYOUT_CACHE_CAPACITY: usize = 512;
