//! Rope edit delta — a flat description of one edit's byte-level
//! effect on the rope, suitable for transforming byte-range metadata
//! (decoration spans, line caches, …) attached to a previous rope
//! revision through to the current one.
//!
//! Where [`crate::EditOp`] is the *input* to a buffer mutation
//! ("insert this text at this position"), [`RopeEditDelta`] is the
//! *output* in byte-only terms. The transform path in
//! `continuity_decorate::Decorations::transformed_through` consumes
//! a slice of these to shift / drop spans without needing access to
//! the rope itself.

/// A single edit's byte-level effect: at position `at` we removed
/// `removed_bytes` and inserted `inserted_bytes`. `at` is in the
/// rope's byte coordinates **as they existed when this delta was
/// applied** — chains of deltas walk forward from the older
/// revision, so each delta's `at` is interpreted against the rope
/// state produced by all prior deltas in the chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RopeEditDelta {
    /// Byte position where the edit happened.
    pub at: usize,
    /// Number of bytes removed at `at` before any insertion.
    pub removed_bytes: usize,
    /// Number of bytes inserted at `at` after removal.
    pub inserted_bytes: usize,
}

impl RopeEditDelta {
    /// Build a delta for a pure insertion.
    #[must_use]
    pub fn insert(at: usize, inserted_bytes: usize) -> Self {
        Self {
            at,
            removed_bytes: 0,
            inserted_bytes,
        }
    }

    /// Build a delta for a pure deletion.
    #[must_use]
    pub fn delete(at: usize, removed_bytes: usize) -> Self {
        Self {
            at,
            removed_bytes,
            inserted_bytes: 0,
        }
    }

    /// Build a delta for a replace.
    #[must_use]
    pub fn replace(at: usize, removed_bytes: usize, inserted_bytes: usize) -> Self {
        Self {
            at,
            removed_bytes,
            inserted_bytes,
        }
    }

    /// Signed byte shift this delta induces for content past
    /// `at + removed_bytes`.
    #[must_use]
    pub fn shift(&self) -> isize {
        self.inserted_bytes as isize - self.removed_bytes as isize
    }

    /// Half-open byte range this delta consumed in the pre-edit rope.
    /// `(at, at + removed_bytes)`. Spans intersecting this range
    /// cannot be safely shifted and should be dropped by the caller.
    #[must_use]
    pub fn pre_edit_range(&self) -> (usize, usize) {
        (self.at, self.at + self.removed_bytes)
    }
}

/// Transform a single byte position through one delta. Returns
/// `None` when the position falls strictly inside the deleted
/// range — caller is expected to drop the surrounding span. A
/// position exactly at `delta.at` is preserved (mapped to itself)
/// to keep spans that *end* at the edit boundary stable.
#[must_use]
pub fn transform_byte_through(byte: usize, delta: RopeEditDelta) -> Option<usize> {
    let (lo, hi) = delta.pre_edit_range();
    if byte <= lo {
        return Some(byte);
    }
    if byte >= hi {
        return Some(((byte as isize) + delta.shift()).max(0) as usize);
    }
    None
}

/// Transform a half-open byte range `[start, end)` through one
/// delta. Returns `None` when the edit's pre-edit range overlaps
/// the span and shifting is not safe (drop the span at the
/// caller). Returns `Some((new_start, new_end))` when the span sits
/// entirely before or entirely after the edit's pre-edit range and
/// can be shifted unambiguously.
#[must_use]
pub fn transform_range_through(
    start: usize,
    end: usize,
    delta: RopeEditDelta,
) -> Option<(usize, usize)> {
    let (lo, hi) = delta.pre_edit_range();
    // Span entirely before the edit (touching the boundary is fine).
    if end <= lo {
        return Some((start, end));
    }
    // Span entirely after the edit's deleted range.
    if start >= hi {
        let shift = delta.shift();
        let new_start = ((start as isize) + shift).max(0) as usize;
        let new_end = ((end as isize) + shift).max(0) as usize;
        return Some((new_start, new_end));
    }
    // Overlap with the edit's deleted range — caller must drop.
    None
}

/// Transform a half-open byte range through every delta in the
/// slice in order. Returns `None` if any delta intersects.
#[must_use]
pub fn transform_range_through_chain(
    start: usize,
    end: usize,
    deltas: &[RopeEditDelta],
) -> Option<(usize, usize)> {
    let mut s = start;
    let mut e = end;
    for delta in deltas {
        let (ns, ne) = transform_range_through(s, e, *delta)?;
        s = ns;
        e = ne;
    }
    Some((s, e))
}

/// Transform a half-open byte range `[start, end)` through one delta
/// using **container semantics**: an edit that falls fully inside the
/// range grows or shrinks the range's `end` by the edit's byte shift
/// instead of dropping it.
///
/// This is the right model for a span that *contains* user edits —
/// e.g. a pipe-table block whose cells are being typed into — where
/// intra-range text edits change the byte extent but not the
/// structural identity of the span. Plain
/// [`transform_range_through`] drops such a span (an interior edit
/// overlaps it), which makes a table flicker to raw markdown on every
/// keystroke while the decoration worker lags one revision behind.
///
/// Returns `None` only when the edit **straddles a boundary**
/// (`start` or `end` lands strictly inside the edit's pre-edit
/// range) — a genuine structural change the caller should drop and
/// let a fresh parse replace. An edit exactly at `start` is treated
/// as "before" (shifts the whole range, so e.g. a newline typed at a
/// table's first byte pushes the table down); an edit exactly at
/// `end` is treated as "after" (leaves the range untouched).
#[must_use]
pub fn transform_container_range_through(
    start: usize,
    end: usize,
    delta: RopeEditDelta,
) -> Option<(usize, usize)> {
    let (lo, hi) = delta.pre_edit_range();
    // Entirely before the range (touching the start boundary is
    // "before"): shift both ends by the edit's byte delta.
    if hi <= start {
        let shift = delta.shift();
        let new_start = ((start as isize) + shift).max(0) as usize;
        let new_end = ((end as isize) + shift).max(0) as usize;
        return Some((new_start, new_end));
    }
    // Entirely after the range (touching the end boundary is
    // "after"): the edit sits outside, range is unchanged.
    if lo >= end {
        return Some((start, end));
    }
    // Fully interior: the edit lives inside `[start, end)`. Keep
    // `start`, move `end` by the byte shift. A contained deletion
    // removes at most `end - start` bytes, so `new_end >= start`;
    // equality means the edit deleted the entire range, which is a
    // structural collapse (e.g. selecting a whole table and deleting
    // it) — drop so the caller doesn't keep an empty span alive and
    // paint ghost chrome over now-blank source.
    if lo >= start && hi <= end {
        let new_end = ((end as isize) + delta.shift()).max(start as isize) as usize;
        if new_end <= start {
            return None;
        }
        return Some((start, new_end));
    }
    // Edit straddles `start` or `end` — structural change, drop.
    None
}

/// Transform a half-open byte range through every delta in order
/// with container semantics ([`transform_container_range_through`]).
/// Returns `None` if any delta straddles a boundary of the range.
#[must_use]
pub fn transform_container_range_through_chain(
    start: usize,
    end: usize,
    deltas: &[RopeEditDelta],
) -> Option<(usize, usize)> {
    let mut s = start;
    let mut e = end;
    for delta in deltas {
        let (ns, ne) = transform_container_range_through(s, e, *delta)?;
        s = ns;
        e = ne;
    }
    Some((s, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_before_edit_is_unchanged() {
        let d = RopeEditDelta::insert(100, 5);
        assert_eq!(transform_range_through(10, 50, d), Some((10, 50)));
    }

    #[test]
    fn range_after_insertion_shifts_forward() {
        let d = RopeEditDelta::insert(100, 5);
        assert_eq!(transform_range_through(200, 250, d), Some((205, 255)));
    }

    #[test]
    fn range_after_deletion_shifts_back() {
        let d = RopeEditDelta::delete(100, 5);
        assert_eq!(transform_range_through(200, 250, d), Some((195, 245)));
    }

    #[test]
    fn range_overlapping_edit_drops() {
        let d = RopeEditDelta::replace(100, 5, 3);
        assert_eq!(transform_range_through(95, 110, d), None);
        assert_eq!(transform_range_through(102, 110, d), None);
        assert_eq!(transform_range_through(80, 102, d), None);
    }

    #[test]
    fn range_touching_edit_boundary_is_preserved() {
        let d = RopeEditDelta::insert(100, 5);
        // `end == at` ⇒ span sits entirely before the insertion.
        assert_eq!(transform_range_through(50, 100, d), Some((50, 100)));
        // `start == at + removed_bytes` ⇒ span sits entirely after.
        assert_eq!(transform_range_through(100, 200, d), Some((105, 205)));
    }

    #[test]
    fn chain_applies_in_order() {
        let chain = [
            RopeEditDelta::insert(50, 3), // [50,50) -> +3 bytes
            RopeEditDelta::insert(60, 2), // applied to post-prior rope
        ];
        // Span [100, 120) — both edits are before it; expect +5 shift.
        assert_eq!(
            transform_range_through_chain(100, 120, &chain),
            Some((105, 125))
        );
    }

    #[test]
    fn chain_drops_if_any_delta_intersects() {
        let chain = [
            RopeEditDelta::insert(50, 3),
            RopeEditDelta::replace(110, 5, 1), // post-shift overlaps target
        ];
        // Span [100, 120) after first shift: [103, 123). Second
        // edit at 110 with removed_bytes 5 overlaps → drop.
        assert_eq!(transform_range_through_chain(100, 120, &chain), None);
    }

    #[test]
    fn container_interior_insert_extends_end_only() {
        // Typing one byte inside a table cell: keep start, grow end.
        let d = RopeEditDelta::insert(110, 1);
        assert_eq!(
            transform_container_range_through(100, 200, d),
            Some((100, 201))
        );
    }

    #[test]
    fn container_interior_delete_shrinks_end_only() {
        // Deleting inside a cell: keep start, shrink end.
        let d = RopeEditDelta::delete(110, 4);
        assert_eq!(
            transform_container_range_through(100, 200, d),
            Some((100, 196))
        );
    }

    #[test]
    fn container_edit_before_shifts_both() {
        let d = RopeEditDelta::insert(50, 5);
        assert_eq!(
            transform_container_range_through(100, 200, d),
            Some((105, 205))
        );
    }

    #[test]
    fn container_edit_after_is_unchanged() {
        let d = RopeEditDelta::insert(300, 5);
        assert_eq!(
            transform_container_range_through(100, 200, d),
            Some((100, 200))
        );
    }

    #[test]
    fn container_insert_at_start_boundary_pushes_range_forward() {
        // A newline typed at a table's first byte must push the whole
        // table down (matches the "Enter at table start" behavior),
        // not extend its end.
        let d = RopeEditDelta::insert(100, 1);
        assert_eq!(
            transform_container_range_through(100, 200, d),
            Some((101, 201))
        );
    }

    #[test]
    fn container_insert_at_end_boundary_stays_outside() {
        let d = RopeEditDelta::insert(200, 1);
        assert_eq!(
            transform_container_range_through(100, 200, d),
            Some((100, 200))
        );
    }

    #[test]
    fn container_full_range_delete_collapses_to_none() {
        // Selecting a whole table and deleting it must drop the span
        // (not keep an empty range), so the chrome painter takes its
        // delete-lag path instead of painting ghost chrome.
        let d = RopeEditDelta::delete(100, 100);
        assert_eq!(transform_container_range_through(100, 200, d), None);
    }

    #[test]
    fn container_edit_straddling_start_boundary_drops() {
        // Edit reaches from before `start` into the interior — a
        // structural change; drop and let a fresh parse replace it.
        let d = RopeEditDelta::replace(90, 20, 3);
        assert_eq!(transform_container_range_through(100, 200, d), None);
    }

    #[test]
    fn container_edit_straddling_end_boundary_drops() {
        let d = RopeEditDelta::replace(190, 30, 3);
        assert_eq!(transform_container_range_through(100, 200, d), None);
    }

    #[test]
    fn container_chain_threads_interior_edits() {
        let chain = [
            RopeEditDelta::insert(50, 5),  // before → shifts to [105, 205)
            RopeEditDelta::insert(150, 2), // interior → end grows to 207
            RopeEditDelta::delete(160, 1), // interior → end shrinks to 206
        ];
        assert_eq!(
            transform_container_range_through_chain(100, 200, &chain),
            Some((105, 206))
        );
    }
}
