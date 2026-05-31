//! App resource compiler hook.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/continuity.rc");
    println!("cargo:rerun-if-changed=assets/continuity.ico");

    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("windows-msvc") {
        return;
    }

    let Some(rc) = find_resource_compiler() else {
        eprintln!("invariant: rc.exe is required to embed the Continuity app icon");
        std::process::exit(1);
    };
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("invariant: Cargo sets CARGO_MANIFEST_DIR"),
    );
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("invariant: Cargo sets OUT_DIR"));
    let resource = out_dir.join("continuity.res");
    let status = Command::new(rc)
        .current_dir(&manifest_dir)
        .args(["/nologo", "/fo"])
        .arg(&resource)
        .arg("assets/continuity.rc")
        .status()
        .expect("invariant: rc.exe must launch");
    if !status.success() {
        eprintln!("invariant: rc.exe failed to compile Continuity resources");
        std::process::exit(1);
    }
    println!("cargo:rustc-link-arg-bin=continuity={}", resource.display());
}

fn find_resource_compiler() -> Option<PathBuf> {
    if let Some(path) = find_on_path("rc.exe") {
        return Some(path);
    }
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut candidates = Vec::new();
    for version in std::fs::read_dir(kits).ok()?.flatten() {
        let path = version.path().join("x64").join("rc.exe");
        if path.exists() {
            candidates.push(path);
        }
    }
    candidates.sort();
    candidates.pop()
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|path| path.join(name))
        .find(|path| path.exists())
}
