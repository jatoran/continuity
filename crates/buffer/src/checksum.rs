//! Incremental FNV-1a 64-bit checksum maintenance for the buffer's rope.
//!
//! The persisted `buffer_edits.checksum_after` column carries the FNV-1a
//! 64-bit hash of the rope's bytes at each revision. Recovery replays the
//! edit log and halts the first time the replayed rope's hash diverges
//! from the stored checksum (see `continuity-persist`'s recovery path).
//!
//! Computing that hash by walking the entire rope on every persisted
//! edit costs O(rope_bytes) per edit; on a ~711 KB buffer this is ~1 ms
//! p99 — about 3 % of the keystroke budget at the time of writing, and
//! linear in buffer size.
//!
//! FNV-1a's per-byte step is `state = (state ^ byte) * FNV_PRIME`. The
//! prime is odd, so it is invertible modulo `2^64`, and a single mix
//! step can be peeled off the tail of the running state with
//! `state = (state * FNV_PRIME_INV) ^ byte`. We exploit this to update
//! the running hash for an edit in O(suffix_bytes + edit_bytes) instead
//! of O(rope_bytes): unmix the bytes after the edit point, mix in the
//! new (and unmix the removed) bytes, then re-mix the suffix.
//!
//! For an append-at-the-end edit the suffix is empty and the cost is
//! O(edit_bytes); mid-rope edits in large buffers still touch the
//! suffix but never the prefix.
//!
//! A periodic full-walk verification (every `CHECKSUM_VERIFY_INTERVAL`
//! edits or at snapshot boundary, driven by the core thread) bounds
//! damage from any algebraic mistake to that interval and surfaces it
//! through `event:checksum_drift`.

use ropey::Rope;

use continuity_text::EditOp;

use crate::Error;

/// FNV-1a 64-bit offset basis. Same constant used by
/// [`continuity_persist::fnv1a_64`]; the running state for an empty
/// rope is exactly this value.
pub const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64-bit prime.
pub const FNV_PRIME: u64 = 0x0000_0001_0000_01b3;

/// Multiplicative inverse of [`FNV_PRIME`] modulo `2^64`. Computed at
/// compile time from the Newton–Raphson recurrence
/// `x_{k+1} = x_k * (2 - a * x_k)` (mod `2^64`), which doubles the
/// number of correct low bits each iteration; six iterations from
/// `x_0 = 1` (correct mod 2) take us past 64 bits.
pub const FNV_PRIME_INV: u64 = compute_modular_inverse_pow2(FNV_PRIME);

const fn compute_modular_inverse_pow2(a: u64) -> u64 {
    let mut x: u64 = 1;
    let mut i = 0;
    while i < 6 {
        x = x.wrapping_mul(2u64.wrapping_sub(a.wrapping_mul(x)));
        i += 1;
    }
    x
}

/// Periodic verification cadence — the core thread runs a full-walk
/// cross-check every this-many persisted edits per buffer (and at
/// snapshot boundaries). Bounds drift damage to one interval.
pub const CHECKSUM_VERIFY_INTERVAL: u32 = 1000;

/// One forward FNV-1a step. Pure.
#[inline]
#[must_use]
pub fn mix_byte(state: u64, byte: u8) -> u64 {
    (state ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
}

/// One inverse FNV-1a step — peels the last `byte` off the running
/// state. Correct only when `byte` was the most recently mixed byte;
/// to unmix a sequence, call this back-to-front. Pure.
#[inline]
#[must_use]
pub fn unmix_byte(state: u64, byte: u8) -> u64 {
    state.wrapping_mul(FNV_PRIME_INV) ^ u64::from(byte)
}

/// Full FNV-1a hash of `rope`'s bytes. The verification-path fallback
/// and the initial running state for newly constructed buffers both
/// route through here.
#[must_use]
pub fn full_walk_rope(rope: &Rope) -> u64 {
    let mut state = FNV_OFFSET_BASIS;
    for chunk in rope.chunks() {
        for &b in chunk.as_bytes() {
            state = mix_byte(state, b);
        }
    }
    state
}

fn mix_rope_range(state: u64, rope: &Rope, start: usize, end: usize) -> u64 {
    let mut s = state;
    let slice = rope.byte_slice(start..end);
    for chunk in slice.chunks() {
        for &b in chunk.as_bytes() {
            s = mix_byte(s, b);
        }
    }
    s
}

fn unmix_rope_range_reverse(state: u64, rope: &Rope, start: usize, end: usize) -> u64 {
    let mut s = state;
    let slice = rope.byte_slice(start..end);
    let chunks: Vec<_> = slice.chunks().collect();
    for chunk in chunks.into_iter().rev() {
        for &b in chunk.as_bytes().iter().rev() {
            s = unmix_byte(s, b);
        }
    }
    s
}

fn unmix_bytes_reverse(state: u64, bytes: &[u8]) -> u64 {
    let mut s = state;
    for &b in bytes.iter().rev() {
        s = unmix_byte(s, b);
    }
    s
}

/// Update `old_state` (the running FNV-1a hash of `rope_after` before
/// `op` was applied) to the new running hash after `op` has applied
/// to the rope. `rope_after` must be the post-apply rope;
/// `removed_text` is the substring `op` removed (empty for
/// `EditOp::Insert`).
///
/// Cost: `2 * suffix_bytes + edit_bytes`, where `suffix_bytes` is the
/// number of bytes in the rope after the edit point. Append-at-end
/// edits are O(edit_bytes); mid-rope edits in large buffers still walk
/// the suffix but never the prefix.
///
/// # Errors
///
/// Returns [`Error::Text`] when the op's positions cannot be resolved
/// against `rope_after`. The running state must not be advanced from
/// `old_state` in that case.
pub fn update_for_edit(
    old_state: u64,
    rope_after: &Rope,
    op: &EditOp,
    removed_text: &str,
) -> Result<u64, Error> {
    let rope_len = rope_after.len_bytes();
    match op {
        EditOp::Insert { at, text } => {
            let p = at.to_byte_offset(rope_after)?;
            let after = p.saturating_add(text.len()).min(rope_len);
            // suffix bytes = rope_after[after..rope_len]; same bytes as
            // were after `p` in the pre-apply rope. Unmixing the suffix
            // from old_state rewinds the running hash to `hash(prefix)`.
            let mut s = unmix_rope_range_reverse(old_state, rope_after, after, rope_len);
            for &b in text.as_bytes() {
                s = mix_byte(s, b);
            }
            s = mix_rope_range(s, rope_after, after, rope_len);
            Ok(s)
        }
        EditOp::Delete { range } => {
            let p = range.start.to_byte_offset(rope_after)?;
            // rope_after[p..rope_len] is the unchanged suffix.
            let mut s = unmix_rope_range_reverse(old_state, rope_after, p, rope_len);
            s = unmix_bytes_reverse(s, removed_text.as_bytes());
            s = mix_rope_range(s, rope_after, p, rope_len);
            Ok(s)
        }
        EditOp::Replace { range, text } => {
            let p = range.start.to_byte_offset(rope_after)?;
            let after = p.saturating_add(text.len()).min(rope_len);
            let mut s = unmix_rope_range_reverse(old_state, rope_after, after, rope_len);
            s = unmix_bytes_reverse(s, removed_text.as_bytes());
            for &b in text.as_bytes() {
                s = mix_byte(s, b);
            }
            s = mix_rope_range(s, rope_after, after, rope_len);
            Ok(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_text::{Position, Range};
    use proptest::prelude::*;

    fn fnv_bytes(bytes: &[u8]) -> u64 {
        let mut s = FNV_OFFSET_BASIS;
        for &b in bytes {
            s = mix_byte(s, b);
        }
        s
    }

    #[test]
    fn prime_and_inverse_multiply_to_one() {
        assert_eq!(FNV_PRIME.wrapping_mul(FNV_PRIME_INV), 1);
    }

    #[test]
    fn full_walk_empty_rope_is_offset_basis() {
        let rope = Rope::new();
        assert_eq!(full_walk_rope(&rope), FNV_OFFSET_BASIS);
    }

    #[test]
    fn full_walk_matches_byte_walk() {
        let s = "hello world\nlorem ipsum dolor";
        let rope = Rope::from_str(s);
        assert_eq!(full_walk_rope(&rope), fnv_bytes(s.as_bytes()));
    }

    #[test]
    fn mix_then_unmix_round_trips_one_byte() {
        let s = 0xdead_beef_1234_5678u64;
        let s2 = mix_byte(s, b'q');
        assert_eq!(unmix_byte(s2, b'q'), s);
    }

    #[test]
    fn insert_then_delete_returns_running_state() {
        // The "insert abc, then delete abc" round trip pins the
        // algebraic property: unmixing the bytes we just mixed gets us
        // back to bit-for-bit the same running state.
        let rope0 = Rope::from_str("xy");
        let state0 = full_walk_rope(&rope0);

        // Apply insert "abc" at end (Position(0, 2)).
        let mut rope1 = rope0.clone();
        rope1.insert(rope1.byte_to_char(2), "abc");
        let op_insert = EditOp::insert(Position::new(0, 2), "abc".to_string());
        let state1 = update_for_edit(state0, &rope1, &op_insert, "").unwrap();
        assert_eq!(state1, full_walk_rope(&rope1));

        // Apply delete of "abc" at [2..5].
        let mut rope2 = rope1.clone();
        rope2.remove(rope2.byte_to_char(2)..rope2.byte_to_char(5));
        let op_delete = EditOp::delete(Range::new(Position::new(0, 2), Position::new(0, 5)));
        let state2 = update_for_edit(state1, &rope2, &op_delete, "abc").unwrap();
        assert_eq!(state2, state0);
        assert_eq!(state2, full_walk_rope(&rope2));
    }

    #[test]
    fn replace_matches_full_walk() {
        let rope0 = Rope::from_str("hello world");
        let state0 = full_walk_rope(&rope0);
        let mut rope1 = rope0.clone();
        rope1.remove(rope1.byte_to_char(6)..rope1.byte_to_char(11));
        rope1.insert(rope1.byte_to_char(6), "rust");
        assert_eq!(rope1.to_string(), "hello rust");
        let op = EditOp::replace(
            Range::new(Position::new(0, 6), Position::new(0, 11)),
            "rust".to_string(),
        );
        let state1 = update_for_edit(state0, &rope1, &op, "world").unwrap();
        assert_eq!(state1, full_walk_rope(&rope1));
    }

    #[test]
    fn mid_rope_insert_matches_full_walk() {
        // The simple "mix new bytes on top of running state" approach
        // would diverge here because the suffix is mixed against a
        // different prior state in the new rope; this test catches that
        // class of bug.
        let rope0 = Rope::from_str("abcdefghij");
        let state0 = full_walk_rope(&rope0);
        let mut rope1 = rope0.clone();
        rope1.insert(rope1.byte_to_char(3), "XYZ");
        assert_eq!(rope1.to_string(), "abcXYZdefghij");
        let op = EditOp::insert(Position::new(0, 3), "XYZ".to_string());
        let state1 = update_for_edit(state0, &rope1, &op, "").unwrap();
        assert_eq!(state1, full_walk_rope(&rope1));
    }

    /// Apply `op` to ASCII `s`. Returns the new string and the removed
    /// substring (empty for inserts). Asserts the op is well-formed
    /// against `s` so the proptest input filter can reject otherwise.
    fn apply_edit_op_to_ascii(s: &str, op: &EditOp) -> Option<(String, String)> {
        match op {
            EditOp::Insert { at, text } => {
                if at.line != 0 {
                    return None;
                }
                let b = at.byte_in_line as usize;
                if b > s.len() {
                    return None;
                }
                let mut out = String::with_capacity(s.len() + text.len());
                out.push_str(&s[..b]);
                out.push_str(text);
                out.push_str(&s[b..]);
                Some((out, String::new()))
            }
            EditOp::Delete { range } => {
                if range.start.line != 0 || range.end.line != 0 {
                    return None;
                }
                let lo = range.start.byte_in_line as usize;
                let hi = range.end.byte_in_line as usize;
                if lo > hi || hi > s.len() {
                    return None;
                }
                let mut out = String::with_capacity(s.len() - (hi - lo));
                out.push_str(&s[..lo]);
                out.push_str(&s[hi..]);
                Some((out, s[lo..hi].to_string()))
            }
            EditOp::Replace { range, text } => {
                if range.start.line != 0 || range.end.line != 0 {
                    return None;
                }
                let lo = range.start.byte_in_line as usize;
                let hi = range.end.byte_in_line as usize;
                if lo > hi || hi > s.len() {
                    return None;
                }
                let mut out = String::with_capacity(s.len() + text.len());
                out.push_str(&s[..lo]);
                out.push_str(text);
                out.push_str(&s[hi..]);
                Some((out, s[lo..hi].to_string()))
            }
        }
    }

    /// One synthetic edit recipe — proptest input shape so positions
    /// are bounded reproducibly regardless of the buffer's evolving
    /// length. `kind ∈ {0,1,2}` selects insert/delete/replace; `start`
    /// and `len` are byte offsets within the current string (clamped
    /// in the test body); `text` provides any inserted bytes.
    #[derive(Clone, Debug)]
    struct EditRecipe {
        kind: u8,
        start: u8,
        len: u8,
        text: String,
    }

    fn arb_recipe() -> impl Strategy<Value = EditRecipe> {
        (
            0u8..3,
            any::<u8>(),
            any::<u8>(),
            proptest::collection::vec(b'a'..=b'z', 0..=8),
        )
            .prop_map(|(k, s, l, bytes)| EditRecipe {
                kind: k,
                start: s,
                len: l,
                text: String::from_utf8(bytes).expect("invariant: ASCII bytes are valid UTF-8"),
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// Apply a random sequence of insert / delete / replace ops to an
        /// ASCII string and assert the incrementally-maintained running
        /// state equals the full-walk hash of the resulting rope at every
        /// step. Catches any algebraic mistake in
        /// [`update_for_edit`] that diverges from `fnv1a_64(rope)`.
        #[test]
        fn random_op_sequence_running_state_tracks_full_walk(
            initial in "[a-zA-Z0-9 ]{0,32}",
            recipes in proptest::collection::vec(arb_recipe(), 1..=10),
        ) {
            let mut s = initial;
            let mut state = full_walk_rope(&Rope::from_str(&s));
            prop_assert_eq!(state, fnv_bytes(s.as_bytes()));

            for recipe in recipes {
                let p = if s.is_empty() {
                    0
                } else {
                    (recipe.start as usize) % (s.len() + 1)
                };
                let q = p.saturating_add((recipe.len as usize) % (s.len() - p + 1));
                let p_u32 = p as u32;
                let q_u32 = q as u32;
                let op = match recipe.kind % 3 {
                    0 => EditOp::insert(Position::new(0, p_u32), recipe.text.clone()),
                    1 => EditOp::delete(Range::new(
                        Position::new(0, p_u32),
                        Position::new(0, q_u32),
                    )),
                    _ => EditOp::replace(
                        Range::new(Position::new(0, p_u32), Position::new(0, q_u32)),
                        recipe.text.clone(),
                    ),
                };

                let Some((after, removed)) = apply_edit_op_to_ascii(&s, &op) else {
                    continue;
                };
                let rope_after = Rope::from_str(&after);
                state = update_for_edit(state, &rope_after, &op, &removed).unwrap();
                prop_assert_eq!(state, full_walk_rope(&rope_after));
                prop_assert_eq!(state, fnv_bytes(after.as_bytes()));
                s = after;
            }
        }
    }
}
