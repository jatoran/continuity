//! Offset-independent hashing of the decorations overlapping one source
//! line, for the segment-cache content stamp.
//!
//! Every byte offset embedded in a decoration is hashed **relative to the
//! line's `origin`** (its start byte) so a byte-identical line keeps the
//! same stamp after an edit above shifts it down — which is exactly what
//! lets [`crate::segment_cache`]'s `get_shifted` relocate the cached
//! segments by a delta instead of recomputing them. Hashing any *absolute*
//! offset here silently defeats that reuse: it was the cause of a ~0%
//! segment-cache hit rate under editing (the stamp changed for every line
//! below an edit even though the projected segments were byte-identical).
//!
//! The `match` on [`InlineKind`] is intentionally exhaustive: a newly added
//! decoration variant becomes a compile error here rather than a silent
//! mis-key.
//!
//! Thread ownership: pure, callable from any thread.

use std::hash::{Hash, Hasher};
use std::mem::discriminant;

use continuity_decorate::{BlockSpan, EvaluatedTable, InlineColorSpan, InlineKind, InlineSpan};

/// Hash `byte` as an offset relative to `origin`, saturating at 0 for
/// offsets that precede the line start.
fn hash_relative(byte: usize, origin: usize, hasher: &mut impl Hasher) {
    byte.saturating_sub(origin).hash(hasher);
}

/// Hash a block span's identity relative to `origin`.
pub(super) fn hash_block_relative(block: &BlockSpan, origin: usize, hasher: &mut impl Hasher) {
    block.kind.hash(hasher);
    hash_relative(block.start_byte, origin, hasher);
    hash_relative(block.end_byte, origin, hasher);
}

/// Hash an inline span's identity relative to `origin`. The discriminant
/// fixes the variant; every embedded absolute byte range is relativized.
pub(super) fn hash_inline_relative(inline: &InlineSpan, origin: usize, hasher: &mut impl Hasher) {
    hash_relative(inline.range.start, origin, hasher);
    hash_relative(inline.range.end, origin, hasher);
    discriminant(&inline.kind).hash(hasher);
    match &inline.kind {
        InlineKind::Link {
            text_range,
            url_range,
        } => {
            hash_relative(text_range.start, origin, hasher);
            hash_relative(text_range.end, origin, hasher);
            hash_relative(url_range.start, origin, hasher);
            hash_relative(url_range.end, origin, hasher);
        }
        InlineKind::ImageRef {
            alt_range,
            url_range,
        } => {
            hash_relative(alt_range.start, origin, hasher);
            hash_relative(alt_range.end, origin, hasher);
            hash_relative(url_range.start, origin, hasher);
            hash_relative(url_range.end, origin, hasher);
        }
        InlineKind::FootnoteReference { label } => label.hash(hasher),
        InlineKind::FootnoteDefinition { label, body_range } => {
            label.hash(hasher);
            hash_relative(body_range.start, origin, hasher);
            hash_relative(body_range.end, origin, hasher);
        }
        InlineKind::Checkbox {
            checked,
            toggle_byte,
        } => {
            checked.hash(hasher);
            hash_relative(*toggle_byte, origin, hasher);
        }
        InlineKind::Marker(marker) => marker.hash(hasher),
        InlineKind::Strong
        | InlineKind::Emphasis
        | InlineKind::Strikethrough
        | InlineKind::Code => {}
    }
}

/// Hash an inline-color span relative to `origin`. The color `kind` is
/// content; the delimiter / text ranges are relativized.
pub(super) fn hash_color_relative(
    color: &InlineColorSpan,
    origin: usize,
    hasher: &mut impl Hasher,
) {
    color.kind.hash(hasher);
    hash_relative(color.outer.start, origin, hasher);
    hash_relative(color.outer.end, origin, hasher);
    hash_relative(color.inner.start, origin, hasher);
    hash_relative(color.inner.end, origin, hasher);
}

/// Hash an evaluated table relative to `origin`. The substitute display
/// text and cell coordinates are content (hashed verbatim); byte ranges
/// are relativized. Excludes table-suppression state, which the caller
/// hashes separately as an offset-free boolean.
pub(super) fn hash_table_relative(table: &EvaluatedTable, origin: usize, hasher: &mut impl Hasher) {
    hash_relative(table.block_range.start, origin, hasher);
    hash_relative(table.block_range.end, origin, hasher);
    for cell_override in &table.overrides {
        cell_override.cell.col.hash(hasher);
        cell_override.cell.row.hash(hasher);
        hash_relative(cell_override.cell_range.start, origin, hasher);
        hash_relative(cell_override.cell_range.end, origin, hasher);
        cell_override.display.hash(hasher);
    }
}
