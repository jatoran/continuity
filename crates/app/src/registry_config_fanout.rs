//! Registry-side fan-out of persistence and settings events to every live
//! window, plus the owner-routed settings effects that must land on their
//! single owner thread first. Split out of `registry.rs` so that file stays
//! under the conventions cap.
//!
//! Thread ownership: registry thread; reads `LiveState` and sends on the
//! per-window control channels.

use std::path::PathBuf;
use std::sync::Arc;

use continuity_config::{ConfigEvent, Settings};
use continuity_core::SnapshotPolicy;
use continuity_persist::{BackupConfig, BackupScheduler};
use continuity_ui::WindowControl;

use crate::registry::{LiveState, RegistryCtx};

/// δ.3 — fan a persistence-thread event out to every live window.
/// Wraps the event in [`WindowControl::PersistEvent`] so the window's
/// existing control-poll tick handles it like a config event.
pub(crate) fn fan_out_persist_event(state: &LiveState, event: continuity_persist::PersistEvent) {
    for tx in state.control_senders.values() {
        let _ = tx.send(WindowControl::PersistEvent(event.clone()));
    }
}

/// Apply the owner-side effects of a settings change *before* fanning the
/// event out to live windows. Keeps per-owner config (backup cadence,
/// persist sync mode) on its single owner thread instead of through
/// shared mutable state.
pub(crate) fn fan_out_config_event(
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
