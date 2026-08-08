//! Minimal splice bounds for full-document host reconciliation.

use ropey::Rope;

/// Byte bounds of the smallest single replacement turning the rope into `new_text`.
///
/// `start..old_end` is the replaced rope range; `start..new_end` indexes the
/// replacement slice inside `new_text`. All three bounds sit on UTF-8 character
/// boundaries. A host reconcile that splices only this range keeps selections,
/// scroll anchors, and incremental line mirrors stable everywhere else in the
/// document, instead of invalidating every line with a whole-document replace.
#[must_use]
pub fn compute_reconcile_splice(rope: &Rope, new_text: &str) -> ReconcileSplice {
    let old_len = rope.len_bytes();
    let new_bytes = new_text.as_bytes();
    let mut prefix = 0;
    'prefix: for chunk in rope.chunks() {
        for &byte in chunk.as_bytes() {
            if prefix >= new_bytes.len() || new_bytes[prefix] != byte {
                break 'prefix;
            }
            prefix += 1;
        }
    }
    while prefix > 0 && !new_text.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = old_len.min(new_bytes.len()) - prefix;
    let mut suffix = 0;
    for byte in rope.bytes_at(old_len).reversed() {
        if suffix >= max_suffix || new_bytes[new_bytes.len() - 1 - suffix] != byte {
            break;
        }
        suffix += 1;
    }
    // The shared suffix bytes are identical in both texts, so one boundary
    // check covers the rope and the replacement string alike.
    while suffix > 0 && !new_text.is_char_boundary(new_bytes.len() - suffix) {
        suffix -= 1;
    }
    ReconcileSplice {
        start: prefix,
        old_end: old_len - suffix,
        new_end: new_bytes.len() - suffix,
    }
}

/// Character-boundary-safe replacement bounds returned by [`compute_reconcile_splice`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconcileSplice {
    /// First differing byte, shared by both texts.
    pub start: usize,
    /// End of the replaced range in the rope.
    pub old_end: usize,
    /// End of the replacement slice in the new text.
    pub new_end: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn splice(old: &str, new: &str) -> ReconcileSplice {
        let bounds = compute_reconcile_splice(&Rope::from_str(old), new);
        let mut patched = String::new();
        patched.push_str(&old[..bounds.start]);
        patched.push_str(&new[bounds.start..bounds.new_end]);
        patched.push_str(&old[bounds.old_end..]);
        assert_eq!(patched, new, "splice must reproduce the new text");
        bounds
    }

    #[test]
    fn mid_document_insertion_bounds_only_the_inserted_word() {
        let bounds = splice("alpha beta gamma", "alpha beta word gamma");
        assert_eq!(bounds.start, 11);
        assert_eq!(bounds.old_end, 11);
        assert_eq!(bounds.new_end, 16);
    }

    #[test]
    fn mid_document_deletion_bounds_only_the_removed_run() {
        let bounds = splice("alpha beta gamma", "alpha gamma");
        assert_eq!(bounds.old_end - bounds.start, 5);
        assert_eq!(bounds.new_end, bounds.start);
    }

    #[test]
    fn overlapping_repeats_never_cross_prefix_and_suffix() {
        splice("aaaa", "aaa");
        splice("aaa", "aaaa");
        splice("abab", "ababab");
    }

    #[test]
    fn multibyte_replacement_stays_on_character_boundaries() {
        let bounds = splice("héllo wörld", "héllo wärld");
        assert!("héllo wärld".is_char_boundary(bounds.start));
        assert!("héllo wärld".is_char_boundary(bounds.new_end));
    }

    #[test]
    fn disjoint_texts_replace_everything() {
        let bounds = splice("one", "two");
        assert_eq!(bounds.start, 0);
        assert_eq!(bounds.old_end, 3);
        assert_eq!(bounds.new_end, 3);
    }

    #[test]
    fn empty_old_and_empty_new_are_pure_insert_and_delete() {
        splice("", "fresh");
        splice("stale", "");
    }
}
