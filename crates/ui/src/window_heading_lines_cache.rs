//! Heading-line cache entry used by display projection inputs.
//!
//! Thread ownership: UI thread through [`crate::Window`].

use continuity_buffer::BufferId;

/// Cache entry backing [`crate::Window::heading_lines_cache`].
///
/// Key is `(buffer, decoration_revision, rope_line_count)`. The
/// rope's *revision* deliberately is not in the key: typing inside a
/// heading line moves the rope, but neither the heading line number set
/// nor the decoration tree changes between two decoration revisions.
/// Keying on `rope_line_count` catches newline insert/delete changes
/// while letting a typing burst between decoration updates hit the
/// cache. Combined with the `byte_to_line`-via-ropey rewrite of
/// `Window::heading_lines_for_projection`, a miss is O(num_headings log N)
/// rather than O(N * num_headings).
#[derive(Clone)]
pub(crate) struct HeadingLinesCacheEntry {
    pub(crate) buffer: BufferId,
    pub(crate) rope_line_count: usize,
    pub(crate) decoration_revision: Option<u64>,
    pub(crate) headings: Vec<(u32, u8)>,
}
