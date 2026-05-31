//! Buffer metadata that is not part of the rope text.
//!
//! The editor core thread owns this metadata alongside the buffer rope.

use std::path::PathBuf;

/// Association between a buffer and an imported/exported filesystem path.
///
/// **Thread ownership**: stored on [`crate::Buffer`] and mutated only by the
/// editor core thread. Other threads receive cloned snapshots of this value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileAssociation {
    /// Path this buffer saves back to.
    pub path: PathBuf,
    /// Last observed file modification time, in Unix milliseconds.
    pub mtime_ms: i64,
    /// FNV-1a hash of the last imported/saved raw file bytes.
    pub hash: u64,
    /// FNV-1a hash of the decoded text content last imported/saved.
    pub content_hash: u64,
}

impl FileAssociation {
    /// Build a file association from path + observed metadata.
    #[must_use]
    pub fn new(path: PathBuf, mtime_ms: i64, hash: u64) -> Self {
        Self {
            path,
            mtime_ms,
            hash,
            content_hash: hash,
        }
    }

    /// Return this file association with an explicit decoded-content hash.
    #[must_use]
    pub fn with_content_hash(mut self, content_hash: u64) -> Self {
        self.content_hash = content_hash;
        self
    }
}
