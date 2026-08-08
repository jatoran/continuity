//! Unified, non-publishing SDK release staging and immutable-bundle checks.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};

use crate::sdk_release_artifact::{verify_release_directory, write_release_record};

pub(crate) fn check() -> Result<()> {
    let root = env::current_dir().context("resolve workspace root")?;
    let config = crate::sdk_release_manifest::verify(&root)?;
    println!(
        "sdk-release: {} {} configuration is consistent",
        config.sdk.channel, config.sdk.version
    );
    Ok(())
}

pub(crate) fn dry_run(args: &[String]) -> Result<()> {
    let root = env::current_dir().context("resolve workspace root")?;
    let is_dirty_allowed = args.iter().any(|arg| arg == "--allow-dirty");
    reject_unknown_arguments(args, &["--allow-dirty"])?;
    if !cfg!(target_os = "windows") {
        bail!("sdk-release-dry-run currently assembles Windows native SDK artifacts");
    }
    let config = crate::sdk_release_manifest::verify(&root)?;
    let source_dirty = is_tracked_worktree_dirty(&root)?;
    if source_dirty && !is_dirty_allowed {
        bail!("SDK releases require a clean tracked worktree; use --allow-dirty only for local rehearsal");
    }

    let evidence = crate::sdk::check_with_evidence()?;
    let npm_tarball = crate::wasm::package_tarball()?;
    let tag = format!("{}{}", config.sdk.tag_prefix, config.sdk.version);
    verify_ci_tag(&tag)?;
    let release_root = root.join("target").join("sdk-release");
    let output = release_root.join(&tag);
    recreate_release_directory(&root, &output)?;
    stage_cargo_archives(&root, &output, &config)?;
    copy_with_name(&npm_tarball, &output)?;
    stage_python_wheel(&evidence, &output)?;
    stage_c_abi(&root, &evidence, &output, &config)?;
    let sbom = crate::sdk_release_sbom::output_path(&output, &config.sdk.version);
    crate::sdk_release_sbom::write(&root, &sbom, &config)?;

    let source_commit = capture(&root, "git", &["rev-parse", "HEAD"])?;
    let generated = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before Unix epoch")?
        .as_secs();
    write_release_record(
        &output,
        &config.sdk.version,
        &config.sdk.channel,
        &tag,
        source_commit.trim(),
        source_dirty,
        generated,
    )?;
    verify_release_directory(&output, &config.sdk.version)?;
    fs::create_dir_all(&release_root)?;
    fs::write(
        release_root.join("latest-output.txt"),
        output.to_string_lossy().as_bytes(),
    )?;
    println!(
        "sdk-release: immutable dry-run bundle written to {}",
        output.display()
    );
    Ok(())
}

pub(crate) fn verify(args: &[String]) -> Result<()> {
    if args.len() > 1 {
        bail!("usage: cargo xtask sdk-release-verify [bundle-directory]");
    }
    let root = env::current_dir().context("resolve workspace root")?;
    let config = crate::sdk_release_manifest::verify(&root)?;
    let directory = if let Some(path) = args.first() {
        PathBuf::from(path)
    } else {
        let latest = root.join("target/sdk-release/latest-output.txt");
        PathBuf::from(
            fs::read_to_string(&latest)
                .with_context(|| format!("read {}; run sdk-release-dry-run", latest.display()))?
                .trim(),
        )
    };
    let record = verify_release_directory(&directory, &config.sdk.version)?;
    println!(
        "sdk-release: verified {} immutable artifacts for {}",
        record.artifacts.len(),
        record.tag
    );
    Ok(())
}

fn stage_cargo_archives(
    root: &Path,
    output: &Path,
    config: &crate::sdk_release_manifest::ReleaseConfig,
) -> Result<()> {
    for package in &config.cargo.packages {
        let source = root
            .join("target/package")
            .join(format!("{package}-{}.crate", config.sdk.version));
        copy_with_name(&source, output)?;
    }
    Ok(())
}

fn stage_python_wheel(evidence: &Path, output: &Path) -> Result<()> {
    let wheel = find_single_file(&evidence.join("wheels"), "whl")?;
    copy_with_name(&wheel, output)
}

fn stage_c_abi(
    root: &Path,
    evidence: &Path,
    output: &Path,
    config: &crate::sdk_release_manifest::ReleaseConfig,
) -> Result<()> {
    let staging = root.join("target/sdk-release/c-abi-staging");
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .with_context(|| format!("remove stale {}", staging.display()))?;
    }
    fs::create_dir_all(&staging)?;
    fs::copy(
        evidence.join("continuity_engine.h"),
        staging.join("continuity_engine.h"),
    )?;
    let library = root
        .join("target")
        .join(&config.c_abi.target)
        .join("release-sdk");
    for name in ["continuity_engine.dll", "continuity_engine.dll.lib"] {
        fs::copy(library.join(name), staging.join(name))
            .with_context(|| format!("stage C ABI artifact {name}"))?;
    }
    fs::copy(root.join("LICENSE"), staging.join("LICENSE"))?;
    let archive = output.join(format!(
        "continuity-engine-c-{}-windows-x86_64.zip",
        config.sdk.version
    ));
    let status = Command::new("tar")
        .args([OsStr::new("-a"), OsStr::new("-cf"), archive.as_os_str()])
        .arg("-C")
        .arg(&staging)
        .arg(".")
        .status()
        .context("package C ABI zip")?;
    if !status.success() {
        bail!("packaging C ABI zip failed (exit {:?})", status.code());
    }
    fs::remove_dir_all(&staging)?;
    Ok(())
}

fn copy_with_name(source: &Path, output: &Path) -> Result<()> {
    let name = source
        .file_name()
        .ok_or_else(|| anyhow!("artifact path has no file name: {}", source.display()))?;
    fs::copy(source, output.join(name))
        .with_context(|| format!("copy release artifact {}", source.display()))?;
    Ok(())
}

fn find_single_file(directory: &Path, extension: &str) -> Result<PathBuf> {
    let files: Vec<PathBuf> = fs::read_dir(directory)
        .with_context(|| format!("read artifact directory {}", directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension() == Some(OsStr::new(extension)))
        .collect();
    if files.len() != 1 {
        bail!(
            "expected one .{extension} file in {}, found {}",
            directory.display(),
            files.len()
        );
    }
    Ok(files[0].clone())
}

fn recreate_release_directory(root: &Path, output: &Path) -> Result<()> {
    let expected_parent = root.join("target/sdk-release");
    if output.parent() != Some(expected_parent.as_path()) {
        bail!(
            "refusing to recreate unexpected release path {}",
            output.display()
        );
    }
    if output.exists() {
        fs::remove_dir_all(output)
            .with_context(|| format!("remove stale release directory {}", output.display()))?;
    }
    fs::create_dir_all(output).with_context(|| format!("create {}", output.display()))
}

fn is_tracked_worktree_dirty(root: &Path) -> Result<bool> {
    let status = capture(
        root,
        "git",
        &["status", "--porcelain", "--untracked-files=no"],
    )?;
    Ok(!status.trim().is_empty())
}

fn reject_unknown_arguments(args: &[String], allowed: &[&str]) -> Result<()> {
    if let Some(argument) = args
        .iter()
        .find(|argument| !allowed.contains(&argument.as_str()))
    {
        bail!("unknown sdk-release-dry-run argument `{argument}`");
    }
    Ok(())
}

fn verify_ci_tag(expected_tag: &str) -> Result<()> {
    if env::var("GITHUB_REF_TYPE").ok().as_deref() != Some("tag") {
        return Ok(());
    }
    let actual = env::var("GITHUB_REF_NAME").context("GitHub tag name is missing")?;
    if actual != expected_tag {
        bail!("GitHub release tag `{actual}` must match canonical tag `{expected_tag}`");
    }
    Ok(())
}

fn capture(root: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .current_dir(root)
        .args(args)
        .output()
        .with_context(|| format!("run {program} {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).context("command output is UTF-8")
}
