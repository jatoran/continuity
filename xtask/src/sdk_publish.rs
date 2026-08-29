//! Publishes the SDK Cargo closure from an already-staged release bundle.
//!
//! The release workflow used to inline its own `cargo package` / `cargo
//! publish` loop. That copy drifted from the staging path in
//! [`crate::sdk_crate_flags`] and produced archives that could never match the
//! staged hashes. The workflow now calls this command, so there is exactly one
//! description of how these crates are built for publication.
//!
//! Each crate is packaged, compared against the staged archive, published, and
//! compared again. A mismatch aborts before anything reaches the registry,
//! because a registry version cannot be replaced once accepted.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::sdk_crate_flags::{cargo_arguments, PUBLISH_ORDER};

/// Package, verify against the staged bundle, and publish each SDK crate.
///
/// `args` takes the staged bundle directory, plus an optional `--dry-run`
/// that stops short of mutating the registry.
pub(crate) fn publish_crates(args: &[String]) -> Result<()> {
    let mut bundle: Option<PathBuf> = None;
    let mut is_dry_run = false;
    for arg in args {
        match arg.as_str() {
            "--dry-run" => is_dry_run = true,
            other if other.starts_with("--") => bail!("unknown sdk-publish-crates flag `{other}`"),
            other => bundle = Some(PathBuf::from(other)),
        }
    }
    let bundle = bundle.context("sdk-publish-crates needs the staged bundle directory")?;
    let root = env::current_dir().context("resolve workspace root")?;
    let version = crate::sdk_release_manifest::canonical_version(&root)?;

    for package in PUBLISH_ORDER {
        let archive = format!("{package}-{version}.crate");
        let staged = bundle.join(&archive);
        if !staged.is_file() {
            bail!("staged archive {} is missing", staged.display());
        }

        run_cargo(&root, &cargo_arguments("package", package, &[]))?;
        let produced = root.join("target/package").join(&archive);
        assert_same_archive(package, "before publish", &produced, &staged)?;

        let publish_extra: &[&str] = if is_dry_run { &["--dry-run"] } else { &[] };
        run_cargo(&root, &cargo_arguments("publish", package, publish_extra))?;
        // `cargo publish` re-packages into the same path. Comparing again
        // proves the bytes the registry accepted are the attested ones.
        assert_same_archive(package, "after publish", &produced, &staged)?;
    }
    if is_dry_run {
        println!("sdk-publish-crates: dry run matched every staged archive");
    } else {
        println!(
            "sdk-publish-crates: published {} crates",
            PUBLISH_ORDER.len()
        );
    }
    Ok(())
}

/// Fail unless the re-derived archive is byte-identical to the staged one,
/// naming both digests. A silent comparison here once cost a release cycle:
/// the job exited non-zero with nothing but a successful `cargo package` above
/// it, and the cause looked like an authentication problem for two attempts.
fn assert_same_archive(package: &str, stage: &str, produced: &Path, staged: &Path) -> Result<()> {
    let produced_hash = crate::sdk_release_artifact::sha256(produced)?;
    let staged_hash = crate::sdk_release_artifact::sha256(staged)?;
    if produced_hash != staged_hash {
        bail!(
            "{package} ({stage}) does not match the staged bundle\n  \
             produced {produced_hash}  ({})\n  \
             staged   {staged_hash}  ({})",
            produced.display(),
            staged.display()
        );
    }
    println!("sdk-publish-crates: {package} ({stage}) matches {produced_hash}");
    Ok(())
}

fn run_cargo(root: &Path, args: &[&str]) -> Result<()> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(root)
        .env("CARGO_NET_OFFLINE", "false")
        .args(args)
        .status()
        .with_context(|| format!("spawn cargo {}", args.join(" ")))?;
    if !status.success() {
        bail!("cargo {} failed", args.join(" "));
    }
    Ok(())
}
