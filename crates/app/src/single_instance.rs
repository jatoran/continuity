//! Single-instance launch handoff.
//!
//! A second `continuity.exe` launch must not replay the persisted window
//! session — that duplicates every open window. Instead the first process
//! holds a named mutex derived from the database path and runs a hidden
//! message hub; later launches forward their command-line paths to the hub
//! over `WM_COPYDATA` and exit (a bare launch just activates the running
//! instance). `--new-instance` or the e2e insert hook bypass the handoff.
//!
//! Thread ownership: [`claim_or_forward`] runs on the main thread before
//! any worker spawns. The hub callback runs on the hub's message-pump
//! thread and only touches thread-safe handles (`EditorHandle`, the
//! registry `Sender`, the file-buffer index mutex).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use continuity_core::EditorHandle;
use continuity_win::{
    activate_first_visible_window_of_current_process, send_to_instance_hub, InstanceHub,
    SingleInstanceMutex,
};
use crossbeam_channel::Sender;

use crate::registry::{RegistryEvent, SpawnRequest};
use crate::registry_file_buffers::{file_buffer_for_path, register_file_buffer, FileBufferIndex};
use crate::runtime_paths::StartupPaths;
use crate::{startup_file_spawn_request, startup_file_window_origin};

const FORWARD_TIMEOUT_MS: u32 = 3_000;
const FORWARD_RETRY_ATTEMPTS: u32 = 10;
const FORWARD_RETRY_DELAY: Duration = Duration::from_millis(100);

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

/// Spawn the receiving hub in the primary instance. Forwarded file paths
/// open as new top-level windows (reusing live buffers for already-open
/// files); a bare-launch forward activates the top-most existing window.
pub(crate) fn spawn_instance_hub(
    db: &Path,
    editor: Arc<EditorHandle>,
    file_buffer_index: FileBufferIndex,
    tx: Sender<RegistryEvent>,
) -> Option<InstanceHub> {
    let key = instance_key(db);
    let on_payload = Box::new(move |payload: &str| {
        handle_forwarded_payload(payload, &editor, &file_buffer_index, &tx);
    });
    match InstanceHub::spawn(&hub_class_name(&key), on_payload) {
        Ok(hub) => Some(hub),
        Err(e) => {
            eprintln!("continuity: instance hub failed to start: {e}");
            None
        }
    }
}

fn handle_forwarded_payload(
    payload: &str,
    editor: &Arc<EditorHandle>,
    file_buffer_index: &FileBufferIndex,
    tx: &Sender<RegistryEvent>,
) {
    let (files, folders) = parse_forward_payload(payload);
    if files.is_empty() && folders.is_empty() {
        if !activate_first_visible_window_of_current_process() {
            eprintln!("continuity: forwarded activation found no visible window");
        }
        return;
    }
    let mut opened = 0usize;
    for path in files {
        if let Some(request) =
            forwarded_file_spawn_request(&path, editor, file_buffer_index, opened)
        {
            let _ = tx.send(RegistryEvent::Spawn(request));
            opened += 1;
        }
    }
    if !folders.is_empty() {
        let buffer_id = editor.open_buffer("");
        let _ = tx.send(RegistryEvent::Spawn(SpawnRequest {
            initial_buffer_id: buffer_id,
            restored: None,
            activate_on_restore: false,
            explicit_origin: startup_file_window_origin(opened),
            cascade_from: None,
            recovery_notices: Vec::new(),
            open_tutorial_on_init: false,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: folders,
        }));
    }
}

fn forwarded_file_spawn_request(
    path: &Path,
    editor: &Arc<EditorHandle>,
    file_buffer_index: &FileBufferIndex,
    ordinal: usize,
) -> Option<SpawnRequest> {
    let mut notices = Vec::new();
    let buffer_id = match file_buffer_for_path(editor, file_buffer_index, path) {
        Some(buffer_id) => buffer_id,
        None => match continuity_ui::file_io::read_startup_file(path) {
            Ok(opened) => {
                let encoding_notice = opened.encoding_notice;
                let opened_path = opened.file.path.clone();
                let buffer_id = editor.open_file_buffer(opened.content, opened.file);
                register_file_buffer(file_buffer_index, opened_path.clone(), buffer_id);
                if let Some(encoding) = encoding_notice {
                    notices.push(format!(
                        "Opened {} as {encoding}; saving will write UTF-8.",
                        opened_path.display()
                    ));
                }
                buffer_id
            }
            Err(e) => {
                eprintln!(
                    "continuity: forwarded open failed for {}: {e}",
                    path.display()
                );
                return None;
            }
        },
    };
    Some(startup_file_spawn_request(buffer_id, notices, ordinal))
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
    serde_json::json!({ "files": files, "folders": folders }).to_string()
}

/// Forwarded paths must be absolute: the receiving process has a
/// different working directory than the sender.
fn absolute_lossy(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn parse_forward_payload(payload: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return (Vec::new(), Vec::new());
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
    (collect("files"), collect("folders"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_round_trips_files_and_folders() {
        let startup = StartupPaths {
            files: vec![PathBuf::from("a.md")],
            folders: vec![PathBuf::from("notes")],
        };
        let payload = forward_payload_json(&startup);
        let (files, folders) = parse_forward_payload(&payload);
        assert_eq!(files.len(), 1);
        assert_eq!(folders.len(), 1);
        assert!(files[0].is_absolute());
        assert!(folders[0].is_absolute());
        assert!(files[0].ends_with("a.md"));
    }

    #[test]
    fn malformed_payload_parses_to_empty() {
        let (files, folders) = parse_forward_payload("not json");
        assert!(files.is_empty());
        assert!(folders.is_empty());
    }

    #[test]
    fn instance_key_is_path_case_insensitive() {
        let a = instance_key(Path::new("C:\\Data\\continuity.db"));
        let b = instance_key(Path::new("c:\\data\\CONTINUITY.DB"));
        assert_eq!(a, b);
        let c = instance_key(Path::new("D:\\elsewhere\\continuity.db"));
        assert_ne!(a, c);
    }
}
