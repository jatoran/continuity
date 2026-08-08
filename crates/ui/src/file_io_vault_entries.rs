//! Contained create, move, and recycle operations for vault entries.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use continuity_config::VAULT_CONFIG_DIRECTORY;
use windows::core::PCWSTR;
use windows::Win32::UI::Shell::{
    SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
};

use crate::file_io::VaultEntryKind;

pub(crate) fn create_entry(
    root: &Path,
    parent: &Path,
    kind: VaultEntryKind,
) -> io::Result<PathBuf> {
    let (root, parent_path) = contained_directory(root, parent)?;
    let (stem, extension) = match kind {
        VaultEntryKind::File => ("Untitled", ".md"),
        VaultEntryKind::Directory => ("New Folder", ""),
    };
    for number in 1..10_000 {
        let suffix = if number == 1 {
            String::new()
        } else {
            format!(" {number}")
        };
        let name = format!("{stem}{suffix}{extension}");
        let path = parent_path.join(&name);
        let result = match kind {
            VaultEntryKind::File => std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map(drop),
            VaultEntryKind::Directory => std::fs::create_dir(&path),
        };
        match result {
            Ok(()) => {
                return path
                    .strip_prefix(&root)
                    .map(Path::to_path_buf)
                    .map_err(io::Error::other)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unique default entry name is available",
    ))
}

pub(crate) fn move_entry(
    root: &Path,
    source: &Path,
    destination_directory: &Path,
) -> io::Result<(PathBuf, PathBuf)> {
    let root = root.canonicalize()?;
    validate_relative(source)?;
    validate_relative(destination_directory)?;
    protect_config(source)?;
    let source_path = root.join(source).canonicalize()?;
    let destination_directory_path = root.join(destination_directory).canonicalize()?;
    if !source_path.starts_with(&root)
        || !destination_directory_path.starts_with(&root)
        || !destination_directory_path.is_dir()
    {
        return Err(permission_error());
    }
    if source_path.is_dir() && destination_directory_path.starts_with(&source_path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a folder cannot be moved into itself",
        ));
    }
    let name = source_path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "entry has no name"))?;
    let destination_path = destination_directory_path.join(name);
    if destination_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination already exists",
        ));
    }
    std::fs::rename(&source_path, &destination_path)?;
    let destination = destination_path
        .strip_prefix(&root)
        .map(Path::to_path_buf)
        .map_err(io::Error::other)?;
    Ok((source.to_path_buf(), destination))
}

pub(crate) fn rename_entry(
    root: &Path,
    source: &Path,
    new_name: &str,
) -> io::Result<(PathBuf, PathBuf)> {
    validate_entry_name(new_name)?;
    let root = root.canonicalize()?;
    validate_relative(source)?;
    protect_config(source)?;
    let source_path = root.join(source).canonicalize()?;
    if !source_path.starts_with(&root) || source_path == root {
        return Err(permission_error());
    }
    let parent = source_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "entry has no parent"))?;
    let destination_path = parent.join(new_name);
    if destination_path == source_path {
        return Ok((source.to_path_buf(), source.to_path_buf()));
    }
    let is_case_only = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(new_name));
    if destination_path.exists() && !is_case_only {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "an entry with that name already exists",
        ));
    }
    std::fs::rename(&source_path, &destination_path)?;
    let destination = destination_path
        .strip_prefix(&root)
        .map(Path::to_path_buf)
        .map_err(io::Error::other)?;
    Ok((source.to_path_buf(), destination))
}

pub(crate) fn recycle_entry(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let root = root.canonicalize()?;
    validate_relative(relative)?;
    protect_config(relative)?;
    let path = root.join(relative).canonicalize()?;
    if !path.starts_with(&root) || path == root {
        return Err(permission_error());
    }
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.push(0);
    from.push(0);
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut operation) };
    if code != 0 || operation.fAnyOperationsAborted.as_bool() {
        return Err(io::Error::other(format!(
            "Recycle Bin operation failed ({code})"
        )));
    }
    Ok(relative.to_path_buf())
}

fn contained_directory(root: &Path, relative: &Path) -> io::Result<(PathBuf, PathBuf)> {
    validate_relative(relative)?;
    let root = root.canonicalize()?;
    let directory = root.join(relative).canonicalize()?;
    if !directory.starts_with(&root) || !directory.is_dir() {
        return Err(permission_error());
    }
    Ok((root, directory))
}

fn validate_relative(relative: &Path) -> io::Result<()> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        Ok(())
    } else {
        Err(permission_error())
    }
}

pub(crate) fn validate_entry_name(name: &str) -> io::Result<()> {
    let is_single_component = !name.trim().is_empty()
        && Path::new(name)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && !name.contains('/')
        && !name.contains('\\');
    let has_invalid_windows_character = name.chars().any(|character| {
        character <= '\u{1f}' || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
    });
    if !is_single_component
        || has_invalid_windows_character
        || name.ends_with('.')
        || name.ends_with(' ')
        || is_reserved_windows_name(name)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "name is empty or contains characters Windows filenames cannot use",
        ));
    }
    Ok(())
}

fn is_reserved_windows_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

fn protect_config(relative: &Path) -> io::Result<()> {
    if relative
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == VAULT_CONFIG_DIRECTORY)
    {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the .continuity directory is protected",
        ))
    } else {
        Ok(())
    }
}

fn permission_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "entry path escapes the vault root",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_move_are_contained_and_collision_safe() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::create_dir(directory.path().join("target")).expect("target folder");
        let first = create_entry(directory.path(), Path::new(""), VaultEntryKind::File)
            .expect("first note");
        let second = create_entry(directory.path(), Path::new(""), VaultEntryKind::File)
            .expect("second note");
        assert_ne!(first, second);
        let (_, moved) =
            move_entry(directory.path(), &first, Path::new("target")).expect("move note");
        assert!(directory.path().join(moved).is_file());
    }

    #[test]
    fn config_directory_and_escape_are_rejected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::create_dir(directory.path().join(VAULT_CONFIG_DIRECTORY))
            .expect("config directory");
        assert!(move_entry(
            directory.path(),
            Path::new(VAULT_CONFIG_DIRECTORY),
            Path::new("")
        )
        .is_err());
        assert!(create_entry(directory.path(), Path::new(".."), VaultEntryKind::File).is_err());
    }

    #[test]
    fn rename_file_and_directory_stays_contained() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::write(directory.path().join("old.md"), "note").expect("note");
        std::fs::create_dir(directory.path().join("old-folder")).expect("folder");
        let (_, file) =
            rename_entry(directory.path(), Path::new("old.md"), "new.md").expect("rename file");
        let (_, folder) = rename_entry(directory.path(), Path::new("old-folder"), "new-folder")
            .expect("rename folder");
        assert_eq!(file, PathBuf::from("new.md"));
        assert_eq!(folder, PathBuf::from("new-folder"));
        assert!(directory.path().join(file).is_file());
        assert!(directory.path().join(folder).is_dir());
    }

    #[test]
    fn rename_rejects_invalid_reserved_and_colliding_names() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::write(directory.path().join("source.md"), "source").expect("source");
        std::fs::write(directory.path().join("taken.md"), "taken").expect("taken");
        for invalid in [
            "",
            "../escape.md",
            "bad/name.md",
            "bad?.md",
            "bad\u{1f}.md",
            "CON.txt",
        ] {
            assert!(rename_entry(directory.path(), Path::new("source.md"), invalid).is_err());
        }
        assert!(rename_entry(directory.path(), Path::new("source.md"), "taken.md").is_err());
    }
}
