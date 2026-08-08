//! Bounded, config-aware directory listing for the file tree.
//!
//! Enumeration stays shallow and runs only on the file-I/O worker.

use std::cmp::Ordering;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use continuity_config::{VaultConfig, VaultSort};

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

struct EntryCandidate {
    entry: DirectoryEntry,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

/// Read one directory below `root`, applying optional vault policy.
pub(crate) fn read_directory(
    root: &Path,
    relative: &Path,
    config: Option<&VaultConfig>,
) -> io::Result<DirectoryListing> {
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

    let mut candidates = Vec::new();
    let mut truncated = false;
    for (scanned, entry) in std::fs::read_dir(&target)?.enumerate() {
        if scanned >= DIRECTORY_SCAN_MAX_ENTRIES {
            truncated = true;
            break;
        }
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !(file_type.is_dir() || file_type.is_file()) || file_type.is_symlink() {
            continue;
        }
        let name_os = entry.file_name();
        let name = name_os.to_string_lossy().to_string();
        if file_type.is_dir() && should_ignore_directory(&name) {
            continue;
        }
        let entry_relative = relative.join(PathBuf::from(&name_os));
        if config.is_some_and(|vault| vault.is_path_ignored(&entry_relative)) {
            continue;
        }
        if candidates.len() >= DIRECTORY_LIST_MAX_ENTRIES {
            truncated = true;
            break;
        }
        let kind = if file_type.is_dir() {
            DirectoryEntryKind::Directory
        } else {
            DirectoryEntryKind::File
        };
        let metadata = entry.metadata().ok();
        let size_bytes = if kind == DirectoryEntryKind::File {
            metadata.as_ref().map(std::fs::Metadata::len)
        } else {
            None
        };
        candidates.push(EntryCandidate {
            entry: DirectoryEntry {
                relative: entry_relative,
                name,
                kind,
                size_bytes,
            },
            modified: metadata.as_ref().and_then(|value| value.modified().ok()),
            created: metadata.as_ref().and_then(|value| value.created().ok()),
        });
    }
    apply_display_names(&mut candidates, config);
    candidates.sort_by(|left, right| compare_entries(left, right, config));
    Ok(DirectoryListing {
        root,
        relative: relative.to_path_buf(),
        entries: candidates
            .into_iter()
            .map(|candidate| candidate.entry)
            .collect(),
        truncated,
    })
}

fn apply_display_names(candidates: &mut [EntryCandidate], config: Option<&VaultConfig>) {
    if !config.is_some_and(|vault| vault.files.hide_markdown_extensions) {
        return;
    }
    let proposed: Vec<String> = candidates
        .iter()
        .map(|candidate| markdown_display_name(&candidate.entry.name))
        .collect();
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for name in &proposed {
        *counts.entry(name.to_ascii_lowercase()).or_default() += 1;
    }
    for (candidate, proposed_name) in candidates.iter_mut().zip(proposed) {
        if counts
            .get(&proposed_name.to_ascii_lowercase())
            .copied()
            .unwrap_or_default()
            == 1
        {
            candidate.entry.name = proposed_name;
        }
    }
}

fn markdown_display_name(name: &str) -> String {
    let path = Path::new(name);
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .map_or_else(|| name.to_string(), str::to_string)
    } else {
        name.to_string()
    }
}

fn compare_entries(
    left: &EntryCandidate,
    right: &EntryCandidate,
    config: Option<&VaultConfig>,
) -> Ordering {
    let kind_order = if config.is_none_or(|vault| vault.files.folders_first) {
        match (left.entry.kind, right.entry.kind) {
            (DirectoryEntryKind::Directory, DirectoryEntryKind::File) => Ordering::Less,
            (DirectoryEntryKind::File, DirectoryEntryKind::Directory) => Ordering::Greater,
            _ => Ordering::Equal,
        }
    } else {
        Ordering::Equal
    };
    if kind_order != Ordering::Equal {
        return kind_order;
    }
    let mut order = match config.map(|vault| vault.files.sort) {
        Some(VaultSort::Modified) => compare_timestamp(left.modified, right.modified),
        Some(VaultSort::Created) => compare_timestamp(left.created, right.created),
        Some(VaultSort::Name) | None => Ordering::Equal,
    };
    if order == Ordering::Equal {
        order = left
            .entry
            .name
            .to_lowercase()
            .cmp(&right.entry.name.to_lowercase());
    }
    if config.is_some_and(|vault| vault.files.descending) {
        order.reverse()
    } else {
        order
    }
}

fn compare_timestamp(left: Option<SystemTime>, right: Option<SystemTime>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn is_safe_relative(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
        || path.as_os_str().is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_extensions_hide_without_display_collisions() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::write(directory.path().join("alpha.md"), "a").expect("markdown file");
        std::fs::write(directory.path().join("beta.txt"), "b").expect("text file");
        let listing = read_directory(
            directory.path(),
            Path::new(""),
            Some(&VaultConfig::default()),
        )
        .expect("listing");
        let names: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec!["alpha", "beta.txt"]);
    }

    #[test]
    fn markdown_extension_is_revealed_on_collision() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::write(directory.path().join("same.md"), "a").expect("markdown file");
        std::fs::write(directory.path().join("same"), "b").expect("extensionless file");
        let listing = read_directory(
            directory.path(),
            Path::new(""),
            Some(&VaultConfig::default()),
        )
        .expect("listing");
        let names: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec!["same", "same.md"]);
    }
}
