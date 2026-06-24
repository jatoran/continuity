//! Per-window [`continuity_command::Registry`] construction.
//!
//! Split out of [`crate::registry`] to keep that module under the
//! 600-line cap. `build_registry` registers every command family and
//! wires the `window.new_window` / `window.tear_off_focused_tab` /
//! `tab.reopen_closed` handlers that need to talk back to the registry
//! main loop via [`RegistryEvent`].

use std::sync::Arc;

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

use crate::registry::{RegistryCtx, RegistryEvent, SpawnRequest};
use crate::registry_closed_history::smart_reopen_handler;

pub(crate) fn build_registry(ctx: &RegistryCtx) -> Registry {
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
            activate_on_restore: false,
            explicit_origin: None,
            cascade_from: ctx.current_window_rect(),
            recovery_notices: Vec::new(),
            open_tutorial_on_init: false,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: Vec::new(),
            reconcile_on_init: None,
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
            activate_on_restore: false,
            explicit_origin,
            cascade_from,
            recovery_notices: Vec::new(),
            open_tutorial_on_init: false,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: Vec::new(),
            reconcile_on_init: None,
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
