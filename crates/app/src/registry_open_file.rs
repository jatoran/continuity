//! File-open routing on the registry main thread.
//!
//! Split out of [`crate::registry`] to keep that module under the
//! 600-line cap. When a window opens a file, the registry resolves it to
//! one buffer (reusing the existing file buffer for an already-loaded
//! path) and then either reveals it in the window that already owns it —
//! focusing the existing tab instead of spawning a duplicate window — or
//! spawns a fresh window. Both paths reconcile the buffer against the
//! freshly-read disk bytes, which is what fixes a reopen of an
//! externally-changed file showing stale content.

use continuity_buffer::{BufferId, FileAssociation};
use continuity_ui::WindowControl;

use crate::error::Error;
use crate::registry::{spawn_window_thread, LiveState, RegistryCtx, SpawnRequest};
use crate::registry_file_buffers::resolve_open_file_buffer;

/// Inputs for [`handle_open_file_buffer`], grouped so the resolve/reveal/
/// spawn flow takes one payload instead of a long positional list.
pub(crate) struct OpenFileBufferArgs {
    /// Decoded disk content at read time.
    pub content: String,
    /// Filesystem association (path + mtime + raw/content hashes).
    pub file: FileAssociation,
    /// Requested origin, forwarded to a spawned window.
    pub explicit_origin: Option<(i32, i32)>,
    /// Source window rect, used to place a spawned window.
    pub cascade_from: Option<(i32, i32, i32, i32)>,
    /// Launch-time banners to surface in the target window (e.g. an
    /// encoding notice). Empty for ordinary in-process opens.
    pub recovery_notices: Vec<String>,
}

/// Resolve a file open to one buffer and either reveal it in the window
/// that already owns it or spawn a fresh window showing it, reconciling
/// the buffer against the freshly-read disk bytes in both cases.
pub(crate) fn handle_open_file_buffer(
    ctx: &RegistryCtx,
    state: &mut LiveState,
    args: OpenFileBufferArgs,
) -> Result<(), Error> {
    let OpenFileBufferArgs {
        content,
        file,
        explicit_origin,
        cascade_from,
        recovery_notices,
    } = args;
    let buffer_id =
        resolve_open_file_buffer(&ctx.editor, &ctx.file_buffer_index, &content, file.clone());
    // Prefer revealing in the live window that already owns the buffer —
    // focuses the existing tab rather than spawning a duplicate window.
    // Clone the payload only on this less-common reveal path; the spawn
    // fall-through keeps the originals.
    if let Some(home) = state.buffer_home.get(&buffer_id).copied() {
        if let Some(control_tx) = state.control_senders.get(&home) {
            if control_tx
                .send(WindowControl::RevealBufferTab {
                    buffer_id,
                    content: content.clone(),
                    file: file.clone(),
                    notices: recovery_notices.clone(),
                })
                .is_ok()
            {
                return Ok(());
            }
        }
        // Home window is gone (or its channel closed) — drop the stale
        // entry and spawn a fresh window below.
        state.buffer_home.remove(&buffer_id);
    }
    spawn_window_thread(
        ctx,
        state,
        open_file_spawn_request(
            buffer_id,
            content,
            file,
            explicit_origin,
            cascade_from,
            recovery_notices,
        ),
    )
}

/// Build the [`SpawnRequest`] for a freshly-opened file buffer, carrying
/// the disk bytes so the new window reconciles its initial buffer.
fn open_file_spawn_request(
    buffer_id: BufferId,
    content: String,
    file: FileAssociation,
    explicit_origin: Option<(i32, i32)>,
    cascade_from: Option<(i32, i32, i32, i32)>,
    recovery_notices: Vec<String>,
) -> SpawnRequest {
    SpawnRequest {
        initial_buffer_id: buffer_id,
        restored: None,
        activate_on_restore: false,
        explicit_origin,
        cascade_from,
        recovery_notices,
        open_tutorial_on_init: false,
        startup_open_buffer_ids: Vec::new(),
        startup_folder_roots: Vec::new(),
        reconcile_on_init: Some(continuity_ui::PendingReconcile { content, file }),
    }
}
