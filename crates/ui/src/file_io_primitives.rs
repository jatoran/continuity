//! File-I/O primitives: read / write / decode / watch / fingerprint
//! helpers used by the Phase-15 worker in [`crate::file_io`].
//!
//! Pulled out of `file_io.rs` to keep that module under the 600-line
//! cap. All helpers are pure functions or thin wrappers around
//! `std::fs` / `notify`; no shared state lives here.
//!
//! Thread ownership: every helper is callable from any thread, but in
//! practice they are only invoked from the file-I/O worker thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use continuity_buffer::{BufferId, FileAssociation};
use crossbeam_channel::Sender;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::file_io::FileIoEvent;

/// Result of [`read_file`].
pub(crate) struct ReadFileResult {
    pub(crate) content: String,
    pub(crate) file: FileAssociation,
    /// δ.3 — when `Some`, the on-disk bytes were NOT clean UTF-8 and
    /// the returned `content` contains U+FFFD replacement chars.
    /// The string is a short label for the detected encoding so the
    /// banner can name it specifically (`"UTF-16 LE"`, `"UTF-16 BE"`,
    /// or `"non-UTF-8"`).
    pub(crate) encoding_notice: Option<&'static str>,
}

pub(crate) fn read_file(path: &Path) -> std::io::Result<ReadFileResult> {
    let bytes = std::fs::read(path)?;
    let (content, encoding_notice) = decode_file_bytes(&bytes);
    let meta = std::fs::metadata(path)?;
    let mtime_ms = meta.modified().ok().and_then(system_time_ms).unwrap_or(0);
    // Hash with the SAME function the dirty-state check uses
    // (`continuity_persist::fnv1a_64`), never a second local FNV-1a: the
    // worker's `content_hash` is compared byte-for-byte against
    // `continuity_persist::fnv1a_64_chunks(rope)` in `is_tab_dirty`, so a
    // divergent prime would make every saved buffer read permanently dirty.
    let raw_hash = continuity_persist::fnv1a_64(&bytes);
    let content_hash = continuity_persist::fnv1a_64(content.as_bytes());
    let file = FileAssociation::new(path.to_path_buf(), mtime_ms, raw_hash)
        .with_content_hash(content_hash);
    Ok(ReadFileResult {
        content,
        file,
        encoding_notice,
    })
}

/// δ.3 — decode file bytes into a UTF-8 `String`, applying conservative
/// encoding sniffs:
/// 1. UTF-8 BOM (`EF BB BF`) is stripped.
/// 2. UTF-16 LE / BE BOMs trigger a UTF-16 decode (replacement on
///    invalid code units).
/// 3. Otherwise: try strict UTF-8; on failure, fall back to
///    `from_utf8_lossy` and report `"non-UTF-8"`.
///
/// Returns `(content, encoding_notice)`. `encoding_notice == None`
/// means the bytes decoded cleanly as UTF-8.
pub(crate) fn decode_file_bytes(bytes: &[u8]) -> (String, Option<&'static str>) {
    // UTF-16 LE BOM: FF FE.
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        return (
            decode_utf16(&bytes[2..], /*little_endian=*/ true),
            Some("UTF-16 LE"),
        );
    }
    // UTF-16 BE BOM: FE FF.
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        return (
            decode_utf16(&bytes[2..], /*little_endian=*/ false),
            Some("UTF-16 BE"),
        );
    }
    // UTF-8 BOM: EF BB BF — strip silently, this is still UTF-8.
    let text_bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(text_bytes) {
        Ok(s) => (s.to_string(), None),
        Err(_) => (
            String::from_utf8_lossy(text_bytes).into_owned(),
            Some("non-UTF-8"),
        ),
    }
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let unit = if little_endian {
            u16::from_le_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], chunk[1]])
        };
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

pub(crate) fn write_file(path: &Path, content: &str) -> std::io::Result<FileAssociation> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content.as_bytes())?;
    let meta = std::fs::metadata(path)?;
    let mtime_ms = meta.modified().ok().and_then(system_time_ms).unwrap_or(0);
    let content_hash = continuity_persist::fnv1a_64(content.as_bytes());
    Ok(
        FileAssociation::new(path.to_path_buf(), mtime_ms, content_hash)
            .with_content_hash(content_hash),
    )
}

pub(crate) fn install_watch(
    watcher: Option<&mut RecommendedWatcher>,
    watched_dirs: &mut HashSet<PathBuf>,
    file: &Path,
) {
    let Some(watcher) = watcher else {
        return;
    };
    let Some(parent) = file.parent() else {
        return;
    };
    let dir = normalize_path(parent);
    if watched_dirs.contains(&dir) {
        return;
    }
    if watcher.watch(parent, RecursiveMode::NonRecursive).is_ok() {
        watched_dirs.insert(dir);
    }
}

pub(crate) fn send_failed(
    event_tx: &Sender<FileIoEvent>,
    operation: &'static str,
    buffer_id: Option<BufferId>,
    path: Option<PathBuf>,
    error: std::io::Error,
) {
    let _ = event_tx.send(FileIoEvent::Failed {
        buffer_id,
        operation,
        path,
        reason: error.to_string(),
    });
}

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    // Canonicalize via the parent directory then re-attach the file
    // name so the result is stable across the file's lifecycle: if we
    // canonicalize the leaf directly, the call fails once the file is
    // deleted and we fall back to the literal path — producing a
    // different key than the one inserted when the file still existed.
    // The parent dir survives the deletion, so canonicalizing it
    // yields a consistent key for insert + lookup + delete-notify.
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(canonical_parent) = parent.canonicalize() {
            return canonical_parent.join(name);
        }
    }
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Spec §13 self-write suppression. After `file.save` we update the
/// `watched` entry to the expected post-save `(mtime, hash)` fingerprint;
/// any subsequent `ReadDirectoryChangesW` event whose on-disk read matches
/// that fingerprint is the editor's own write coming back through the
/// watcher and must not produce an `ExternalChanged` event.
pub(crate) fn is_self_write(observed: &FileAssociation, expected: &FileAssociation) -> bool {
    observed.hash == expected.hash && observed.mtime_ms == expected.mtime_ms
}

fn system_time_ms(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
}
