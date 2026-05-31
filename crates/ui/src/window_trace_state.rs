//! Rich UI-state snapshots for manual trace files.
//!
//! Thread ownership: all helpers run on one window's UI thread. They
//! read UI-owned pane/view/cache state and may synchronously ask the
//! core thread for snapshots only when tracing is enabled.

use continuity_buffer::BufferId;
use continuity_core::EditorSnapshot;

use crate::pane_tree::Tab;
use crate::pane_tree_kind::TabKind;
use crate::window::Window;

impl Window {
    /// Append compact window context to a WndProc trace detail.
    pub(crate) fn trace_wndproc_detail(&self, base: String) -> String {
        if !crate::paint_trace::is_trace_enabled() {
            return base;
        }
        let active = ActiveTabTrace::from_window(self);
        format!(
            concat!(
                "{} hwnd={} ctx_buffer={} ctx_pane={} ctx_tab={} ",
                "ctx_kind={} overlay={} focused={} minimized={}"
            ),
            base,
            self.hwnd.0 as usize,
            format_buffer_id(self.buffer_id),
            self.tree.focused.0,
            active.tab_id,
            active.kind,
            self.overlays.is_active(),
            self.is_window_focused,
            self.is_window_minimized,
        )
    }

    /// Log one focused-paint state snapshot after a live core snapshot
    /// has been acquired.
    pub(crate) fn trace_paint_window_state(&self, snapshot: &EditorSnapshot) {
        if !crate::paint_trace::is_trace_enabled() {
            return;
        }
        crate::paint_trace::log_event(
            "paint:window_state",
            &self.trace_window_state_detail("paint", SnapshotTrace::Live(snapshot)),
        );
    }

    /// Log the state that caused paint to clear instead of render.
    pub(crate) fn trace_missing_snapshot(&self, reason: &'static str) {
        if !crate::paint_trace::is_trace_enabled() {
            return;
        }
        crate::paint_trace::log_event(
            "paint:no_snapshot",
            &self.trace_window_state_detail(reason, SnapshotTrace::Missing),
        );
    }

    fn trace_window_state_detail(&self, reason: &str, snapshot: SnapshotTrace<'_>) -> String {
        let active = ActiveTabTrace::from_window(self);
        let body = self.focused_body_rect();
        let leaves = self.tree.root.leaf_ids().len();
        let (snapshot_state, revision, lines, selections, primary_line, primary_byte, non_caret) =
            snapshot_fields(snapshot);
        let (missing_tabs, first_missing_tab, first_missing_buffer) = match snapshot {
            SnapshotTrace::Missing => self.missing_buffer_tabs_summary(),
            SnapshotTrace::Live(_) => (0, "none".to_string(), "none".to_string()),
        };
        let last_frame_rows = self
            .last_painted_frame_display
            .as_ref()
            .map(|(_, frame)| frame.display_line_count())
            .unwrap_or(0);
        let spectator_cache = self.spectator_frame_cache.borrow();
        format!(
            concat!(
                "reason={} hwnd={} snapshot={} rev={} lines={} ",
                "selections={} primary_line={} primary_byte={} non_caret={} ",
                "window_buffer={} focused_pane={} active_tab={} active_kind={} ",
                "tab_buffer={} tab_matches_window_buffer={} leaves={} groups={} ",
                "tabs={} saved_panes={} recently_closed={} maximized={} ",
                "client={}x{} dpi={} dpi_scale={:.3} body={:.1}x{:.1} viewport={:.1}x{:.1} ",
                "scroll_y={:.1} wrap={} zoom={:.3} font_state={} ",
                "overlay={} focused={} minimized={} live_resize={} ",
                "renderer={} text_format={} last_frame_rows={} ",
                "spectator_hits={} spectator_misses={} missing_buffer_tabs={} ",
                "first_missing_tab={} first_missing_buffer={}"
            ),
            reason,
            self.hwnd.0 as usize,
            snapshot_state,
            revision,
            lines,
            selections,
            primary_line,
            primary_byte,
            non_caret,
            format_buffer_id(self.buffer_id),
            self.tree.focused.0,
            active.tab_id,
            active.kind,
            active.buffer_id,
            active.buffer == Some(self.buffer_id),
            leaves,
            self.tree.groups.len(),
            self.tree.tabs.len(),
            self.panes.len(),
            self.tree.recently_closed.len(),
            self.tree
                .maximized
                .map(|pane| pane.0.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.client_width,
            self.client_height,
            self.window_dpi,
            self.dpi_scale(),
            body.w,
            body.h,
            self.view.viewport_width_dip,
            self.view.viewport_height_dip,
            self.view.scroll_y_dip,
            self.view.soft_wrap,
            self.view.font_size_scale,
            self.font_state.0,
            self.overlays.is_active(),
            self.is_window_focused,
            self.is_window_minimized,
            self.is_live_resizing,
            self.renderer.is_some(),
            self.text_format.is_some(),
            last_frame_rows,
            spectator_cache.hits(),
            spectator_cache.misses(),
            missing_tabs,
            first_missing_tab,
            first_missing_buffer,
        )
    }

    fn missing_buffer_tabs_summary(&self) -> (usize, String, String) {
        let mut count = 0usize;
        let mut first_tab = String::from("none");
        let mut first_buffer = String::from("none");
        for (tab_id, tab) in &self.tree.tabs {
            if !tab.is_buffer() || self.editor.snapshot(tab.buffer_id).is_some() {
                continue;
            }
            if count == 0 {
                first_tab = tab_id.0.to_string();
                first_buffer = format_buffer_id(tab.buffer_id);
            }
            count += 1;
        }
        (count, first_tab, first_buffer)
    }
}

struct ActiveTabTrace {
    tab_id: String,
    kind: &'static str,
    buffer: Option<BufferId>,
    buffer_id: String,
}

impl ActiveTabTrace {
    fn from_window(window: &Window) -> Self {
        match window.tree.active_tab() {
            Some(tab) => Self::from_tab(tab),
            None => Self {
                tab_id: "missing".to_string(),
                kind: "missing",
                buffer: None,
                buffer_id: "none".to_string(),
            },
        }
    }

    fn from_tab(tab: &Tab) -> Self {
        let buffer = tab.buffer_id_opt();
        Self {
            tab_id: tab.id.0.to_string(),
            kind: tab_kind_label(tab.kind),
            buffer,
            buffer_id: buffer
                .map(format_buffer_id)
                .unwrap_or_else(|| "none".to_string()),
        }
    }
}

#[derive(Clone, Copy)]
enum SnapshotTrace<'a> {
    Live(&'a EditorSnapshot),
    Missing,
}

fn snapshot_fields(
    snapshot: SnapshotTrace<'_>,
) -> (&'static str, i64, i64, usize, i64, i64, usize) {
    match snapshot {
        SnapshotTrace::Missing => ("missing", -1, -1, 0, -1, -1, 0),
        SnapshotTrace::Live(snapshot) => {
            let primary = snapshot.selections().first();
            let non_caret = snapshot
                .selections()
                .iter()
                .filter(|selection| !selection.is_caret())
                .count();
            (
                "live",
                snapshot.rope_snapshot().revision().get() as i64,
                snapshot.rope_snapshot().rope().len_lines() as i64,
                snapshot.selections().len(),
                primary
                    .map(|selection| selection.head.line as i64)
                    .unwrap_or(-1),
                primary
                    .map(|selection| selection.head.byte_in_line as i64)
                    .unwrap_or(-1),
                non_caret,
            )
        }
    }
}

fn tab_kind_label(kind: TabKind) -> &'static str {
    match kind {
        TabKind::Buffer => "buffer",
        TabKind::BufferHistory => "buffer_history",
    }
}

fn format_buffer_id(buffer_id: BufferId) -> String {
    buffer_id.as_uuid().to_string()
}
