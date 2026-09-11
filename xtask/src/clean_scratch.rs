//! Sweep one-off scratch out of `target/` so build output stops accumulating.
//!
//! `target/` mixes two kinds of content. Cargo profile trees and tool-owned
//! caches are reused across runs and must survive. Everything else -- per-run
//! packaging roots, downloaded release bundles, hand-dumped CI logs, smoke-test
//! browser profiles -- is written once and never read again, and historically
//! grew without bound because nothing owned deleting it.
//!
//! This task removes only the second kind. `target/scratch/` is the sanctioned
//! home for ad-hoc dumps; anything placed there is disposable by definition.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::release::workspace_root;

/// Directory under `target/` that ad-hoc dumps belong in. Always swept whole.
const SCRATCH_DIR: &str = "scratch";

/// Name prefixes of leaked scratch that predates `target/scratch/`.
///
/// These are matched against entries directly under `target/`. Each one is a
/// location some task or session wrote a one-off artifact to; none is a cache
/// that anything reads back.
const SCRATCH_PREFIXES: &[&str] = &[
    // Per-run Electron packaging and smoke profiles.
    "desktop-web-smoke-",
    "desktop-web-installer-",
    "desktop-web-packaged-smoke-",
    "desktop-web-portable-data-",
    // Release bundles downloaded for verification, per the release playbook.
    "hosted-sdk-v",
    // Hand-dumped CI logs and job transcripts.
    "ci-",
    "check-all-job-",
    "linux-desktop-job-",
    "macos-job-",
    "native-sdk-job-",
    "wasm-sdk-job-",
    "windows-desktop-job-",
    // One-off milestone and bake-off evidence.
    "milestone-",
    "editor-bakeoff-",
    "m0-traces",
    "desktop-linux-evidence",
    "desktop-linux-gate",
    "cm-test",
];

/// Remove disposable scratch under `target/` and report what was reclaimed.
///
/// Pass `--dry-run` to list candidates without deleting. An entry that cannot
/// be removed (a live file lock, typically) is reported and skipped rather than
/// failing the task -- this is hygiene, not a gate.
pub(crate) fn run(args: &[String]) -> Result<()> {
    let is_dry_run = args.iter().any(|arg| arg == "--dry-run");
    let target = workspace_root().join("target");
    if !target.is_dir() {
        println!("clean-scratch: no target/ directory, nothing to do");
        return Ok(());
    }

    let mut removed_bytes = 0_u64;
    let mut removed_count = 0_usize;
    for path in collect_scratch_paths(&target)? {
        let size = directory_size(&path);
        if is_dry_run {
            println!("would remove {} ({})", path.display(), format_bytes(size));
            removed_bytes += size;
            removed_count += 1;
            continue;
        }
        match remove(&path) {
            Ok(()) => {
                println!("removed {} ({})", path.display(), format_bytes(size));
                removed_bytes += size;
                removed_count += 1;
            }
            Err(error) => eprintln!("clean-scratch: skipped {} ({error})", path.display()),
        }
    }

    let verb = if is_dry_run {
        "would reclaim"
    } else {
        "reclaimed"
    };
    println!(
        "clean-scratch: {verb} {} across {removed_count} entries",
        format_bytes(removed_bytes)
    );
    Ok(())
}

/// Gather every disposable path under `target/`, outermost first.
fn collect_scratch_paths(target: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();

    let scratch = target.join(SCRATCH_DIR);
    if scratch.exists() {
        paths.push(scratch);
    }

    for entry in fs::read_dir(target)?.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if SCRATCH_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            paths.push(path);
        }
    }

    // Per-run Forge output roots. `desktop-check` prunes these itself, but a
    // run killed partway through leaves them behind.
    let desktop_web = target.join("desktop-web");
    if desktop_web.is_dir() {
        for entry in fs::read_dir(&desktop_web)?.flatten() {
            let path = entry.path();
            let is_output_root = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("out-"));
            if is_output_root {
                paths.push(path);
            }
        }
    }

    paths.sort();
    Ok(paths)
}

/// Delete a file or directory tree.
fn remove(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Total bytes under `path`, counting the file itself when it is not a
/// directory. Unreadable entries contribute zero rather than aborting.
fn directory_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| directory_size(&entry.path()))
        .sum()
}

/// Render a byte count for a human reading task output.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_directory_and_prefixed_entries_are_collected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let target = temp.path();
        fs::create_dir_all(target.join(SCRATCH_DIR).join("nested"))?;
        fs::create_dir_all(target.join("hosted-sdk-v0.2.37"))?;
        fs::create_dir_all(target.join("desktop-web").join("out-windows-1234"))?;
        fs::write(target.join("ci-run-99.log"), b"log")?;

        // Reusable trees that must survive.
        fs::create_dir_all(target.join("debug"))?;
        fs::create_dir_all(target.join("release-small"))?;
        fs::create_dir_all(target.join("npm-cache"))?;

        let collected = collect_scratch_paths(target)?;
        let names: Vec<String> = collected
            .iter()
            .map(|path| path.display().to_string())
            .collect();

        assert_eq!(collected.len(), 4, "collected: {names:?}");
        assert!(collected.contains(&target.join(SCRATCH_DIR)));
        assert!(collected.contains(&target.join("hosted-sdk-v0.2.37")));
        assert!(collected.contains(&target.join("ci-run-99.log")));
        assert!(collected.contains(&target.join("desktop-web").join("out-windows-1234")));
        Ok(())
    }

    #[test]
    fn reusable_build_trees_are_never_collected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let target = temp.path();
        for keep in [
            "debug",
            "release",
            "release-small",
            "release-sdk",
            "release-wasm",
            "npm-cache",
            "wasm-sdk",
            "sdk-check",
            "package",
            "release-artifacts",
            "x86_64-pc-windows-msvc",
        ] {
            fs::create_dir_all(target.join(keep))?;
        }
        assert!(collect_scratch_paths(target)?.is_empty());
        Ok(())
    }

    #[test]
    fn directory_size_sums_nested_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("a/b"))?;
        fs::write(temp.path().join("a/one"), b"1234")?;
        fs::write(temp.path().join("a/b/two"), b"567")?;
        assert_eq!(directory_size(temp.path()), 7);
        Ok(())
    }
}
