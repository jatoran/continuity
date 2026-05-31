//! View-toggle command implementations on `Window`: line numbers,
//! current-line highlight, indent guides, whitespace markers, trailing
//! whitespace, minimap, sticky breadcrumb, outline sidebar, ruler
//! columns. All operate on `self.view_options` and trigger a repaint.
//!
//! Thread ownership: UI thread.

use crate::window_helpers::{invalidate_hwnd, invalidate_hwnd_with_reason};
use crate::Window;

impl Window {
    pub(crate) fn toggle_line_numbers_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.line_numbers = !self.view_options.line_numbers;
        self.persist_toggle_or_log("ui", "show_line_numbers", self.view_options.line_numbers);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn toggle_relative_line_numbers_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.relative_line_numbers = !self.view_options.relative_line_numbers;
        self.persist_toggle_or_log(
            "ui",
            "relative_line_numbers",
            self.view_options.relative_line_numbers,
        );
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn toggle_all_line_numbers_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.gutter_caret_line_only = !self.view_options.gutter_caret_line_only;
        self.persist_toggle_or_log(
            "ui",
            "show_all_line_numbers",
            !self.view_options.gutter_caret_line_only,
        );
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn toggle_current_line_highlight_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.current_line_highlight = !self.view_options.current_line_highlight;
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn toggle_indent_guides_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.indent_guides = !self.view_options.indent_guides;
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn toggle_whitespace_markers_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.whitespace_markers = !self.view_options.whitespace_markers;
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn toggle_trailing_whitespace_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.trailing_whitespace = !self.view_options.trailing_whitespace;
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn toggle_minimap_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.minimap = !self.view_options.minimap;
        self.remember_current_right_edge_chrome_state();
        self.view_options.minimap_layout = None;
        self.view_options.search_minimap_layout = None;
        self.view_options.outline_layout = None;
        self.outline_entries_cache
            .borrow_mut()
            .clear_for_buffer(self.buffer_id);
        let _ = self.try_dispatch_projection_worker_early("toggle_minimap", "layout_change");
        invalidate_hwnd_with_reason(self.hwnd, "view_toggle_minimap");
        Ok(())
    }

    pub(crate) fn toggle_sticky_breadcrumb_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.show_sticky_breadcrumb = !self.view_options.show_sticky_breadcrumb;
        self.persist_toggle_or_log(
            "ui",
            "show_sticky_breadcrumb",
            self.view_options.show_sticky_breadcrumb,
        );
        // Reset the cached layout — next paint rebuilds it.
        self.view_options.breadcrumb_layout = None;
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn toggle_outline_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.show_outline_sidebar = !self.view_options.show_outline_sidebar;
        self.remember_current_right_edge_chrome_state();
        self.view_options.outline_layout = None;
        self.view_options.minimap_layout = None;
        self.view_options.search_minimap_layout = None;
        self.outline_entries_cache
            .borrow_mut()
            .clear_for_buffer(self.buffer_id);
        let _ = self.try_dispatch_projection_worker_early("toggle_outline", "layout_change");
        invalidate_hwnd_with_reason(self.hwnd, "view_toggle_outline");
        Ok(())
    }

    pub(crate) fn set_ruler_columns_impl(
        &mut self,
        mut columns: Vec<u32>,
    ) -> Result<(), crate::Error> {
        columns.sort_unstable();
        columns.dedup();
        self.view_options.ruler_columns = columns;
        invalidate_hwnd(self.hwnd);
        Ok(())
    }
}
