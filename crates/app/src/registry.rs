//! Multi-window registry (Phase 14, evolved by Phase 16.5).
//!
//! Each top-level [`Window`] runs on its own UI thread. The registry owns
//! the channel through which threads request new windows and announce
//! their exit. The main thread sits in the registry loop until the live
//! count returns to zero.
//!
//! ## Phase 16.5 evolution
//!
//! - [`RegistryEvent::Closed`] now carries the closing window's id. Every
//!   graceful close — including the final window — archives the window to
//!   the closed-history stack and tombstones its row. Intentional quits
//!   therefore leave the next launch clean; only a crash (which never
//!   reaches this handler) leaves rows behind to auto-restore.
//! - The [`continuity_config::SettingsWatcher`] is owned here, not in the
//!   first window. The registry's main loop multiplexes its receiver
//!   alongside [`RegistryEvent`]s and fans
//!   [`continuity_ui::WindowControl::ConfigChanged`] out to every live
//!   window through that window's dedicated control channel.
//! - Settings changes that affect non-window owners (backup cadence,
//!   persistence mode) are routed through *typed* owner methods —
//!   [`BackupScheduler::set_config`] and [`PersistClient::set_synchronous`]
//!   — not through shared mutable config.
//!
//! Single-writer rule: every [`Window`] is constructed and used only
//! inside its dedicated thread (HWND owner), and persistence callbacks are
//! invoked only from that thread. The registry channel is the
//! cross-thread funnel.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::SystemTime;

use crate::error::Error;
use crate::registry_closed_history::{archive_closed_window, smart_reopen_handler};
use crate::registry_file_buffers::{
    make_open_file_window_handler, make_register_file_buffer_handler, FileBufferIndex,
};
use continuity_buffer::{BufferId, WindowId};
use continuity_command::{
    register_buffer_history_commands, register_clipboard_commands, register_diagnostics_commands,
    register_editor_primitives, register_file_commands, register_help_commands,
    register_indent_commands, register_keymap_commands, register_markdown_commands,
    register_markdown_links_clipboard, register_motion_extras, register_pane_commands,
    register_rich_editing, register_search_commands, register_selection_commands,
    register_settings_commands, register_spell_commands, register_tab_commands,
    register_theme_commands, register_undo_commands, register_view_commands,
    register_window_commands, Registry,
};
use continuity_config::{ConfigEvent, Settings};
use continuity_core::{EditorHandle, SnapshotPolicy};
use continuity_keymap::Keymap;
use continuity_persist::{BackupConfig, BackupScheduler, PersistClient, WindowRow};
use continuity_ui::file_io::FileIoClient;
use continuity_ui::{
    LiveReload, RestoredState, Window, WindowCommands, WindowConfig, WindowControl,
    WindowControlRx, WindowControlTx, WindowPersistence, WindowStateSnapshot,
};
use continuity_win::ComGuard;
use crossbeam_channel::{select, unbounded, Receiver, Sender};

/// Cross-thread events flowing into the registry's main loop.
pub enum RegistryEvent {
    /// A UI thread is asking the registry to spawn another window. The
    /// registry handles the spawn on the main thread (the only one that
    /// owns the persist client + spawn machinery).
    Spawn(SpawnRequest),
    /// A UI thread has finished its message pump. The registry archives
    /// the window to the closed-history stack and tombstones its row —
    /// for every graceful close, including the last window. A crash never
    /// sends this event, so its rows survive and that session restores.
    Closed {
        /// Which window finished. `None` when the close came from a thread
        /// that never reached Window construction (in which case there's
        /// nothing to tombstone).
        window_id: Option<WindowId>,
    },
}

/// What kind of window the registry is being asked to open.
pub(crate) struct SpawnRequest {
    /// Initial buffer in the new window's singleton tree.
    pub initial_buffer_id: BufferId,
    /// Optional restored state (used at launch time when reopening
    /// a saved window). `None` means a brand-new window.
    pub restored: Option<(WindowId, RestoredState)>,
    /// Exact initial outer top-left in screen pixels. Runtime drag
    /// tear-off uses this so the spawned window lands where the tab was
    /// dropped. Ignored when restored placement takes over.
    pub explicit_origin: Option<(i32, i32)>,
    /// Outer rect of the window the user spawned this from (in screen
    /// pixels). The registry cascades from this rect when present and a
    /// restored placement is *not* taking over. `None` ⇒ let Win32 pick
    /// the position (`CW_USEDEFAULT`).
    pub cascade_from: Option<(i32, i32, i32, i32)>,
    /// δ.3 — pre-formatted recovery halt banners. Threaded through to
    /// the spawning window's [`WindowCommands::initial_banners`] so
    /// `Window::new` raises the first transient `FileBanner` at startup.
    /// Empty for runtime-spawned windows (`window.new_window`,
    /// tear-off).
    pub recovery_notices: Vec<String>,
    /// First-launch flag — set only on the very first `SpawnRequest`
    /// when `%APPDATA%\continuity\.tutorial_seen` is missing. Causes
    /// `Window::new` to dispatch `help.tutorial` so the tutorial is
    /// the active tab on a fresh install. The sentinel is touched as
    /// part of the same operation so subsequent launches see
    /// `false` (idempotent).
    pub open_tutorial_on_init: bool,
    /// Extra buffers to adopt as tabs during window construction.
    /// Runtime and startup file opens normally use separate
    /// [`SpawnRequest`]s so each file lands in its own top-level window.
    pub startup_open_buffer_ids: Vec<BufferId>,
    /// Folder roots supplied on the process command line.
    pub startup_folder_roots: Vec<PathBuf>,
}

/// Shared, clone-able registry context handed to each window thread.
#[derive(Clone)]
pub(crate) struct RegistryCtx {
    /// Persist client used by every window for state save/delete and by
    /// the registry for typed owner messages
    /// ([`PersistClient::set_synchronous`]).
    pub persist: PersistClient,
    /// Editor handle (single shared editor core thread). Wrapped in `Arc`
    /// so each window thread holds a reference to the same handle; the
    /// last drop joins the core thread.
    pub editor: Arc<EditorHandle>,
    /// Compiled-in default keymap TOML, layered with the user keymap.
    pub default_keymap_toml: &'static str,
    /// `%APPDATA%\continuity\keymap.toml`, when defined.
    pub user_keymap_path: Option<PathBuf>,
    /// Cross-thread channel into the registry main loop.
    pub tx: Sender<RegistryEvent>,
    /// Phase-12/-16.5 live-reload metadata (paths + initial settings +
    /// persist-mode applier). Cloned to every window so every window can
    /// surface `settings.open` and apply the user's TOML at launch.
    /// The watcher receiver itself lives on the registry, not here.
    pub live_reload: Option<LiveReload>,
    /// Shared file-I/O worker client.
    pub file_io: FileIoClient,
    /// Cross-window file-path-to-buffer index used to reuse an existing
    /// file buffer when opening the same file into a fresh window.
    pub file_buffer_index: FileBufferIndex,
}

/// Inputs to the registry's main loop that aren't routed through
/// [`RegistryEvent`].
pub(crate) struct RegistryRuntime {
    /// Watcher events surface here. `None` when the watcher couldn't be
    /// constructed (no `%APPDATA%\continuity` directory, etc.).
    pub config_rx: Option<Receiver<ConfigEvent>>,
    /// δ.3 — persistence-thread events. Drained alongside config
    /// events and fanned out to every live window via
    /// [`WindowControl::PersistEvent`]. `None` only if the caller
    /// doesn't want to surface persist failures (test windows).
    pub persist_event_rx: Option<Receiver<continuity_persist::PersistEvent>>,
    /// Owner-side reference to the backup scheduler; the registry calls
    /// [`BackupScheduler::set_config`] on it when the user updates
    /// `[backup]`.
    pub backup: Option<Arc<BackupScheduler>>,
}

/// Run the registry's main loop until every window has exited.
///
/// `rx` is the receiver paired with `ctx.tx` (built via [`make_channel`]).
/// Pre-fills the channel with one [`RegistryEvent::Spawn`] per restored
/// row (or a single fresh-window spawn on first launch), then handles
/// further `Spawn` requests originated by `window.new_window` /
/// `window.tear_off_focused_tab` commands.
///
/// # Errors
///
/// Returns the error of the first failed thread spawn. Errors emitted
/// inside an already-running window thread are logged on that thread and
/// surface as `Closed` events — the loop continues.
pub fn run(
    ctx: RegistryCtx,
    rx: Receiver<RegistryEvent>,
    runtime: RegistryRuntime,
    initial: Vec<SpawnRequest>,
) -> Result<(), Error> {
    let mut state = LiveState::default();
    for req in initial {
        ctx.tx
            .send(RegistryEvent::Spawn(req))
            .map_err(|_| Error::RegistryClosed)?;
    }

    // Destructure runtime up front so the receiver and the backup arc
    // can be borrowed independently below — `select!` borrows
    // `config_rx`, while `apply_owner_routed_settings` borrows `backup`.
    let RegistryRuntime {
        config_rx,
        persist_event_rx,
        backup,
    } = runtime;
    // The watcher receiver may be `None` (couldn't construct it). The
    // select! arms below handle both cases by sourcing from a never-
    // disconnecting fallback receiver when there is no real watcher.
    let never: Receiver<ConfigEvent> = unbounded().1;
    let config_rx = config_rx.unwrap_or(never);
    // δ.3 — same pattern for persist events. The variable holds a
    // live receiver while the persist thread is healthy; after a
    // disconnect (clean or panic) we swap to the never receiver so
    // the select! arm goes idle instead of spinning on Err.
    let never_persist: Receiver<continuity_persist::PersistEvent> = unbounded().1;
    let mut persist_event_rx = persist_event_rx.unwrap_or_else(|| never_persist.clone());
    let mut persist_stopped_announced = false;

    loop {
        select! {
            recv(rx) -> ev => match ev {
                Ok(RegistryEvent::Spawn(req)) => {
                    spawn_window_thread(&ctx, &mut state, req)?;
                }
                Ok(RegistryEvent::Closed { window_id }) => {
                    // Every *graceful* close — one window among many or the
                    // final window — archives the window to the closed-
                    // history stack and tombstones its row. The last window
                    // is no longer preserved for auto-restore: an
                    // intentional quit leaves the next launch clean, and
                    // Ctrl+Shift+T (closed history) brings the window back.
                    // A crash never reaches this handler, so its rows
                    // survive and that session *is* restored next launch.
                    if let Some(id) = window_id {
                        state.control_senders.remove(&id);
                        archive_closed_window(&ctx.persist, id);
                    }
                    state.live = state.live.saturating_sub(1);
                    if state.live == 0 {
                        return Ok(());
                    }
                }
                Err(_) => return Ok(()), // tx side dropped
            },
            recv(config_rx) -> ev => {
                if let Ok(event) = ev {
                    fan_out_config_event(&ctx, backup.as_ref(), &state, event);
                }
                // A disconnected watcher is non-fatal — windows keep running.
            },
            recv(persist_event_rx) -> ev => {
                match ev {
                    Ok(event) => {
                        let is_thread_stopped =
                            matches!(event, continuity_persist::PersistEvent::ThreadStopped);
                        fan_out_persist_event(&state, event);
                        if is_thread_stopped {
                            persist_stopped_announced = true;
                            // Stop polling — further recvs would only
                            // see a channel-disconnect.
                            persist_event_rx = never_persist.clone();
                        }
                    }
                    Err(_) => {
                        // Channel disconnected without a clean
                        // ThreadStopped — the persist thread panicked
                        // rather than exiting via the Shutdown
                        // message path. Synthesize the banner so the
                        // user still sees the durability surface.
                        if !persist_stopped_announced {
                            fan_out_persist_event(
                                &state,
                                continuity_persist::PersistEvent::ThreadStopped,
                            );
                            persist_stopped_announced = true;
                        }
                        persist_event_rx = never_persist.clone();
                    }
                }
            },
        }
    }
}

/// Construct the registry channel.
#[must_use]
pub(crate) fn make_channel() -> (Sender<RegistryEvent>, Receiver<RegistryEvent>) {
    unbounded::<RegistryEvent>()
}

#[derive(Default)]
struct LiveState {
    /// Number of currently-running window threads.
    live: usize,
    /// Per-window control sender. Used to fan out
    /// [`WindowControl::ConfigChanged`] events to live windows.
    control_senders: HashMap<WindowId, WindowControlTx>,
}

fn spawn_window_thread(
    ctx: &RegistryCtx,
    state: &mut LiveState,
    req: SpawnRequest,
) -> Result<(), Error> {
    let ctx_for_thread = ctx.clone();
    // Stable id for this window — generated up front so we can wire the
    // control sender into LiveState before the thread starts.
    let window_id = req.restored.as_ref().map(|(id, _)| *id).unwrap_or_default();
    let (control_tx, control_rx) = unbounded::<WindowControl>();
    state.control_senders.insert(window_id, control_tx);
    state.live += 1;
    thread::Builder::new()
        .name("continuity-window".into())
        .spawn(move || {
            let res = run_window(ctx_for_thread.clone(), req, window_id, control_rx);
            if let Err(e) = res {
                eprintln!("continuity: window thread exited with error: {e}");
            }
            // Always announce the close; missing it would make the registry
            // hang forever waiting for live count to drop.
            let _ = ctx_for_thread.tx.send(RegistryEvent::Closed {
                window_id: Some(window_id),
            });
        })
        .map_err(Error::SpawnThread)?;
    Ok(())
}

fn run_window(
    ctx: RegistryCtx,
    req: SpawnRequest,
    window_id: WindowId,
    control_rx: WindowControlRx,
) -> Result<(), Error> {
    let _com = ComGuard::new()?;
    let registry = build_registry(&ctx);
    let keymap = load_keymap(ctx.default_keymap_toml, ctx.user_keymap_path.as_ref())?;
    let initial_state = req.restored.as_ref().map(|(_, s)| s.clone());
    let persistence = make_persistence(&ctx, window_id, initial_state);
    const DEFAULT_WINDOW_WIDTH: i32 = 1200;
    const DEFAULT_WINDOW_HEIGHT: i32 = 800;
    // Cascade from the source window when present, but only when this
    // spawn isn't going to be repositioned by a restored placement blob
    // (which would otherwise snap the window back to its persisted spot
    // on the next paint tick). Standard Win32 cascade step is 30 px.
    const CASCADE_STEP_PX: i32 = 30;
    let initial_origin = if req.restored.is_some() {
        None
    } else if let Some(origin) = req.explicit_origin {
        Some(origin)
    } else {
        req.cascade_from.and_then(|rect| {
            continuity_win::cascade_origin_on_source_monitor(
                rect,
                (DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT),
                CASCADE_STEP_PX,
            )
            .or_else(|| {
                Some((
                    rect.0.saturating_add(CASCADE_STEP_PX),
                    rect.1.saturating_add(CASCADE_STEP_PX),
                ))
            })
        })
    };
    let window = Window::new(
        WindowConfig {
            title: "continuity".into(),
            width: DEFAULT_WINDOW_WIDTH,
            height: DEFAULT_WINDOW_HEIGHT,
            initial_origin,
        },
        Arc::clone(&ctx.editor),
        req.initial_buffer_id,
        WindowCommands {
            registry,
            keymap,
            default_keymap_toml: ctx.default_keymap_toml,
            user_keymap_path: ctx.user_keymap_path.clone(),
            live_reload: ctx.live_reload.clone(),
            control_rx: Some(control_rx),
            persistence: Some(persistence),
            file_io: Some(ctx.file_io.clone()),
            open_file_window: Some(make_open_file_window_handler(&ctx)),
            register_file_buffer: Some(make_register_file_buffer_handler(&ctx)),
            persist_client: Some(ctx.persist.clone()),
            initial_banners: req.recovery_notices,
            open_tutorial_on_init: req.open_tutorial_on_init,
            startup_open_buffer_ids: req.startup_open_buffer_ids,
            startup_folder_roots: req.startup_folder_roots,
        },
    )?;
    window.run()?;
    Ok(())
}

fn make_persistence(
    ctx: &RegistryCtx,
    window_id: WindowId,
    initial: Option<RestoredState>,
) -> WindowPersistence {
    let persist_for_save = ctx.persist.clone();
    let id_for_save = window_id;
    WindowPersistence {
        window_id,
        initial,
        save: Arc::new(move |snap: WindowStateSnapshot| {
            let row = WindowRow {
                id: id_for_save,
                virtual_desktop_guid: snap.virtual_desktop_guid,
                monitor_id: snap.monitor_id,
                placement_blob: snap.placement_blob,
                pane_tree_json: snap.pane_tree_json,
                last_seen_ms: unix_ms_now(),
            };
            if let Err(e) = persist_for_save.save_window(row) {
                eprintln!("continuity: save_window failed: {e}");
            }
        }),
    }
}

fn build_registry(ctx: &RegistryCtx) -> Registry {
    let mut registry = Registry::new();
    register_editor_primitives(&mut registry);
    register_diagnostics_commands(&mut registry);
    register_selection_commands(&mut registry);
    register_keymap_commands(&mut registry);
    register_motion_extras(&mut registry);
    register_rich_editing(&mut registry);
    register_indent_commands(&mut registry);
    register_markdown_commands(&mut registry);
    register_markdown_links_clipboard(&mut registry);
    register_undo_commands(&mut registry);
    register_view_commands(&mut registry);
    register_search_commands(&mut registry);
    register_settings_commands(&mut registry);
    register_pane_commands(&mut registry);
    register_tab_commands(&mut registry);
    register_theme_commands(&mut registry);
    register_file_commands(&mut registry);
    register_clipboard_commands(&mut registry);
    register_help_commands(&mut registry);
    register_buffer_history_commands(&mut registry);
    register_spell_commands(&mut registry);
    let editor_for_new = Arc::clone(&ctx.editor);
    let tx_new = ctx.tx.clone();
    let new_window_handler = move |_args: &serde_json::Value,
                                   ctx: &mut dyn continuity_command::Context|
          -> Result<(), continuity_command::Error> {
        let buffer_id = editor_for_new.open_buffer("");
        let _ = tx_new.send(RegistryEvent::Spawn(SpawnRequest {
            initial_buffer_id: buffer_id,
            restored: None,
            explicit_origin: None,
            cascade_from: ctx.current_window_rect(),
            recovery_notices: Vec::new(),
            open_tutorial_on_init: false,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: Vec::new(),
        }));
        Ok(())
    };
    let tx_tear = ctx.tx.clone();
    let tear_off_handler = move |args: &serde_json::Value,
                                 ctx: &mut dyn continuity_command::Context|
          -> Result<(), continuity_command::Error> {
        let cascade_from = ctx.current_window_rect();
        let explicit_origin = parse_tear_off_origin(args);
        let buffer_id = ctx.tear_off_focused_tab()?;
        let _ = tx_tear.send(RegistryEvent::Spawn(SpawnRequest {
            initial_buffer_id: buffer_id,
            restored: None,
            explicit_origin,
            cascade_from,
            recovery_notices: Vec::new(),
            open_tutorial_on_init: false,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: Vec::new(),
        }));
        Ok(())
    };
    register_window_commands(&mut registry, new_window_handler, tear_off_handler);
    // Re-register `tab.reopen_closed` with the registry-aware smart
    // handler — pops the most recent unit from either the local
    // recently-closed list OR the schema-v5 closed-history stack.
    // Must run after `register_tab_commands` so this registration
    // replaces the default.
    let predicate = continuity_command::ContextPredicate::parse("editor.focused");
    registry.register(
        continuity_command::TAB_REOPEN_CLOSED,
        predicate,
        smart_reopen_handler(ctx.persist.clone(), Arc::clone(&ctx.editor), ctx.tx.clone()),
    );
    registry
}

fn parse_tear_off_origin(args: &serde_json::Value) -> Option<(i32, i32)> {
    let x = args.get("drop_screen_x")?.as_i64()?;
    let y = args.get("drop_screen_y")?.as_i64()?;
    let x = i32::try_from(x).ok()?;
    let y = i32::try_from(y).ok()?;
    Some((x, y))
}

/// δ.3 — fan a persistence-thread event out to every live window.
/// Wraps the event in [`WindowControl::PersistEvent`] so the window's
/// existing control-poll tick handles it like a config event.
fn fan_out_persist_event(state: &LiveState, event: continuity_persist::PersistEvent) {
    for tx in state.control_senders.values() {
        let _ = tx.send(WindowControl::PersistEvent(event.clone()));
    }
}

/// Apply the owner-side effects of a settings change *before* fanning the
/// event out to live windows. Keeps per-owner config (backup cadence,
/// persist sync mode) on its single owner thread instead of through
/// shared mutable state.
fn fan_out_config_event(
    ctx: &RegistryCtx,
    backup: Option<&Arc<BackupScheduler>>,
    state: &LiveState,
    event: ConfigEvent,
) {
    if let ConfigEvent::Settings(settings) = &event {
        apply_owner_routed_settings(ctx, backup, settings.as_ref());
        // Update the shared `LiveReload.initial` cell so any window
        // spawned *after* this commit observes the new settings on
        // its `maybe_apply_initial_settings` call. The watcher
        // fanout below only reaches windows that are already live;
        // without this replace, a new-window construction triggered
        // right after a commit would replay the process-start
        // snapshot and ignore the runtime change.
        if let Some(reload) = ctx.live_reload.as_ref() {
            reload.replace_settings(settings.as_ref().clone());
        }
    }
    for tx in state.control_senders.values() {
        let _ = tx.send(WindowControl::ConfigChanged(event.clone()));
    }
}

fn apply_owner_routed_settings(
    ctx: &RegistryCtx,
    backup: Option<&Arc<BackupScheduler>>,
    settings: &Settings,
) {
    // Persistence mode → persist owner via typed message.
    let pragma = settings.persistence_mode().synchronous_pragma();
    if let Err(e) = ctx.persist.set_synchronous(pragma) {
        eprintln!("continuity: set_synchronous({pragma}) failed: {e}");
    }
    // Backup cadence + retention → backup-scheduler owner via typed message.
    if let Some(backup) = backup {
        let backup_dir = continuity_persist::backups_dir().unwrap_or_else(|_| PathBuf::from("."));
        let interval =
            std::time::Duration::from_secs(u64::from(settings.backup.interval_minutes) * 60);
        let retain = settings.backup.hourly_retention as usize;
        backup.set_config(BackupConfig {
            directory: backup_dir,
            interval,
            retain,
        });
    }
    // Snapshot policy → core owner via typed message.
    // `interval_ms` is not user-tunable in `settings.toml` today, so
    // carry the previous default forward — only the byte/edit thresholds
    // come from settings.
    let policy = SnapshotPolicy {
        edits: settings.persistence.snapshot_every_edits,
        bytes: settings.persistence.snapshot_every_bytes as usize,
        interval_ms: SnapshotPolicy::default().interval_ms,
    };
    ctx.editor.set_snapshot_policy(policy);
}

fn load_keymap(default_toml: &str, user_path: Option<&PathBuf>) -> Result<Keymap, Error> {
    let base = Keymap::from_toml(default_toml)?;
    let Some(path) = user_path else {
        return Ok(base);
    };
    if !path.exists() {
        return Ok(base);
    }
    let user_toml = std::fs::read_to_string(path).map_err(|source| Error::ReadKeymap {
        path: path.clone(),
        source,
    })?;
    let user = Keymap::from_toml(&user_toml)?;
    Ok(Keymap::layered(base, user))
}

fn unix_ms_now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
