//! Single-instance launch handoff.
//!
//! A second `continuity.exe` launch must not replay the persisted window
//! session — that duplicates every open window. Instead the first process
//! holds a named mutex derived from the database path and runs a hidden
//! message hub; later launches forward their command-line paths to the hub
//! over `WM_COPYDATA` and exit. A bare launch activates a Continuity window
//! on the current virtual desktop, or opens a blank window there when none
//! exists. `--new-instance` or the e2e insert hook bypass the handoff.
//!
//! Thread ownership: [`claim_or_forward`] runs on the main thread before
//! any worker spawns. The hub callback only sends parsed paths to the bounded
//! handoff channel. The batching thread owns collection timing and synchronous
//! file reads, then sends typed events to the registry main thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use continuity_core::EditorHandle;
use continuity_win::{
    activate_first_visible_window_of_current_process_on_current_desktop, send_to_instance_hub,
    InstanceHub, SingleInstanceMutex,
};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::registry::{RegistryEvent, SpawnRequest};
use crate::registry_open_file::OpenFileBatchEntry;
use crate::runtime_paths::StartupPaths;
use crate::startup_file_window_origin;

const FORWARD_TIMEOUT_MS: u32 = 3_000;
const FORWARD_RETRY_ATTEMPTS: u32 = 10;
const FORWARD_RETRY_DELAY: Duration = Duration::from_millis(100);
const BATCH_QUIET_WINDOW: Duration = Duration::from_millis(120);
const BATCH_MAX_WINDOW: Duration = Duration::from_millis(350);

/// Owns the hidden handoff HWND and the external-open batching thread.
pub(crate) struct InstanceHandoff {
    hub: Option<InstanceHub>,
    input_tx: Option<Sender<StartupPaths>>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for InstanceHandoff {
    fn drop(&mut self) {
        drop(self.hub.take());
        drop(self.input_tx.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Outcome of the startup instance check.
pub(crate) enum InstanceClaim {
    /// This process is (or acts as) the primary instance. The guard is
    /// `None` only when mutex acquisition itself failed — still run, just
    /// without a claim.
    Primary(Option<SingleInstanceMutex>),
    /// The launch was forwarded to an already-running instance; exit now.
    Forwarded,
}

/// Claim the single-instance mutex, or forward this launch's paths to the
/// already-running instance.
pub(crate) fn claim_or_forward(db: &Path, startup: &StartupPaths) -> InstanceClaim {
    let key = instance_key(db);
    match SingleInstanceMutex::acquire(&mutex_name(&key)) {
        Ok(Some(guard)) => InstanceClaim::Primary(Some(guard)),
        Ok(None) => {
            let payload = forward_payload_json(startup);
            // The running instance may still be starting up; give its hub
            // a moment to appear before falling back to standalone mode.
            for _ in 0..FORWARD_RETRY_ATTEMPTS {
                match send_to_instance_hub(&hub_class_name(&key), &payload, FORWARD_TIMEOUT_MS) {
                    Ok(true) => return InstanceClaim::Forwarded,
                    Ok(false) => std::thread::sleep(FORWARD_RETRY_DELAY),
                    Err(e) => {
                        eprintln!("continuity: instance handoff send failed: {e}");
                        break;
                    }
                }
            }
            eprintln!(
                "continuity: another instance is running but unreachable; starting standalone"
            );
            InstanceClaim::Primary(None)
        }
        Err(e) => {
            eprintln!("continuity: single-instance mutex unavailable: {e}");
            InstanceClaim::Primary(None)
        }
    }
}

/// Spawn the receiving hub in the primary instance. File launches arriving
/// within one shell activation window are collected into one registry batch,
/// including the primary process's first path.
pub(crate) fn spawn_instance_hub(
    db: &Path,
    editor: Arc<EditorHandle>,
    tx: Sender<RegistryEvent>,
    startup_paths: StartupPaths,
) -> Option<InstanceHandoff> {
    let key = instance_key(db);
    let (input_tx, input_rx) = crossbeam_channel::bounded(256);
    let worker = std::thread::Builder::new()
        .name("continuity-open-batch".into())
        .spawn(move || run_handoff_batcher(input_rx, &editor, &tx))
        .ok()?;
    let callback_tx = input_tx.clone();
    let on_payload = Box::new(move |payload: &str| {
        if callback_tx.send(parse_forward_payload(payload)).is_err() {
            eprintln!("continuity: external-open batcher stopped before handoff delivery");
        }
    });
    let hub = match InstanceHub::spawn(&hub_class_name(&key), on_payload) {
        Ok(hub) => Some(hub),
        Err(e) => {
            eprintln!("continuity: instance hub failed to start: {e}");
            None
        }
    };
    if !startup_paths.is_empty() {
        let _ = input_tx.send(startup_paths);
    }
    Some(InstanceHandoff {
        hub,
        input_tx: Some(input_tx),
        worker: Some(worker),
    })
}

fn run_handoff_batcher(
    input_rx: Receiver<StartupPaths>,
    editor: &Arc<EditorHandle>,
    tx: &Sender<RegistryEvent>,
) {
    while let Ok(mut batch) = input_rx.recv() {
        let started = Instant::now();
        let mut disconnected = false;
        loop {
            let remaining = BATCH_MAX_WINDOW.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            match input_rx.recv_timeout(BATCH_QUIET_WINDOW.min(remaining)) {
                Ok(next) => batch.append(next),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        handle_forwarded_paths(batch, editor, tx);
        if disconnected {
            break;
        }
    }
}

fn handle_forwarded_paths(
    paths: StartupPaths,
    editor: &Arc<EditorHandle>,
    tx: &Sender<RegistryEvent>,
) {
    if paths.is_empty() {
        if activate_first_visible_window_of_current_process_on_current_desktop() {
            return;
        }
        let buffer_id = editor.open_buffer("");
        let _ = tx.send(RegistryEvent::Spawn(SpawnRequest {
            initial_buffer_id: buffer_id,
            restored: None,
            activate_on_restore: false,
            explicit_origin: startup_file_window_origin(0),
            cascade_from: None,
            recovery_notices: Vec::new(),
            open_tutorial_on_init: false,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: Vec::new(),
            startup_reconciles: Vec::new(),
        }));
        return;
    }
    for root in paths.vaults {
        let _ = tx.send(RegistryEvent::Vault(
            crate::registry_vaults::VaultRegistryEvent::Open(root),
        ));
    }
    let requested_files = !paths.files.is_empty();
    let mut files = Vec::new();
    let mut failed_notices = Vec::new();
    for path in &paths.files {
        match read_forwarded_file(path) {
            Ok(entry) => files.push(entry),
            Err(message) => failed_notices.push(message),
        }
    }
    if let Some(first) = files.first_mut() {
        first.recovery_notices.append(&mut failed_notices);
    }
    let has_open_files = !files.is_empty();
    if has_open_files {
        eprintln!(
            "continuity: opening {} external files in one window",
            files.len()
        );
        let _ = tx.send(RegistryEvent::OpenFileBatch {
            files,
            explicit_origin: startup_file_window_origin(0),
        });
    }
    if !paths.folders.is_empty() {
        let buffer_id = editor.open_buffer("");
        let _ = tx.send(RegistryEvent::Spawn(SpawnRequest {
            initial_buffer_id: buffer_id,
            restored: None,
            activate_on_restore: false,
            explicit_origin: startup_file_window_origin(usize::from(has_open_files)),
            cascade_from: None,
            recovery_notices: Vec::new(),
            open_tutorial_on_init: false,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: paths.folders,
            startup_reconciles: Vec::new(),
        }));
    } else if requested_files && !has_open_files {
        let buffer_id = editor.open_buffer("");
        let _ = tx.send(RegistryEvent::Spawn(SpawnRequest {
            initial_buffer_id: buffer_id,
            restored: None,
            activate_on_restore: false,
            explicit_origin: startup_file_window_origin(0),
            cascade_from: None,
            recovery_notices: failed_notices,
            open_tutorial_on_init: false,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: Vec::new(),
            startup_reconciles: Vec::new(),
        }));
    }
}

/// Read one forwarded file for a grouped registry activation. Reading on
/// the hub thread (rather than enqueueing to the file-I/O worker) keeps the
/// cross-process handoff self-contained because no window owns the request, while
/// still handing the registry fresh disk bytes for reconciliation.
fn read_forwarded_file(path: &Path) -> Result<OpenFileBatchEntry, String> {
    match continuity_ui::file_io::read_startup_file(path) {
        Ok(opened) => {
            let mut recovery_notices = Vec::new();
            if let Some(encoding) = opened.encoding_notice {
                recovery_notices.push(format!(
                    "Opened {} as {encoding}; saving will write UTF-8.",
                    opened.file.path.display()
                ));
            }
            Ok(OpenFileBatchEntry {
                content: opened.content,
                file: opened.file,
                recovery_notices,
            })
        }
        Err(e) => {
            let message = format!("Open failed for {}: {e}", path.display());
            eprintln!("continuity: forwarded {message}");
            Err(message)
        }
    }
}

/// Stable per-data-dir key so a portable instance and an installed
/// instance never collide. Reuses the persist-crate FNV so there is one
/// FNV in the workspace; this is naming, not content checksumming.
fn instance_key(db: &Path) -> String {
    let normalized = db.to_string_lossy().to_lowercase();
    format!(
        "{:016x}",
        continuity_persist::fnv1a_64(normalized.as_bytes())
    )
}

fn mutex_name(key: &str) -> String {
    format!("Local\\continuity-instance-{key}")
}

fn hub_class_name(key: &str) -> String {
    format!("ContinuityInstanceHub_{key}")
}

fn forward_payload_json(startup: &StartupPaths) -> String {
    let files: Vec<String> = startup.files.iter().map(|p| absolute_lossy(p)).collect();
    let folders: Vec<String> = startup.folders.iter().map(|p| absolute_lossy(p)).collect();
    let vaults: Vec<String> = startup.vaults.iter().map(|p| absolute_lossy(p)).collect();
    serde_json::json!({ "files": files, "folders": folders, "vaults": vaults }).to_string()
}

/// Forwarded paths must be absolute: the receiving process has a
/// different working directory than the sender.
fn absolute_lossy(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn parse_forward_payload(payload: &str) -> StartupPaths {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return StartupPaths::default();
    };
    let collect = |key: &str| -> Vec<PathBuf> {
        value
            .get(key)
            .and_then(|v| v.as_array())
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    StartupPaths {
        files: collect("files"),
        folders: collect("folders"),
        vaults: collect("vaults"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_round_trips_files_and_folders() {
        let startup = StartupPaths {
            files: vec![PathBuf::from("a.md")],
            folders: vec![PathBuf::from("notes")],
            vaults: vec![PathBuf::from("work")],
        };
        let payload = forward_payload_json(&startup);
        let parsed = parse_forward_payload(&payload);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.folders.len(), 1);
        assert!(parsed.files[0].is_absolute());
        assert!(parsed.folders[0].is_absolute());
        assert_eq!(parsed.vaults.len(), 1);
        assert!(parsed.vaults[0].is_absolute());
        assert!(parsed.files[0].ends_with("a.md"));
    }

    #[test]
    fn malformed_payload_parses_to_empty() {
        assert!(parse_forward_payload("not json").is_empty());
    }

    #[test]
    fn instance_key_is_path_case_insensitive() {
        let a = instance_key(Path::new("C:\\Data\\continuity.db"));
        let b = instance_key(Path::new("c:\\data\\CONTINUITY.DB"));
        assert_eq!(a, b);
        let c = instance_key(Path::new("D:\\elsewhere\\continuity.db"));
        assert_ne!(a, c);
    }

    #[test]
    fn forwarded_open_builds_batch_entry_with_disk_bytes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("forwarded.md");
        std::fs::write(&path, "current disk content").expect("write file");
        let entry = read_forwarded_file(&path).expect("entry for readable file");
        assert_eq!(entry.content, "current disk content");
        assert_eq!(entry.file.path, path);
        assert!(entry.recovery_notices.is_empty());
    }

    #[test]
    fn forwarded_open_surfaces_encoding_notice() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("latin1.txt");
        // 0xE9 alone is invalid UTF-8 → lossy decode + encoding notice.
        std::fs::write(&path, [b'h', b'i', 0xE9]).expect("write file");
        let entry = read_forwarded_file(&path).expect("entry for readable file");
        assert_eq!(entry.recovery_notices.len(), 1);
        assert!(entry.recovery_notices[0].contains("saving will write UTF-8"));
    }

    #[test]
    fn forwarded_open_missing_file_yields_no_event() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("absent.md");
        assert!(read_forwarded_file(&path).is_err());
    }
}
