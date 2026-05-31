//! `cargo xtask bench [--fast]` — Phase 17 performance-gate runner.
//!
//! Drives the `#[ignore = "perf_gate"]` integration tests across the
//! workspace (`cargo test --release -- --ignored perf_gate_`) plus the
//! binary-size budget assertion. Exits non-zero on any miss.
//!
//! ## `--fast` subset
//!
//! `--fast` runs only the gates that complete in well under a minute, so
//! the workflow on `windows-latest` can use it without blowing the CI
//! budget. The full set, including the 100 MiB file open and 50 MiB
//! find-in-all gates, lives behind `cargo xtask bench` without the flag.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail, Result};

/// Spec §15 binary-size budget for the stripped release executable.
const BINARY_SIZE_BUDGET_BYTES: u64 = 8 * 1024 * 1024;
const BINARY_SIZE_PROFILE: &str = "release-small";

/// One perf-gate target (crate + test binary name + `#[ignore]` tag).
struct Gate {
    crate_name: &'static str,
    test_target: &'static str,
    fast: bool,
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
    let mut ran: Vec<&Gate> = Vec::new();

    for gate in GATES {
        if fast_only && !gate.fast {
            continue;
        }
        ran.push(gate);
        eprintln!(
            "---- perf gate: {} :: {} ----",
            gate.crate_name, gate.test_target
        );
        let status = Command::new(env!("CARGO"))
            .args([
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
            ])
            .status()?;
        if !status.success() {
            misses.push(format!("{} :: {}", gate.crate_name, gate.test_target));
        }
    }

    eprintln!();
    eprintln!("---- binary size gate ----");
    let bin_result = check_binary_size();
    let bin_label = match &bin_result {
        Ok(bytes) => format!("ok ({} B ≤ {} B)", bytes, BINARY_SIZE_BUDGET_BYTES),
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
    if bytes > BINARY_SIZE_BUDGET_BYTES {
        bail!(
            "stripped binary {} is {} bytes, budget {} bytes",
            path.display(),
            bytes,
            BINARY_SIZE_BUDGET_BYTES
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
