#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Continuity editor entry point. Wires every crate together and runs the
//! top-level event loop.
//!
//! Phase 14 — multi-window registry:
//!   1. Resolve `%APPDATA%\continuity\continuity.db`.
//!   2. Spawn the persistence thread (PersistHandle).
//!   3. Spawn the editor core thread (EditorHandle).
//!   4. Spawn the backup scheduler (15-min cadence, 24 retained).
//!   5. Load `windows` rows. For each row, build a `SpawnRequest` whose
//!      initial buffer is the persisted active buffer (after the buffer's
//!      own snapshot+replay). When no rows exist (first launch), enqueue
//!      a single fresh-window request seeded with the most-recent buffer.
//!   6. Run the registry loop until every window has exited.
//!   7. On exit: drop window threads' join handles → editor flushes final
//!      snapshots → watcher joins → backup scheduler drops → persist
//!      drops (final WAL checkpoint).
//!
//! Phase 16.5 — the registry is now the single owner of the
//! [`SettingsWatcher`]; the watcher's receiver is handed to the registry
//! via [`registry::RegistryRuntime`], which fans events out to every
//! live window through that window's typed control channel and applies
//! owner-routed settings (persistence mode, backup cadence) to their
//! single owner threads via typed messages.

mod error;
mod main_initial_requests;
mod registry;
mod registry_closed_history;
mod runtime_paths;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use continuity_buffer::BufferId;
use continuity_config::{Settings, SettingsWatcher, WatchPaths};
use continuity_core::{EditorHandle, SystemClock};
use continuity_keymap::DEFAULT_KEYMAP_TOML;
use continuity_persist::{backups_dir, db_path, BackupConfig, BackupScheduler, PersistHandle};
use continuity_ui::LiveReload;
use continuity_win::{set_per_monitor_dpi_v2, ComGuard};

use main_initial_requests::build_initial_requests;
use registry::{make_channel, run, RegistryCtx, RegistryRuntime, SpawnRequest};
use runtime_paths::StartupOptions;

fn main() -> Result<()> {
    // The main thread also touches COM (used by the desktop manager + later
    // by any modal dialogs we open from the registry hub itself).
    let _com = ComGuard::new()?;
    set_per_monitor_dpi_v2()?;
    let startup_options = StartupOptions::from_env()?;
    startup_options.runtime_paths.apply_process_overrides();

    let db = db_path().context("resolving continuity database path")?;
    let persist = PersistHandle::spawn(&db)
        .with_context(|| format!("opening persistence database at {}", db.display()))?;
    let editor = Arc::new(EditorHandle::spawn(persist.client(), Arc::new(SystemClock)));

    // Phase 17.9 §E1 — unattended-insert e2e hook. When
    // CONTINUITY_E2E_INSERT is set, open one buffer, apply an insert,
    // wait for the edit to land durably, write a marker file, then
    // sleep until the test TerminateProcess-es us. The registry / window
    // pipeline is intentionally skipped — we're testing recovery, not
    // the GUI. See `crates/app/tests/e2e_crash_recovery.rs`.
    if let Ok(text) = std::env::var("CONTINUITY_E2E_INSERT") {
        return run_e2e_insert(&db, &editor, &text);
    }

    // Backup scheduler runs in the background until app exit. The Arc lets
    // the registry call set_config() on it from its main loop.
    let backup_dir = match startup_options.runtime_paths.backups_dir.as_ref() {
        Some(path) => {
            std::fs::create_dir_all(path)
                .with_context(|| format!("creating backups directory {}", path.display()))?;
            path.clone()
        }
        None => backups_dir().context("resolving backups directory")?,
    };
    let backup = Arc::new(BackupScheduler::spawn(
        persist.client(),
        BackupConfig::with_defaults(backup_dir),
    ));
    let file_io = continuity_ui::file_io::FileIoService::spawn();

    // Phase 12: load + validate settings.toml, spawn the live-reload
    // watcher. Phase 16.5: the watcher is owned by the registry — its
    // receiver is handed off via RegistryRuntime, not per-window.
    let settings_path = startup_options.runtime_paths.settings_path.clone();
    let themes_dir = startup_options.runtime_paths.themes_dir.clone();
    let initial_settings = load_initial_settings(&settings_path);
    let watcher = settings_path
        .parent()
        .map(|cfg_dir| {
            SettingsWatcher::spawn(WatchPaths {
                settings: settings_path.clone(),
                keymap: cfg_dir.join("keymap.toml"),
                themes_dir: themes_dir.clone(),
            })
        })
        .transpose()
        .map_err(|e| {
            eprintln!("continuity: settings watcher failed to start: {e}");
            e
        })
        .ok()
        .flatten();
    let config_rx = watcher.as_ref().map(|w| w.events());
    let live_reload = {
        let persist_for_callback = persist.client();
        LiveReload {
            apply_sync_mode: Arc::new(move |value: &str| {
                if let Err(e) = persist_for_callback.set_synchronous(value) {
                    eprintln!("continuity: set_synchronous({value}) failed: {e}");
                }
            }),
            settings_path: settings_path.clone(),
            themes_dir,
            initial: Arc::new(Mutex::new(initial_settings)),
        }
    };

    let user_keymap_path = Some(startup_options.runtime_paths.keymap_path.clone());
    let (tx, rx) = make_channel();
    let ctx = RegistryCtx {
        persist: persist.client(),
        editor: editor.clone(),
        default_keymap_toml: DEFAULT_KEYMAP_TOML,
        user_keymap_path: user_keymap_path.clone(),
        tx,
        live_reload: Some(live_reload),
        file_io: file_io.client(),
    };
    let runtime = RegistryRuntime {
        config_rx,
        persist_event_rx: Some(persist.events()),
        backup: Some(Arc::clone(&backup)),
    };
    let mut initial_requests = build_initial_requests(&persist.client(), &editor)?;
    attach_startup_open_files(
        &mut initial_requests,
        &editor,
        &startup_options.startup_paths.files,
    );
    attach_startup_open_folders(
        &mut initial_requests,
        &startup_options.startup_paths.folders,
    );
    run(ctx, rx, runtime, initial_requests).context("registry loop")?;

    // Explicit drop order so shutdown reads clearly: window threads were
    // joined inside the registry loop; here we tear down the rest.
    drop(watcher);
    drop(file_io);
    drop(backup);
    drop(persist);
    Ok(())
}

/// §E1 unattended-insert: write `text` into a fresh buffer, wait for
/// the persist thread to flush, drop a marker file naming the buffer
/// id, then block forever so the test can `TerminateProcess` us.
fn run_e2e_insert(db: &std::path::Path, editor: &Arc<EditorHandle>, text: &str) -> Result<()> {
    use continuity_text::{EditOp, Position};
    let buffer_id = editor.open_buffer("");
    if !text.is_empty() {
        editor
            .apply_edit(buffer_id, EditOp::insert(Position::new(0, 0), text))
            .map_err(|e| anyhow::anyhow!("e2e insert failed: {e}"))?;
    }
    // Spec §15: edits are durable within 400 ms p99. Sleep 1.5× that to
    // make the marker a reliable "the kill is now safe" signal.
    std::thread::sleep(std::time::Duration::from_millis(600));
    let marker = db
        .parent()
        .map(|p| p.join(".e2e_inserted"))
        .unwrap_or_else(|| PathBuf::from(".e2e_inserted"));
    std::fs::write(&marker, buffer_id.as_uuid().to_string())
        .with_context(|| format!("writing e2e marker {}", marker.display()))?;
    eprintln!(
        "e2e: inserted {} bytes into {}; marker {}",
        text.len(),
        buffer_id.as_uuid(),
        marker.display()
    );
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

fn load_initial_settings(path: &PathBuf) -> Settings {
    if !path.exists() {
        return Settings::default();
    }
    match std::fs::read_to_string(path) {
        Ok(text) => match Settings::from_toml_validated(&text) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "continuity: settings.toml at {} invalid ({e}); using defaults",
                    path.display()
                );
                Settings::default()
            }
        },
        Err(e) => {
            eprintln!(
                "continuity: settings.toml at {} unreadable ({e}); using defaults",
                path.display()
            );
            Settings::default()
        }
    }
}

fn attach_startup_open_files(
    requests: &mut [SpawnRequest],
    editor: &Arc<EditorHandle>,
    paths: &[PathBuf],
) {
    if paths.is_empty() {
        return;
    }
    let mut known_file_paths = restored_file_paths(requests, editor);
    let Some(first) = requests.first_mut() else {
        return;
    };
    first.open_tutorial_on_init = false;
    for path in paths {
        if contains_same_file_path(&known_file_paths, path) {
            continue;
        }
        match continuity_ui::file_io::read_startup_file(path) {
            Ok(opened) => {
                let encoding_notice = opened.encoding_notice;
                let opened_path = opened.file.path.clone();
                let buffer_id = editor.open_file_buffer(opened.content, opened.file);
                first.startup_open_buffer_ids.push(buffer_id);
                known_file_paths.push(opened_path.clone());
                if let Some(encoding) = encoding_notice {
                    first.recovery_notices.push(format!(
                        "Opened {} as {encoding}; saving will write UTF-8.",
                        opened_path.display()
                    ));
                }
            }
            Err(e) => {
                let message = format!("Open failed for {}: {e}", path.display());
                eprintln!("continuity: {message}");
                first.recovery_notices.push(message);
            }
        }
    }
}

fn attach_startup_open_folders(requests: &mut [SpawnRequest], folders: &[PathBuf]) {
    if folders.is_empty() {
        return;
    }
    let Some(first) = requests.first_mut() else {
        return;
    };
    first.open_tutorial_on_init = false;
    first.startup_folder_roots.extend(folders.iter().cloned());
}

fn restored_file_paths(requests: &[SpawnRequest], editor: &Arc<EditorHandle>) -> Vec<PathBuf> {
    let mut buffer_ids = HashSet::new();
    for request in requests {
        buffer_ids.insert(request.initial_buffer_id);
        if let Some((_, restored)) = request.restored.as_ref() {
            if let Ok(restored_ids) =
                continuity_ui::pane_tree_codec::buffer_ids_in_json(&restored.pane_tree_json)
            {
                buffer_ids.extend(restored_ids);
            }
        }
    }
    buffer_ids
        .into_iter()
        .filter_map(|buffer_id: BufferId| editor.snapshot(buffer_id))
        .filter_map(|snapshot| snapshot.file.map(|file| file.path))
        .collect()
}

fn contains_same_file_path(paths: &[PathBuf], candidate: &Path) -> bool {
    paths
        .iter()
        .any(|existing| is_same_existing_file_path(existing, candidate))
}

fn is_same_existing_file_path(left: &Path, right: &Path) -> bool {
    let left = normalize_existing_path(left);
    let right = normalize_existing_path(right);
    left == right
        || left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
}

fn normalize_existing_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
