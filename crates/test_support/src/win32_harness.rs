//! Hidden Win32 window harness for end-to-end UI tests (Phase 17.9 §C1).
//!
//! Spawns a real `continuity_ui::Window` on a dedicated worker thread,
//! runs the production message pump via [`Window::run_hidden`], and
//! exposes a test-thread handle that can dispatch input messages and
//! inspect editor state. The wndproc, renderer, layout cache,
//! decoration pool — every part of the production paint pipeline —
//! runs through this harness. The only differences from a normal
//! window are:
//!
//! 1. The window is positioned far off-screen and never `ShowWindow`-ed.
//! 2. Live-reload, file-IO, persistence, and per-window control
//!    channels are `None` (matches the `WindowCommands` documented
//!    "tests" path).
//! 3. The keymap is empty so tests can drive input by sending raw
//!    `WM_CHAR` / `WM_KEYDOWN` through the production message
//!    pipeline rather than going through chord lookup.
//!
//! Thread model: the worker thread owns the HWND and runs the message
//! pump. The test thread holds an `Arc<EditorHandle>` for state
//! inspection. Cross-thread input goes through `SendMessageW` /
//! `PostMessageW`, both of which are documented thread-safe — the OS
//! marshals the call onto the window's owning thread. Cleanup
//! (`Drop`) posts `WM_CLOSE` and joins the worker.

use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use continuity_buffer::BufferId;
use continuity_command::{
    register_editor_primitives, register_pane_commands, register_rich_editing,
    register_tab_commands, Registry,
};
use continuity_core::{Clock, EditorHandle};
use continuity_keymap::Keymap;
use continuity_persist::PersistHandle;
use continuity_ui::window_config::{WindowCommands, WindowConfig};
use continuity_ui::window_control::{WindowControl, WindowControlTx};
use continuity_ui::window_timers::CONFIG_POLL_TIMER_ID;
use continuity_ui::Window;
use continuity_win::ComGuard;
use crossbeam_channel::unbounded;
use tempfile::TempDir;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, SendMessageW, WM_CHAR, WM_CLOSE, WM_DPICHANGED, WM_KEYDOWN, WM_KEYUP, WM_PAINT,
    WM_TIMER,
};

/// Test keymap shipped with the harness. Binds **modifier-free**
/// chords (F-keys) to commands tests need to drive, because
/// `GetKeyState`-based modifier reads on the wndproc thread don't
/// reliably observe modifier state synthesized from another thread
/// via `SendMessageW`. Tests that need a different binding can in
/// principle wrap the harness; today the F-key set covers C2–C5.
///
/// F1  → pane.split_horizontal (§C3)
/// F2  → pane.split_vertical
/// F3  → tab.close
/// F4  → editor.insert_newline_smart  (ε.7 — lets edit-apply trace
///                                       tests drive the smart-newline
///                                       UI funnel without modifier
///                                       chord plumbing; plain Enter
///                                       (`WM_CHAR 0x0d`) is filtered
///                                       by `window_commanding`'s
///                                       `code < 0x20` guard.)
/// F5  → pane.layout_single
const TEST_KEYMAP_TOML: &str = "\
[[binding]]
keys = [\"F1\"]
command = \"pane.split_horizontal\"

[[binding]]
keys = [\"F2\"]
command = \"pane.split_vertical\"

[[binding]]
keys = [\"F3\"]
command = \"tab.close\"

[[binding]]
keys = [\"F4\"]
command = \"editor.insert_newline_smart\"

[[binding]]
keys = [\"F5\"]
command = \"pane.layout_single\"
";

/// Wall-clock implementation for the harness's editor thread.
struct WallClock;

impl Clock for WallClock {
    fn now_ms(&self) -> i64 {
        i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        )
        .unwrap_or(0)
    }
}

/// Handle to a spawned hidden Win32 window driven by the production
/// wndproc + Renderer pipeline.
///
/// The struct is `Send` because the HWND is a stable handle the
/// Win32 API is documented to accept from any thread for
/// `SendMessageW` / `PostMessageW`; the OS marshals the call onto
/// the window's owning thread. Mutation of `Window` internals is
/// confined to the worker thread.
pub struct Win32Harness {
    hwnd: HWND,
    editor: Arc<EditorHandle>,
    buffer_id: BufferId,
    worker: Option<JoinHandle<()>>,
    /// Producer side of the window's control channel. Populated for
    /// harnesses spawned via [`Self::spawn`]; `None` on secondary
    /// harnesses (the secondary window doesn't receive control
    /// events in tests). Used by [`Self::fire_window_control`] to
    /// dispatch `ConfigChanged` events the §C5 e2e test relies on.
    control_tx: Option<WindowControlTx>,
    // Owned by the *primary* harness (created via `spawn`). The
    // secondary (`spawn_sharing`) doesn't own these — it borrows
    // the primary's editor by Arc-clone and lets the primary's
    // Drop tear down the persist thread + tempdir. This means a
    // secondary harness must always be dropped *before* its
    // primary; the test thread enforces that by lexical scoping.
    _persist: Option<PersistHandle>,
    _tempdir: Option<TempDir>,
}

// SAFETY: HWND is a Win32 handle that the OS treats as a global
// identifier; `SendMessageW` / `PostMessageW` accept it from any
// thread. The editor handle's interior channels are already Send +
// Sync. No Rust references to thread-local data cross threads
// through this struct.
unsafe impl Send for Win32Harness {}
unsafe impl Sync for Win32Harness {}

impl Win32Harness {
    /// Spawn a fresh hidden window backed by an empty buffer. The
    /// worker thread is created, the production `Window::new` runs,
    /// the HWND is returned to the test thread, and the message pump
    /// starts running. Returns once the HWND is ready for input.
    ///
    /// # Errors
    ///
    /// Returns an error string describing any setup failure
    /// (persist, COM, window construction, hwnd-handoff channel).
    pub fn spawn() -> Result<Self, String> {
        let tempdir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
        let db = tempdir.path().join("harness.db");
        let persist = PersistHandle::spawn(&db).map_err(|e| format!("persist: {e}"))?;
        let editor = Arc::new(EditorHandle::spawn(persist.client(), Arc::new(WallClock)));
        let buffer_id = editor.open_buffer("");

        // Per-window control channel. The test thread holds
        // `control_tx`; the worker's `Window` receives via the rx
        // every `CONFIG_POLL_TIMER_ID` tick. C5 fires
        // `WindowControl::ConfigChanged` through this pair.
        let (control_tx, control_rx) = unbounded::<WindowControl>();

        // HWND isn't Send, so funnel it across threads as `isize`.
        let (tx, rx) = channel::<isize>();
        let editor_for_worker = Arc::clone(&editor);
        let worker = std::thread::Builder::new()
            .name("win32-harness".into())
            .spawn(move || {
                let _com = ComGuard::new().expect("invariant: COM init on harness worker");
                let _ = continuity_win::set_per_monitor_dpi_v2();
                // Register the production command bundles that
                // the e2e tests need: text-input primitives
                // (`editor.insert_char` etc.) for WM_CHAR
                // dispatch, plus pane commands
                // (`pane.split_horizontal` etc.) bound through
                // `TEST_KEYMAP_TOML` for C3 onwards. Without
                // these the wndproc logs `unknown command` and
                // the buffer/tree never mutates — see §C2
                // dev_log for the discovery.
                let mut registry = Registry::new();
                register_editor_primitives(&mut registry);
                register_rich_editing(&mut registry);
                register_pane_commands(&mut registry);
                register_tab_commands(&mut registry);
                let keymap =
                    Keymap::from_toml(TEST_KEYMAP_TOML).expect("invariant: test keymap parses");
                let window = Window::new(
                    WindowConfig {
                        title: "continuity-harness".into(),
                        width: 1200,
                        height: 800,
                        // Off-screen so the never-shown window can't
                        // briefly flash if anything calls ShowWindow.
                        initial_origin: Some((-32_000, -32_000)),
                    },
                    editor_for_worker,
                    buffer_id,
                    WindowCommands {
                        registry,
                        keymap,
                        default_keymap_toml: TEST_KEYMAP_TOML,
                        user_keymap_path: None::<PathBuf>,
                        live_reload: None,
                        control_rx: Some(control_rx),
                        persistence: None,
                        file_io: None,
                        open_file_window: None,
                        register_file_buffer: None,
                        persist_client: None,
                        initial_banners: Vec::new(),
                        open_tutorial_on_init: false,
                        startup_open_buffer_ids: Vec::new(),
                        startup_folder_roots: Vec::new(),
                    },
                )
                .expect("Window::new on harness worker");
                let hwnd_raw = window.hwnd().0 as isize;
                tx.send(hwnd_raw).expect("hwnd handoff");
                let _ = window.run_hidden();
            })
            .map_err(|e| format!("spawn worker: {e}"))?;

        let hwnd_raw = rx.recv().map_err(|e| format!("hwnd channel: {e}"))?;
        let hwnd = HWND(hwnd_raw as *mut _);

        Ok(Self {
            hwnd,
            editor,
            buffer_id,
            worker: Some(worker),
            control_tx: Some(control_tx),
            _persist: Some(persist),
            _tempdir: Some(tempdir),
        })
    }

    /// Spawn an **additional** hidden window backed by the same
    /// `EditorHandle` and `BufferId` as `primary`. Used by the §C4
    /// multi-window e2e test: typing in either window mutates the
    /// shared buffer rope. The secondary harness does *not* own
    /// the persist thread or the tempdir — they stay owned by
    /// `primary`, so the secondary must be dropped before the
    /// primary (which the test thread enforces by lexical scope).
    ///
    /// # Errors
    ///
    /// Same as [`Self::spawn`].
    pub fn spawn_sharing(primary: &Self) -> Result<Self, String> {
        let editor = Arc::clone(&primary.editor);
        let buffer_id = primary.buffer_id;

        let (tx, rx) = channel::<isize>();
        let editor_for_worker = Arc::clone(&editor);
        let worker = std::thread::Builder::new()
            .name("win32-harness-secondary".into())
            .spawn(move || {
                let _com = ComGuard::new().expect("invariant: COM init on harness worker");
                let _ = continuity_win::set_per_monitor_dpi_v2();
                let mut registry = Registry::new();
                register_editor_primitives(&mut registry);
                register_rich_editing(&mut registry);
                register_pane_commands(&mut registry);
                register_tab_commands(&mut registry);
                let keymap =
                    Keymap::from_toml(TEST_KEYMAP_TOML).expect("invariant: test keymap parses");
                let window = Window::new(
                    WindowConfig {
                        title: "continuity-harness-2".into(),
                        width: 1200,
                        height: 800,
                        initial_origin: Some((-32_000, -32_000)),
                    },
                    editor_for_worker,
                    buffer_id,
                    WindowCommands {
                        registry,
                        keymap,
                        default_keymap_toml: TEST_KEYMAP_TOML,
                        user_keymap_path: None::<PathBuf>,
                        live_reload: None,
                        control_rx: None,
                        persistence: None,
                        file_io: None,
                        open_file_window: None,
                        register_file_buffer: None,
                        persist_client: None,
                        initial_banners: Vec::new(),
                        open_tutorial_on_init: false,
                        startup_open_buffer_ids: Vec::new(),
                        startup_folder_roots: Vec::new(),
                    },
                )
                .expect("Window::new on secondary harness worker");
                let hwnd_raw = window.hwnd().0 as isize;
                tx.send(hwnd_raw).expect("hwnd handoff");
                let _ = window.run_hidden();
            })
            .map_err(|e| format!("spawn secondary worker: {e}"))?;

        let hwnd_raw = rx.recv().map_err(|e| format!("hwnd channel: {e}"))?;
        let hwnd = HWND(hwnd_raw as *mut _);

        Ok(Self {
            hwnd,
            editor,
            buffer_id,
            worker: Some(worker),
            control_tx: None,
            _persist: None,
            _tempdir: None,
        })
    }

    /// HWND of the spawned hidden window.
    #[must_use]
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Shared editor handle. Use this from the test thread to read
    /// the latest buffer rope / decorations / selections without
    /// touching the window.
    #[must_use]
    pub fn editor(&self) -> &Arc<EditorHandle> {
        &self.editor
    }

    /// Buffer id the window was opened against.
    #[must_use]
    pub fn buffer_id(&self) -> BufferId {
        self.buffer_id
    }

    /// Dispatch a Win32 message synchronously to the window's
    /// wndproc. Returns the wndproc's `LRESULT`.
    pub fn send_message(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe { SendMessageW(self.hwnd, msg, Some(wparam), Some(lparam)) }
    }

    /// δ.3 probe: read the focused pane's `view.scroll_y_dip`. The
    /// wndproc returns the value scaled by 1000 through `LRESULT`;
    /// this method scales back. Used by the caret-anchor integration
    /// test to verify the scroll position shifts as expected when a
    /// reflow surface fires.
    #[must_use]
    pub fn view_scroll_y_dip(&self) -> f32 {
        let probe = windows::Win32::UI::WindowsAndMessaging::WM_USER + 2;
        let raw = self.send_message(probe, WPARAM(0), LPARAM(0)).0;
        raw as f32 / 1000.0
    }

    /// Spectator cache probe: returns `(hits, misses)` since the
    /// window was spawned. Hits increase when a spectator pane reuses
    /// the prior paint's `FrameDisplay` instead of cold-building.
    #[must_use]
    pub fn spectator_cache_counters(&self) -> (u32, u32) {
        let probe = windows::Win32::UI::WindowsAndMessaging::WM_USER + 3;
        let raw = self.send_message(probe, WPARAM(0), LPARAM(0)).0;
        let hits = ((raw >> 32) & 0xFFFF_FFFF) as u32;
        let misses = (raw & 0xFFFF_FFFF) as u32;
        (hits, misses)
    }

    /// Number of times the UI thread entered caret-anchor capture.
    /// Used by pane/layout regressions to prove a no-op geometry
    /// refresh stayed on the unanchored path.
    #[must_use]
    pub fn caret_anchor_capture_count(&self) -> u64 {
        let probe = windows::Win32::UI::WindowsAndMessaging::WM_USER + 4;
        self.send_message(probe, WPARAM(0), LPARAM(0)).0 as u64
    }

    /// Current per-window DPI tracked by the UI thread.
    #[must_use]
    pub fn window_dpi(&self) -> u32 {
        let probe = windows::Win32::UI::WindowsAndMessaging::WM_USER + 5;
        self.send_message(probe, WPARAM(0), LPARAM(0)).0 as u32
    }

    /// Active font-state key bit pattern.
    #[must_use]
    pub fn font_state_bits(&self) -> u64 {
        let probe = windows::Win32::UI::WindowsAndMessaging::WM_USER + 6;
        self.send_message(probe, WPARAM(0), LPARAM(0)).0 as u64
    }

    /// Number of cached layouts still carrying `font_state_bits`.
    #[must_use]
    pub fn layout_cache_entries_for_font_state(&self, font_state_bits: u64) -> usize {
        let probe = windows::Win32::UI::WindowsAndMessaging::WM_USER + 7;
        self.send_message(probe, WPARAM(font_state_bits as usize), LPARAM(0))
            .0 as usize
    }

    /// Current primary caret-line screen y in pane-body DIPs.
    #[must_use]
    pub fn primary_caret_screen_y_dip(&self) -> Option<f32> {
        let probe = windows::Win32::UI::WindowsAndMessaging::WM_USER + 8;
        let raw = self.send_message(probe, WPARAM(0), LPARAM(0)).0;
        (raw != isize::MIN).then_some(raw as f32 / 1000.0)
    }

    /// Synchronously dispatch `WM_DPICHANGED` with Windows-style x/y DPI
    /// packed into `WPARAM` and a suggested top-level window rect.
    pub fn send_dpi_changed(&self, new_dpi: u32, width_px: i32, height_px: i32) {
        let mut rect = RECT {
            left: -32_000,
            top: -32_000,
            right: -32_000 + width_px.max(1),
            bottom: -32_000 + height_px.max(1),
        };
        let packed = (usize::try_from(new_dpi).unwrap_or(96) & 0xffff)
            | ((usize::try_from(new_dpi).unwrap_or(96) & 0xffff) << 16);
        let ptr = std::ptr::addr_of_mut!(rect);
        self.send_message(WM_DPICHANGED, WPARAM(packed), LPARAM(ptr as isize));
    }

    /// Send `WM_CHAR` for each UTF-16 code unit of `c`. Mirrors what
    /// the OS does when a user types `c`. Returns after the wndproc
    /// has finished processing every code unit.
    pub fn send_char(&self, c: char) {
        let mut buf = [0u16; 2];
        let units = c.encode_utf16(&mut buf);
        for u in units.iter() {
            self.send_message(WM_CHAR, WPARAM(*u as usize), LPARAM(1));
        }
    }

    /// Send a key-down / key-up pair for the virtual-key code `vk`.
    /// Use this for non-character keys (arrows, Esc, F-keys).
    pub fn send_keystroke(&self, vk: u32) {
        self.send_message(WM_KEYDOWN, WPARAM(vk as usize), LPARAM(1));
        self.send_message(
            WM_KEYUP,
            WPARAM(vk as usize),
            // Mirror typical key-up lParam: repeat count 1, prev down,
            // transition up.
            LPARAM(0xC000_0001u32 as isize),
        );
    }

    /// Force a paint cycle: invalidate the client area and
    /// synchronously dispatch `WM_PAINT`. Returns after the
    /// wndproc's paint handler has returned (which in production
    /// includes `BeginPaint` / draw / `EndPaint` and the
    /// `swap_chain.Present(0, ...)` call).
    pub fn wait_for_paint(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
        self.send_message(WM_PAINT, WPARAM(0), LPARAM(0));
    }

    /// Fire a `WindowControl` event into the window's
    /// control channel. Used by the §C5 e2e test to inject a
    /// `WindowControl::ConfigChanged(ConfigEvent::Settings(..))`
    /// without standing up a real `SettingsWatcher`. Combine with
    /// [`Self::tick_config_poll`] to force the wndproc to drain the
    /// channel and apply the event.
    ///
    /// No-op (and returns `Ok(())`) when called on a harness
    /// spawned via [`Self::spawn_sharing`] — the secondary doesn't
    /// have a control channel wired.
    ///
    /// # Errors
    ///
    /// Returns a string describing why the send failed (channel
    /// disconnected — only possible if the worker thread has
    /// already shut down).
    pub fn fire_window_control(&self, ctrl: WindowControl) -> Result<(), String> {
        let Some(tx) = &self.control_tx else {
            return Ok(());
        };
        tx.send(ctrl).map_err(|e| format!("control_tx send: {e}"))
    }

    /// Synchronously fire `WM_TIMER` with [`CONFIG_POLL_TIMER_ID`]
    /// to make the wndproc drain its control channel right now.
    /// Production drains every 250 ms; the harness can't wait
    /// reliably on the OS timer to fire under `SendMessageW`-style
    /// testing, so this synchronous tick is the deterministic
    /// substitute.
    pub fn tick_config_poll(&self) {
        self.send_message(WM_TIMER, WPARAM(CONFIG_POLL_TIMER_ID), LPARAM(0));
    }

    /// Send `WM_ACTIVATEAPP` to simulate the app-level focus toggle
    /// the OS dispatches when the user switches between top-level
    /// windows. Use this to drive focus-return regression tests
    /// without standing up a second visible window.
    pub fn set_app_active(&self, active: bool) {
        use windows::Win32::UI::WindowsAndMessaging::WM_ACTIVATEAPP;
        let wparam = if active { WPARAM(1) } else { WPARAM(0) };
        self.send_message(WM_ACTIVATEAPP, wparam, LPARAM(0));
    }
}

impl Drop for Win32Harness {
    fn drop(&mut self) {
        // Posting `WM_CLOSE` (rather than sending) lets the wndproc
        // drive its own shutdown sequence (`DestroyWindow` →
        // `WM_DESTROY` → `PostQuitMessage`) which makes the
        // `GetMessageW` loop in `run_hidden` exit cleanly.
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: spawn and drop the harness without crashing.
    /// Proves the message pump comes up, the HWND is valid, and the
    /// shutdown path (`WM_CLOSE` → join) completes.
    #[test]
    fn harness_spawns_and_drops_cleanly() {
        let h = Win32Harness::spawn().expect("spawn");
        assert!(!h.hwnd().is_invalid(), "hwnd should be valid");
        // Drop runs at end of scope — joins the worker.
    }
}
