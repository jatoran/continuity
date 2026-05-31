//! Tiny deterministic 64-bit hash for edit-log integrity (FNV-1a).
//!
//! Not cryptographic; sufficient for catching corruption at recovery.

/// FNV-1a 64-bit hash.
#[must_use]
pub fn fnv1a_64(data: &[u8]) -> u64 {
    fnv1a_64_chunks(std::iter::once(data))
}

/// FNV-1a 64-bit hash over a sequence of byte chunks. Equivalent to
/// concatenating the chunks and feeding them to [`fnv1a_64`], but without
/// allocating the concatenation. Useful for hashing a `ropey::Rope` in place
/// via its `chunks()` iterator.
#[must_use]
pub fn fnv1a_64_chunks<'a, I: IntoIterator<Item = &'a [u8]>>(chunks: I) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for chunk in chunks {
        for byte in chunk {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0001_0000_01b3);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_offset_basis() {
        assert_eq!(fnv1a_64(&[]), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn deterministic() {
        assert_eq!(fnv1a_64(b"hello world"), fnv1a_64(b"hello world"));
    }

    #[test]
    fn small_changes_propagate() {
        assert_ne!(fnv1a_64(b"hello"), fnv1a_64(b"hellp"));
    }

    #[test]
    fn chunks_match_concatenation() {
        let one = fnv1a_64(b"hello world");
        let chunks: Vec<&[u8]> = vec![b"hel", b"lo wor", b"ld"];
        let many = fnv1a_64_chunks(chunks);
        assert_eq!(one, many);
    }
}
