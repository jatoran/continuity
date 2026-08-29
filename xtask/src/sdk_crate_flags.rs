//! Canonical `cargo` flags for the three published SDK crates.
//!
//! Staging, the publish dry run, and the real publication must all invoke
//! `cargo` identically, because the release gate requires the `.crate` a
//! publish job re-derives to be byte-identical to the one `cargo xtask
//! sdk-check` staged, hashed, and attested.
//!
//! These flags used to be written out three times - once in `sdk::
//! package_rust_crates`, once in `sdk::publish_dry_run`, and once inline in
//! `.github/workflows/sdk-release.yml`. The workflow copy drifted: it omitted
//! the `--config patch.crates-io.*` overrides, so the archive it produced
//! embedded a `Cargo.lock` that resolved `continuity-text` and
//! `continuity-test-support` from the registry instead of from local paths.
//! The hash comparison could never pass for `continuity-buffer` or
//! `continuity-engine`. `continuity-text` matched only because it is the one
//! crate that needs no patch overrides, which is why sdk-v0.2.38 published a
//! third of its closure before failing.
//!
//! Anything that packages or publishes these crates goes through this module.

/// Publication order. `continuity-engine` depends on `continuity-buffer`,
/// which depends on `continuity-text`, and crates.io rejects a crate whose
/// dependencies it cannot resolve.
pub(crate) const PUBLISH_ORDER: &[&str] =
    &["continuity-text", "continuity-buffer", "continuity-engine"];

/// `--config patch.crates-io.*` overrides that let a crate be packaged before
/// the workspace siblings it depends on exist on the registry. Without these,
/// `cargo` resolves those names from crates.io and writes a different
/// `Cargo.lock` into the archive.
///
/// `continuity-test-support` and `continuity-test-fixtures` are dev-only and
/// never published at all, so they must always resolve locally.
#[must_use]
pub(crate) fn patch_overrides(package: &str) -> &'static [&'static str] {
    match package {
        "continuity-buffer" => &[
            "patch.crates-io.continuity-text.path=\"crates/text\"",
            "patch.crates-io.continuity-test-support.path=\"crates/test_support\"",
        ],
        "continuity-engine" => &[
            "patch.crates-io.continuity-text.path=\"crates/text\"",
            "patch.crates-io.continuity-buffer.path=\"crates/buffer\"",
            "patch.crates-io.continuity-test-fixtures.path=\"crates/test_fixtures\"",
        ],
        _ => &[],
    }
}

/// Whether `cargo` should skip building the packaged crate before accepting it.
///
/// Only `continuity-text` is verified. The other two depend on workspace
/// siblings that are not on the registry when they are packaged, so a
/// verification build would resolve against a registry version that does not
/// exist yet.
#[must_use]
pub(crate) fn is_verified_on_package(package: &str) -> bool {
    package == "continuity-text"
}

/// Full argument list for `cargo package` / `cargo publish` for one crate.
///
/// `subcommand` is `"package"` or `"publish"`; `extra` carries subcommand-only
/// flags such as `--dry-run`. `--allow-dirty` is always passed so a staged
/// archive and a re-derived one agree on `.cargo_vcs_info.json` regardless of
/// whether the invoking tree happens to be clean.
#[must_use]
pub(crate) fn cargo_arguments<'a>(
    subcommand: &'a str,
    package: &'a str,
    extra: &[&'a str],
) -> Vec<&'a str> {
    let mut arguments = vec![subcommand, "-p", package, "--locked", "--allow-dirty"];
    arguments.extend_from_slice(extra);
    if !is_verified_on_package(package) {
        arguments.push("--no-verify");
    }
    for patch in patch_overrides(package) {
        arguments.push("--config");
        arguments.push(patch);
    }
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_the_only_crate_without_patch_overrides() {
        assert!(patch_overrides("continuity-text").is_empty());
        assert!(!patch_overrides("continuity-buffer").is_empty());
        assert!(!patch_overrides("continuity-engine").is_empty());
    }

    #[test]
    fn only_text_is_verified_on_package() {
        assert!(is_verified_on_package("continuity-text"));
        assert!(!is_verified_on_package("continuity-buffer"));
        assert!(!is_verified_on_package("continuity-engine"));
    }

    #[test]
    fn package_and_publish_agree_on_every_archive_shaping_flag() {
        // The hash gate only holds while the two subcommands differ solely by
        // the subcommand name and its own extra flags.
        for package in PUBLISH_ORDER {
            let packaged = cargo_arguments("package", package, &[]);
            let published = cargo_arguments("publish", package, &[]);
            assert_eq!(packaged[1..], published[1..]);
            assert_eq!(packaged[0], "package");
            assert_eq!(published[0], "publish");
        }
    }

    #[test]
    fn buffer_arguments_carry_both_patch_overrides() {
        let arguments = cargo_arguments("package", "continuity-buffer", &[]);
        assert!(arguments.contains(&"--no-verify"));
        assert_eq!(
            arguments.iter().filter(|arg| **arg == "--config").count(),
            2
        );
    }

    #[test]
    fn extra_flags_precede_the_generated_ones() {
        let arguments = cargo_arguments("publish", "continuity-engine", &["--dry-run"]);
        assert!(arguments.contains(&"--dry-run"));
        assert_eq!(
            arguments.iter().filter(|arg| **arg == "--config").count(),
            3
        );
    }
}
