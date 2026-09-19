//! SHA-256 verification of downloaded release assets against the
//! release's `SHA256SUMS.txt` (the same file `sync-public.ps1` writes
//! and the release uploads).

use sha2::{Digest, Sha256};

use super::UpdateError;

/// Lower-case hex SHA-256 of `bytes`.
#[must_use]
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Find `asset_name`'s expected digest in a `SHA256SUMS.txt` body
/// (`<hex>  <name>` or `<hex> *<name>` per line; case-insensitive hex).
#[must_use]
pub(crate) fn expected_sha256(sums: &str, asset_name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hex = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name.eq_ignore_ascii_case(asset_name) && hex.len() == 64).then(|| hex.to_ascii_lowercase())
    })
}

/// Fail unless `bytes` hashes to the digest listed for `asset_name`. A
/// missing entry is a failure too: an unlisted asset is not verified.
pub(crate) fn verify(sums: &str, asset_name: &str, bytes: &[u8]) -> Result<(), UpdateError> {
    let expected = expected_sha256(sums, asset_name)
        .ok_or_else(|| UpdateError::Checksum(asset_name.into()))?;
    if sha256_hex(bytes) == expected {
        Ok(())
    } else {
        Err(UpdateError::Checksum(asset_name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{expected_sha256, sha256_hex, verify};

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn hashes_and_parses_the_sums_file() {
        assert_eq!(sha256_hex(b"abc"), ABC);
        let sums = format!("{ABC}  continuity-0.4.12-setup.msi\n{ABC} *SHA256SUMS.txt\n");
        assert_eq!(
            expected_sha256(&sums, "continuity-0.4.12-setup.msi").as_deref(),
            Some(ABC)
        );
        assert_eq!(
            expected_sha256(&sums, "sha256sums.txt").as_deref(),
            Some(ABC)
        );
        assert_eq!(expected_sha256(&sums, "missing.zip"), None);
    }

    #[test]
    fn verify_rejects_mismatch_and_unlisted_assets() {
        let sums = format!("{ABC}  a.msi\n");
        assert!(verify(&sums, "a.msi", b"abc").is_ok());
        assert!(verify(&sums, "a.msi", b"abd").is_err());
        assert!(verify(&sums, "b.msi", b"abc").is_err());
    }
}
