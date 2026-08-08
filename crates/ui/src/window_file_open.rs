//! Routing for successful file-open reads.
//!
//! Runtime file opens belong in fresh top-level windows. The UI thread
//! owns only the source HWND and asks the app registry to choose/reuse the
//! file buffer and spawn the window. Test harnesses without a registry
//! keep the local-tab fallback.

use continuity_buffer::FileAssociation;
use continuity_command::ViewContext;

use crate::pane_tree::PaneId;
use crate::window::Window;
use crate::window_config::{FileOpenDisposition, OpenFileWindowRequest};

impl Window {
    pub(crate) fn handle_opened_file(
        &mut self,
        target_pane: Option<PaneId>,
        content: String,
        file: FileAssociation,
        disposition: FileOpenDisposition,
    ) {
        if let Some(open_file_window) = self.open_file_window.as_ref() {
            open_file_window(OpenFileWindowRequest {
                content,
                file,
                explicit_origin: None,
                cascade_from: ViewContext::current_window_rect(self),
                recovery_notices: Vec::new(),
                disposition,
                source_window_id: self.persistence.as_ref().map(|state| state.window_id),
                vault_root: self.vault.root().map(std::path::Path::to_path_buf),
            });
            return;
        }
        self.adopt_opened_file(target_pane, content, file, disposition);
    }

    fn adopt_opened_file(
        &mut self,
        target_pane: Option<PaneId>,
        content: String,
        file: FileAssociation,
        disposition: FileOpenDisposition,
    ) {
        let buffer_id = self.editor.open_file_buffer(content, file.clone());
        if let Some(register_file_buffer) = self.register_file_buffer.as_ref() {
            register_file_buffer(buffer_id, file.clone());
        }
        self.save_current_right_edge_chrome_state();
        if let Some(pane) = target_pane {
            self.switch_focus(pane);
        }
        self.adopt_routed_file_buffer(buffer_id, disposition);
        self.mark_tab_file_associated(buffer_id, &file);
        self.refresh_focused_viewport();
        self.refresh_language();
        self.maybe_submit_decoration();
        self.apply_vault_tab_restore_on_open(&file.path.clone(), buffer_id);
        if let Some(file_io) = self.file_io.as_ref() {
            let _ = file_io.watch_file(buffer_id, file);
        }
        let _ = self.try_dispatch_projection_worker_early("file_open", "focus_change");
        self.retarget_find_bar_to_focused_pane();
    }

    pub(crate) fn adopt_routed_file_buffer(
        &mut self,
        buffer_id: continuity_buffer::BufferId,
        disposition: FileOpenDisposition,
    ) {
        if disposition == FileOpenDisposition::Preview
            && self.replace_focused_file_tree_tab(buffer_id)
        {
            return;
        }
        let tab_id = self.tree.insert_fresh_buffer_tab(buffer_id, self.now_ms());
        if let Some(group) = self.tree.groups.get_mut(&self.tree.focused) {
            group.push_tab(tab_id, true);
        }
        if disposition == FileOpenDisposition::Preview {
            self.file_tree_preview_tabs
                .insert(self.tree.focused, tab_id);
        }
        self.apply_new_pane_state(buffer_id);
    }

    fn replace_focused_file_tree_tab(&mut self, buffer_id: continuity_buffer::BufferId) -> bool {
        let pane = self.tree.focused;
        let Some(tab_id) = self.tree.groups.get(&pane).map(|group| group.active) else {
            return false;
        };
        if let Some(tab) = self.tree.tabs.get_mut(&tab_id) {
            tab.buffer_id = buffer_id;
            tab.label_override = None;
            tab.file_associated = true;
            tab.pinned = false;
        } else {
            return false;
        }
        self.file_tree_preview_tabs.insert(pane, tab_id);
        if let Some(group) = self.tree.groups.get_mut(&pane) {
            group.activate(tab_id);
        }
        self.adopt_focused_tab();
        self.request_state_save();
        true
    }
}
