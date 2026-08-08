//! Packed-artifact gates for the native-language SDK surfaces.

mod visual_studio;

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};

use self::visual_studio::visual_studio_toolchain;

const PACKAGES: &[&str] = &["continuity-text", "continuity-buffer", "continuity-engine"];
const FORBIDDEN_ARCHIVE_PARTS: &[&str] = &[
    ".docs/",
    "scratchpad",
    ".env",
    "credential",
    "password",
    "perf-snapshot",
    "trace.tsv",
];

pub(crate) fn check() -> Result<()> {
    check_with_evidence()?;
    Ok(())
}

pub(crate) fn check_with_evidence() -> Result<PathBuf> {
    let root = env::current_dir().context("resolve workspace root")?;
    let version = crate::sdk_release_manifest::canonical_version(&root)?;
    let run_directory = create_run_directory(&root)?;
    check_header(&root, &run_directory, &version)?;
    package_rust_crates(&root)?;
    audit_and_extract_crates(&root, &run_directory, &version)?;
    run_rust_consumer(&root, &run_directory, &version)?;

    if cfg!(target_os = "windows") {
        run_c_consumer(&root, &run_directory)?;
        run_python_consumer(&root, &run_directory)?;
    } else {
        bail!("sdk-check native distribution lane currently supports Windows x86-64 only");
    }

    println!("sdk: packed Cargo, C ABI, and Python consumers passed");
    println!("sdk: evidence staged at {}", run_directory.display());
    Ok(run_directory)
}

fn create_run_directory(root: &Path) -> Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before Unix epoch")?
        .as_millis();
    let path = root
        .join("target")
        .join("sdk-check")
        .join(timestamp.to_string());
    fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
    Ok(path)
}

fn check_header(root: &Path, run_directory: &Path, version: &str) -> Result<()> {
    let header_path = root.join("crates/c_api/include/continuity_engine.h");
    let header = fs::read_to_string(&header_path)
        .with_context(|| format!("read {}", header_path.display()))?;
    let template = fs::read_to_string(root.join("crates/c_api/continuity_engine.h.in"))?;
    let generated = template
        .replace("@ABI_MAJOR@", "1")
        .replace("@ABI_MINOR@", "0")
        .replace("@SDK_VERSION@", version);
    let api_path = root.join("crates/c_api/src/api.rs");
    let api =
        fs::read_to_string(&api_path).with_context(|| format!("read {}", api_path.display()))?;
    for line in api.lines() {
        let Some(start) = line.find("fn continuity_engine_") else {
            continue;
        };
        let symbol = line[start + 3..]
            .split('(')
            .next()
            .ok_or_else(|| anyhow!("parse C ABI symbol from {line}"))?;
        if !header.contains(symbol) {
            bail!("generated header is missing exported symbol `{symbol}`");
        }
    }
    for required in [
        "#define CONTINUITY_ENGINE_ABI_MAJOR 1".to_owned(),
        "#define CONTINUITY_ENGINE_ABI_MINOR 0".to_owned(),
        format!("#define CONTINUITY_ENGINE_SDK_VERSION \"{version}\""),
        "CONTINUITY_ENGINE_REENTRANT_CALL".to_owned(),
        "continuity_engine_utf16_string_free".to_owned(),
    ] {
        if !header.contains(&required) {
            bail!("generated header is missing contract `{required}`");
        }
    }
    let staged = run_directory.join("continuity_engine.h");
    fs::write(&staged, generated.as_bytes())
        .with_context(|| format!("generate {}", staged.display()))?;
    let regenerated = fs::read_to_string(&staged).context("read regenerated C header as UTF-8")?;
    let checked = fs::read_to_string(&header_path).context("read checked C header as UTF-8")?;
    if compute_normalized_line_endings(&regenerated) != compute_normalized_line_endings(&checked) {
        bail!("generated C header drifted from {}", header_path.display());
    }
    check_package_versions(root, &header, version)?;
    println!("sdk: generated C header contract is current");
    Ok(())
}

fn check_package_versions(root: &Path, header: &str, canonical_version: &str) -> Result<()> {
    for manifest in [
        "crates/text/Cargo.toml",
        "crates/buffer/Cargo.toml",
        "crates/engine/Cargo.toml",
        "crates/c_api/Cargo.toml",
        "bindings/python/Cargo.toml",
    ] {
        let contents = fs::read_to_string(root.join(manifest))?;
        let value: toml::Value = toml::from_str(&contents)?;
        let version = value
            .get("package")
            .and_then(|package| package.get("version"))
            .and_then(toml::Value::as_str);
        if version != Some(canonical_version) {
            bail!("{manifest} does not carry canonical SDK version {canonical_version}");
        }
    }
    let pyproject = fs::read_to_string(root.join("bindings/python/pyproject.toml"))?;
    if !pyproject.contains(&format!("version = \"{canonical_version}\"")) {
        bail!("Python project does not carry canonical SDK version {canonical_version}");
    }
    if !header.contains(&format!(
        "CONTINUITY_ENGINE_SDK_VERSION \"{canonical_version}\""
    )) {
        bail!("C header does not carry canonical SDK version {canonical_version}");
    }
    Ok(())
}

fn package_rust_crates(root: &Path) -> Result<()> {
    run_cargo(
        root,
        &[
            "package",
            "-p",
            "continuity-text",
            "--locked",
            "--allow-dirty",
        ],
    )?;
    run_cargo(
        root,
        &[
            "package",
            "-p",
            "continuity-buffer",
            "--locked",
            "--allow-dirty",
            "--no-verify",
            "--config",
            "patch.crates-io.continuity-text.path=\"crates/text\"",
            "--config",
            "patch.crates-io.continuity-test-support.path=\"crates/test_support\"",
        ],
    )?;
    run_cargo(
        root,
        &[
            "package",
            "-p",
            "continuity-engine",
            "--locked",
            "--allow-dirty",
            "--no-verify",
            "--config",
            "patch.crates-io.continuity-text.path=\"crates/text\"",
            "--config",
            "patch.crates-io.continuity-buffer.path=\"crates/buffer\"",
            "--config",
            "patch.crates-io.continuity-test-fixtures.path=\"crates/test_fixtures\"",
        ],
    )?;

    publish_dry_run(root, "continuity-text", &[])?;
    publish_dry_run(
        root,
        "continuity-buffer",
        &[
            "patch.crates-io.continuity-text.path=\"crates/text\"",
            "patch.crates-io.continuity-test-support.path=\"crates/test_support\"",
        ],
    )?;
    publish_dry_run(
        root,
        "continuity-engine",
        &[
            "patch.crates-io.continuity-text.path=\"crates/text\"",
            "patch.crates-io.continuity-buffer.path=\"crates/buffer\"",
            "patch.crates-io.continuity-test-fixtures.path=\"crates/test_fixtures\"",
        ],
    )?;
    Ok(())
}

fn publish_dry_run(root: &Path, package: &str, patches: &[&str]) -> Result<()> {
    let mut command = Command::new(cargo());
    command
        .current_dir(root)
        .env("CARGO_NET_OFFLINE", "false")
        .args([
            "publish",
            "-p",
            package,
            "--dry-run",
            "--locked",
            "--allow-dirty",
        ]);
    if package != "continuity-text" {
        command.arg("--no-verify");
    }
    for patch in patches {
        command.args(["--config", patch]);
    }
    run(&mut command, &format!("cargo publish --dry-run {package}"))
}

fn audit_and_extract_crates(root: &Path, run_directory: &Path, version: &str) -> Result<()> {
    let archive_root = run_directory.join("cargo-archives");
    fs::create_dir_all(&archive_root).context("create Cargo archive extraction directory")?;
    for package in PACKAGES {
        let archive = root
            .join("target/package")
            .join(format!("{package}-{version}.crate"));
        let mut list = Command::new("tar");
        list.args([OsStr::new("-tf"), archive.as_os_str()]);
        let output = capture(&mut list, &format!("list {}", archive.display()))?;
        let listing = String::from_utf8(output.stdout).context("Cargo archive listing is UTF-8")?;
        let normalized = listing.to_ascii_lowercase().replace('\\', "/");
        for forbidden in FORBIDDEN_ARCHIVE_PARTS {
            if normalized.contains(forbidden) {
                bail!(
                    "{} contains forbidden archive path `{forbidden}`",
                    archive.display()
                );
            }
        }
        let mut extract = Command::new("tar");
        extract.args([
            OsStr::new("-xf"),
            archive.as_os_str(),
            OsStr::new("-C"),
            archive_root.as_os_str(),
        ]);
        run(&mut extract, &format!("extract {}", archive.display()))?;
    }
    Ok(())
}

fn run_rust_consumer(root: &Path, run_directory: &Path, version: &str) -> Result<()> {
    let consumer = run_directory.join("rust-consumer");
    fs::create_dir_all(consumer.join("src")).context("create Rust consumer")?;
    fs::copy(
        root.join("sdk/consumers/rust/main.rs"),
        consumer.join("src/main.rs"),
    )
    .context("stage Rust packed consumer")?;
    let archives = run_directory.join("cargo-archives");
    let manifest = format!(
        "[package]\nname = \"continuity-packed-consumer\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n\n[dependencies]\ncontinuity-engine = {{ path = \"{}\" }}\ncontinuity-text = {{ path = \"{}\" }}\nserde_json = \"1\"\n\n[patch.crates-io]\ncontinuity-buffer = {{ path = \"{}\" }}\ncontinuity-text = {{ path = \"{}\" }}\n",
        toml_path(&archives.join(format!("continuity-engine-{version}"))),
        toml_path(&archives.join(format!("continuity-text-{version}"))),
        toml_path(&archives.join(format!("continuity-buffer-{version}"))),
        toml_path(&archives.join(format!("continuity-text-{version}"))),
    );
    fs::write(consumer.join("Cargo.toml"), manifest).context("write Rust consumer manifest")?;
    let manifest_path = consumer.join("Cargo.toml");
    run_cargo_os(
        root,
        &[
            "generate-lockfile".as_ref(),
            "--manifest-path".as_ref(),
            manifest_path.as_os_str(),
        ],
    )?;
    let fixture = root.join("crates/test_fixtures/fixtures/wasm_engine_parity.json");
    run_cargo_os(
        root,
        &[
            "run".as_ref(),
            "--locked".as_ref(),
            "--manifest-path".as_ref(),
            manifest_path.as_os_str(),
            "--".as_ref(),
            fixture.as_os_str(),
        ],
    )
}

fn run_c_consumer(root: &Path, run_directory: &Path) -> Result<()> {
    let target =
        env::var("CARGO_BUILD_TARGET").unwrap_or_else(|_| "x86_64-pc-windows-msvc".to_string());
    run_cargo(
        root,
        &[
            "build",
            "-p",
            "continuity-engine-c",
            "--profile",
            "release-sdk",
            "--target",
            &target,
        ],
    )?;
    let library_directory = root.join("target").join(target).join("release-sdk");
    let output_directory = run_directory.join("c-consumer");
    fs::create_dir_all(&output_directory).context("create C consumer directory")?;
    generate_c_fixture_header(root, &output_directory)?;
    let executable = output_directory.join("consumer.exe");
    let toolchain = visual_studio_toolchain()?;
    let mut executable_paths = vec![toolchain.compiler_directory.clone()];
    executable_paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let mut compile = Command::new(&toolchain.compiler);
    compile
        .current_dir(root)
        .env("INCLUDE", env::join_paths(&toolchain.includes)?)
        .env("LIB", env::join_paths(&toolchain.libraries)?)
        .env("PATH", env::join_paths(executable_paths)?)
        .args(["/nologo", "/W4", "/WX"])
        .arg(format!("/I{}", root.join("crates/c_api/include").display()))
        .arg(format!("/I{}", output_directory.display()))
        .arg(root.join("crates/c_api/tests/packed_consumer.c"))
        .arg(library_directory.join("continuity_engine.dll.lib"))
        .arg(format!(
            "/Fo{}",
            output_directory.join("packed_consumer.obj").display()
        ))
        .arg(format!("/Fe{}", executable.display()));
    run(&mut compile, "compile checked C consumer")?;
    fs::copy(
        library_directory.join("continuity_engine.dll"),
        output_directory.join("continuity_engine.dll"),
    )
    .context("stage C ABI DLL beside consumer")?;
    run(&mut Command::new(&executable), "run packed C consumer")
}

fn generate_c_fixture_header(root: &Path, output_directory: &Path) -> Result<()> {
    let fixture: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        root.join("crates/test_fixtures/fixtures/wasm_engine_parity.json"),
    )?)?;
    let multi = &fixture["multiCursor"];
    let deletion = &fixture["deleteBackward"];
    let undo = &fixture["undo"];
    let branch = &fixture["undoBranch"];
    let mut header =
        String::from("#ifndef CONTINUITY_PARITY_FIXTURE_H\n#define CONTINUITY_PARITY_FIXTURE_H\n");
    for (name, value) in [
        ("MULTI_INITIAL", &multi["initialText"]),
        ("MULTI_INSERT", &multi["insertText"]),
        ("MULTI_EXPECTED", &multi["expectedText"]),
        ("DELETE_INITIAL", &deletion["initialText"]),
        ("DELETE_EXPECTED", &deletion["expectedText"]),
        ("TYPING0", &undo["typing"][0]),
        ("TYPING1", &undo["typing"][1]),
        ("TYPING2", &undo["typing"][2]),
        ("TYPING_EXPECTED", &undo["expectedText"]),
        ("TYPING_AFTER_UNDO", &undo["expectedAfterUndo"]),
        ("TYPING_AFTER_REDO", &undo["expectedAfterRedo"]),
        ("BRANCH_PREFIX", &branch["inputs"][0]),
        ("BRANCH_OLD", &branch["inputs"][1]),
        ("BRANCH_NEW", &branch["inputs"][2]),
        ("BRANCH_REPLACEMENT", &branch["expectedReplacement"]),
        ("BRANCH_ALTERNATE", &branch["expectedAlternate"]),
    ] {
        let value = value
            .as_str()
            .ok_or_else(|| anyhow!("parity fixture `{name}` must be a string"))?;
        header.push_str(&format!(
            "#define CONTINUITY_FIXTURE_{name} {}\n",
            c_string_literal(value)
        ));
    }
    for (name, value) in [
        ("MULTI_REVISION", &multi["expectedRevision"]),
        ("MULTI_CARET0_LINE", &multi["selections"][0]["head"]["line"]),
        (
            "MULTI_CARET0_BYTE",
            &multi["selections"][0]["head"]["byteInLine"],
        ),
        ("MULTI_CARET1_LINE", &multi["selections"][1]["head"]["line"]),
        (
            "MULTI_CARET1_BYTE",
            &multi["selections"][1]["head"]["byteInLine"],
        ),
        ("MULTI_EXPECTED_CARET0_BYTE", &multi["expectedCarets"][0][1]),
        ("MULTI_EXPECTED_CARET1_BYTE", &multi["expectedCarets"][1][1]),
        ("MULTI_DELTA0_AT", &multi["expectedDeltas"][0]["at"]),
        ("MULTI_DELTA1_AT", &multi["expectedDeltas"][1]["at"]),
        ("DELETE_CARET_LINE", &deletion["selection"]["head"]["line"]),
        (
            "DELETE_CARET_BYTE",
            &deletion["selection"]["head"]["byteInLine"],
        ),
        ("DELETE_EXPECTED_CARET_LINE", &deletion["expectedCaret"][0]),
        ("DELETE_EXPECTED_CARET_BYTE", &deletion["expectedCaret"][1]),
    ] {
        let value = value
            .as_u64()
            .ok_or_else(|| anyhow!("parity fixture `{name}` must be an integer"))?;
        header.push_str(&format!("#define CONTINUITY_FIXTURE_{name} {value}\n"));
    }
    header.push_str("#endif\n");
    fs::write(output_directory.join("continuity_parity_fixture.h"), header)?;
    Ok(())
}

fn c_string_literal(value: &str) -> String {
    let mut output = String::from("\"");
    for byte in value.bytes() {
        match byte {
            b'\\' => output.push_str("\\\\"),
            b'\"' => output.push_str("\\\""),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x20..=0x7e => output.push(char::from(byte)),
            _ => output.push_str(&format!("\\x{byte:02X}\"\"")),
        }
    }
    output.push('\"');
    output
}

fn run_python_consumer(root: &Path, run_directory: &Path) -> Result<()> {
    let wheels = run_directory.join("wheels");
    fs::create_dir_all(&wheels).context("create wheel directory")?;
    let mut build = Command::new("python");
    build
        .current_dir(root)
        .env("CARGO_NET_OFFLINE", "false")
        .args([
            "-m",
            "pip",
            "wheel",
            "bindings/python",
            "--no-deps",
            "--wheel-dir",
        ])
        .arg(&wheels);
    run(&mut build, "build Python wheel with maturin")?;
    let wheel = fs::read_dir(&wheels)
        .context("read wheel directory")?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension() == Some(OsStr::new("whl")))
        .ok_or_else(|| anyhow!("maturin produced no wheel"))?;
    audit_wheel(&wheel)?;

    let venv = run_directory.join("python-venv");
    let mut create = Command::new("python");
    create.args(["-m", "venv"]).arg(&venv);
    run(&mut create, "create clean Python venv")?;
    let python = venv.join("Scripts/python.exe");
    let mut install = Command::new(&python);
    install
        .args(["-m", "pip", "install", "--no-deps"])
        .arg(&wheel);
    run(&mut install, "install packed Python wheel")?;
    let mut smoke = Command::new(&python);
    smoke
        .arg(root.join("bindings/python/tests/packed_consumer.py"))
        .arg(root.join("crates/test_fixtures/fixtures/wasm_engine_parity.json"));
    run(&mut smoke, "run packed Python consumer")
}

fn audit_wheel(wheel: &Path) -> Result<()> {
    let mut command = Command::new("python");
    command.args(["-m", "zipfile", "-l"]).arg(wheel);
    let output = capture(&mut command, "list Python wheel")?;
    let listing = String::from_utf8(output.stdout).context("wheel listing is UTF-8")?;
    let normalized = listing.to_ascii_lowercase().replace('\\', "/");
    for forbidden in FORBIDDEN_ARCHIVE_PARTS {
        if normalized.contains(forbidden) {
            bail!(
                "{} contains forbidden wheel path `{forbidden}`",
                wheel.display()
            );
        }
    }
    Ok(())
}

fn run_cargo(root: &Path, args: &[&str]) -> Result<()> {
    let mut command = Command::new(cargo());
    command.current_dir(root).args(args);
    run(&mut command, &format!("cargo {}", args.join(" ")))
}

fn run_cargo_os(root: &Path, args: &[&OsStr]) -> Result<()> {
    let mut command = Command::new(cargo());
    command.current_dir(root).args(args);
    run(&mut command, "Cargo packed-consumer command")
}

fn cargo() -> std::ffi::OsString {
    env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

fn run(command: &mut Command, label: &str) -> Result<()> {
    let status = command.status().with_context(|| format!("start {label}"))?;
    if !status.success() {
        bail!("{label} failed with {status}");
    }
    Ok(())
}

fn capture(command: &mut Command, label: &str) -> Result<Output> {
    let output = command.output().with_context(|| format!("start {label}"))?;
    if !output.status.success() {
        bail!("{label} failed with {}", output.status);
    }
    Ok(output)
}

fn toml_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

fn compute_normalized_line_endings(value: &str) -> String {
    value.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::compute_normalized_line_endings;

    #[test]
    fn generated_header_comparison_accepts_platform_line_endings() {
        let generated = "#define SDK 1\nvoid create(void);\n";
        let checked_out_on_windows = "#define SDK 1\r\nvoid create(void);\r\n";

        assert_eq!(
            compute_normalized_line_endings(generated),
            compute_normalized_line_endings(checked_out_on_windows)
        );
    }
}
