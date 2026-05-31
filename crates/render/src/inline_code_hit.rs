//! Per-frame hit rect for one painted inline-code span. The renderer
//! pushes one entry per visible `` `code` `` span during the body
//! paint; the UI reads the slice on `WM_MOUSEMOVE` to drive the
//! inline copy-button hover affordance.

/// One inline-code span the renderer painted on the current frame.
///
/// `rect_client` is in client DIPs (the window's coordinate space),
/// already translated through `body_origin`, so the UI mouse handler
/// can compare cursor coordinates without re-deriving the body
/// translate.
///
/// `inner_*_byte` excludes the backtick delimiters and matches the
/// content that should land on the clipboard when the user clicks
/// the inline copy button. `outer_*_byte` includes the delimiters
/// and matches the `InlineSpan::range` the decoration cache exposes.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineCodeHit {
    /// Absolute byte offset of the outer `` `code` `` run (delimiters
    /// included).
    pub outer_start_byte: usize,
    /// Exclusive end byte of the outer run.
    pub outer_end_byte: usize,
    /// Absolute byte offset of the inner content (no backticks).
    pub inner_start_byte: usize,
    /// Exclusive end byte of the inner content.
    pub inner_end_byte: usize,
    /// Painted rect in client DIPs `(x, y, width, height)`.
    pub rect_client: (f32, f32, f32, f32),
}
