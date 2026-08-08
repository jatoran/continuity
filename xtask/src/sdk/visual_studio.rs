//! Visual Studio and Windows SDK discovery for the checked C consumer.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

pub(super) struct VisualStudioToolchain {
    pub(super) compiler: PathBuf,
    pub(super) compiler_directory: PathBuf,
    pub(super) includes: Vec<PathBuf>,
    pub(super) libraries: Vec<PathBuf>,
}

pub(super) fn visual_studio_toolchain() -> Result<VisualStudioToolchain> {
    let program_files = env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("ProgramFiles(x86) is unavailable"))?;
    let installation = resolve_installation(&program_files)?;
    let msvc_root = newest_directory(&installation.join("VC/Tools/MSVC"))?;
    let compiler_directory = msvc_root.join("bin/Hostx64/x64");
    let compiler = compiler_directory.join("cl.exe");
    if !compiler.is_file() {
        bail!("MSVC compiler was not found at {}", compiler.display());
    }

    let windows_kit = program_files.join("Windows Kits/10");
    let kit_include = newest_directory(&windows_kit.join("Include"))?;
    let kit_version = kit_include
        .file_name()
        .ok_or_else(|| anyhow!("Windows SDK include version has no name"))?;
    let kit_library = windows_kit.join("Lib").join(kit_version);
    let includes = vec![
        msvc_root.join("include"),
        kit_include.join("ucrt"),
        kit_include.join("shared"),
        kit_include.join("um"),
    ];
    let libraries = vec![
        msvc_root.join("lib/x64"),
        kit_library.join("ucrt/x64"),
        kit_library.join("um/x64"),
    ];
    if includes.iter().chain(&libraries).any(|path| !path.is_dir()) {
        bail!("Visual Studio or Windows SDK include/library layout is incomplete");
    }
    Ok(VisualStudioToolchain {
        compiler,
        compiler_directory,
        includes,
        libraries,
    })
}

fn resolve_installation(program_files: &Path) -> Result<PathBuf> {
    let vswhere = program_files.join("Microsoft Visual Studio/Installer/vswhere.exe");
    if !vswhere.is_file() {
        bail!(
            "Visual Studio installer discovery tool was not found at {}",
            vswhere.display()
        );
    }
    let output = Command::new(&vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
            "-utf8",
        ])
        .output()
        .context("run Visual Studio installer discovery")?;
    if !output.status.success() {
        bail!(
            "Visual Studio installer discovery exited with {}",
            output.status
        );
    }
    parse_installation_path(&output.stdout)
}

fn parse_installation_path(stdout: &[u8]) -> Result<PathBuf> {
    let output = std::str::from_utf8(stdout).context("Visual Studio installation path is UTF-8")?;
    let path = output.trim();
    if path.is_empty() {
        bail!("Visual Studio with x64 C++ tools was not found");
    }
    Ok(PathBuf::from(path))
}

fn newest_directory(root: &Path) -> Result<PathBuf> {
    let mut directories: Vec<_> = fs::read_dir(root)
        .with_context(|| format!("read toolchain directory {}", root.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    directories.sort();
    directories
        .pop()
        .ok_or_else(|| anyhow!("no toolchain versions under {}", root.display()))
}

#[cfg(test)]
mod tests {
    use super::parse_installation_path;

    #[test]
    fn installation_path_trims_vswhere_line_ending() {
        let path = parse_installation_path(
            b"C:\\Program Files\\Microsoft Visual Studio\\18\\Enterprise\r\n",
        )
        .expect("invariant: path parses");
        assert_eq!(
            path.to_string_lossy(),
            "C:\\Program Files\\Microsoft Visual Studio\\18\\Enterprise"
        );
    }

    #[test]
    fn installation_path_rejects_missing_cpp_toolchain() {
        let error = parse_installation_path(b"\r\n").expect_err("empty output must fail");
        assert!(error.to_string().contains("x64 C++ tools"));
    }
}
