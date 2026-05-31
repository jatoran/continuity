//! Phase F5 — hash-deduplicated shared image store.
//!
//! Pasted clipboard images and dropped image files land in a single
//! per-user directory (`%APPDATA%\continuity\images` by default; see
//! [`continuity_config::MarkdownConfig::images_dir`]). Each file is
//! named after the FNV-1a 64-bit hash of its canonical bytes plus the
//! original extension:
//!
//! ```text
//! %APPDATA%\continuity\images\
//!     a3f9b1e0c4d2e5f6.png
//!     7b8c9d1e2f3a4b5c.jpg
//! ```
//!
//! The buffer references the file with a plain markdown image link
//! using a forward-slash relative path: `![](images/<hash>.<ext>)`.
//! [`continuity_decorate::image_link::is_shared_store_reference`] is
//! the renderer's gate for "resolve via the shared store".
//!
//! Dedup invariant: importing the same bytes twice produces the same
//! relative reference and writes the file exactly once. Repeat calls
//! after the first are filesystem-stat checks.
//!
//! Thread ownership: callable from any thread; the only mutable state
//! is the filesystem under `images_dir`, which is treated as a
//! shared-by-content store (writes are name-stable so concurrent
//! writers cannot disagree on contents). The UI thread is the
//! production caller today.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// FNV-1a 64-bit hash of an image's canonical bytes. The dedup key in
/// [`import_bytes`]. Newtype so it does not collide with rope
/// revisions, persist checksums, or other 64-bit ids on the same
/// surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageHash(pub u64);

impl ImageHash {
    /// Render the hash as a 16-char lowercase hex string. Used as the
    /// stem of the on-disk filename.
    #[must_use]
    pub fn as_filename_stem(self) -> String {
        format!("{:016x}", self.0)
    }
}

const FNV_OFFSET_BASIS_64: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME_64: u64 = 0x0000_0100_0000_01b3;

/// Compute the FNV-1a 64-bit hash of `bytes`. Matches the persist-layer
/// checksum style (also FNV-1a) so the binary cost is shared.
#[must_use]
pub fn fnv1a_64(bytes: &[u8]) -> ImageHash {
    let mut h: u64 = FNV_OFFSET_BASIS_64;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(FNV_PRIME_64);
    }
    ImageHash(h)
}

/// Normalise a caller-supplied extension into a lowercase suffix
/// without the leading dot. Unknown / empty extensions fall back to
/// `"bin"` so the on-disk file still has a deterministic name. Defends
/// against malformed clipboard / drop-file inputs.
#[must_use]
pub fn normalise_extension(ext: &str) -> String {
    let trimmed = ext.trim().trim_start_matches('.');
    if trimmed.is_empty() {
        return "bin".to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    // Reject anything with path separators or whitespace — the caller
    // gave us something that is not an extension and we refuse to put
    // it on disk verbatim.
    if lower.chars().any(|c| {
        c == '/' || c == '\\' || c == '.' || c == ':' || c.is_whitespace() || c.is_control()
    }) {
        return "bin".to_string();
    }
    lower
}

/// Predicate: is `ext` (with or without a leading dot) one of the
/// image extensions the drag-drop / paste pipelines recognise? See
/// [`SUPPORTED_IMAGE_EXTENSIONS`].
#[must_use]
pub fn is_supported_image_extension(ext: &str) -> bool {
    let norm = normalise_extension(ext);
    SUPPORTED_IMAGE_EXTENSIONS.iter().any(|e| *e == norm)
}

/// Image extensions the F5 drag-drop and paste paths recognise. The
/// pixel decoder (WIC) supports a superset; this is the conservative
/// list used to route a dropped file through the import path rather
/// than the tab-open path.
pub const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// The result of an [`import_bytes`] call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedImage {
    /// FNV-1a 64-bit hash of the canonical bytes.
    pub hash: ImageHash,
    /// Markdown-ready relative reference: `images/<hash>.<ext>`. The
    /// forward-slash form survives round-tripping through markdown
    /// sources on all platforms;
    /// [`continuity_decorate::image_link::is_shared_store_reference`]
    /// accepts both separators on the consumer side.
    pub markdown_reference: String,
    /// Absolute on-disk path the bytes ended up at. Useful for the
    /// renderer's bitmap cache lookup and for tests.
    pub absolute_path: PathBuf,
    /// True when the file was written by this call. False when the
    /// hash-named file already existed (the dedup hit).
    pub was_written: bool,
}

/// Import `bytes` into the shared image store at `images_dir`. The
/// extension is normalised via [`normalise_extension`]. Idempotent: a
/// second call with the same bytes reports `was_written: false` and
/// does NOT touch the file.
///
/// Creates the directory if missing.
///
/// # Errors
///
/// Propagates the underlying filesystem error from `create_dir_all` /
/// `write`. Does not panic on duplicate writes — the existence check
/// is the dedup short-circuit, not the failure path.
pub fn import_bytes(bytes: &[u8], ext: &str, images_dir: &Path) -> io::Result<ImportedImage> {
    let normalised_ext = normalise_extension(ext);
    let hash = fnv1a_64(bytes);
    let filename = format!("{}.{}", hash.as_filename_stem(), normalised_ext);
    let markdown_reference = format!("images/{filename}");
    let absolute_path = images_dir.join(&filename);

    fs::create_dir_all(images_dir)?;

    let was_written = if absolute_path.exists() {
        false
    } else {
        fs::write(&absolute_path, bytes)?;
        true
    };

    Ok(ImportedImage {
        hash,
        markdown_reference,
        absolute_path,
        was_written,
    })
}

/// Import an image file from `source_path` into the shared store. Reads
/// the bytes from disk, derives the extension from `source_path`, and
/// delegates to [`import_bytes`]. Used by the drag-drop branch.
///
/// # Errors
///
/// Propagates `read` and `import_bytes` filesystem errors.
pub fn import_path(source_path: &Path, images_dir: &Path) -> io::Result<ImportedImage> {
    let bytes = fs::read(source_path)?;
    let ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    import_bytes(&bytes, ext, images_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fnv1a_64_known_vector_empty() {
        // FNV-1a 64 of the empty string is the offset basis.
        assert_eq!(fnv1a_64(b"").0, FNV_OFFSET_BASIS_64);
    }

    #[test]
    fn fnv1a_64_distinguishes_inputs() {
        assert_ne!(fnv1a_64(b"abc").0, fnv1a_64(b"abd").0);
    }

    #[test]
    fn import_writes_file_with_expected_relative_reference() {
        let dir = tempdir().expect("tempdir");
        let bytes = b"fake-png-bytes-\x89PNG";
        let result = import_bytes(bytes, "png", dir.path()).expect("import");

        let expected_stem = fnv1a_64(bytes).as_filename_stem();
        assert_eq!(
            result.markdown_reference,
            format!("images/{expected_stem}.png")
        );
        assert!(result.was_written);
        assert!(result.absolute_path.exists());
        let on_disk = fs::read(&result.absolute_path).expect("read");
        assert_eq!(on_disk, bytes);
    }

    #[test]
    fn import_is_idempotent_on_duplicate_bytes() {
        let dir = tempdir().expect("tempdir");
        let bytes = b"same-bytes";
        let first = import_bytes(bytes, "png", dir.path()).expect("first");
        assert!(first.was_written);

        let second = import_bytes(bytes, "png", dir.path()).expect("second");
        assert!(!second.was_written);
        assert_eq!(first.markdown_reference, second.markdown_reference);
        assert_eq!(first.absolute_path, second.absolute_path);
    }

    #[test]
    fn import_normalises_dotted_extension() {
        let dir = tempdir().expect("tempdir");
        let result = import_bytes(b"x", ".PNG", dir.path()).expect("import");
        assert!(result.markdown_reference.ends_with(".png"));
    }

    #[test]
    fn import_creates_missing_directory() {
        let parent = tempdir().expect("tempdir");
        let nested = parent.path().join("nested").join("images");
        assert!(!nested.exists());
        let result = import_bytes(b"x", "png", &nested).expect("import");
        assert!(result.absolute_path.exists());
        assert!(nested.is_dir());
    }

    #[test]
    fn import_path_reads_and_dedups() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("input.PNG");
        fs::write(&source, b"payload").expect("write source");

        let images_dir = dir.path().join("store");
        let first = import_path(&source, &images_dir).expect("first");
        assert!(first.was_written);
        assert!(first.markdown_reference.ends_with(".png"));

        let second = import_path(&source, &images_dir).expect("second");
        assert!(!second.was_written);
        assert_eq!(first.markdown_reference, second.markdown_reference);
    }

    #[test]
    fn normalise_extension_handles_pathological_input() {
        assert_eq!(normalise_extension(""), "bin");
        assert_eq!(normalise_extension("."), "bin");
        assert_eq!(normalise_extension(".png"), "png");
        assert_eq!(normalise_extension("PNG"), "png");
        assert_eq!(normalise_extension("a/b"), "bin");
        assert_eq!(normalise_extension("a b"), "bin");
    }

    #[test]
    fn supported_extension_predicate_matches_documented_set() {
        assert!(is_supported_image_extension("png"));
        assert!(is_supported_image_extension(".JPG"));
        assert!(is_supported_image_extension("jpeg"));
        assert!(is_supported_image_extension("webp"));
        assert!(is_supported_image_extension("bmp"));
        assert!(!is_supported_image_extension("md"));
        assert!(!is_supported_image_extension("svg"));
    }
}
