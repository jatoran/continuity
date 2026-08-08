//! `cargo xtask bench [--fast]` — Phase 17 performance-gate runner.
//!
//! Drives the `#[ignore = "perf_gate"]` integration tests across the
//! workspace (`cargo test --release -- --ignored perf_gate_`) plus the
//! binary-size budget assertion. Exits non-zero on any miss.
//!
//! ## `--fast` subset
//!
//! `--fast` runs only the gates that complete in well under a minute, so
//! the hosted Windows workflow can use it without blowing the CI
//! budget. The full set, including the 100 MiB file open and 50 MiB
//! find-in-all gates, lives behind `cargo xtask bench` without the flag.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Result};

use crate::artifact_budget::WINDOWS_BINARY_SIZE_BUDGET_BYTES;

const BINARY_SIZE_PROFILE: &str = "release-small";

/// One perf-gate target (crate + test binary name + `#[ignore]` tag).
struct Gate {
    crate_name: &'static str,
    test_target: &'static str,
    fast: bool,
}

#[derive(Debug, Eq, PartialEq)]
struct GateBatch<'a> {
    packages: Vec<&'a str>,
    test_targets: Vec<&'a str>,
}

/// All Phase 17 perf gates. `fast = true` means the gate runs as part of
/// the `--fast` CI subset (and as part of the unflagged run as well).
const GATES: &[Gate] = &[
    Gate {
        crate_name: "continuity-buffer",
        test_target: "perf_gates",
        fast: true,
    },
    Gate {
        crate_name: "continuity-core",
        test_target: "perf_gates",
        fast: true,
    },
    Gate {
        crate_name: "continuity-decorate",
        test_target: "perf_gates",
        fast: true,
    },
    Gate {
        crate_name: "continuity-display-map",
        test_target: "perf_gates",
        fast: true,
    },
    Gate {
        crate_name: "continuity-render",
        test_target: "perf_gates",
        fast: true,
    },
    Gate {
        crate_name: "continuity-persist",
        test_target: "perf_gates",
        fast: true,
    },
    Gate {
        crate_name: "continuity-search",
        test_target: "perf_gates",
        fast: false,
    },
    Gate {
        crate_name: "continuity-ui",
        test_target: "perf_gates",
        fast: false,
    },
    Gate {
        crate_name: "continuity-ui",
        test_target: "editor_control_host",
        fast: true,
    },
    Gate {
        crate_name: "continuity-test-support",
        test_target: "perf_gates_memory_empty",
        fast: true,
    },
    Gate {
        crate_name: "continuity-test-support",
        test_target: "perf_gates_memory_50",
        fast: true,
    },
    Gate {
        crate_name: "continuity-test-support",
        test_target: "perf_gates_memory_200",
        fast: false,
    },
];

/// Run every Phase 17 gate. `fast_only = true` selects the CI-friendly
/// subset.
pub fn run(fast_only: bool) -> Result<()> {
    let mut misses: Vec<String> = Vec::new();
    let ran: Vec<&Gate> = GATES
        .iter()
        .filter(|gate| !fast_only || gate.fast)
        .collect();

    eprintln!("---- batched perf gates ----");
    if !run_gate_batches(&ran)? {
        eprintln!("batched perf run failed; rerunning each target to localize failures");
        for gate in &ran {
            if !run_single_gate(gate)?.success() {
                misses.push(format!("{} :: {}", gate.crate_name, gate.test_target));
            }
        }
    }

    eprintln!();
    eprintln!("---- binary size gate ----");
    let bin_result = check_binary_size();
    let bin_label = match &bin_result {
        Ok(bytes) => format!("ok ({} B ≤ {} B)", bytes, WINDOWS_BINARY_SIZE_BUDGET_BYTES),
        Err(e) => format!("FAIL: {e}"),
    };

    eprintln!();
    eprintln!("====================== Phase 17 perf-gate report ======================");
    for gate in &ran {
        let status = if misses
            .iter()
            .any(|m| m == &format!("{} :: {}", gate.crate_name, gate.test_target))
        {
            "FAIL"
        } else {
            "ok"
        };
        eprintln!(
            "  {:<28}  {:<22}  {}",
            gate.crate_name, gate.test_target, status
        );
    }
    eprintln!(
        "  {:<28}  {:<22}  {}",
        "binary-size", "release exe", bin_label
    );
    eprintln!("=======================================================================");

    if !misses.is_empty() {
        bail!(
            "{} perf gate(s) over budget: {}",
            misses.len(),
            misses.join(", ")
        );
    }
    bin_result.map(|_| ())
}

fn run_gate_batches(gates: &[&Gate]) -> Result<bool> {
    let coordinates = gates
        .iter()
        .map(|gate| (gate.crate_name, gate.test_target))
        .collect::<Vec<_>>();
    run_gate_coordinate_batches(&coordinates, None)
}

pub(crate) fn run_gate_coordinate_batches(
    gates: &[(&str, &str)],
    environment: Option<(&str, &Path)>,
) -> Result<bool> {
    for batch in compute_gate_batches(gates) {
        let mut command = Command::new(env!("CARGO"));
        command.args(["test", "--release"]);
        for package in batch.packages {
            command.args(["-p", package]);
        }
        for target in batch.test_targets {
            command.args(["--test", target]);
        }
        command.args(["--", "--ignored", "--test-threads=1", "--nocapture"]);
        if let Some((name, value)) = environment {
            command.env(name, value);
        }
        configure_perf_process(&mut command);
        if !command.status()?.success() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn compute_gate_batches<'a>(gates: &[(&'a str, &'a str)]) -> Vec<GateBatch<'a>> {
    let mut packages_by_target: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for &(crate_name, test_target) in gates {
        packages_by_target
            .entry(test_target)
            .or_default()
            .insert(crate_name);
    }

    // Cargo applies every `--test` target to every selected package. Merge
    // targets only when their package sets are identical, which avoids both
    // nonexistent cross-product targets and redundant feature-graph builds.
    let mut targets_by_packages: BTreeMap<Vec<&str>, Vec<&str>> = BTreeMap::new();
    for (target, packages) in packages_by_target {
        targets_by_packages
            .entry(packages.into_iter().collect())
            .or_default()
            .push(target);
    }

    targets_by_packages
        .into_iter()
        .map(|(packages, test_targets)| GateBatch {
            packages,
            test_targets,
        })
        .collect()
}

fn run_single_gate(gate: &Gate) -> Result<std::process::ExitStatus> {
    eprintln!(
        "---- perf gate fallback: {} :: {} ----",
        gate.crate_name, gate.test_target
    );
    let mut command = Command::new(env!("CARGO"));
    command.args([
        "test",
        "--release",
        "-p",
        gate.crate_name,
        "--test",
        gate.test_target,
        "--",
        "--ignored",
        "--test-threads=1",
        "--nocapture",
    ]);
    configure_perf_process(&mut command);
    Ok(command.status()?)
}

#[cfg(windows)]
fn configure_perf_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const HIGH_PRIORITY_CLASS: u32 = 0x0000_0080;
    command.creation_flags(HIGH_PRIORITY_CLASS);
}

#[cfg(not(windows))]
fn configure_perf_process(_command: &mut Command) {}

/// Build the shipping binary and confirm the stripped size is ≤ §15 budget.
fn check_binary_size() -> Result<u64> {
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--profile",
            BINARY_SIZE_PROFILE,
            "-p",
            "continuity-app",
        ])
        .status()?;
    if !status.success() {
        bail!("cargo build --profile {BINARY_SIZE_PROFILE} -p continuity-app failed");
    }
    let path = release_binary_path()?;
    let meta = std::fs::metadata(&path).map_err(|e| anyhow!("stat {}: {e}", path.display()))?;
    let bytes = meta.len();
    if bytes > WINDOWS_BINARY_SIZE_BUDGET_BYTES {
        bail!(
            "stripped binary {} is {} bytes, budget {} bytes",
            path.display(),
            bytes,
            WINDOWS_BINARY_SIZE_BUDGET_BYTES
        );
    }
    Ok(bytes)
}

fn release_binary_path() -> Result<PathBuf> {
    // The workspace pins `targets = ["x86_64-pc-windows-msvc"]`, so the
    // build can land either at `target/<profile>/` (host-triple build) or at
    // `target/x86_64-pc-windows-msvc/<profile>/` (cross-triple build).
    let target = workspace_root().join("target");
    let candidates = [
        target.join(BINARY_SIZE_PROFILE),
        target
            .join("x86_64-pc-windows-msvc")
            .join(BINARY_SIZE_PROFILE),
    ];
    let names = ["continuity.exe", "continuity"];
    for dir in &candidates {
        for name in names {
            let p = dir.join(name);
            if p.exists() {
                return Ok(p);
            }
        }
    }
    Err(anyhow!(
        "no continuity[.exe] under {} — was the release build skipped?",
        target.display()
    ))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::compute_gate_batches;

    #[test]
    fn batching_merges_targets_only_for_identical_package_sets() {
        let coordinates = [
            ("alpha", "shared"),
            ("beta", "shared"),
            ("alpha", "alpha_only_one"),
            ("alpha", "alpha_only_two"),
        ];

        let batches = compute_gate_batches(&coordinates);

        assert_eq!(batches.len(), 2);
        assert!(batches.iter().any(|batch| {
            batch.packages == ["alpha"]
                && batch.test_targets == ["alpha_only_one", "alpha_only_two"]
        }));
        assert!(batches.iter().any(|batch| {
            batch.packages == ["alpha", "beta"] && batch.test_targets == ["shared"]
        }));
    }
}
