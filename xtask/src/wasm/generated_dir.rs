//! Retry-safe lifecycle for generated WebAssembly staging directories.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};

const REMOVE_ATTEMPTS: usize = 20;

pub(super) fn recreate_generated_dir(workspace: &Path, path: &Path) -> Result<()> {
    let generated_root = workspace.join("target/wasm-sdk");
    if !path.starts_with(&generated_root) {
        bail!("refusing to replace non-generated path {}", path.display());
    }
    if path.exists() {
        remove_generated_dir_with_retry(path)?;
    }
    fs::create_dir_all(path)
        .with_context(|| format!("creating generated directory {}", path.display()))?;
    Ok(())
}

fn remove_generated_dir_with_retry(path: &Path) -> Result<()> {
    let mut attempt = 1;
    loop {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if is_retryable_error(&error) && attempt < REMOVE_ATTEMPTS => {
                let delay = Duration::from_millis(((attempt as u64) * 25).min(250));
                eprintln!(
                    "wasm: generated directory busy; retrying {attempt}/{REMOVE_ATTEMPTS}: {}",
                    path.display()
                );
                thread::sleep(delay);
                attempt += 1;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("removing generated directory {}", path.display()));
            }
        }
    }
}

fn is_retryable_error(error: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(5 | 32))
    }
    #[cfg(not(windows))]
    {
        let _ = error;
        false
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn windows_sharing_errors_are_retryable() {
        assert!(super::is_retryable_error(
            &std::io::Error::from_raw_os_error(32)
        ));
        assert!(super::is_retryable_error(
            &std::io::Error::from_raw_os_error(5)
        ));
        assert!(!super::is_retryable_error(
            &std::io::Error::from_raw_os_error(2)
        ));
    }
}
