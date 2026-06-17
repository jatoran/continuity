//! `Window::new` constructor — registers the window class, creates the
//! DirectWrite factory, restores persisted state (pane tree, fold set,
//! inline-image expand state), initializes the full per-window field
//! bundle, invokes `CreateWindowExW`, and wires the resulting HWND
//! into the registry, DnD, IVirtualDesktopManager, and the various
//! UI-thread timers.
//!
//! Split out of `window.rs` to keep the parent under the 600-line
//! conventions cap; the symbol path `crate::window::Window::new`
//! remains unchanged because inherent impl blocks may live in any
//! file in the same crate.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;

use continuity_buffer::BufferId;
use continuity_core::{EditorHandle, WpmTracker};
use continuity_decorate::DecorationCache;
use continuity_display_map::{SegmentCache, WrapCache};
use continuity_layout::{DWriteFactory, FontStateId, LayoutCache, RunCache, ViewState};
use continuity_persist::MetricsDailyDelta;
use continuity_win::WindowClass;
use windows::core::HSTRING;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::DragAcceptFiles;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SetWindowPos, CW_USEDEFAULT, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    WINDOW_EX_STYLE, WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW,
};

use crate::display_prewarm_cache::DisplayMapPrewarm;
use crate::overlays::Overlays;
use crate::pane_tree::PaneTree;
use crate::window::{Window, FONT_FAMILY, FONT_LOCALE, FONT_SIZE_DIP, LAYOUT_CACHE_CAPACITY};
use crate::window_config::{WindowCommands, WindowConfig};
use crate::window_dispatch::wndproc;
use crate::window_mouse_hover::wall_clock_ms;
use crate::window_theme::ActiveTheme;
use crate::Error;

const MAIN_WINDOW_EX_STYLE: WINDOW_EX_STYLE = WINDOW_EX_STYLE(WS_EX_NOREDIRECTIONBITMAP.0);

impl Window {
    /// Create the window, register its class, set up DirectWrite, and bind
    /// it to a buffer in the supplied editor handle.
    ///
    /// The returned `Box<Window>` must be passed to [`Self::run`] for the
    /// message pump to take ownership and drive it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Win`] if class registration / window creation fails,
    /// or [`Error::Layout`] if the DirectWrite factory cannot be created.
    pub fn new(
        config: WindowConfig,
        editor: Arc<EditorHandle>,
        buffer_id: BufferId,
        commands: WindowCommands,
    ) -> Result<Box<Self>, Error> {
        let class = WindowClass::register_unique_with_proc("ContinuityWindow", Some(wndproc))?;
        let dwrite = DWriteFactory::new()?;
        let startup_open_buffer_ids = commands.startup_open_buffer_ids;
        let startup_folder_roots = commands.startup_folder_roots;
        let (file_open_tx, file_open_rx) =
            crossbeam_channel::bounded(crate::file_io_worker::CHANNEL_CAPACITY);

        let font_state = FontStateId::from_parts(FONT_FAMILY, FONT_SIZE_DIP, FONT_LOCALE, 1.0);
        let active_theme = ActiveTheme::bundled().unwrap_or_else(|_| ActiveTheme::neutral());
        let now_ms = wall_clock_ms();
        // Phase 14: replay the persisted pane tree if the app provided one;
        // otherwise fall back to the singleton tree built from `buffer_id`.
        let fallback_tree = PaneTree::singleton(buffer_id, now_ms);
        // §H3 — recover the per-window `folded_lines` set alongside the
        // pane tree so the user's last fold state is restored on
        // launch. F5 also recovers the per-`(BufferId, URL)` inline-
        // image expand state. Old blobs that predate either field
        // decode with empty vectors, matching the pre-§H3 / pre-F5
        // behavior.
        let (mut tree, restored_folded_lines, restored_image_expand_state) =
            crate::window_placement_persistence::restore_with_state_or_singleton(
                commands
                    .persistence
                    .as_ref()
                    .and_then(|p| p.initial.as_ref()),
                fallback_tree,
            );
        crate::window_buffer_tab_repair::repair_pane_tree_structure(&mut tree, &editor);
        crate::window_buffer_tab_repair::repair_missing_buffer_tabs(&mut tree, &editor);
        let buffer_id = tree.active_buffer();
        let virtual_desktop = continuity_win::VirtualDesktopManager::new()
            .map_err(|e| {
                eprintln!("continuity-ui: VirtualDesktopManager unavailable: {e}");
                e
            })
            .ok();
        let mut window = Box::new(Self {
            hwnd: HWND::default(),
            _class: class,
            editor,
            buffer_id,
            registry: commands.registry,
            keymap: commands.keymap,
            default_keymap_toml: commands.default_keymap_toml,
            user_keymap_path: commands.user_keymap_path,
            keymap_conflicts: Vec::new(),
            pending_chord_sequence: Vec::new(),
            dwrite,
            renderer: None,
            text_format: None,
            shift_held: false,
            client_width: 0,
            client_height: 0,
            deferred_renderer_resize: None,
            window_dpi: 96,
            is_applying_dpi_change: false,
            inited: false,
            overlays: Overlays::idle(),
            overlay_input_focused: false,
            mouse_state: crate::mouse::MouseState::default(),
            view: ViewState::new(),
            cache: LayoutCache::new(LAYOUT_CACHE_CAPACITY),
            walker_run_cache: Arc::new(RunCache::default()),
            walker_wrap_cache: Arc::new(WrapCache::default()),
            walker_segment_cache: Arc::new(SegmentCache::default()),
            font_state,
            scroll_anim_active: false,
            scroll_inertia: crate::window_scroll::ScrollInertia::default(),
            motion_timer_active: false,
            decorate_pool: None,
            decoration_cache: DecorationCache::new(),
            last_submitted_decoration_revision: None,
            last_submitted_decoration_revision_per_buffer: RefCell::new(HashMap::new()),
            last_tree_prune_keep: RefCell::new(Vec::new()),
            last_focused_table_layouts: RefCell::new(HashMap::new()),
            decoration_worker_watchdog_timeout_ms: continuity_config::WorkerConfig::default()
                .decoration_watchdog_ms,
            decoration_watchdog_poll_active: false,
            language: Self::default_language(),
            language_revision: None,
            active_theme,
            titlebar_dark_applied: None,
            window_title_applied: None,
            view_options: crate::window_view_options::ViewOptions::default(),
            right_edge_chrome_defaults:
                crate::window_right_edge_chrome::RightEdgeChromeState::default(),
            right_edge_chrome_by_view: HashMap::new(),
            prose_font_family: FONT_FAMILY.to_string(),
            font_size_dip_override: None,
            pending_font_change: None,
            font_swap_settle_deadline: None,
            settings_projections: crate::window_settings_projections::SettingsProjections::default(
            ),
            caret_blink_visible: true,
            last_input_tick: 0,
            caret_blink_active: false,
            live_reload: commands.live_reload,
            control_rx: commands.control_rx,
            config_poll_active: false,
            persistence: commands.persistence,
            file_io: commands.file_io,
            file_open_tx,
            file_open_rx,
            open_file_window: commands.open_file_window,
            register_file_buffer: commands.register_file_buffer,
            file_io_poll_active: false,
            file_tree: crate::file_tree::FileTreeState::default(),
            state_save_pending: false,
            file_banner: None,
            unsaved_close_arm: None,
            status_notices: Vec::new(),
            virtual_desktop,
            tree,
            panes: HashMap::new(),
            paste_history: crate::window_clipboard::PasteHistory::new(),
            ime_state: crate::window_ime::ImeState::default(),
            spell_state: crate::window_spell::SpellState::default(),
            auto_pair: continuity_core::AutoPairConfig::default(),
            indent: crate::window_indent::IndentConfig::default(),
            intended_columns: Vec::new(),
            intended_display_columns: Vec::new(),
            intended_columns_for: Vec::new(),
            jump_glow: None,
            edit_pulse: None,
            caret_tween: None,
            motion_policy: crate::motion::MotionPolicy::new(false),
            stagger_scheduler: crate::motion::StaggerScheduler::default(),
            overlay_motion: crate::surface_motion::SurfaceMotionState::default(),
            chrome_motion: crate::chrome_motion::ChromeMotionState::default(),
            status_motion: crate::status_motion::StatusMotionState::default(),
            chord_hud: crate::chord_hud::HudState::default(),
            chord_hud_motion: crate::surface_motion::SurfaceMotionState::default(),
            loading_overlay_state: crate::window_loading_overlay::LoadingOverlayState::new(),
            trim_trailing_whitespace_on_save: true,
            slash_commands_enabled: true,
            slash_commands_palette: None,
            image_store_dir: None,
            inline_images_enabled: true,
            image_cache_bytes_target: 64 * 1024 * 1024,
            image_expand_state: restored_image_expand_state
                .into_iter()
                .map(|(buf, source_byte, expanded)| ((buf, source_byte), expanded))
                .collect(),
            find_memory: HashMap::new(),
            find_persist_per_buffer: true,
            persist_client: commands.persist_client,
            wpm_tracker: WpmTracker::default(),
            metrics_pending: MetricsDailyDelta::default(),
            metrics_last_flush_ms: 0,
            metrics_last_keystroke_ms: 0,
            wpm_frozen: 0,
            metrics_repaint_active: false,
            time_machine_preview: None,
            time_machine_drag: None,
            palette_command_recency: HashMap::new(),
            palette_recency_tick: 0,
            last_edit_stack: HashMap::new(),
            is_window_focused: true,
            has_keyboard_focus: true,
            activate_on_show: config.activate_on_show,
            is_window_minimized: false,
            is_live_resizing: false,
            tab_drag_ghost_window: None,
            resize_anchor: None,
            caret_anchor_capture_count: std::cell::Cell::new(0),
            resize_changed: false,
            pending_doc_end_scroll: false,
            geometry_anchor: crate::window_view::geometry_anchor::GeometryAnchorState::default(),
            pending_doc_end_scroll_attempts: 0,
            jump_offthread_polls: 0,
            background_paint_tick: 0,
            display_map_prewarm: DisplayMapPrewarm::new(),
            display_prewarm_timer_active: false,
            buffer_history_tabs: HashMap::new(),
            last_painted_frame_display: None,
            last_painted_decorations: None,
            last_painted_decoration_parse_revision: None,
            status_bar_rope_counts: std::cell::RefCell::new(HashMap::new()),
            projection_worker: None,
            projection_request_seq: 0,
            last_early_dispatch_stamp: None,
            tab_dirty_cache: RefCell::new(HashMap::new()),
            pending_save_baseline: HashMap::new(),
            heading_lines_cache: RefCell::new(None),
            outline_entries_cache: RefCell::new(
                crate::window_outline_entries_cache::OutlineEntriesCache::default(),
            ),
            last_activation_tick: 0,
            spectator_frame_cache: RefCell::new(
                crate::window_spectator_cache::SpectatorFrameCache::new(),
            ),
            mouse_hit_test_frame_cache: RefCell::new(None),
            row_index_cache: RefCell::new(crate::window_row_index_cache::RowIndexCache::new()),
            tab_session: crate::window_panes::TabSessionState::default(),
        });
        // §H3 — install the persisted fold set into PaneModesState
        // *before* settings load runs (settings load may touch
        // pane_modes too, but folded_lines is purely runtime state
        // that no [focus]/[focus_*] key controls). Out-of-range
        // indices are dropped here so a smaller-rope-after-reload
        // doesn't make later code assume the line exists.
        window.install_restored_folded_lines(restored_folded_lines);
        window.refresh_keymap_conflicts();
        window.install_decorate_pool();
        window.refresh_language();
        window.maybe_submit_decoration();
        window.maybe_apply_initial_settings();
        if let Some(file) = window
            .editor
            .snapshot(window.buffer_id)
            .and_then(|snap| snap.file)
        {
            window.mark_tab_file_associated(window.buffer_id, &file);
        }
        // δ.3 — raise the registry-supplied initial banner (today only
        // recovery-halt notices, but the field is generic enough for any
        // launch-time message). Multiple notices collapse into a single
        // transient banner so the chrome stays a one-line surface — the
        // first message is shown verbatim and additional notices are
        // summarized by count.
        if let Some(first) = commands.initial_banners.first() {
            let text = if commands.initial_banners.len() > 1 {
                format!("{first} (+ {} more)", commands.initial_banners.len() - 1)
            } else {
                first.clone()
            };
            window.file_banner = Some(crate::window_file::FileBanner::transient_for(
                text, now_ms, 10_000,
            ));
        }
        window.maybe_open_tutorial_on_first_launch(commands.open_tutorial_on_init);

        let title = HSTRING::from(&config.title);
        let class_name = window._class.name().clone();
        let hinstance = window._class.hinstance();
        let lparam_ptr = window.as_mut() as *mut Window as *mut c_void;
        let (origin_x, origin_y) = config
            .initial_origin
            .unwrap_or((CW_USEDEFAULT, CW_USEDEFAULT));
        let hwnd = unsafe {
            CreateWindowExW(
                MAIN_WINDOW_EX_STYLE,
                windows::core::PCWSTR(class_name.as_ptr()),
                &title,
                WS_OVERLAPPEDWINDOW,
                origin_x,
                origin_y,
                config.width,
                config.height,
                None,
                None,
                Some(hinstance.into()),
                Some(lparam_ptr),
            )
        }
        .map_err(|e| continuity_win::Error::win32("CreateWindowExW", e))?;

        if let Some((origin_x, origin_y)) = config.initial_origin {
            unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    origin_x,
                    origin_y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
            }
            .map_err(|e| continuity_win::Error::win32("SetWindowPos", e))?;
        }

        window.hwnd = hwnd;
        // Match the OS title bar to the active theme before the window is
        // shown (in `apply_initial_placement` below), so the first frame
        // has no light/dark caption flash.
        window.sync_titlebar_theme();
        // Attach the embedded app icon to the caption / Alt-Tab entry
        // before the window is shown, mirroring the titlebar-theme call
        // above so the first frame carries the icon with no flash.
        window.apply_window_icon();
        window.window_dpi = continuity_win::dpi_for_window(hwnd);
        window.font_state = window.current_font_state_id();
        crate::window_registry::register(hwnd);
        unsafe {
            DragAcceptFiles(hwnd, true);
        }
        window.start_decoration_watchdog_poll(hwnd);
        window.start_config_poll(hwnd);
        window.start_file_io_poll(hwnd);
        window.start_display_prewarm_timer(hwnd);
        window.start_trace_summary_timer(hwnd);
        window.apply_initial_placement(hwnd);
        window.adopt_startup_open_buffers(startup_open_buffer_ids);
        window.adopt_startup_folder_roots(startup_folder_roots);
        window.watch_existing_file_tabs();
        Ok(window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_window_ex_style_bypasses_dwm_redirection_bitmap() {
        assert_ne!(
            MAIN_WINDOW_EX_STYLE.0 & WS_EX_NOREDIRECTIONBITMAP.0,
            0,
            "main HWND should avoid DWM redirection-surface resize artifacts"
        );
    }
}
