//! Native Windows desktop shortcut creation for vault roots.

use std::path::{Path, PathBuf};

use windows::core::{Interface, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, IShellLinkW, SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT,
};

pub(crate) fn create_vault_desktop_shortcut(root: &Path) -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let desktop = known_desktop_path()?;
    let name = root
        .components()
        .next_back()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_else(|| "Vault".into());
    let shortcut_path = collision_safe_shortcut_path(&desktop, &sanitize_shortcut_name(&name));
    let link: IShellLinkW = unsafe {
        CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| error.to_string())?
    };
    let executable_wide = wide_null(&executable.to_string_lossy());
    let arguments_wide = wide_null(&format!("--vault \"{}\"", root.display()));
    let working_wide = wide_null(&root.to_string_lossy());
    unsafe {
        link.SetPath(PCWSTR(executable_wide.as_ptr()))
            .map_err(|error| error.to_string())?;
        link.SetArguments(PCWSTR(arguments_wide.as_ptr()))
            .map_err(|error| error.to_string())?;
        link.SetWorkingDirectory(PCWSTR(working_wide.as_ptr()))
            .map_err(|error| error.to_string())?;
        link.SetIconLocation(PCWSTR(executable_wide.as_ptr()), 0)
            .map_err(|error| error.to_string())?;
    }
    let persist: IPersistFile = link.cast().map_err(|error| error.to_string())?;
    let shortcut_wide = wide_null(&shortcut_path.to_string_lossy());
    unsafe {
        persist
            .Save(PCWSTR(shortcut_wide.as_ptr()), true)
            .map_err(|error| error.to_string())?;
    }
    Ok(shortcut_path)
}

fn known_desktop_path() -> Result<PathBuf, String> {
    let path = unsafe {
        SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None)
            .map_err(|error| error.to_string())?
    };
    let text = unsafe {
        PCWSTR(path.0)
            .to_string()
            .map_err(|error| error.to_string())?
    };
    unsafe { CoTaskMemFree(Some(path.0.cast())) };
    Ok(text.into())
}

fn collision_safe_shortcut_path(desktop: &Path, vault_name: &str) -> PathBuf {
    let base = vault_name.to_string();
    let first = desktop.join(format!("{base}.lnk"));
    if !first.exists() {
        return first;
    }
    for suffix in 2..10_000 {
        let candidate = desktop.join(format!("{base} ({suffix}).lnk"));
        if !candidate.exists() {
            return candidate;
        }
    }
    desktop.join(format!("{base} — {}.lnk", std::process::id()))
}

fn sanitize_shortcut_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            other => other,
        })
        .collect();
    let trimmed = sanitized.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "Vault".into()
    } else {
        trimmed.into()
    }
}

fn wide_null(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_names_remove_windows_invalid_characters() {
        assert_eq!(sanitize_shortcut_name("work:notes?"), "work_notes_");
        assert_eq!(sanitize_shortcut_name("..."), "Vault");
    }

    #[test]
    fn first_shortcut_uses_readable_vault_name() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            collision_safe_shortcut_path(directory.path(), "Work"),
            directory.path().join("Work.lnk")
        );
    }

    #[test]
    fn existing_shortcut_uses_numbered_vault_name() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("Work.lnk"), []).unwrap();
        assert_eq!(
            collision_safe_shortcut_path(directory.path(), "Work"),
            directory.path().join("Work (2).lnk")
        );
    }
}
