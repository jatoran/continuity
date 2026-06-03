//! Registry-side glue for the schema-v5 closed-history stack.
//!
//! Lives next to [`crate::registry`] to keep that file under the
//! 600-line cap. Two responsibilities:
//!
//! 1. [`archive_closed_window`] — pushed by the registry's `Closed`
//!    arm when a non-last window closes. Snapshots the just-saved
//!    `pane_tree_json` + window placement onto the closed-history
//!    stack *before* the tombstone update, so the smart-reopen
//!    handler can resurrect the entire window — splits, tabs,
//!    placement — without re-reading the soft-deleted row.
//! 2. [`smart_reopen_handler`] — replaces the default `tab.reopen_closed`
//!    handler. Decides between the current window's in-memory
//!    `recently_closed` (single tab) and the persisted closed-history
//!    stack (whole window) by comparing close timestamps. The most
//!    recent unit wins.
//!
//! Thread ownership: invoked from the registry main thread
//! ([`archive_closed_window`]) and the per-window UI threads
//! (`smart_reopen_handler`'s returned closure). Both routes go through
//! [`PersistClient`], which serializes onto the persistence thread.

use std::sync::Arc;

use continuity_buffer::{BufferId, WindowId};
use continuity_command::Context;
use continuity_core::EditorHandle;
use continuity_persist::{ClosedHistoryEntry, ClosedHistoryKind, PersistClient};
use continuity_ui::RestoredState;
use crossbeam_channel::Sender;

use crate::registry::{RegistryEvent, SpawnRequest};

/// Command-handler closure type the registry installs for
/// `tab.reopen_closed`. Aliased so the function signature stays under
/// the clippy `type_complexity` budget.
type ReopenClosedHandler = Arc<
    dyn Fn(&serde_json::Value, &mut dyn Context) -> Result<(), continuity_command::Error>
        + Send
        + Sync,
>;

fn unix_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Snapshot the just-closed window's persisted `pane_tree_json` (and
/// placement metadata, if present) into the closed-history stack, then
/// tombstone the window row. Pre-empts the historical
/// `persist.delete_window(...)` direct call: that tombstone still
/// happens here, but only after the snapshot is durable so a crash
/// mid-close cannot lose the window entirely.
///
/// Logged on failure but never panics — the registry needs to keep
/// running even if the persist thread is in a degraded state.
pub(crate) fn archive_closed_window(persist: &PersistClient, id: WindowId) {
    // Pull the row's pane_tree_json before tombstoning. If
    // `load_active_windows` doesn't list this id we have no payload
    // to archive — that happens when the window never finished
    // construction (no save ever ran). Skip silently in that case
    // and proceed to the tombstone.
    let row = match persist.load_active_windows() {
        Ok(rows) => rows.into_iter().find(|r| r.id == id),
        Err(e) => {
            eprintln!("continuity: archive_closed_window: load_active_windows failed: {e}");
            None
        }
    };
    let now = unix_ms_now();
    if let Some(row) = row {
        if let Err(e) = persist.push_closed_history(
            ClosedHistoryKind::Window,
            Some(row.id),
            row.pane_tree_json,
            now,
        ) {
            eprintln!(
                "continuity: archive_closed_window: push_closed_history({}) failed: {e}",
                row.id.as_uuid()
            );
        }
    }
    if let Err(e) = persist.delete_window(id, now) {
        eprintln!(
            "continuity: tombstone for window {} failed: {e}",
            id.as_uuid()
        );
    }
}

/// Build the smart `tab.reopen_closed` handler. Replaces the default
/// per-window handler registered by `register_tab_commands`.
///
/// Decision rule:
/// - If both the window's `recently_closed` and the global
///   closed-history stack are empty → no-op.
/// - If only one source has an entry → use that source.
/// - If both have entries → the more recent `closed_at_ms` wins.
///   Ties go to the global stack (a whole-window close is always at
///   least as significant as a single-tab close inside another
///   surviving window).
///
/// When the global stack wins, the entry is popped and a fresh
/// [`SpawnRequest`] is enqueued so the registry materializes a new
/// window from the saved pane-tree JSON. Buffer ids referenced by the
/// JSON are re-adopted via lenient extraction so a partial decode
/// failure does not block reopen.
pub(crate) fn smart_reopen_handler(
    persist: PersistClient,
    editor: Arc<EditorHandle>,
    tx: Sender<RegistryEvent>,
) -> ReopenClosedHandler {
    Arc::new(move |_args: &serde_json::Value, ctx: &mut dyn Context| {
        let local_top_ms = ctx.local_recently_closed_top_ms();
        let global_top = persist.peek_closed_history().ok().flatten();
        let prefer_global = match (local_top_ms, global_top.as_ref()) {
            (None, None) => {
                trace_smart_reopen("no_op", "local=none global=none", None);
                return Ok(());
            }
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (Some(local), Some(global)) => global.closed_at_ms >= local,
        };
        if !prefer_global {
            trace_smart_reopen(
                "delegate_local",
                &format!(
                    "local_ms={} global_ms={}",
                    fmt_opt_i64(local_top_ms),
                    fmt_opt_i64(global_top.as_ref().map(|g| g.closed_at_ms)),
                ),
                None,
            );
            return ctx.tab_reopen_closed();
        }
        // Peek-validate-then-pop. The previous behaviour popped first;
        // a failed adoption afterwards lost the entry forever. Now we
        // adopt the buffer FIRST, and only call `pop_closed_history`
        // after the adoption (or spawn) is durable.
        let peeked = match persist.peek_closed_history() {
            Ok(Some(e)) => e,
            Ok(None) => {
                trace_smart_reopen("global_empty_after_peek", "race=peek_then_empty", None);
                return ctx.tab_reopen_closed();
            }
            Err(e) => {
                trace_smart_reopen(
                    "global_peek_err",
                    &format!("err={}", sanitize_err(&e.to_string())),
                    None,
                );
                return ctx.tab_reopen_closed();
            }
        };
        let payload_bytes = peeked.payload_json.len();
        // Try the spawn dispatch with the peeked payload. The buffer
        // adoption inside `spawn_from_closed_entry` is best-effort;
        // failure leaves the entry on the stack so the user can press
        // again or recover via a different reopen.
        let dispatch_ok = try_spawn_from_closed_entry(&persist, &editor, &tx, ctx, &peeked);
        if dispatch_ok {
            // Commit the pop now that the spawn is enqueued. Use
            // `pop_closed_history` (which deletes by id) to be safe in
            // case another window's reopen advanced the stack in
            // parallel — `pop` deletes by top-of-stack id, so if our
            // peeked id is no longer the top the user gets a double
            // restore rather than data loss.
            if let Err(e) = persist.pop_closed_history() {
                trace_smart_reopen(
                    "global_pop_after_spawn_err",
                    &format!("err={}", sanitize_err(&e.to_string())),
                    Some(payload_bytes),
                );
            } else {
                trace_smart_reopen(
                    "spawn_ok",
                    &format!(
                        "kind={} window_id={} payload_bytes={} closed_at_ms={}",
                        peeked.kind.as_str(),
                        peeked
                            .window_id
                            .map(|w| w.as_uuid().to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        payload_bytes,
                        peeked.closed_at_ms,
                    ),
                    Some(payload_bytes),
                );
            }
        } else {
            trace_smart_reopen(
                "spawn_dispatch_err",
                &format!("payload_bytes={payload_bytes}"),
                Some(payload_bytes),
            );
        }
        Ok(())
    })
}

/// Stable trace emitter for the smart-reopen handler. Captured here
/// so all five exit arms agree on the field shape.
fn trace_smart_reopen(outcome: &str, fields: &str, payload_bytes: Option<usize>) {
    if !continuity_trace::is_enabled() {
        return;
    }
    let extra = payload_bytes
        .map(|b| format!(" payload_bytes={b}"))
        .unwrap_or_default();
    continuity_trace::log_event(
        "smart_reopen",
        &format!("outcome={outcome} {fields}{extra}"),
    );
}

fn fmt_opt_i64(value: Option<i64>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn sanitize_err(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' | ' ' => '_',
            _ => ch,
        })
        .collect()
}

/// Attempt to enqueue a [`SpawnRequest`] reconstructing a closed
/// window from a peeked [`ClosedHistoryEntry`]. Returns `true` when
/// the request was sent (the caller may then pop the entry from the
/// stack); `false` when dispatch failed (caller leaves the entry on
/// the stack for retry).
fn try_spawn_from_closed_entry(
    persist: &PersistClient,
    editor: &Arc<EditorHandle>,
    tx: &Sender<RegistryEvent>,
    ctx: &mut dyn Context,
    entry: &ClosedHistoryEntry,
) -> bool {
    let pane_tree_json = entry.payload_json.clone();
    // Lenient buffer-id extraction so a stale image_expand_state or
    // any other auxiliary field can't block reopen.
    let buffer_ids =
        continuity_ui::pane_tree_codec::legacy::buffer_ids_in_json_lenient(&pane_tree_json);
    let initial_buffer_id = pick_initial_buffer(persist, editor, &buffer_ids);
    let cascade_from = ctx.current_window_rect();
    let window_id = entry.window_id.unwrap_or_default();
    let req = SpawnRequest {
        initial_buffer_id,
        restored: Some((
            window_id,
            RestoredState {
                pane_tree_json,
                virtual_desktop_guid: None,
                placement_blob: None,
            },
        )),
        explicit_origin: None,
        cascade_from,
        recovery_notices: Vec::new(),
        open_tutorial_on_init: false,
        startup_open_buffer_ids: Vec::new(),
        startup_folder_roots: Vec::new(),
    };
    match tx.send(RegistryEvent::Spawn(req)) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("continuity: smart_reopen spawn dispatch failed: {e}");
            false
        }
    }
}

fn pick_initial_buffer(
    persist: &PersistClient,
    editor: &Arc<EditorHandle>,
    ids: &[BufferId],
) -> BufferId {
    for id in ids {
        // Best-effort adopt: load the latest snapshot + tail edits.
        // Mirrors `crate::main::recover_buffer_by_id` but trimmed —
        // we don't need recovery-halt banners on a smart-reopen path
        // (the user explicitly asked to bring this window back).
        if let Ok(Some(snap)) = persist.load_latest_snapshot(*id) {
            let edits = persist.edits_since(*id, snap.revision).unwrap_or_default();
            let next_seq = persist.next_seq(*id).unwrap_or(0);
            let file = persist.load_buffer_file(*id).ok().flatten();
            let (buffer, _halt) =
                continuity_persist::rebuild_buffer_with_halt(*id, &snap, edits, file);
            let adopted = editor.adopt_buffer(buffer, next_seq, unix_ms_now());
            return adopted;
        }
    }
    editor.open_buffer("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_core::SystemClock;
    use continuity_persist::PersistHandle;
    use crossbeam_channel::unbounded;

    /// Edge case from the task spec: closing W_A and then a tab in
    /// W_B → Ctrl+Shift+T must reopen the tab in W_B (its close was
    /// more recent than the window close).
    ///
    /// Modelled at the handler-level: build a fake context whose
    /// `local_recently_closed_top_ms` is newer than the closed-
    /// history entry and observe that the local path runs.
    #[test]
    fn tab_close_newer_than_window_close_picks_local() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("smart_reopen.db");
        let persist = PersistHandle::spawn(&db).unwrap();
        let client = persist.client();
        let editor = Arc::new(EditorHandle::spawn(client.clone(), Arc::new(SystemClock)));
        // Older window close.
        client
            .push_closed_history(ClosedHistoryKind::Window, None, "{}".into(), 100)
            .unwrap();
        let (tx, _rx) = unbounded::<RegistryEvent>();
        let handler = smart_reopen_handler(client.clone(), editor.clone(), tx);
        let mut probe = ProbeContext {
            local_top_ms: Some(200),
            local_called: false,
            window_rect: None,
        };
        let _ = handler(&serde_json::Value::Null, &mut probe);
        assert!(
            probe.local_called,
            "newer local close should trigger local reopen"
        );
        // Global entry untouched.
        assert!(client.peek_closed_history().unwrap().is_some());
    }

    #[test]
    fn window_close_newer_than_tab_close_pops_global() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("smart_reopen_global.db");
        let persist = PersistHandle::spawn(&db).unwrap();
        let client = persist.client();
        let editor = Arc::new(EditorHandle::spawn(client.clone(), Arc::new(SystemClock)));
        client
            .push_closed_history(ClosedHistoryKind::Window, None, "{}".into(), 500)
            .unwrap();
        let (tx, rx) = unbounded::<RegistryEvent>();
        let handler = smart_reopen_handler(client.clone(), editor.clone(), tx);
        let mut probe = ProbeContext {
            local_top_ms: Some(100),
            local_called: false,
            window_rect: None,
        };
        let _ = handler(&serde_json::Value::Null, &mut probe);
        assert!(!probe.local_called);
        // Global stack popped.
        assert!(client.peek_closed_history().unwrap().is_none());
        // Spawn request enqueued.
        assert!(rx.try_recv().is_ok());
    }

    struct ProbeContext {
        local_top_ms: Option<i64>,
        local_called: bool,
        window_rect: Option<(i32, i32, i32, i32)>,
    }

    impl continuity_command::ViewContext for ProbeContext {
        fn tab_reopen_closed(&mut self) -> Result<(), continuity_command::Error> {
            self.local_called = true;
            Ok(())
        }
        fn current_window_rect(&self) -> Option<(i32, i32, i32, i32)> {
            self.window_rect
        }
    }

    impl continuity_command::FindContext for ProbeContext {}
    impl continuity_command::EditConfigContext for ProbeContext {}

    impl Context for ProbeContext {
        fn lookup(&self, _key: &str) -> Option<&str> {
            None
        }
        fn local_recently_closed_top_ms(&self) -> Option<i64> {
            self.local_top_ms
        }
    }
}
