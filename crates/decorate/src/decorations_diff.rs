//! ε.3D — `Decorations::diff_dirty_lines` and supporting helpers.
//!
//! Computes the source-line indices whose decoration-driven styling
//! differs between two `Decorations` snapshots. The caller drives the
//! result into `DisplayMapBuilder::rebuild_dirty`'s dirty list so a
//! decoration revision bump only re-realizes the source lines whose
//! display content genuinely changed — not the whole viewport.
//!
//! ## Transform-then-diff contract
//!
//! The diff must compare the **transformed** previous snapshot
//! against the new one. ε.0's `Decorations::transformed_through` walks
//! `Vec<RopeEditDelta>` and shifts every span into the new rope's
//! byte coordinates (dropping spans that intersect a removed range).
//! Diffing the raw cached snapshot against the new one would see
//! every span's byte range as "moved" after even a 1-byte insert and
//! mark the whole document dirty — defeating the point.
//!
//! Callers therefore:
//! 1. Fetch the previously cached `Decorations` for this buffer.
//! 2. Call `prev.transformed_through(deltas, new_revision)` to align
//!    its byte coordinates with the new rope.
//! 3. Call `diff_dirty_lines(&transformed_prev, &new, rope_after)` to
//!    get the dirty source-line list.

use std::cmp::Ordering;

use ropey::Rope;

use crate::decorations::Decorations;
use crate::inline::{ByteRange, InlineSpan};
use crate::spans::BlockSpan;

impl Decorations {
    /// Dirty source lines between two `Decorations` snapshots —
    /// every source line whose styling spans changed between
    /// `prev` (already transformed through the rope deltas) and
    /// `self` (the new snapshot, computed against `rope`).
    ///
    /// Output is sorted, deduplicated, and never includes a source
    /// line whose spans are byte-for-byte identical in both
    /// snapshots. Walks the four span lists (`blocks`, `inlines`,
    /// `inline_color_spans`, `evaluated_tables`) in parallel: each
    /// list is already in document order, so a linear merge yields
    /// the symmetric difference in `O(n + m)`.
    ///
    /// Two snapshots with identical span contents (apart from
    /// `revision`) produce an empty dirty list — the typical case
    /// when the worker re-parses a buffer that hasn't actually
    /// changed.
    #[must_use]
    pub fn diff_dirty_lines(&self, prev: &Decorations, rope: &Rope) -> Vec<u32> {
        let mut dirty: Vec<u32> = Vec::new();
        diff_blocks(&prev.blocks, &self.blocks, rope, &mut dirty);
        diff_inlines(&prev.inlines, &self.inlines, rope, &mut dirty);
        diff_inline_colors(
            &prev.inline_color_spans,
            &self.inline_color_spans,
            rope,
            &mut dirty,
        );
        diff_evaluated_tables(
            &prev.evaluated_tables,
            &self.evaluated_tables,
            rope,
            &mut dirty,
        );
        dirty.sort_unstable();
        dirty.dedup();
        debug_assert!(
            dirty.windows(2).all(|w| w[0] < w[1]),
            "diff_dirty_lines must return sorted, deduplicated entries",
        );
        dirty
    }
}

fn diff_blocks(prev: &[BlockSpan], new: &[BlockSpan], rope: &Rope, dirty: &mut Vec<u32>) {
    let mut p = 0;
    let mut n = 0;
    while p < prev.len() || n < new.len() {
        match (prev.get(p), new.get(n)) {
            (Some(pb), Some(nb)) if pb == nb => {
                p += 1;
                n += 1;
            }
            (Some(pb), Some(nb)) => match pb.start_byte.cmp(&nb.start_byte) {
                Ordering::Less => {
                    push_lines_for_range(rope, pb.start_byte, pb.end_byte, dirty);
                    p += 1;
                }
                Ordering::Greater => {
                    push_lines_for_range(rope, nb.start_byte, nb.end_byte, dirty);
                    n += 1;
                }
                Ordering::Equal => {
                    push_lines_for_range(rope, pb.start_byte, pb.end_byte, dirty);
                    push_lines_for_range(rope, nb.start_byte, nb.end_byte, dirty);
                    p += 1;
                    n += 1;
                }
            },
            (Some(pb), None) => {
                push_lines_for_range(rope, pb.start_byte, pb.end_byte, dirty);
                p += 1;
            }
            (None, Some(nb)) => {
                push_lines_for_range(rope, nb.start_byte, nb.end_byte, dirty);
                n += 1;
            }
            (None, None) => break,
        }
    }
}

fn diff_inlines(prev: &[InlineSpan], new: &[InlineSpan], rope: &Rope, dirty: &mut Vec<u32>) {
    let mut p = 0;
    let mut n = 0;
    while p < prev.len() || n < new.len() {
        match (prev.get(p), new.get(n)) {
            (Some(pi), Some(ni)) if pi == ni => {
                p += 1;
                n += 1;
            }
            (Some(pi), Some(ni)) => match pi.range.start.cmp(&ni.range.start) {
                Ordering::Less => {
                    push_lines_for_byte_range(rope, &pi.range, dirty);
                    p += 1;
                }
                Ordering::Greater => {
                    push_lines_for_byte_range(rope, &ni.range, dirty);
                    n += 1;
                }
                Ordering::Equal => {
                    push_lines_for_byte_range(rope, &pi.range, dirty);
                    push_lines_for_byte_range(rope, &ni.range, dirty);
                    p += 1;
                    n += 1;
                }
            },
            (Some(pi), None) => {
                push_lines_for_byte_range(rope, &pi.range, dirty);
                p += 1;
            }
            (None, Some(ni)) => {
                push_lines_for_byte_range(rope, &ni.range, dirty);
                n += 1;
            }
            (None, None) => break,
        }
    }
}

fn diff_inline_colors(
    prev: &[crate::inline_color::InlineColorSpan],
    new: &[crate::inline_color::InlineColorSpan],
    rope: &Rope,
    dirty: &mut Vec<u32>,
) {
    let mut p = 0;
    let mut n = 0;
    while p < prev.len() || n < new.len() {
        match (prev.get(p), new.get(n)) {
            (Some(pi), Some(ni)) if pi == ni => {
                p += 1;
                n += 1;
            }
            (Some(pi), Some(ni)) => match pi.outer.start.cmp(&ni.outer.start) {
                Ordering::Less => {
                    push_lines_for_range(rope, pi.outer.start, pi.outer.end, dirty);
                    p += 1;
                }
                Ordering::Greater => {
                    push_lines_for_range(rope, ni.outer.start, ni.outer.end, dirty);
                    n += 1;
                }
                Ordering::Equal => {
                    push_lines_for_range(rope, pi.outer.start, pi.outer.end, dirty);
                    push_lines_for_range(rope, ni.outer.start, ni.outer.end, dirty);
                    p += 1;
                    n += 1;
                }
            },
            (Some(pi), None) => {
                push_lines_for_range(rope, pi.outer.start, pi.outer.end, dirty);
                p += 1;
            }
            (None, Some(ni)) => {
                push_lines_for_range(rope, ni.outer.start, ni.outer.end, dirty);
                n += 1;
            }
            (None, None) => break,
        }
    }
}

fn diff_evaluated_tables(
    prev: &[crate::table_eval::EvaluatedTable],
    new: &[crate::table_eval::EvaluatedTable],
    rope: &Rope,
    dirty: &mut Vec<u32>,
) {
    let mut p = 0;
    let mut n = 0;
    while p < prev.len() || n < new.len() {
        match (prev.get(p), new.get(n)) {
            (Some(pt), Some(nt)) if pt == nt => {
                p += 1;
                n += 1;
            }
            (Some(pt), Some(nt)) => match pt.block_range.start.cmp(&nt.block_range.start) {
                Ordering::Less => {
                    push_lines_for_range(rope, pt.block_range.start, pt.block_range.end, dirty);
                    p += 1;
                }
                Ordering::Greater => {
                    push_lines_for_range(rope, nt.block_range.start, nt.block_range.end, dirty);
                    n += 1;
                }
                Ordering::Equal => {
                    push_lines_for_range(rope, pt.block_range.start, pt.block_range.end, dirty);
                    push_lines_for_range(rope, nt.block_range.start, nt.block_range.end, dirty);
                    p += 1;
                    n += 1;
                }
            },
            (Some(pt), None) => {
                push_lines_for_range(rope, pt.block_range.start, pt.block_range.end, dirty);
                p += 1;
            }
            (None, Some(nt)) => {
                push_lines_for_range(rope, nt.block_range.start, nt.block_range.end, dirty);
                n += 1;
            }
            (None, None) => break,
        }
    }
}

fn push_lines_for_range(rope: &Rope, start_byte: usize, end_byte: usize, dirty: &mut Vec<u32>) {
    let len = rope.len_bytes();
    let s = start_byte.min(len);
    let e = end_byte.min(len);
    if e == 0 {
        return;
    }
    let line_lo = rope.byte_to_line(s) as u32;
    let line_hi = rope.byte_to_line(e.saturating_sub(1)) as u32;
    for line in line_lo..=line_hi {
        dirty.push(line);
    }
}

fn push_lines_for_byte_range(rope: &Rope, range: &ByteRange, dirty: &mut Vec<u32>) {
    push_lines_for_range(rope, range.start, range.end, dirty);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inline::{ByteRange, InlineKind, MarkerKind};
    use crate::spans::{BlockKind, BlockSpan};

    fn rope_for(text: &str) -> Rope {
        Rope::from_str(text)
    }

    #[test]
    fn identical_snapshots_produce_no_dirty_lines() {
        let mut a = Decorations::empty(1);
        a.blocks.push(BlockSpan {
            kind: BlockKind::Paragraph,
            start_byte: 0,
            end_byte: 5,
        });
        let b = a.clone();
        let rope = rope_for("hello");
        assert!(b.diff_dirty_lines(&a, &rope).is_empty());
    }

    #[test]
    fn added_block_marks_its_source_lines() {
        let prev = Decorations::empty(1);
        let mut new = Decorations::empty(2);
        new.blocks.push(BlockSpan {
            kind: BlockKind::Paragraph,
            start_byte: 0,
            end_byte: 5,
        });
        let rope = rope_for("hello\nworld");
        let dirty = new.diff_dirty_lines(&prev, &rope);
        assert_eq!(dirty, vec![0]);
    }

    #[test]
    fn removed_inline_marks_its_source_line() {
        let mut prev = Decorations::empty(1);
        prev.inlines.push(InlineSpan {
            kind: InlineKind::Strong,
            range: ByteRange::new(6, 10),
        });
        let new = Decorations::empty(2);
        let rope = rope_for("hello\nworld\nbye");
        let dirty = new.diff_dirty_lines(&prev, &rope);
        assert_eq!(dirty, vec![1]);
    }

    #[test]
    fn discontiguous_changes_yield_sorted_unique_lines() {
        // prev has a block on line 0 and an inline on line 3; new
        // moves the inline to line 5 and adds a block on line 2.
        let mut prev = Decorations::empty(1);
        prev.blocks.push(BlockSpan {
            kind: BlockKind::Paragraph,
            start_byte: 0,
            end_byte: 5,
        });
        prev.inlines.push(InlineSpan {
            kind: InlineKind::Strong,
            range: ByteRange::new(20, 25),
        });
        let mut new = Decorations::empty(2);
        new.blocks.push(BlockSpan {
            kind: BlockKind::Paragraph,
            start_byte: 0,
            end_byte: 5,
        });
        new.blocks.push(BlockSpan {
            kind: BlockKind::Paragraph,
            start_byte: 12,
            end_byte: 17,
        });
        new.inlines.push(InlineSpan {
            kind: InlineKind::Strong,
            range: ByteRange::new(32, 35),
        });
        // Rope: line 0 "hello", line 1 "world", line 2 "third", line 3 "fourth",
        // line 4 "fifth", line 5 "sixth".
        let rope = rope_for("hello\nworld\nthird\nfourth\nfifth\nsixth");
        let dirty = new.diff_dirty_lines(&prev, &rope);
        // Removed prev inline @ bytes 20..25 → line 3 (text "fourth"
        // starts at byte 18; 20..25 falls inside).
        // Added new block @ bytes 12..17 → line 2 (text "third"
        // starts at byte 12; 12..17 falls inside).
        // Added new inline @ bytes 32..35 → line 5.
        // Block @ bytes 0..5 is identical in both → not dirty.
        assert_eq!(dirty, vec![2, 3, 5]);
        // Sorted invariant.
        assert!(dirty.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn edit_overlapping_span_lands_in_dirty_after_transform_then_diff() {
        use continuity_text::RopeEditDelta;

        // Original rope: "**bold** rest\n".
        // Prev decorations: one inline span (Strong) covering "bold"
        // at bytes 2..6.
        let mut prev = Decorations::empty(1);
        prev.inlines.push(InlineSpan {
            kind: InlineKind::Strong,
            range: ByteRange::new(2, 6),
        });

        // User edit: delete a character INSIDE the span (e.g. byte 3,
        // turning "bold" into "bld"). This intersects the span's
        // pre-edit range so `transformed_through` drops the span.
        let deltas = vec![RopeEditDelta::delete(3, 1)];
        let transformed = prev.transformed_through(&deltas, 2);
        assert!(
            transformed.inlines.is_empty(),
            "transformed_through should drop spans whose pre-edit range was intersected by the edit",
        );

        // New decorations: re-parsed against the post-edit rope finds
        // the same span at the new byte range "bld" (2..5).
        let mut new = Decorations::empty(2);
        new.inlines.push(InlineSpan {
            kind: InlineKind::Strong,
            range: ByteRange::new(2, 5),
        });

        // Post-edit rope: "**bld** rest\n".
        let rope_after = rope_for("**bld** rest\n");

        // Diff transformed-prev (empty) vs new (one inline span) ⇒ the
        // span's source line is dirty. Without the transform step the
        // diff would have seen prev's stale 2..6 as different from
        // new's 2..5 and dirtied for the wrong reason; the transform
        // collapses the prev side to "no span here" so the diff
        // attributes the change to the post-edit content alone.
        let dirty = new.diff_dirty_lines(&transformed, &rope_after);
        assert_eq!(dirty, vec![0], "edited line must end up in dirty set");
    }

    #[test]
    fn changed_kind_on_same_byte_range_marks_dirty() {
        let mut prev = Decorations::empty(1);
        prev.inlines.push(InlineSpan {
            kind: InlineKind::Strong,
            range: ByteRange::new(0, 5),
        });
        let mut new = Decorations::empty(2);
        new.inlines.push(InlineSpan {
            kind: InlineKind::Marker(MarkerKind::EmphasisDelim),
            range: ByteRange::new(0, 5),
        });
        let rope = rope_for("hello\nworld");
        let dirty = new.diff_dirty_lines(&prev, &rope);
        // Same byte range, different kind → both prev and new push
        // the same source line; dedup to a single entry.
        assert_eq!(dirty, vec![0]);
    }
}
