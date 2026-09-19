//! Download, verify, and stage the install of a newer release.
//!
//! Every install kind ends the same way: a `.cmd` script in the temp
//! directory that waits for this process to exit, performs the install,
//! and relaunches the executable at its current path. The script is
//! spawned detached before the windows are asked to close, so the app
//! never has to outlive its own replacement.
//!
//! - **MSI**: `msiexec /i <msi> /passive /norestart`. The package is
//!   per-machine, so Windows Installer raises the UAC consent itself;
//!   `MajorUpgrade` replaces the installed files and keeps shortcuts and
//!   file associations. winget installs the same MSI and tracks the
//!   Add/Remove Programs entry, so a later `winget upgrade` still agrees.
//! - **Portable / standalone**: the standalone zip's `continuity.exe` is
//!   extracted with PowerShell's `Expand-Archive`, then the script renames
//!   the running executable to `.old`, moves the new one into place, and
//!   deletes the old copy once the new one has started. `data\` is never
//!   touched.
//!
//! Thread ownership: called on an update worker thread; blocks on the
//! downloads and the archive extraction.

use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::github::{fetch_asset, ReleaseInfo};
use super::{checksum, InstallKind, UpdateError};

/// `CREATE_NO_WINDOW` — the helper script must not flash a console.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `DETACHED_PROCESS` — the script outlives this process.
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// Fetch the right asset for `kind`, verify it, and launch the staged
/// installer script. Returns once the script is running; the caller then
/// closes every window.
pub(crate) fn download_and_stage(
    kind: InstallKind,
    exe_path: &Path,
    release: &ReleaseInfo,
) -> Result<(), UpdateError> {
    let staging = staging_dir(&release.version)?;
    let sums_url = release
        .sums_url
        .as_deref()
        .ok_or_else(|| UpdateError::Release("release has no SHA256SUMS.txt".into()))?;
    let sums = String::from_utf8_lossy(&fetch_asset(sums_url)?).into_owned();
    let script = match kind {
        InstallKind::Msi => {
            let name = format!("continuity-{}-setup.msi", release.version);
            let msi = fetch_verified(release.msi_url.as_deref(), &name, &sums, &staging)?;
            msi_script(exe_path, &msi)
        }
        InstallKind::Portable | InstallKind::Standalone => {
            let name = format!("continuity-{}-standalone.zip", release.version);
            let zip = fetch_verified(
                release.standalone_zip_url.as_deref(),
                &name,
                &sums,
                &staging,
            )?;
            let extracted = staging.join("extracted");
            expand_archive(&zip, &extracted)?;
            let new_exe = find_file(&extracted, "continuity.exe").ok_or_else(|| {
                UpdateError::Release(format!("{name} does not contain continuity.exe"))
            })?;
            swap_script(exe_path, &new_exe)
        }
    };
    let script_path = staging.join("apply-update.cmd");
    fs::write(&script_path, script)?;
    Command::new("cmd.exe")
        .arg("/c")
        .arg(&script_path)
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()?;
    Ok(())
}

fn staging_dir(version: &str) -> Result<PathBuf, UpdateError> {
    let dir = std::env::temp_dir().join(format!("continuity-update-{version}"));
    if dir.exists() {
        // A previous attempt's leftovers must not be trusted or reused.
        let _ = fs::remove_dir_all(&dir);
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn fetch_verified(
    url: Option<&str>,
    name: &str,
    sums: &str,
    staging: &Path,
) -> Result<PathBuf, UpdateError> {
    let url = url.ok_or_else(|| UpdateError::Release(format!("release has no {name}")))?;
    let bytes = fetch_asset(url)?;
    checksum::verify(sums, name, &bytes)?;
    let path = staging.join(name);
    fs::write(&path, bytes)?;
    Ok(path)
}

fn expand_archive(zip: &Path, destination: &Path) -> Result<(), UpdateError> {
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ])
        .arg(format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            zip.display(),
            destination.display()
        ))
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if !status.success() {
        return Err(UpdateError::Release(format!(
            "could not extract {} (Expand-Archive exit {status})",
            zip.display()
        )));
    }
    Ok(())
}

fn find_file(root: &Path, file_name: &str) -> Option<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case(file_name))
            {
                return Some(path);
            }
        }
    }
    None
}

/// Wait for this process to exit, then run the MSI and relaunch.
fn msi_script(exe_path: &Path, msi: &Path) -> String {
    format!(
        "@echo off\r\n{wait}\r\nmsiexec.exe /i \"{msi}\" /passive /norestart\r\n\
         if exist \"{exe}\" start \"\" \"{exe}\"\r\n",
        wait = wait_for_exit_lines(),
        msi = msi.display(),
        exe = exe_path.display(),
    )
}

/// Wait for this process to exit, swap the executable, relaunch, tidy.
fn swap_script(exe_path: &Path, new_exe: &Path) -> String {
    let old = format!("{}.old", exe_path.display());
    format!(
        "@echo off\r\n{wait}\r\nif exist \"{old}\" del /f /q \"{old}\"\r\n\
         move /y \"{exe}\" \"{old}\" >nul\r\nmove /y \"{new}\" \"{exe}\" >nul\r\n\
         start \"\" \"{exe}\"\r\ntimeout /t 3 /nobreak >nul\r\ndel /f /q \"{old}\" >nul 2>&1\r\n",
        wait = wait_for_exit_lines(),
        exe = exe_path.display(),
        new = new_exe.display(),
    )
}

/// Batch loop that returns once this process id is gone.
fn wait_for_exit_lines() -> String {
    let pid = std::process::id();
    format!(
        ":wait\r\ntasklist /FI \"PID eq {pid}\" /NH 2>nul | find \" {pid} \" >nul\r\n\
         if not errorlevel 1 (timeout /t 1 /nobreak >nul & goto wait)"
    )
}

#[cfg(test)]
mod tests {
    use super::{find_file, msi_script, swap_script};
    use std::path::Path;

    #[test]
    fn scripts_wait_for_this_process_then_install_and_relaunch() {
        let exe = Path::new("C:\\Program Files\\Continuity\\continuity.exe");
        let msi = msi_script(exe, Path::new("C:\\tmp\\continuity-0.4.12-setup.msi"));
        assert!(msi.contains(&format!("PID eq {}", std::process::id())));
        assert!(msi.contains(
            "msiexec.exe /i \"C:\\tmp\\continuity-0.4.12-setup.msi\" /passive /norestart"
        ));
        assert!(msi.contains("start \"\" \"C:\\Program Files\\Continuity\\continuity.exe\""));

        let swap = swap_script(
            Path::new("D:\\apps\\continuity.exe"),
            Path::new("C:\\tmp\\new\\continuity.exe"),
        );
        assert!(
            swap.contains("move /y \"D:\\apps\\continuity.exe\" \"D:\\apps\\continuity.exe.old\"")
        );
        assert!(
            swap.contains("move /y \"C:\\tmp\\new\\continuity.exe\" \"D:\\apps\\continuity.exe\"")
        );
        assert!(swap.contains("start \"\" \"D:\\apps\\continuity.exe\""));
    }

    #[test]
    fn finds_the_executable_anywhere_under_the_extraction_root() {
        let root = tempfile::tempdir().expect("tempdir");
        let nested = root.path().join("continuity-0.4.12-standalone");
        std::fs::create_dir_all(&nested).expect("nested");
        std::fs::write(nested.join("Continuity.exe"), b"x").expect("exe");
        let found = find_file(root.path(), "continuity.exe").expect("found");
        assert!(found.ends_with("Continuity.exe"));
        assert!(find_file(root.path(), "missing.exe").is_none());
    }
}
