//! `WindowConfig` and `WindowCommands` — the construction-time inputs to
//! [`crate::Window::new`].
//!
//! Lives next to `window.rs` to keep that file under the 600-line cap. No
//! runtime state here; just plain data structs.

use std::path::PathBuf;
use std::sync::Arc;

use continuity_buffer::{BufferId, FileAssociation};
use continuity_command::Registry;
use continuity_keymap::Keymap;
use continuity_persist::PersistClient;

/// Window construction parameters.
pub struct WindowConfig {
    /// Title shown in the title bar.
    pub title: String,
    /// Initial window width in pixels (outer, including borders).
    pub width: i32,
    /// Initial window height in pixels.
    pub height: i32,
    /// Optional initial outer top-left in screen pixels. `None` ⇒
    /// `CW_USEDEFAULT` (Win32 picks the position). Used by
    /// `window.new_window` / tear-off to cascade from the focused window.
    /// Ignored when a restored placement blob takes over after creation.
    pub initial_origin: Option<(i32, i32)>,
    /// Whether [`crate::Window::run`] may bring the window to the
    /// foreground when it is first shown. `true` for user-initiated
    /// windows (new window, tear-off, file open); `false` for
    /// session-restored windows other than the most recent one, so a
    /// multi-window launch doesn't flash focus across every restored
    /// window. Forced `false` at placement time when the window is
    /// moved to a non-active virtual desktop — activating it there
    /// would switch the user's desktop.
    pub activate_on_show: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "continuity".into(),
            width: 1200,
            height: 800,
            initial_origin: None,
            activate_on_show: true,
        }
    }
}

/// File-open request that should land in a fresh top-level window.
pub struct OpenFileWindowRequest {
    /// Decoded file content. The app registry may ignore this when an
    /// existing file-associated buffer already represents `file`.
    pub content: String,
    /// Filesystem association metadata captured by the file-I/O worker.
    pub file: FileAssociation,
    /// Exact requested origin. Rarely used for file opens; `None` lets
    /// the registry cascade from the source window on its monitor.
    pub explicit_origin: Option<(i32, i32)>,
    /// Source window outer rect in screen pixels. Used by the registry
    /// to place the spawned window on the focused/source monitor.
    pub cascade_from: Option<(i32, i32, i32, i32)>,
    /// Launch-time banners to show in the spawned window.
    pub recovery_notices: Vec<String>,
}

/// Callback used by UI-owned file commands to ask the app registry for
/// a fresh top-level window showing a file buffer.
pub type OpenFileWindow = Arc<dyn Fn(OpenFileWindowRequest) + Send + Sync>;

/// Callback used by save/open paths to keep the app registry's
/// file-path-to-buffer index current across windows.
pub type RegisterFileBuffer = Arc<dyn Fn(BufferId, FileAssociation) + Send + Sync>;

/// Command and keymap state used by a window.
pub struct WindowCommands {
    /// Registered command handlers.
    pub registry: Registry,
    /// Active layered keymap.
    pub keymap: Keymap,
    /// Compiled-in default keymap TOML, used by `keymap.reload`.
    pub default_keymap_toml: &'static str,
    /// Optional user keymap path layered over the default on reload.
    pub user_keymap_path: Option<PathBuf>,
    /// Phase-12 live-reload metadata: initial settings + paths +
    /// persistence-mode applier. The watcher receiver itself is no longer
    /// per-window — the registry owns the watcher and fans out
    /// [`crate::WindowControl::ConfigChanged`] events through `control_rx`.
    /// `None` disables `settings.open` (e.g. tests / headless windows).
    pub live_reload: Option<crate::live_reload::LiveReload>,
    /// Phase-16.5 per-window control receiver from the registry. Drained
    /// from the same `WM_TIMER` tick that used to read the watcher
    /// directly. `None` disables registry-driven control flow (tests).
    pub control_rx: Option<crate::WindowControlRx>,
    /// Phase-14 per-window persistence + virtual-desktop restoration.
    /// `None` disables save/restore (test windows, the canary harness).
    pub persistence: Option<crate::window_placement_persistence::WindowPersistence>,
    /// Phase-15 file-I/O worker client. `None` disables file commands.
    pub file_io: Option<crate::file_io::FileIoClient>,
    /// Registry-owned file open router. When present, file opens spawn a
    /// fresh top-level window instead of inserting a tab into this one.
    pub open_file_window: Option<OpenFileWindow>,
    /// Registry-owned file-buffer index updater. Save-as uses this to
    /// make future opens of the same path reuse the saved buffer.
    pub register_file_buffer: Option<RegisterFileBuffer>,
    /// Phase-I2 metrics persistence client. Used to record per-keystroke
    /// WPM samples and to purge the `metrics_daily` table on the
    /// `metrics.purge` command. `None` disables metrics recording (test
    /// windows, headless canary).
    pub persist_client: Option<PersistClient>,
    /// δ.3 — banner strings to raise at window construction, before the
    /// first paint. Used by the registry to surface recovery halts
    /// (checksum mismatch, decode failure) that previously went only
    /// to `stderr`. Multiple notices are joined into a single transient
    /// banner; empty for windows without launch-time notices.
    pub initial_banners: Vec<String>,
    /// First-launch flag: when `true`, [`crate::Window::new`] dispatches
    /// `help.tutorial` immediately after the window is initialised so
    /// the tutorial tab is the first thing the user sees on a fresh
    /// install. The registry sets this only for the very first
    /// `SpawnRequest` of the first launch (presence of the
    /// `tutorial_seen` sentinel disables it on every subsequent run).
    pub open_tutorial_on_init: bool,
    /// Extra buffers to adopt into the focused pane after session
    /// restore. File opens normally arrive as separate windows; this is
    /// retained for callers that intentionally seed tabs.
    pub startup_open_buffer_ids: Vec<BufferId>,
    /// Folder roots supplied at process startup.
    pub startup_folder_roots: Vec<PathBuf>,
}
