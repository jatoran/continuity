//! Build the initial set of [`SpawnRequest`]s the registry runs through
//! at process startup.
//!
//! Lives next to [`crate::main`] for the 600-line cap. The job here is
//! to:
//!
//! 1. Load every non-tombstoned `windows` row.
//! 2. For each row, materialize the buffers its pane tree references.
//! 3. Surface any recovery halt (checksum mismatch, decoder failure)
//!    as a launch banner on the first restored window — δ.3 promise:
//!    the user always sees that recovery halted.
//! 4. Bug A fix — never silently drop a row. A pane-tree JSON that
//!    fails strict decode now produces a [`SeedOutcome::PartialRestore`]:
//!    the window keeps its id + placement, opens with a fresh tree
//!    built around the best recovered buffer or a fresh untitled buffer,
//!    and surfaces the
//!    decoder error verbatim in the banner.
//!
//! Thread ownership: every function here runs on the registry's main
//! thread, before window threads spawn.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use continuity_buffer::BufferId;
use continuity_core::EditorHandle;
use continuity_persist::{
    rebuild_buffer_with_halt, PersistClient, RecoveryHalt, RecoveryHaltReason, WindowRow,
};
use continuity_ui::RestoredState;

use crate::error::Error;
use crate::registry::SpawnRequest;

fn persist_ctx<T>(
    context: &str,
    r: std::result::Result<T, continuity_persist::Error>,
) -> std::result::Result<T, Error> {
    r.map_err(|source| Error::Persist {
        context: context.to_string(),
        source,
    })
}

/// Outcome of seeding a window row's initial buffer at restore time.
pub(crate) enum SeedOutcome {
    /// The pane-tree JSON decoded cleanly; the contained buffer id is
    /// ready to land in the restored window.
    Restored(BufferId),
    /// The pane-tree JSON failed strict decode. We fell back to lenient
    /// buffer-id extraction (or, last-resort, a fresh untitled buffer).
    /// The window keeps its id + placement but opens with a fresh
    /// tree; the surfaced banner names the decoder error.
    PartialRestore {
        /// The fallback buffer id (lenient extraction's best pick, or
        /// a fresh untitled buffer when extraction failed).
        buffer_id: BufferId,
        /// Decoder error message — surfaced verbatim in the banner.
        error: String,
    },
}

/// Build the initial set of [`SpawnRequest`]s for the registry.
///
/// On a fresh install: one request seeded with `recover_or_open` (the
/// previous behavior). On a returning install: one request per
/// persisted `windows` row, each carrying the pane tree JSON; the
/// registry decodes it during window construction.
///
/// Recovery halts (checksum mismatch, decode failure, apply failure)
/// from any buffer reached on this path are collected and attached to
/// the **first** returned request as a `recovery_notices` payload so
/// the first window banners them on launch. δ.3 promise.
///
/// # Errors
///
/// Propagates persistence-thread failures from `load_active_windows`
/// and `most_recent_buffer` (both are infrastructure-level errors —
/// the database itself is degraded). Strict-decode failures on a
/// pane_tree_json do **not** propagate here: they funnel into a
/// `SpawnRequest` with `recovery_notices` populated so the window
/// survives.
pub(crate) fn build_initial_requests(
    persist: &PersistClient,
    editor: &Arc<EditorHandle>,
    db: &Path,
) -> std::result::Result<Vec<SpawnRequest>, Error> {
    // Consume the clean-exit marker up front. `true` means the previous
    // run quit gracefully (the user intentionally closed everything), so
    // an empty window set starts blank rather than resurrecting the
    // most-recent buffer. `false` (crash / kill / first launch) keeps the
    // recovery path.
    let clean_exit = consume_clean_exit_flag(db);
    let rows = persist_ctx("loading persisted windows", persist.load_active_windows())?;
    let mut halts: Vec<RecoveryHalt> = Vec::new();
    if rows.is_empty() {
        let buffer_id = initial_buffer_for_empty_session(persist, editor, &mut halts, clean_exit)?;
        let open_tutorial_on_init = take_first_launch_flag();
        return Ok(vec![SpawnRequest {
            initial_buffer_id: buffer_id,
            restored: None,
            explicit_origin: None,
            cascade_from: None,
            recovery_notices: format_recovery_halts(&halts),
            open_tutorial_on_init,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: Vec::new(),
        }]);
    }
    let mut out = Vec::with_capacity(rows.len());
    let mut adopted = HashSet::new();
    for row in rows {
        let outcome = seed_buffer_for_row(persist, editor, &row, &mut adopted, &mut halts);
        let (buffer_id, restored, partial_notice) = match outcome {
            SeedOutcome::Restored(b) => (
                b,
                Some((
                    row.id,
                    RestoredState {
                        pane_tree_json: row.pane_tree_json,
                        virtual_desktop_guid: row.virtual_desktop_guid,
                        placement_blob: row.placement_blob,
                    },
                )),
                None,
            ),
            SeedOutcome::PartialRestore { buffer_id, error } => {
                let banner = format!(
                    "Partial session restore — window {} had decoder error ({error}). \
                     Opened a fresh tab; closed-history retains the original.",
                    row.id.as_uuid()
                );
                (
                    buffer_id,
                    Some((
                        row.id,
                        RestoredState {
                            pane_tree_json: fallback_pane_tree_json(buffer_id),
                            virtual_desktop_guid: row.virtual_desktop_guid,
                            placement_blob: row.placement_blob,
                        },
                    )),
                    Some(banner),
                )
            }
        };
        let mut recovery_notices: Vec<String> = Vec::new();
        if let Some(b) = partial_notice {
            recovery_notices.push(b);
        }
        out.push(SpawnRequest {
            initial_buffer_id: buffer_id,
            restored,
            explicit_origin: None,
            cascade_from: None,
            recovery_notices,
            open_tutorial_on_init: false,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: Vec::new(),
        });
    }
    if out.is_empty() {
        let buffer_id = initial_buffer_for_empty_session(persist, editor, &mut halts, clean_exit)?;
        let open_tutorial_on_init = take_first_launch_flag();
        return Ok(vec![SpawnRequest {
            initial_buffer_id: buffer_id,
            restored: None,
            explicit_origin: None,
            cascade_from: None,
            recovery_notices: format_recovery_halts(&halts),
            open_tutorial_on_init,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: Vec::new(),
        }]);
    }
    if let Some(first) = out.first_mut() {
        let mut existing = std::mem::take(&mut first.recovery_notices);
        let mut halt_notices = format_recovery_halts(&halts);
        existing.append(&mut halt_notices);
        first.recovery_notices = existing;
        first.open_tutorial_on_init = take_first_launch_flag();
    }
    Ok(out)
}

/// Materialize the buffer referenced by a row's pane tree (if any) and
/// return its id. Bug A fix: never returns `Err` for any decode shape.
pub(crate) fn seed_buffer_for_row(
    persist: &PersistClient,
    editor: &Arc<EditorHandle>,
    row: &WindowRow,
    adopted: &mut HashSet<BufferId>,
    halts: &mut Vec<RecoveryHalt>,
) -> SeedOutcome {
    let mut strict_error = continuity_ui::pane_tree_codec::decode_with_state(&row.pane_tree_json)
        .err()
        .map(|e| e.to_string());
    let ids = if strict_error.is_none() {
        match continuity_ui::pane_tree_codec::buffer_ids_in_json(&row.pane_tree_json) {
            Ok(ids) => ids,
            Err(e) => {
                strict_error = Some(e.to_string());
                continuity_ui::pane_tree_codec::legacy::buffer_ids_in_json_lenient(
                    &row.pane_tree_json,
                )
            }
        }
    } else {
        continuity_ui::pane_tree_codec::legacy::buffer_ids_in_json_lenient(&row.pane_tree_json)
    };
    let active = continuity_ui::pane_tree_codec::active_buffer_id_in_json(&row.pane_tree_json).ok();
    let mut materialized = HashSet::new();
    let mut first_alive: Option<BufferId> = None;
    for id in &ids {
        if adopted.contains(id) {
            materialized.insert(*id);
            first_alive.get_or_insert(*id);
            continue;
        }
        if let Ok(Some(_)) = recover_buffer_by_id(persist, editor, *id, halts) {
            adopted.insert(*id);
            materialized.insert(*id);
            first_alive.get_or_insert(*id);
        }
    }
    let active_materialized = match active {
        Some(active) if materialized.contains(&active) => Some(active),
        _ => None,
    };
    if let Some(error) = strict_error {
        let buffer_id = active_materialized
            .or(first_alive)
            .unwrap_or_else(|| editor.open_buffer(""));
        return SeedOutcome::PartialRestore { buffer_id, error };
    }
    let buffer_id = active_materialized.unwrap_or_else(|| {
        recover_or_open(persist, editor, halts).unwrap_or_else(|_| editor.open_buffer(""))
    });
    SeedOutcome::Restored(buffer_id)
}

fn fallback_pane_tree_json(buffer_id: BufferId) -> String {
    continuity_ui::pane_tree_codec::encode(&continuity_ui::pane_tree::PaneTree::singleton(
        buffer_id, 0,
    ))
}

/// Pick the initial buffer when no window rows survive. A clean previous
/// exit starts blank (intentional close ⇒ don't resurrect the buffer the
/// user just closed); otherwise (crash / first launch) recover the
/// most-recent buffer so unsaved work is never silently dropped.
///
/// # Errors
///
/// Propagates `recover_or_open`'s infrastructure failures.
fn initial_buffer_for_empty_session(
    persist: &PersistClient,
    editor: &Arc<EditorHandle>,
    halts: &mut Vec<RecoveryHalt>,
    clean_exit: bool,
) -> std::result::Result<BufferId, Error> {
    if clean_exit {
        return Ok(editor.open_buffer(""));
    }
    recover_or_open(persist, editor, halts)
}

/// Restore the most-recently-touched buffer if one exists; otherwise
/// open a fresh empty buffer.
///
/// # Errors
///
/// Propagates `most_recent_buffer` infrastructure failures.
pub(crate) fn recover_or_open(
    persist: &PersistClient,
    editor: &Arc<EditorHandle>,
    halts: &mut Vec<RecoveryHalt>,
) -> std::result::Result<BufferId, Error> {
    let Some(prev_id) = persist_ctx("querying most-recent buffer", persist.most_recent_buffer())?
    else {
        return Ok(editor.open_buffer(""));
    };
    match recover_buffer_by_id(persist, editor, prev_id, halts)? {
        Some(id) => Ok(id),
        None => Ok(editor.open_buffer("")),
    }
}

pub(crate) fn recover_buffer_by_id(
    persist: &PersistClient,
    editor: &Arc<EditorHandle>,
    buffer_id: BufferId,
    halts: &mut Vec<RecoveryHalt>,
) -> std::result::Result<Option<BufferId>, Error> {
    let uuid = buffer_id.as_uuid();
    let Some(snap) = persist_ctx(
        &format!("loading latest snapshot for {uuid}"),
        persist.load_latest_snapshot(buffer_id),
    )?
    else {
        return Ok(None);
    };
    let edits = persist_ctx(
        &format!("loading trailing edit log for {uuid}"),
        persist.edits_since(buffer_id, snap.revision),
    )?;
    let next_seq = persist_ctx(
        &format!("loading next edit seq for {uuid}"),
        persist.next_seq(buffer_id),
    )?;
    let file = persist_ctx(
        &format!("loading file metadata for {uuid}"),
        persist.load_buffer_file(buffer_id),
    )?;
    let (buffer, halt) = rebuild_buffer_with_halt(buffer_id, &snap, edits, file);
    if let Some(halt) = halt {
        eprintln!(
            "continuity: {}",
            format_recovery_halt_for_log(&halt, buffer_id)
        );
        halts.push(halt);
    }
    let now = unix_ms_now();
    Ok(Some(editor.adopt_buffer(buffer, next_seq, now)))
}

/// Format a `RecoveryHalt` into the user-facing banner string.
pub(crate) fn format_recovery_halts(halts: &[RecoveryHalt]) -> Vec<String> {
    halts.iter().map(format_recovery_halt).collect()
}

pub(crate) fn format_recovery_halt(halt: &RecoveryHalt) -> String {
    let reason = match &halt.reason {
        RecoveryHaltReason::DecodeFailed(msg) => format!("decode error: {msg}"),
        RecoveryHaltReason::ApplyFailed(msg) => format!("apply error: {msg}"),
        RecoveryHaltReason::ChecksumMismatch { computed, expected } => {
            format!("checksum mismatch (got {computed:016x}, expected {expected:016x})")
        }
    };
    format!(
        "Recovery halted for buffer {} at seq {} — {}. Opened read-only at revision {}.",
        halt.buffer_id.as_uuid(),
        halt.halted_at_seq,
        reason,
        halt.last_valid_revision.get(),
    )
}

fn format_recovery_halt_for_log(halt: &RecoveryHalt, buffer_id: BufferId) -> String {
    let reason = match &halt.reason {
        RecoveryHaltReason::DecodeFailed(msg) => format!("decode error: {msg}"),
        RecoveryHaltReason::ApplyFailed(msg) => format!("apply error: {msg}"),
        RecoveryHaltReason::ChecksumMismatch { computed, expected } => {
            format!("checksum mismatch (computed {computed:016x}, expected {expected:016x})")
        }
    };
    format!(
        "replay halted for buffer {} at seq {} — {} (last valid revision {})",
        buffer_id.as_uuid(),
        halt.halted_at_seq,
        reason,
        halt.last_valid_revision.get(),
    )
}

/// `true` exactly once per fresh install — when the `tutorial_seen`
/// sentinel is missing. Creates the sentinel before returning so a
/// crash mid-launch never replays the tutorial open on the next run.
fn take_first_launch_flag() -> bool {
    let Ok(path) = continuity_persist::tutorial_seen_path() else {
        return false;
    };
    if path.exists() {
        return false;
    }
    match std::fs::write(&path, b"") {
        Ok(()) => true,
        Err(e) => {
            eprintln!(
                "continuity: tutorial first-launch sentinel write failed at {}: {e}",
                path.display()
            );
            false
        }
    }
}

/// Resolve the clean-exit sentinel path beside the active database. Tying
/// it to the db directory (rather than a global path) keeps it out of
/// unrelated data dirs — including the tempdir DBs unit tests use.
fn clean_exit_marker(db: &Path) -> Option<std::path::PathBuf> {
    db.parent().map(|parent| parent.join(".clean_exit"))
}

/// Read and clear the clean-exit marker. `true` means the previous run
/// exited gracefully, so this launch should start blank instead of
/// recovering the most-recent buffer. The marker is removed immediately
/// so a crash *this* run leaves it absent ⇒ the next launch recovers.
fn consume_clean_exit_flag(db: &Path) -> bool {
    let Some(path) = clean_exit_marker(db) else {
        return false;
    };
    if !path.exists() {
        return false;
    }
    if let Err(e) = std::fs::remove_file(&path) {
        // The marker existed (the meaningful signal); a failed delete only
        // risks the next launch also reading it as clean. A graceful exit
        // rewrites it and a crash cannot, so this self-corrects.
        eprintln!(
            "continuity: failed to clear clean-exit marker at {}: {e}",
            path.display()
        );
    }
    true
}

/// Write the clean-exit marker. Called from `main` only after the
/// registry loop returns — i.e. every window closed gracefully. A crash,
/// panic, or kill skips this, leaving the marker absent so the next
/// launch restores the session.
pub(crate) fn mark_clean_exit(db: &Path) {
    let Some(path) = clean_exit_marker(db) else {
        return;
    };
    if let Err(e) = std::fs::write(&path, b"") {
        eprintln!(
            "continuity: failed to write clean-exit marker at {}: {e}",
            path.display()
        );
    }
}

fn unix_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use continuity_buffer::{BufferId, Revision};
    use continuity_core::SystemClock;
    use continuity_persist::{EditRow, PersistHandle, SnapshotRow};

    use super::*;

    #[test]
    fn recovered_snapshot_decoration_populates_inline_colors_and_formulas() {
        let id = BufferId::new();
        let content = "{#ff0000:red text}\n\n| A | B |\n|---|---|\n| 1 | =1+2 |\n";
        let snapshot = SnapshotRow {
            id: Some(1),
            buffer_id: id,
            revision: Revision(4),
            content: content.to_string(),
            byte_len: content.len() as u64,
            line_count: content.lines().count() as u32,
            checksum: 0,
            label: None,
            created_at_ms: 0,
        };
        let (buffer, halt) = rebuild_buffer_with_halt(id, &snapshot, Vec::new(), None);
        assert!(halt.is_none(), "empty edit log should not halt");
        let source = buffer.rope().to_string();
        let decorations =
            continuity_decorate::Decorations::compute(&source, buffer.revision().get())
                .expect("decorations compute for recovered snapshot");
        assert!(!decorations.inline_color_spans.is_empty());
        assert!(!decorations.evaluated_tables.is_empty());
        assert_eq!(decorations.revision, 4);
    }

    #[test]
    fn format_recovery_halt_mentions_revision_and_reason() {
        let buffer_id = BufferId::new();
        let halt = RecoveryHalt {
            buffer_id,
            halted_at_seq: 42,
            last_valid_revision: Revision(7),
            reason: RecoveryHaltReason::ChecksumMismatch {
                computed: 0x1234_5678_9abc_def0,
                expected: 0xfedc_ba98_7654_3210,
            },
        };
        let banner = format_recovery_halt(&halt);
        assert!(banner.contains(&buffer_id.as_uuid().to_string()));
        assert!(banner.contains("seq 42"));
        assert!(banner.contains("revision 7"));
        assert!(banner.contains("checksum mismatch"));
    }

    /// δ.3 acceptance — corrupt the persisted edit log for the
    /// most-recent buffer and assert `build_initial_requests` returns
    /// a SpawnRequest with a non-empty `recovery_notices` payload.
    #[test]
    fn corrupt_edit_row_surfaces_recovery_notice() {
        use continuity_buffer::Buffer;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("recovery_notice.db");
        let persist = PersistHandle::spawn(&db).unwrap();
        let client = persist.client();
        let editor = Arc::new(EditorHandle::spawn(client.clone(), Arc::new(SystemClock)));
        let buf = Buffer::from_text("hi");
        let buffer_id = buf.id();
        client.upsert_buffer(buffer_id, 1, 1).unwrap();
        client
            .save_snapshot_blocking(buffer_id, buf.snapshot())
            .unwrap();
        let bad_row = EditRow {
            buffer_id,
            seq: 1,
            revision: Revision(1),
            ts_ms: 2,
            op_kind: "insert".into(),
            range_start_line: Some(0),
            range_start_byte: Some(2),
            range_end_line: None,
            range_end_byte: None,
            inserted_text: Some("!".into()),
            removed_text: None,
            selections_before_json: None,
            selections_after_json: None,
            undo_group_id: None,
            checksum_after: 0xDEAD_BEEF_DEAD_BEEF,
        };
        client.append_edit(bad_row).unwrap();
        let _ = client.most_recent_buffer().unwrap();
        let requests =
            build_initial_requests(&client, &editor, &db).expect("build_initial_requests");
        assert_eq!(requests.len(), 1);
        let notices = &requests[0].recovery_notices;
        assert!(!notices.is_empty());
        let banner = &notices[0];
        assert!(banner.contains("checksum mismatch"));
        assert!(banner.contains("revision 0"));
        assert!(banner.contains(&buffer_id.as_uuid().to_string()));
    }

    /// Bug A acceptance — a window row whose pane_tree_json is total
    /// gibberish must still produce a `SpawnRequest`, never get
    /// silently dropped. The request carries a partial-restore
    /// banner naming the decoder error and keeps the window's id.
    #[test]
    fn malformed_pane_tree_produces_partial_restore_request_not_skip() {
        use continuity_buffer::{Buffer, WindowId};
        use continuity_persist::WindowRow;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("partial_restore.db");
        let persist = PersistHandle::spawn(&db).unwrap();
        let client = persist.client();
        let editor = Arc::new(EditorHandle::spawn(client.clone(), Arc::new(SystemClock)));
        // Stage a buffer so recover_or_open has something to fall back to.
        let buf = Buffer::from_text("seed");
        let buffer_id = buf.id();
        client.upsert_buffer(buffer_id, 1, 1).unwrap();
        client
            .save_snapshot_blocking(buffer_id, buf.snapshot())
            .unwrap();
        // Stage a windows row whose pane_tree_json fails strict decode.
        let win = WindowId::new();
        let row = WindowRow {
            id: win,
            virtual_desktop_guid: None,
            monitor_id: None,
            placement_blob: None,
            pane_tree_json: "not even json at all".into(),
            last_seen_ms: 42,
        };
        client.save_window(row).unwrap();
        let requests =
            build_initial_requests(&client, &editor, &db).expect("build_initial_requests");
        assert_eq!(
            requests.len(),
            1,
            "malformed window must still produce a spawn request"
        );
        let req = &requests[0];
        let restored = req.restored.as_ref().expect("restored info preserved");
        assert_eq!(
            restored.0, win,
            "window id preserved across partial restore"
        );
        assert!(
            req.recovery_notices
                .iter()
                .any(|n| n.contains("decoder error")),
            "partial-restore banner missing: {:?}",
            req.recovery_notices
        );
    }
}
