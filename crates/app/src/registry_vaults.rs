//! Known-vault registration and current-virtual-desktop launch routing.

use std::path::PathBuf;
use std::sync::Arc;

use continuity_persist::PersistClient;

use crate::error::Error;
use crate::registry::{LiveState, RegistryCtx, RegistryEvent, SpawnRequest};
use crate::registry_time::unix_ms_now;

pub(crate) enum VaultRegistryEvent {
    Open(PathBuf),
    Activated {
        window_id: continuity_buffer::WindowId,
        root: PathBuf,
    },
}

pub(crate) fn handle_event(
    ctx: &RegistryCtx,
    state: &mut LiveState,
    event: VaultRegistryEvent,
) -> Result<(), Error> {
    match event {
        VaultRegistryEvent::Open(root) => handle_open_vault(ctx, state, root),
        VaultRegistryEvent::Activated { window_id, root } => {
            state.vault_home.insert(window_id, root.clone());
            remember_known_vault(&ctx.persist, root);
            Ok(())
        }
    }
}

pub(crate) fn make_open_handler(
    ctx: &RegistryCtx,
) -> continuity_ui::window_config::OpenVaultWindow {
    let tx = ctx.tx.clone();
    Arc::new(move |root| {
        let _ = tx.send(RegistryEvent::Vault(VaultRegistryEvent::Open(root)));
    })
}

pub(crate) fn make_activated_handler(
    ctx: &RegistryCtx,
    window_id: continuity_buffer::WindowId,
) -> continuity_ui::window_config::VaultActivated {
    let tx = ctx.tx.clone();
    Arc::new(move |root| {
        let _ = tx.send(RegistryEvent::Vault(VaultRegistryEvent::Activated {
            window_id,
            root,
        }));
    })
}

pub(crate) fn handle_open_vault(
    ctx: &RegistryCtx,
    state: &mut LiveState,
    root: PathBuf,
) -> Result<(), Error> {
    for (window_id, active_root) in &state.vault_home {
        if active_root != &root {
            continue;
        }
        if let Some(raw_window) = state.control_windows.get(window_id).copied() {
            if continuity_win::activate_window_if_on_current_desktop(raw_window) {
                remember_known_vault(&ctx.persist, root);
                return Ok(());
            }
        }
    }
    let buffer_id = ctx.editor.open_buffer("");
    ctx.tx
        .send(RegistryEvent::Spawn(SpawnRequest {
            initial_buffer_id: buffer_id,
            restored: None,
            activate_on_restore: false,
            explicit_origin: None,
            cascade_from: None,
            recovery_notices: Vec::new(),
            open_tutorial_on_init: false,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: vec![root],
            reconcile_on_init: None,
        }))
        .map_err(|_| Error::RegistryClosed)
}

pub(crate) fn remember_known_vault(persist: &PersistClient, root: PathBuf) {
    let display_name = root
        .components()
        .next_back()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    let vault = continuity_persist::KnownVault {
        root_path: root,
        display_name,
        pinned: false,
        last_opened_ms: unix_ms_now(),
    };
    if let Err(error) = persist.upsert_known_vault(vault) {
        eprintln!("continuity: remember known vault failed: {error}");
    }
}
