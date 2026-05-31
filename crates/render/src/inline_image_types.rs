//! Plain data shared by inline-image placement and hit testing.
//!
//! The UI builds placements from decoration spans, the renderer paints them
//! and records hit rects for the next mouse event.

/// Phase F5 Pass 2: one inline-image placement built per frame by
/// the UI. The painter resolves [`Self::path`] against the renderer's
/// `ImageCache`, lays out using
/// [`crate::image_layout::compute_image_layout`], and `DrawBitmap`s
/// the result.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineImagePlacement {
    /// Absolute filesystem path to the image bytes. Resolved by the
    /// UI against `[markdown].images_dir` for the
    /// `is_shared_store_reference` case; passed verbatim for other
    /// `file://` references.
    pub path: std::path::PathBuf,
    /// Pre-parsed alt-pipe attributes (alt text + optional width
    /// hint). The painter consumes `width` via
    /// [`crate::image_layout::compute_image_layout`].
    pub attrs: continuity_decorate::image_link::ImageLinkAttrs,
    /// First *display* line index (post-fold, post-soft-wrap) the
    /// image reference occupies.
    pub display_line: u32,
    /// F5 redesign: when `false` (default), paint the collapsed
    /// affordance; when `true`, paint the full bitmap at fit-pane-width.
    pub is_expanded: bool,
    /// F5 redesign: the URL exactly as it appears in the rope.
    pub url: String,
    /// Source byte offset of the URL the placement was built from.
    pub source_byte: usize,
}

/// F5 redesign — hit-test rect for the collapsed affordance. The
/// renderer fills this for every collapsed placement on each frame
/// so the UI's mouse handler can detect clicks on the thumbnail or
/// chevron without re-deriving the layout.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineImageHit {
    /// Source byte offset matching [`InlineImagePlacement::source_byte`].
    pub source_byte: usize,
    /// Full affordance rect (thumb + label + chevron) in pane-body
    /// coordinates: `(x, y, width, height)`.
    pub rect: (f32, f32, f32, f32),
}
