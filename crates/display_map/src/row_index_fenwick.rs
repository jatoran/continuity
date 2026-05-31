//! Fenwick (binary indexed) tree over `u32` row counts.
//!
//! Backs [`crate::row_index::DisplayRowIndex`]'s O(log n) source-line →
//! display-row prefix queries and the inverse display-row → source-line
//! lookup. Edits are O(log n); a parallel `values` vector keeps `get(i)`
//! at O(1).
//!
//! ## Layout invariants
//!
//! The internal `tree` array is 1-indexed (slot `0` is unused) and stores
//! partial sums in the conventional Fenwick layout: `tree[i]` holds the
//! sum of `values[(i - LSB(i))..i]` where `LSB(i) = i & i.wrapping_neg()`.
//! Running sums are `u64` so an entire 6000-line document of u32-max
//! values cannot overflow the prefix tree.
//!
//! ## Thread ownership
//!
//! Built and read on whichever thread owns the parent `DisplayMap`
//! (worker for build, UI for read). The struct is `Clone`; clones are
//! independent copies, not shared `Arc`s.

use std::fmt;

/// Fenwick tree of `u32` values backing a row-count index.
#[derive(Clone)]
pub(crate) struct Fenwick {
    /// 1-indexed Fenwick array; `tree[0]` is unused.
    tree: Vec<u64>,
    /// Mirror of the original values for O(1) `get` and O(log n) `set`.
    values: Vec<u32>,
}

impl Fenwick {
    /// Build from a slice of row counts. O(n).
    #[must_use]
    pub(crate) fn from_values(values: &[u32]) -> Self {
        let n = values.len();
        let mut tree = vec![0u64; n + 1];
        // Propagate each value to its parent in one linear pass.
        for i in 0..n {
            tree[i + 1] += u64::from(values[i]);
            let parent = (i + 1) + ((i + 1) & (i + 1).wrapping_neg());
            if parent <= n {
                let v = tree[i + 1];
                tree[parent] += v;
            }
        }
        Self {
            tree,
            values: values.to_vec(),
        }
    }

    /// Number of source slots the tree was built over.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    /// `true` when the tree has no slots.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Borrow the value at `i`. Panics if `i` is out of range.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn get(&self, i: usize) -> u32 {
        self.values[i]
    }

    /// Sum of `values[0..i]`. `prefix_sum(0) == 0`;
    /// `prefix_sum(len()) == total()`.
    #[must_use]
    pub(crate) fn prefix_sum(&self, mut i: usize) -> u64 {
        debug_assert!(
            i <= self.len(),
            "Fenwick::prefix_sum index out of bounds: {i} > {}",
            self.len()
        );
        let mut sum: u64 = 0;
        while i > 0 {
            sum += self.tree[i];
            i -= i & i.wrapping_neg();
        }
        sum
    }

    /// Total of all values (== `prefix_sum(len())`).
    #[must_use]
    pub(crate) fn total(&self) -> u64 {
        self.prefix_sum(self.len())
    }

    /// Replace the value at `i` with `value`, fixing up the tree in
    /// O(log n).
    pub(crate) fn set(&mut self, i: usize, value: u32) {
        let old = self.values[i];
        if value == old {
            return;
        }
        let n = self.len();
        if value > old {
            let delta = u64::from(value - old);
            self.values[i] = value;
            let mut k = i + 1;
            while k <= n {
                self.tree[k] += delta;
                k += k & k.wrapping_neg();
            }
        } else {
            let delta = u64::from(old - value);
            self.values[i] = value;
            let mut k = i + 1;
            while k <= n {
                self.tree[k] -= delta;
                k += k & k.wrapping_neg();
            }
        }
    }

    /// Find the source slot containing display position `target`.
    ///
    /// Returns `Some((i, offset))` where `prefix_sum(i) + offset == target`
    /// and `offset < values[i]`. Returns `None` if `target >= total()`.
    /// Folded slots (`values[i] == 0`) are transparently skipped: the
    /// returned `i` always has `values[i] > 0`.
    #[must_use]
    pub(crate) fn find_by_prefix(&self, target: u64) -> Option<(usize, u32)> {
        let n = self.len();
        if n == 0 || target >= self.total() {
            return None;
        }
        let mut idx = 0usize;
        let mut bit = 1usize;
        while bit * 2 <= n {
            bit *= 2;
        }
        let mut remaining = target;
        while bit > 0 {
            let next = idx + bit;
            if next <= n && self.tree[next] <= remaining {
                remaining -= self.tree[next];
                idx = next;
            }
            bit >>= 1;
        }
        // The bit-walk leaves `idx` at the largest 1-based index whose
        // prefix sum still fits under `target`. If the source slot at
        // 0-based `idx` is folded (`values[idx] == 0`), step forward
        // through the folded run — `target < total()` guarantees the
        // next non-folded slot exists within bounds.
        while idx < n && self.values[idx] == 0 {
            idx += 1;
        }
        if idx >= n {
            return None;
        }
        debug_assert!(remaining < u64::from(self.values[idx]));
        Some((idx, remaining as u32))
    }
}

impl fmt::Debug for Fenwick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fenwick")
            .field("len", &self.values.len())
            .field("total", &self.total())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_prefix(values: &[u32], i: usize) -> u64 {
        values[..i].iter().copied().map(u64::from).sum()
    }

    #[test]
    fn empty_tree_reports_zero_total_and_no_lookups() {
        let f = Fenwick::from_values(&[]);
        assert_eq!(f.len(), 0);
        assert!(f.is_empty());
        assert_eq!(f.total(), 0);
        assert_eq!(f.prefix_sum(0), 0);
        assert_eq!(f.find_by_prefix(0), None);
    }

    #[test]
    fn single_value_round_trip() {
        let f = Fenwick::from_values(&[5]);
        assert_eq!(f.get(0), 5);
        assert_eq!(f.total(), 5);
        assert_eq!(f.prefix_sum(0), 0);
        assert_eq!(f.prefix_sum(1), 5);
        assert_eq!(f.find_by_prefix(0), Some((0, 0)));
        assert_eq!(f.find_by_prefix(4), Some((0, 4)));
        assert_eq!(f.find_by_prefix(5), None);
    }

    #[test]
    fn prefix_sum_matches_naive_for_assorted_lengths() {
        for values in [
            vec![1u32, 2, 3, 4, 5],
            vec![0, 0, 0, 1, 0, 0, 2],
            vec![10; 33],
            (0..100).collect::<Vec<u32>>(),
        ] {
            let f = Fenwick::from_values(&values);
            for i in 0..=values.len() {
                assert_eq!(f.prefix_sum(i), naive_prefix(&values, i), "i={i}");
            }
            assert_eq!(f.total(), naive_prefix(&values, values.len()));
        }
    }

    #[test]
    fn find_by_prefix_skips_folded_slots() {
        // Slots 1 and 2 are folded (count 0); display rows 1..=1 must
        // land on slot 3.
        let f = Fenwick::from_values(&[1, 0, 0, 1]);
        assert_eq!(f.find_by_prefix(0), Some((0, 0)));
        assert_eq!(f.find_by_prefix(1), Some((3, 0)));
        assert_eq!(f.find_by_prefix(2), None);
    }

    #[test]
    fn find_by_prefix_returns_offset_within_multirow_slot() {
        // Slot 0 occupies 3 display rows; slot 1 occupies 2.
        let f = Fenwick::from_values(&[3, 2]);
        assert_eq!(f.find_by_prefix(0), Some((0, 0)));
        assert_eq!(f.find_by_prefix(1), Some((0, 1)));
        assert_eq!(f.find_by_prefix(2), Some((0, 2)));
        assert_eq!(f.find_by_prefix(3), Some((1, 0)));
        assert_eq!(f.find_by_prefix(4), Some((1, 1)));
        assert_eq!(f.find_by_prefix(5), None);
    }

    #[test]
    fn set_updates_prefix_sums() {
        let mut f = Fenwick::from_values(&[1, 2, 3, 4]);
        assert_eq!(f.total(), 10);
        f.set(1, 7);
        assert_eq!(f.get(1), 7);
        assert_eq!(f.total(), 15);
        assert_eq!(f.prefix_sum(2), 8);
        f.set(1, 0);
        assert_eq!(f.total(), 8);
        // Slot 1 is now folded; find_by_prefix must skip it.
        assert_eq!(f.find_by_prefix(1), Some((2, 0)));
    }

    #[test]
    fn set_to_same_value_is_a_noop() {
        let mut f = Fenwick::from_values(&[1, 2, 3]);
        let before = f.total();
        f.set(1, 2);
        assert_eq!(f.total(), before);
    }
}
