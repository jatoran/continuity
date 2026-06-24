#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Continuity editor entry point. Wires every crate together and runs the
//! top-level event loop.
//!
//! Phase 14 — multi-window registry:
//!   1. Resolve the runtime data directory (`%APPDATA%\continuity` or
//!      beside-exe portable `data\`).
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
mod registry_build;
mod registry_closed_history;
mod registry_file_buffers;
mod registry_open_file;
mod registry_time;
mod runtime_paths;
mod single_instance;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use continuity_buffer::BufferId;
use continuity_config::{Settings, SettingsWatcher, WatchPaths};
use continuity_core::{EditorHandle, SystemClock};
use continuity_keymap::DEFAULT_KEYMAP_TOML;
use continuity_persist::{backups_dir, db_path, BackupConfig, BackupScheduler, PersistHandle};
use continuity_ui::LiveReload;
use continuity_win::{set_per_monitor_dpi_v2, ComGuard};

use main_initial_requests::{build_initial_requests, mark_clean_exit};
use registry::{make_channel, run, RegistryCtx, RegistryRuntime, SpawnRequest};
use registry_file_buffers::{
    build_file_buffer_index, file_buffer_for_path, register_file_buffer, FileBufferIndex,
};
use runtime_paths::StartupOptions;
use single_instance::{claim_or_forward, spawn_instance_hub, InstanceClaim};

fn main() -> Result<()> {
    // The main thread also touches COM (used by the desktop manager + later
    // by any modal dialogs we open from the registry hub itself).
    let _com = ComGuard::new()?;
    set_per_monitor_dpi_v2()?;
    let startup_options = StartupOptions::from_env()?;
    startup_options.runtime_paths.apply_process_overrides();

    let db = db_path().context("resolving continuity database path")?;

    // Single-instance handoff: a second launch would otherwise replay the
    // persisted window session and duplicate every open window. Forward
    // this launch's paths to the running instance and exit instead. The
    // e2e insert hook and `--new-instance` bypass the check (their data
    // dirs are isolated / intentionally shared).
    let bypass_single_instance =
        startup_options.new_instance || std::env::var_os("CONTINUITY_E2E_INSERT").is_some();
    let instance_guard = if bypass_single_instance {
        None
    } else {
        match claim_or_forward(&db, &startup_options.startup_paths) {
            InstanceClaim::Primary(guard) => guard,
            InstanceClaim::Forwarded => return Ok(()),
        }
    };

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
    let mut initial_requests = build_initial_requests(&persist.client(), &editor, &db)?;
    let file_buffer_index = build_file_buffer_index(&initial_requests, &editor);
    attach_startup_open_files(
        &mut initial_requests,
        &editor,
        &startup_options.startup_paths.files,
        &file_buffer_index,
    );
    attach_startup_open_folders(
        &mut initial_requests,
        &startup_options.startup_paths.folders,
    );
    // The hub only exists in a claimed primary; bypassed instances must
    // not receive forwards meant for the real session.
    let instance_hub = instance_guard
        .as_ref()
        .and_then(|_| spawn_instance_hub(&db, editor.clone(), tx.clone()));
    let ctx = RegistryCtx {
        persist: persist.client(),
        editor: editor.clone(),
        default_keymap_toml: DEFAULT_KEYMAP_TOML,
        user_keymap_path: user_keymap_path.clone(),
        tx,
        live_reload: Some(live_reload),
        file_io: file_io.client(),
        file_buffer_index,
    };
    let runtime = RegistryRuntime {
        config_rx,
        persist_event_rx: Some(persist.events()),
        backup: Some(Arc::clone(&backup)),
    };
    run(ctx, rx, runtime, initial_requests).context("registry loop")?;

    // Stop accepting forwarded launches and release the instance claim
    // first, so a relaunch racing this shutdown becomes the new primary
    // instead of forwarding into a dying process.
    drop(instance_hub);
    drop(instance_guard);

    // The registry loop returns only when every window closed gracefully.
    // Mark the exit clean so the next launch starts blank instead of
    // restoring this (intentionally closed) session. A crash, panic, or
    // kill skips this line, leaving the marker absent ⇒ next launch
    // restores. The marker lives beside the database so it tracks the
    // active data dir (and stays out of unrelated test data dirs).
    mark_clean_exit(&db);

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
    requests: &mut Vec<SpawnRequest>,
    editor: &Arc<EditorHandle>,
    paths: &[PathBuf],
    file_buffer_index: &FileBufferIndex,
) {
    if paths.is_empty() {
        return;
    }
    let mut file_requests = Vec::new();
    let mut failed_notices = Vec::new();
    for path in paths {
        let mut notices = Vec::new();
        let buffer_id = match file_buffer_for_path(editor, file_buffer_index, path) {
            Some(buffer_id) => Some(buffer_id),
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
                    Some(buffer_id)
                }
                Err(e) => {
                    let message = format!("Open failed for {}: {e}", path.display());
                    eprintln!("continuity: {message}");
                    failed_notices.push(message);
                    None
                }
            },
        };
        if let Some(buffer_id) = buffer_id {
            file_requests.push(startup_file_spawn_request(
                buffer_id,
                notices,
                file_requests.len(),
            ));
        }
    }
    if file_requests.is_empty() {
        if let Some(first) = requests.first_mut() {
            first.recovery_notices.append(&mut failed_notices);
        }
        return;
    }
    if let Some(first) = requests.first_mut() {
        first.open_tutorial_on_init = false;
    }
    if should_replace_initial_blank_request(requests, editor) {
        let mut first_file = file_requests.remove(0);
        if let Some(existing_first) = requests.first_mut() {
            let mut existing_notices = std::mem::take(&mut existing_first.recovery_notices);
            existing_notices.append(&mut failed_notices);
            existing_notices.append(&mut first_file.recovery_notices);
            first_file.recovery_notices = existing_notices;
            *existing_first = first_file;
        }
    } else if let Some(first) = requests.first_mut() {
        first.recovery_notices.append(&mut failed_notices);
    }
    requests.extend(file_requests);
}

pub(crate) fn startup_file_spawn_request(
    buffer_id: BufferId,
    recovery_notices: Vec<String>,
    ordinal: usize,
) -> SpawnRequest {
    SpawnRequest {
        initial_buffer_id: buffer_id,
        restored: None,
        activate_on_restore: false,
        explicit_origin: startup_file_window_origin(ordinal),
        cascade_from: None,
        recovery_notices,
        open_tutorial_on_init: false,
        startup_open_buffer_ids: Vec::new(),
        startup_folder_roots: Vec::new(),
        reconcile_on_init: None,
    }
}

pub(crate) fn startup_file_window_origin(ordinal: usize) -> Option<(i32, i32)> {
    const STARTUP_WINDOW_SIZE: (i32, i32) = (1200, 800);
    const STARTUP_CASCADE_STEP_PX: i32 = 30;
    let (x, y) = continuity_win::centered_origin_on_focused_monitor(STARTUP_WINDOW_SIZE)?;
    let ordinal = i32::try_from(ordinal).unwrap_or(i32::MAX / STARTUP_CASCADE_STEP_PX);
    let offset = ordinal.saturating_mul(STARTUP_CASCADE_STEP_PX);
    Some((x.saturating_add(offset), y.saturating_add(offset)))
}

fn should_replace_initial_blank_request(
    requests: &[SpawnRequest],
    editor: &Arc<EditorHandle>,
) -> bool {
    let [request] = requests else {
        return false;
    };
    if request.restored.is_some()
        || request.explicit_origin.is_some()
        || request.cascade_from.is_some()
        || !request.startup_open_buffer_ids.is_empty()
        || !request.startup_folder_roots.is_empty()
        || !request.recovery_notices.is_empty()
    {
        return false;
    }
    let Some(snapshot) = editor.snapshot(request.initial_buffer_id) else {
        return false;
    };
    snapshot.file.is_none() && snapshot.rope_snapshot().rope().len_bytes() == 0
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
