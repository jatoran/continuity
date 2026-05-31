//! Bounded directory listing for the file tree.
//!
//! Directory enumeration runs on the file-I/O worker thread. It is
//! intentionally shallow: one requested directory is listed, sorted,
//! capped, and returned to the UI. The UI decides which directory to
//! expand next, so opening a huge repository never recursively walks it.

use std::cmp::Ordering;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Maximum entries returned for one directory expansion.
pub(crate) const DIRECTORY_LIST_MAX_ENTRIES: usize = 512;
const DIRECTORY_SCAN_MAX_ENTRIES: usize = 4096;

/// One listed filesystem entry under an opened folder root.
#[derive(Clone, Debug)]
pub struct DirectoryEntry {
    /// Relative path from the opened root.
    pub relative: PathBuf,
    /// Display name for the entry.
    pub name: String,
    /// Entry kind.
    pub kind: DirectoryEntryKind,
    /// File size when known. Directories carry `None`.
    pub size_bytes: Option<u64>,
}

/// File-tree entry kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryEntryKind {
    /// Directory entry.
    Directory,
    /// Regular file entry.
    File,
}

/// Bounded listing result.
#[derive(Clone, Debug)]
pub(crate) struct DirectoryListing {
    pub(crate) root: PathBuf,
    pub(crate) relative: PathBuf,
    pub(crate) entries: Vec<DirectoryEntry>,
    pub(crate) truncated: bool,
}

/// Read one directory below `root`.
pub(crate) fn read_directory(root: &Path, relative: &Path) -> io::Result<DirectoryListing> {
    if !is_safe_relative(relative) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory path escapes the opened root",
        ));
    }
    let root = root.canonicalize()?;
    let target = if relative.as_os_str().is_empty() {
        root.clone()
    } else {
        root.join(relative)
    };
    let target = target.canonicalize()?;
    if !target.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory path escapes the opened root",
        ));
    }

    let mut entries = Vec::new();
    let mut truncated = false;
    for (scanned, entry) in std::fs::read_dir(&target)?.enumerate() {
        if scanned >= DIRECTORY_SCAN_MAX_ENTRIES {
            truncated = true;
            break;
        }
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !(file_type.is_dir() || file_type.is_file()) {
            continue;
        }
        if file_type.is_symlink() {
            continue;
        }
        let name_os = entry.file_name();
        let name = name_os.to_string_lossy().to_string();
        if file_type.is_dir() && should_ignore_directory(&name) {
            continue;
        }
        if entries.len() >= DIRECTORY_LIST_MAX_ENTRIES {
            truncated = true;
            break;
        }
        let kind = if file_type.is_dir() {
            DirectoryEntryKind::Directory
        } else {
            DirectoryEntryKind::File
        };
        let size_bytes = if kind == DirectoryEntryKind::File {
            entry.metadata().ok().map(|metadata| metadata.len())
        } else {
            None
        };
        entries.push(DirectoryEntry {
            relative: relative.join(PathBuf::from(name_os)),
            name,
            kind,
            size_bytes,
        });
    }
    entries.sort_by(compare_entries);
    Ok(DirectoryListing {
        root,
        relative: relative.to_path_buf(),
        entries,
        truncated,
    })
}

fn is_safe_relative(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
        || path.as_os_str().is_empty()
}

fn compare_entries(left: &DirectoryEntry, right: &DirectoryEntry) -> Ordering {
    match (left.kind, right.kind) {
        (DirectoryEntryKind::Directory, DirectoryEntryKind::File) => Ordering::Less,
        (DirectoryEntryKind::File, DirectoryEntryKind::Directory) => Ordering::Greater,
        _ => left.name.to_lowercase().cmp(&right.name.to_lowercase()),
    }
}

fn should_ignore_directory(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git"
            | ".hg"
            | ".svn"
            | ".cache"
            | ".mypy_cache"
            | ".pytest_cache"
            | ".ruff_cache"
            | ".next"
            | ".nuxt"
            | ".turbo"
            | ".venv"
            | ".vs"
            | "__pycache__"
            | "build"
            | "coverage"
            | "dist"
            | "node_modules"
            | "target"
            | "venv"
    )
}
