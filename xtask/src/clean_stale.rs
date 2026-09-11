//! Age-based garbage collection for Cargo build artifacts.
//!
//! Cargo never reclaims `target/`. Every time a crate's fingerprint changes it
//! writes a fresh `<crate>-<hash>.exe`, `.pdb`, `.rmeta`, and incremental
//! directory, and the previous generation stays on disk forever. A workspace
//! this size, built `--all-targets` by both git hooks, strands roughly one test
//! executable per crate per commit. Over months that reaches hundreds of
//! gigabytes of artifacts that nothing will ever read again.
//!
//! This task deletes artifacts untouched for longer than a cutoff. Removing a
//! live artifact costs a rebuild, never correctness: Cargo checks its
//! fingerprint, finds the output missing, and recompiles. Artifacts in active
//! use are rewritten by those rebuilds and so stay young.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;

use crate::release::workspace_root;

/// Default cutoff. Long enough to keep a fortnight of working artifacts warm,
/// short enough that a months-old generation cannot pile up.
const DEFAULT_STALE_DAYS: u64 = 14;

/// Subdirectories of a profile directory that hold reclaimable artifacts.
///
/// `deps` and `build` hold the per-hash outputs, `incremental` the per-session
/// caches, `.fingerprint` the staleness records Cargo rebuilds on demand.
/// Everything else in a profile directory (the current top-level binaries and
/// rlibs) is left alone so the newest build stays usable.
const RECLAIMABLE_SUBDIRECTORIES: &[&str] = &["deps", "build", "incremental", ".fingerprint"];

/// Delete Cargo artifacts older than the cutoff and report what was reclaimed.
///
/// `--days N` overrides the cutoff, `--dry-run` lists without deleting.
pub(crate) fn run(args: &[String]) -> Result<()> {
    let is_dry_run = args.iter().any(|arg| arg == "--dry-run");
    let stale_days = parse_days(args)?;
    let target = workspace_root().join("target");
    if !target.is_dir() {
        println!("clean-stale: no target/ directory, nothing to do");
        return Ok(());
    }

    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(stale_days * 24 * 60 * 60))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let mut reclaimed_bytes = 0_u64;
    let mut reclaimed_count = 0_usize;
    let mut kept_count = 0_usize;

    for directory in collect_artifact_directories(&target) {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_newer_than(&path, cutoff) {
                kept_count += 1;
                continue;
            }
            let size = entry_size(&path);
            if is_dry_run {
                reclaimed_bytes += size;
                reclaimed_count += 1;
                continue;
            }
            match remove(&path) {
                Ok(()) => {
                    reclaimed_bytes += size;
                    reclaimed_count += 1;
                }
                Err(error) => eprintln!("clean-stale: skipped {} ({error})", path.display()),
            }
        }
    }

    let verb = if is_dry_run {
        "would reclaim"
    } else {
        "reclaimed"
    };
    println!(
        "clean-stale: {verb} {} across {reclaimed_count} artifacts older than {stale_days} days ({kept_count} kept)",
        format_bytes(reclaimed_bytes)
    );
    Ok(())
}

/// Read the `--days N` override, falling back to [`DEFAULT_STALE_DAYS`].
fn parse_days(args: &[String]) -> Result<u64> {
    let Some(index) = args.iter().position(|arg| arg == "--days") else {
        return Ok(DEFAULT_STALE_DAYS);
    };
    let value = args
        .get(index + 1)
        .ok_or_else(|| anyhow::anyhow!("usage: cargo xtask clean-stale [--days N] [--dry-run]"))?;
    let days: u64 = value
        .parse()
        .map_err(|_| anyhow::anyhow!("--days expects a whole number of days, got `{value}`"))?;
    if days == 0 {
        anyhow::bail!("--days must be at least 1; use `cargo clean` to remove everything");
    }
    Ok(days)
}

/// Find every reclaimable artifact directory under `target/`.
///
/// Profile directories sit either directly under `target/` (host builds) or one
/// level deeper under a target triple (`target/x86_64-pc-windows-msvc/debug`),
/// so both depths are searched.
fn collect_artifact_directories(target: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut profile_roots = vec![target.to_path_buf()];

    if let Ok(entries) = fs::read_dir(target) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                profile_roots.push(path);
            }
        }
    }

    for root in profile_roots {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let profile = entry.path();
            if !profile.is_dir() {
                continue;
            }
            for name in RECLAIMABLE_SUBDIRECTORIES {
                let candidate = profile.join(name);
                if candidate.is_dir() {
                    directories.push(candidate);
                }
            }
        }
    }

    directories.sort();
    directories.dedup();
    directories
}

/// Whether `path` was modified at or after `cutoff`. Unreadable metadata counts
/// as new so an entry is never deleted on the strength of a failed stat.
fn is_newer_than(path: &Path, cutoff: SystemTime) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return true;
    };
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    modified >= cutoff
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

/// Total bytes under `path`. Unreadable entries contribute zero.
fn entry_size(path: &Path) -> u64 {
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
        .map(|entry| entry_size(&entry.path()))
        .sum()
}

/// Render a byte count for a human reading task output.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
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
    fn days_override_is_parsed() -> Result<()> {
        assert_eq!(parse_days(&[])?, DEFAULT_STALE_DAYS);
        assert_eq!(parse_days(&["--days".into(), "30".into()])?, 30);
        assert!(parse_days(&["--days".into()]).is_err());
        assert!(parse_days(&["--days".into(), "zero".into()]).is_err());
        assert!(parse_days(&["--days".into(), "0".into()]).is_err());
        Ok(())
    }

    #[test]
    fn artifact_directories_are_found_at_both_depths() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let target = temp.path();
        fs::create_dir_all(target.join("debug/deps"))?;
        fs::create_dir_all(target.join("debug/incremental"))?;
        fs::create_dir_all(target.join("x86_64-pc-windows-msvc/debug/deps"))?;
        fs::create_dir_all(target.join("x86_64-pc-windows-msvc/debug/.fingerprint"))?;
        // Not an artifact directory: must not be swept.
        fs::create_dir_all(target.join("debug/some-output"))?;

        let found = collect_artifact_directories(target);
        assert!(found.contains(&target.join("debug/deps")));
        assert!(found.contains(&target.join("debug/incremental")));
        assert!(found.contains(&target.join("x86_64-pc-windows-msvc/debug/deps")));
        assert!(found.contains(&target.join("x86_64-pc-windows-msvc/debug/.fingerprint")));
        assert!(!found.contains(&target.join("debug/some-output")));
        Ok(())
    }

    #[test]
    fn recent_entries_are_kept_and_old_entries_are_swept() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let deps = temp.path().join("debug/deps");
        fs::create_dir_all(&deps)?;
        let fresh = deps.join("fresh.rlib");
        fs::write(&fresh, b"fresh")?;

        // A cutoff in the future makes every existing entry stale.
        let future = SystemTime::now() + Duration::from_secs(60);
        assert!(!is_newer_than(&fresh, future));

        // A cutoff in the past keeps it.
        let past = SystemTime::now() - Duration::from_secs(60);
        assert!(is_newer_than(&fresh, past));
        Ok(())
    }
}
