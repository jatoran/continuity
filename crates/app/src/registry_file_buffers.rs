//! Cross-window file-buffer routing for the registry.
//!
//! The registry owns the app-level file-path index. UI windows can ask
//! for a fresh top-level file window without owning cross-window state.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use continuity_buffer::{BufferId, FileAssociation};
use continuity_core::EditorHandle;
use continuity_ui::window_config::{OpenFileWindow, OpenFileWindowRequest, RegisterFileBuffer};

use crate::registry::{RegistryCtx, RegistryEvent, SpawnRequest};

/// Shared app-level index from normalized file paths to live buffers.
///
/// The registry owns the logical contents; callbacks from UI threads take
/// the mutex briefly to look up or refresh one path.
pub(crate) type FileBufferIndex = Arc<Mutex<Vec<(PathBuf, BufferId)>>>;

/// Build the initial file-buffer index from restored launch requests.
#[must_use]
pub(crate) fn build_file_buffer_index(
    requests: &[SpawnRequest],
    editor: &Arc<EditorHandle>,
) -> FileBufferIndex {
    let mut buffer_ids = std::collections::HashSet::new();
    for request in requests {
        buffer_ids.insert(request.initial_buffer_id);
        if let Some((_, restored)) = request.restored.as_ref() {
            if let Ok(restored_ids) =
                continuity_ui::pane_tree_codec::buffer_ids_in_json(&restored.pane_tree_json)
            {
                buffer_ids.extend(restored_ids);
            }
        }
        buffer_ids.extend(request.startup_open_buffer_ids.iter().copied());
    }
    let mut entries = Vec::new();
    for buffer_id in buffer_ids {
        let Some(snapshot) = editor.snapshot(buffer_id) else {
            continue;
        };
        let Some(file) = snapshot.file else {
            continue;
        };
        upsert_file_buffer_entry(&mut entries, file.path, buffer_id);
    }
    Arc::new(Mutex::new(entries))
}

/// Register or refresh one file-associated buffer in the app-level index.
pub(crate) fn register_file_buffer(index: &FileBufferIndex, path: PathBuf, buffer_id: BufferId) {
    let Ok(mut entries) = index.lock() else {
        eprintln!("continuity: file-buffer index lock poisoned while registering file");
        return;
    };
    entries.retain(|(_, existing_buffer_id)| *existing_buffer_id != buffer_id);
    upsert_file_buffer_entry(&mut entries, path, buffer_id);
}

/// Return the live buffer already associated with `path`, if any.
pub(crate) fn file_buffer_for_path(
    editor: &Arc<EditorHandle>,
    index: &FileBufferIndex,
    path: &Path,
) -> Option<BufferId> {
    find_live_file_buffer(editor, index, path)
}

/// Build the callback UI windows use to open files in fresh windows.
pub(crate) fn make_open_file_window_handler(ctx: &RegistryCtx) -> OpenFileWindow {
    let tx = ctx.tx.clone();
    let editor = Arc::clone(&ctx.editor);
    let file_buffer_index = Arc::clone(&ctx.file_buffer_index);
    Arc::new(move |request: OpenFileWindowRequest| {
        let file = request.file;
        let buffer_id = buffer_id_for_open_file(&editor, &file_buffer_index, request.content, file);
        let _ = tx.send(RegistryEvent::Spawn(SpawnRequest {
            initial_buffer_id: buffer_id,
            restored: None,
            activate_on_restore: false,
            explicit_origin: request.explicit_origin,
            cascade_from: request.cascade_from,
            recovery_notices: request.recovery_notices,
            open_tutorial_on_init: false,
            startup_open_buffer_ids: Vec::new(),
            startup_folder_roots: Vec::new(),
        }));
    })
}

/// Build the callback UI windows use to keep file-buffer routing fresh.
pub(crate) fn make_register_file_buffer_handler(ctx: &RegistryCtx) -> RegisterFileBuffer {
    let file_buffer_index = Arc::clone(&ctx.file_buffer_index);
    Arc::new(move |buffer_id, file| {
        register_file_buffer(&file_buffer_index, file.path, buffer_id);
    })
}

fn buffer_id_for_open_file(
    editor: &Arc<EditorHandle>,
    index: &FileBufferIndex,
    content: String,
    file: FileAssociation,
) -> BufferId {
    if let Some(buffer_id) = find_live_file_buffer(editor, index, &file.path) {
        return buffer_id;
    }
    let path = file.path.clone();
    let buffer_id = editor.open_file_buffer(content, file);
    register_file_buffer(index, path, buffer_id);
    buffer_id
}

fn find_live_file_buffer(
    editor: &Arc<EditorHandle>,
    index: &FileBufferIndex,
    path: &Path,
) -> Option<BufferId> {
    let candidate = {
        let Ok(entries) = index.lock() else {
            eprintln!("continuity: file-buffer index lock poisoned while opening file");
            return None;
        };
        entries
            .iter()
            .find(|(existing, _)| is_same_existing_file_path(existing, path))
            .map(|(_, buffer_id)| *buffer_id)
    };
    let buffer_id = candidate?;
    if editor.snapshot(buffer_id).is_some() {
        return Some(buffer_id);
    }
    remove_file_buffer(index, buffer_id);
    None
}

fn remove_file_buffer(index: &FileBufferIndex, buffer_id: BufferId) {
    let Ok(mut entries) = index.lock() else {
        eprintln!("continuity: file-buffer index lock poisoned while pruning file");
        return;
    };
    entries.retain(|(_, existing_buffer_id)| *existing_buffer_id != buffer_id);
}

fn upsert_file_buffer_entry(
    entries: &mut Vec<(PathBuf, BufferId)>,
    path: PathBuf,
    buffer_id: BufferId,
) {
    if let Some((existing_path, existing_buffer_id)) = entries
        .iter_mut()
        .find(|(existing, _)| is_same_existing_file_path(existing, &path))
    {
        *existing_path = path;
        *existing_buffer_id = buffer_id;
        return;
    }
    entries.push((path, buffer_id));
}

fn is_same_existing_file_path(left: &Path, right: &Path) -> bool {
    let left = normalize_existing_path(left);
    let right = normalize_existing_path(right);
    left == right
        || left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
}

fn normalize_existing_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
