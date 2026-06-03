//! Phase 12 — settings live-reload + `settings.open`.
//!
//! Thread ownership: the UI thread polls a `crossbeam_channel::Receiver`
//! on a `WM_TIMER` tick (every [`crate::window::CONFIG_POLL_TIMER_MS`]).
//! The config-watcher thread is the only producer; the UI thread is the
//! only consumer. The `PersistClient` carried here is a clone-only
//! request sender — calls funnel through to the persist thread for the
//! actual `PRAGMA synchronous` write.

use continuity_config::{ConfigEvent, FocusMode, Settings};
use continuity_keymap::Keymap;
use continuity_theme::{Theme, ThemeSet};
use windows::core::HSTRING;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{SetTimer, SHOW_WINDOW_CMD, SW_SHOWNORMAL};

use crate::window::{CONFIG_POLL_TIMER_ID, CONFIG_POLL_TIMER_MS};
use crate::window_right_edge_chrome::RightEdgeChromeState;
use crate::window_view_options::CaretStyle;
use crate::Window;

impl Window {
    /// Apply the latest committed `Settings` at window-creation time.
    /// The snapshot comes from the shared [`crate::LiveReload`] cell,
    /// which the registry updates on every settings-watcher event — so
    /// a window opened *after* a runtime commit observes the new
    /// state, not the process-start snapshot. Idempotent; safe to call
    /// from `Window::new`.
    pub(crate) fn maybe_apply_initial_settings(&mut self) {
        let Some(reload) = self.live_reload.as_ref() else {
            return;
        };
        let snapshot = reload.current_settings();
        let apply = reload.apply_sync_mode.clone();
        let pragma = snapshot.persistence_mode().synchronous_pragma();
        apply(pragma);
        self.apply_settings(&snapshot);
        // §H1 — `[focus].initial_mode` is honored only at startup; runtime
        // `view.cycle_focus` and friends survive hot reloads.
        self.apply_focus_initial_mode(&snapshot);
        // §H2 — `[focus].distraction_free_on_launch` likewise applies
        // once at startup so a later F11 keypress isn't silently
        // un-done by a hot-reloaded settings file.
        self.apply_focus_distraction_free_initial(&snapshot);
    }

    /// Install a `WM_TIMER` ticking at [`CONFIG_POLL_TIMER_MS`] that drains
    /// the registry's per-window control channel. Idempotent.
    pub(crate) fn start_config_poll(&mut self, hwnd: HWND) {
        if self.config_poll_active || self.control_rx.is_none() {
            return;
        }
        unsafe {
            let _ = SetTimer(Some(hwnd), CONFIG_POLL_TIMER_ID, CONFIG_POLL_TIMER_MS, None);
        }
        self.config_poll_active = true;
    }

    /// Drain every queued [`crate::WindowControl`] message and apply
    /// each one. Stops at the first empty slot so the UI thread is not
    /// starved.
    pub(crate) fn on_config_poll_tick(&mut self, hwnd: HWND) {
        let Some(rx) = self.control_rx.as_ref().cloned() else {
            return;
        };
        let mut dirty = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                crate::WindowControl::ConfigChanged(event) => {
                    self.handle_config_event(event);
                }
                crate::WindowControl::PersistEvent(event) => {
                    self.handle_persist_event(event);
                }
            }
            dirty = true;
        }
        if dirty {
            self.invalidate_with_reason(hwnd, "theme_apply");
        }
    }

    fn handle_config_event(&mut self, event: ConfigEvent) {
        match event {
            ConfigEvent::Settings(settings) => {
                // δ.6 Tier 3 — contract (C): if this event is the echo
                // of our own `persist_boolean_setting` writeback, skip
                // re-applying. The runtime field was already flipped
                // by the toggle command and the file now matches.
                if self.consume_writeback_echo() {
                    return;
                }
                if let Some(reload) = self.live_reload.as_ref() {
                    let pragma = settings.persistence_mode().synchronous_pragma();
                    (reload.apply_sync_mode)(pragma);
                }
                self.apply_settings(settings.as_ref());
            }
            ConfigEvent::Keymap(toml) => match Keymap::from_toml(&toml) {
                Ok(user) => {
                    let base = match Keymap::from_toml(self.default_keymap_toml) {
                        Ok(k) => k,
                        Err(e) => {
                            eprintln!("continuity: bundled default keymap re-parse failed: {e}");
                            return;
                        }
                    };
                    self.keymap = Keymap::layered(base, user);
                    self.refresh_keymap_conflicts();
                }
                Err(e) => eprintln!("continuity: keymap reload failed: {e}"),
            },
            ConfigEvent::Theme { name, toml } => match Theme::load(&toml) {
                Ok(theme) => {
                    self.swap_theme_by_name(&name, theme);
                }
                Err(e) => {
                    // δ.5 — surface validation/parse failures as the
                    // same banner the watcher uses for settings/keymap
                    // failures. The previous theme stays active because
                    // we never call `swap_theme_by_name` on this path.
                    self.file_banner = Some(crate::window_file::FileBanner::new(format!(
                        "theme `{name}`: {e}",
                    )));
                }
            },
            ConfigEvent::Failed { path, reason } => {
                // E1: validation errors surface as a non-blocking banner.
                // The banner reuses the Phase-15 `FileBanner` surface so
                // dismissal (Esc / next file event) is uniform with the
                // file-watcher banners.
                let label = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                self.file_banner = Some(crate::window_file::FileBanner::new(format!(
                    "{label}: {reason}"
                )));
            }
        }
    }

    /// δ.3 — react to a [`continuity_persist::PersistEvent`] forwarded
    /// by the registry: raise a sticky `FileBanner` so the durability
    /// promise stays visible. Banners are sticky (no auto-dismiss)
    /// because the underlying condition is severe and the user
    /// should explicitly acknowledge.
    fn handle_persist_event(&mut self, event: continuity_persist::PersistEvent) {
        self.file_banner = Some(crate::window_file::FileBanner::new(event.banner_text()));
    }

    /// Project a validated [`Settings`] onto the per-window
    /// [`crate::window_view_options::ViewOptions`] + theme + font surface.
    pub(crate) fn apply_settings(&mut self, s: &Settings) {
        // -- theme mode -------------------------------------------------
        let target_mode = match s.theme_mode() {
            continuity_config::ThemeMode::Dark => continuity_theme::Mode::Dark,
            continuity_config::ThemeMode::Light => continuity_theme::Mode::Light,
            continuity_config::ThemeMode::System => continuity_theme::Mode::System,
        };
        if self.active_theme.mode != target_mode {
            self.active_theme.set_mode(target_mode);
        }
        // -- theme bindings --------------------------------------------
        // `[ui].theme_dark` / `theme_light` resolve into actual
        // `Theme`s via the bundled set + user themes directory. This is
        // the sole in-memory mutation site for committed theme changes
        // — both at startup (so the user's saved selection is honored)
        // and on hot reload (so a commit in one window applies to every
        // sibling). Preview-only mutations happen separately through
        // `apply_theme_entry` and never reach this path.
        self.apply_settings_theme_bindings(s);

        // -- view options ----------------------------------------------
        self.view_options.line_numbers = s.ui.show_line_numbers;
        self.view_options.relative_line_numbers = s.ui.relative_line_numbers;
        self.view_options.gutter_caret_line_only = !s.ui.show_all_line_numbers;
        self.view_options.show_status_bar = s.ui.show_status_bar;
        self.apply_reduced_motion(s.ui.reduced_motion);
        self.view_options.show_sticky_breadcrumb = s.ui.show_sticky_breadcrumb;
        self.view_options.outline_sidebar_width_dip = s.ui.outline_sidebar_width_dip as f32;
        self.set_right_edge_chrome_defaults(RightEdgeChromeState::new(
            s.ui.show_minimap,
            s.ui.show_outline_sidebar,
        ));
        // Phase C1: project the ordered segment list. validate() has
        // already guaranteed each id parses.
        self.view_options.status_bar_segments = s.status_bar_segments();
        if let Ok(mode) = continuity_config::TabCloseButton::parse(&s.ui.tab_close_button) {
            self.view_options.tab_close_button = mode;
        }
        self.view_options.ruler_columns = s.editor.ruler_columns.clone();
        self.view_options.caret_style = match s.caret_style() {
            continuity_config::CaretStyle::Bar => CaretStyle::Bar,
            continuity_config::CaretStyle::Block => CaretStyle::Block,
            continuity_config::CaretStyle::Underline => CaretStyle::Underline,
        };
        self.view_options.caret_blink_ms = s.editor.caret_blink_ms;
        self.view_options.caret_width_px = s.editor.caret_width_px;
        self.view_options.caret_blink_on_typing_pause = s.editor.caret_blink_on_typing_pause;
        self.view_options.caret_typing_pause_ms = s.editor.caret_typing_pause_ms;
        self.view_options.caret_long_idle_ms = s.editor.caret_long_idle_ms;
        self.view_options.caret_color = s.editor.caret_color.clone();
        self.view_options.caret_secondary_color = s.editor.caret_secondary_color.clone();
        self.view_options.caret_tween_enabled = s.editor.caret_tween_enabled;
        self.view_options.caret_tween_threshold_rows = s.editor.caret_tween_threshold_rows;
        self.view_options.caret_tween_duration_ms = s.editor.caret_tween_duration_ms;
        self.view_options.smooth_scroll = s.editor.smooth_scroll;
        self.view_options.scroll_past_end = s.editor.scroll_past_end;
        self.view_options.mouse_wheel_scroll_speed = s.editor.mouse_wheel_scroll_speed;
        self.view_options.ligatures = s.editor.ligatures;
        self.trim_trailing_whitespace_on_save = s.editor.trim_trailing_whitespace_on_save;
        self.decoration_worker_watchdog_timeout_ms = s.workers.decoration_watchdog_ms;
        if let Some(pool) = self.decorate_pool.as_ref() {
            pool.set_watchdog_timeout(std::time::Duration::from_millis(u64::from(
                self.decoration_worker_watchdog_timeout_ms,
            )));
        }
        // §H5: project the slash-command palette settings.
        self.slash_commands_enabled = s.editor.slash_commands_enabled;
        self.slash_commands_palette = s.editor.slash_commands_palette.clone();
        // F5: project the markdown image-store config. `resolve_images_dir`
        // expands `%APPDATA%`; on hosts where the env is unset the typed
        // error becomes `None`, and the drag-drop image branch then
        // falls through to plain tab-open (no silent crash).
        self.image_store_dir = s.markdown.resolve_images_dir().ok();
        self.inline_images_enabled = s.markdown.inline_images;
        // F5 Pass 2: push the inline-image cache cap to the renderer.
        // `inline_images = false` zeroes the cap so cached bitmaps
        // get evicted; `true` honours `[ui].image_cache_bytes`. We
        // also stash the resolved value on `Window` so the lazy
        // `ensure_renderer` path can apply it on first creation
        // (otherwise the cap stays at the renderer's default of 0
        // and `get_or_decode` returns `Ok(None)` for every image).
        let target_cap = if s.markdown.inline_images {
            usize::try_from(s.ui.image_cache_bytes).unwrap_or(0)
        } else {
            0
        };
        self.image_cache_bytes_target = target_cap;
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.set_image_cache_capacity(target_cap);
        }
        // G2: mirror `[find].persist_per_buffer`. Flipping the toggle off
        // also clears any in-memory state — the setting is a privacy lever
        // as much as it is a behavior toggle.
        let new_persist = s.find.persist_per_buffer;
        if !new_persist {
            self.find_memory.clear();
        }
        self.find_persist_per_buffer = new_persist;

        // -- font + zoom -----------------------------------------------
        // δ.3 — each font/wrap reflow below is wrapped in
        // `with_caret_line_anchored` so the caret line keeps its screen
        // y across the change. Wrap-on under a large font-size delta
        // can move the caret by many wrap rows; without the anchor
        // every settings.toml save would re-target the user's eye.
        let new_family = s.editor.font_family_prose.trim().to_string();
        if !new_family.is_empty() && new_family != self.prose_font_family {
            // Route through the deferred-swap path so a settings.toml
            // edit doesn't flash overflow on large buffers either —
            // the body keeps painting the previous font until the
            // worker delivers a display map for the new one. The swap
            // itself is wrapped in `with_caret_line_anchored` inside
            // `try_apply_pending_font_swap`. See `window_font_swap`.
            self.request_font_change(Some(new_family), None);
        }
        // δ.6 Tier 2 — `[editor].font_family_mono`. Parallel to the
        // prose-family path: trim the candidate, ignore empty strings
        // (would yield a fallback chain confusion), invalidate font
        // state on a delta so the layout cache drops stale entries.
        let new_mono = s.editor.font_family_mono.trim().to_string();
        if !new_mono.is_empty() && new_mono != self.settings_projections.mono_font_family {
            self.with_caret_line_anchored(|w| {
                w.settings_projections.mono_font_family = new_mono.clone();
                w.invalidate_font_state();
            });
        }
        let new_size = s.editor.font_size;
        if (self
            .font_size_dip_override
            .unwrap_or(crate::window::FONT_SIZE_DIP)
            - new_size)
            .abs()
            > f32::EPSILON
        {
            // Same deferred-swap routing as the family branch above —
            // see `window_font_swap` for the wrap-overflow rationale.
            self.request_font_change(None, Some(new_size));
        }
        // δ.6 Tier 2 — `[editor].line_height`. validate() guarantees
        // `0.5 ≤ value ≤ 4.0`. A delta invalidates font state because
        // every cached `IDWriteTextLayout` was built against the old
        // multiplier (the layout cache key does not include line
        // height directly, but the rendered metrics do).
        let new_line_height = s.editor.line_height;
        if (self.settings_projections.line_height_multiplier - new_line_height).abs() > f32::EPSILON
        {
            self.with_caret_line_anchored(|w| {
                w.settings_projections.line_height_multiplier = new_line_height;
                w.invalidate_font_state();
            });
        }
        // Global text scale (zoom). Zoom is a single durable multiplier
        // sourced from `[editor].text_scale`, not a per-pane runtime
        // tweak: project it onto the focused mirror *and* every entry in
        // `self.panes` so an unfocused pane does not pop to a stale scale
        // on the next focus switch. `with_caret_line_anchored` anchors
        // this window's focused pane across the reflow; spectator panes
        // have no tracked caret-y to preserve. validate() guarantees the
        // value sits in `MIN_ZOOM..=MAX_ZOOM`, the same range the runtime
        // clamp uses, so the clamp here is a defensive no-op rather than a
        // re-clamp that could drift a valid file value.
        let new_scale = s
            .editor
            .text_scale
            .clamp(continuity_layout::MIN_ZOOM, continuity_layout::MAX_ZOOM);
        if (self.view.font_size_scale - new_scale).abs() > f32::EPSILON
            || self
                .panes
                .values()
                .any(|p| (p.view.font_size_scale - new_scale).abs() > f32::EPSILON)
        {
            self.with_caret_line_anchored(|w| {
                w.view.font_size_scale = new_scale;
                for pane in w.panes.values_mut() {
                    pane.view.font_size_scale = new_scale;
                }
                w.invalidate_font_state();
            });
        }

        // -- soft wrap -------------------------------------------------
        if self.view.soft_wrap != s.editor.word_wrap {
            let new_wrap = s.editor.word_wrap;
            self.with_caret_line_anchored(|w| {
                w.view.soft_wrap = new_wrap;
                // Wrap-width changes drop other-wrap-width entries.
                let key = w.view.wrap_width_key();
                w.cache.invalidate_other_wrap_widths(key);
            });
        }

        // -- auto-pair toggles (Phase 16.5) ----------------------------
        self.apply_auto_pair_settings(s);

        // -- indentation (type / width / tab width) --------------------
        // Mirrors `[editor].indent_type` / `indent_width` / `tab_width`
        // onto the runtime indent config + `view_options.indent_size` /
        // `tab_width`. A tab-width change reflows anchored on the caret
        // line (the rendered tab stop is part of the font state).
        self.apply_indent_settings(s);

        // -- §H1 focus-mode dim alpha ----------------------------------
        // `dim_alpha` is propagated on every reload so users can tune
        // overlay strength without restarting. `initial_mode` is *not*
        // re-applied here — once a window is open, runtime cycling
        // wins. See `apply_focus_initial_mode` for the startup-only
        // half.
        let alpha = s.focus.dim_alpha.clamp(0.0, 1.0);
        if (self.view_options.pane_modes.focus_dim_alpha - alpha).abs() > f32::EPSILON {
            self.view_options.pane_modes.focus_dim_alpha = alpha;
        }
        // §H2 — `[focus].max_column_width` caps the centered body
        // column while DF is active. Live-applied so a runtime change
        // resizes the next paint without a restart.
        let max_w = s.focus.max_column_width.max(1);
        if self.view_options.pane_modes.distraction_free_max_width != max_w {
            self.view_options.pane_modes.distraction_free_max_width = max_w;
        }

        // -- markdown render toggles -----------------------------------
        // `[markdown].render_*` flip the projected segment list (emphasis
        // styling, marker hiding, highlight / divider / setext
        // rendering) AND the soft-wrap row counts (toggling italic off
        // makes the `*` markers visible, widening lines). The toggle set
        // is folded into `current_font_state_id`, so a change shifts the
        // font state; invalidating it here drops every cached layout /
        // frame / segment list / wrap profile built against the previous
        // toggles. Wrapped in `with_caret_line_anchored` because a wrap
        // reflow under the change can move the caret by many wrap rows.
        let new_markdown_toggles =
            crate::window_settings_projections::markdown_render_toggles_from_config(&s.markdown);
        if new_markdown_toggles != self.settings_projections.markdown_render_toggles {
            self.with_caret_line_anchored(|w| {
                w.settings_projections.markdown_render_toggles = new_markdown_toggles;
                w.invalidate_font_state();
            });
        }

        // -- δ.6 Tier 2 markdown + edit-behaviour projections ----------
        // Bundled on `SettingsProjections` so the canonical `Window`
        // struct stays under the 600-line cap. Each field sits on
        // contract (A) — TOML wins on reload. Downstream consumers
        // (decoration / autocorrect / render) read these per-paint or
        // per-keystroke so the next frame after a reload sees the new
        // value without further wiring.
        self.settings_projections.apply_from_settings(s);

        // -- δ.6 Tier 2 launch-only justifications (contract B) -------
        // The settings below are *intentionally* not projected here.
        // Each is consumed by the persist or backup thread (or the
        // app-level launcher) and live-tuning requires a control
        // message that does not yet exist. They follow contract (B)
        // from `.docs/design/defaults.md`: read once at startup, ignored
        // on hot reload until the persist-control machinery lands as a
        // separate phase entry. Listing them here makes the decision
        // visible in the projection site rather than silently absent.
        //
        //   * `[persistence].debounce_ms`         — core thread snapshot policy
        //   * `[persistence].snapshot_every_edits`— core thread snapshot policy
        //   * `[persistence].snapshot_every_bytes`— core thread snapshot policy
        //   * `[persistence].trash_retention_days`— consumed by `purge_expired`
        //   * `[backup].interval_minutes`         — backup task cadence
        //   * `[backup].hourly_retention`         — backup task retention
        //   * `[backup].daily_retention`          — backup task retention
        //   * `[backup].location`                 — backup destination (path baked at startup)
        //
        // To live-tune these, add a `WindowControl::PersistConfigChanged`
        // analog to the existing `apply_sync_mode` pathway and a
        // matching control message on `PersistClient`.
        let _ = (
            s.persistence.debounce_ms,
            s.persistence.snapshot_every_edits,
            s.persistence.snapshot_every_bytes,
            s.persistence.trash_retention_days,
            s.backup.interval_minutes,
            s.backup.hourly_retention,
            s.backup.daily_retention,
            &s.backup.location,
        );
    }

    /// §H2 — apply `[focus].distraction_free_on_launch` at window
    /// startup. Called once from [`Self::maybe_apply_initial_settings`];
    /// *not* called on hot reload so a runtime F11 toggle is not
    /// overridden when the user merely edits another `settings.toml`
    /// key. Re-uses [`Self::toggle_distraction_free_mode_impl`] so the
    /// chrome-snapshot + restore guarantees apply identically to the
    /// launch path.
    pub(crate) fn apply_focus_distraction_free_initial(&mut self, s: &Settings) {
        if s.focus.distraction_free_on_launch && !self.view_options.pane_modes.distraction_free {
            let _ = self.toggle_distraction_free_mode_impl();
        }
    }

    /// §H1 — apply `[focus].initial_mode` at window startup. Called once
    /// from [`Self::maybe_apply_initial_settings`]; *not* called on hot
    /// reload so runtime `view.cycle_focus` overrides are preserved.
    /// Unknown mode strings silently leave the existing mode untouched
    /// (validation already rejects malformed values at config-load time).
    pub(crate) fn apply_focus_initial_mode(&mut self, s: &Settings) {
        if let Ok(mode) = FocusMode::parse(&s.focus.initial_mode) {
            self.view_options.pane_modes.focus_mode = mode;
        }
    }

    fn swap_theme_by_name(&mut self, name: &str, theme: Theme) {
        let mut set = self.active_theme.set.clone();
        let mut matched = false;
        if set.dark.name == name {
            set.dark = theme.clone();
            matched = true;
        }
        if set.light.name == name {
            set.light = theme;
            matched = true;
        }
        if !matched {
            // No installed slot named like that; ignore. The user can
            // wire it via [ui].theme_dark / theme_light to install it.
            return;
        }
        self.active_theme.set_installed(ThemeSet {
            dark: set.dark,
            light: set.light,
        });
    }

    /// Implementation of `settings.open`: open `settings.toml` as a buffer
    /// inside continuity (Phase A §A3 of roadmap_v2.md).
    ///
    /// The file is created with a commented template on first use so the
    /// user has something to edit instead of an empty buffer. Saves to the
    /// buffer flow through the existing file watcher → live-reload path,
    /// so edits take effect on Ctrl+S like any other config tweak.
    ///
    /// Falls back to a shell-execute against the user's `.toml` file
    /// association when the file I/O thread isn't available (test stubs,
    /// early-init).
    pub(crate) fn open_settings_impl(&mut self) -> Result<(), crate::Error> {
        let Some(reload) = self.live_reload.as_ref() else {
            return Err(crate::Error::Command(
                continuity_command::Error::UnsupportedContext("open_settings"),
            ));
        };
        let settings_path = reload.settings_path.clone();
        crate::window_settings_seed::ensure_settings_file(&settings_path).map_err(|e| {
            crate::Error::Command(continuity_command::Error::Other(format!(
                "failed to create {}: {e}",
                settings_path.display()
            )))
        })?;

        // Preferred path: open as a buffer through the file I/O thread.
        // The existing file_open path deduplicates against already-open
        // associations, so repeated `settings.open` invocations focus the
        // existing tab instead of stacking duplicates.
        if self.file_io.is_some() {
            return self
                .file_open_paths_impl(vec![settings_path])
                .map_err(crate::Error::Command);
        }

        // Fallback: shell-execute (no file-I/O thread → no buffer to open
        // into). Mostly relevant to integration tests and headless runs.
        let path_w = HSTRING::from(settings_path.as_os_str());
        let verb = HSTRING::from("open");
        let result = unsafe {
            ShellExecuteW(
                Some(self.hwnd),
                &verb,
                &path_w,
                windows::core::PCWSTR::null(),
                windows::core::PCWSTR::null(),
                SHOW_WINDOW_CMD(SW_SHOWNORMAL.0),
            )
        };
        if result.0 as isize <= 32 {
            return Err(crate::Error::Command(continuity_command::Error::Other(
                format!(
                    "ShellExecuteW failed (code={}) for {}",
                    result.0 as isize,
                    settings_path.display()
                ),
            )));
        }
        Ok(())
    }
}

// Seed + legacy-cheatsheet migration live in
// [`crate::window_settings_seed`] — see that module for the
// rationale.
