//! View command registration.
//!
//! Phase 9 wired zoom, soft-wrap toggle, and scroll commands. Phase 11
//! adds theme + font + view-toggle dispatch through the [`crate::Context`]
//! surface. Each toggle is a `Context` method so the per-window UI thread
//! is the only writer of view-options state.

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

macro_rules! view_id {
    ($name:ident, $id:literal) => {
        #[doc = concat!("View command id `", $id, "`.")]
        pub const $name: CommandId = CommandId($id);
    };
}

view_id!(VIEW_ZOOM_IN, "view.zoom_in");
view_id!(VIEW_ZOOM_OUT, "view.zoom_out");
view_id!(VIEW_ZOOM_RESET, "view.zoom_reset");
view_id!(VIEW_TOGGLE_WRAP, "view.toggle_wrap");
view_id!(VIEW_TOGGLE_FILE_TREE, "view.toggle_file_tree");
view_id!(VIEW_TOGGLE_LINE_NUMBERS, "view.toggle_line_numbers");
view_id!(
    VIEW_TOGGLE_RELATIVE_LINE_NUMBERS,
    "view.toggle_relative_line_numbers"
);
view_id!(VIEW_TOGGLE_ALL_LINE_NUMBERS, "view.toggle_all_line_numbers");
view_id!(VIEW_TOGGLE_MINIMAP, "view.toggle_minimap");
view_id!(VIEW_TOGGLE_WHITESPACE, "view.toggle_whitespace");
view_id!(VIEW_TOGGLE_INDENT_GUIDES, "view.toggle_indent_guides");
view_id!(
    VIEW_TOGGLE_CURRENT_LINE_HIGHLIGHT,
    "view.toggle_current_line_highlight"
);
view_id!(
    VIEW_TOGGLE_TRAILING_WHITESPACE,
    "view.toggle_trailing_whitespace"
);
view_id!(VIEW_TOGGLE_LIGATURES, "view.toggle_ligatures");
view_id!(VIEW_CYCLE_CARET_STYLE, "view.cycle_caret_style");
view_id!(VIEW_SET_FONT_FAMILY, "view.set_font_family");
view_id!(VIEW_PICK_FONT, "view.pick_font");
view_id!(VIEW_SET_FONT_SIZE, "view.set_font_size");
view_id!(VIEW_SET_RULER_COLUMNS, "view.set_ruler_columns");
view_id!(VIEW_CYCLE_THEME, "view.cycle_theme");
view_id!(VIEW_PICK_THEME, "view.pick_theme");
view_id!(THEME_RELOAD, "theme.reload");

view_id!(
    VIEW_TOGGLE_STICKY_BREADCRUMB,
    "view.toggle_sticky_breadcrumb"
);
view_id!(VIEW_TOGGLE_OUTLINE, "view.toggle_outline");

view_id!(VIEW_SCROLL_PAGE_UP, "view.scroll_page_up");
view_id!(VIEW_SCROLL_PAGE_DOWN, "view.scroll_page_down");
view_id!(VIEW_SCROLL_LINE_UP, "view.scroll_line_up");
view_id!(VIEW_SCROLL_LINE_DOWN, "view.scroll_line_down");
view_id!(VIEW_SCROLL_DOC_START, "view.scroll_doc_start");
view_id!(VIEW_SCROLL_DOC_END, "view.scroll_doc_end");

/// 10 % zoom factor matches the spec §5 default.
const ZOOM_STEP_FACTOR: f32 = 1.10;

/// JSON arg helper: `{ "size": 16.0 }` for `view.set_font_size`.
fn parse_font_size(args: &serde_json::Value) -> Option<f32> {
    args.get("size")
        .and_then(serde_json::Value::as_f64)
        .map(|v| v as f32)
}

/// JSON arg helper: `{ "columns": [80, 100] }` for `view.set_ruler_columns`.
fn parse_ruler_columns(args: &serde_json::Value) -> Vec<u32> {
    args.get("columns")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_u64)
                .map(|n| n.min(u64::from(u32::MAX)) as u32)
                .collect()
        })
        .unwrap_or_default()
}

/// Register every view command. Phase 11 wires the full set: themes,
/// fonts, and view toggles. Each handler delegates to a [`crate::Context`]
/// method whose production implementor is `ui::Window`.
pub fn register_view_commands(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");

    registry.register(
        VIEW_ZOOM_IN,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.adjust_zoom(ZOOM_STEP_FACTOR)),
    );
    registry.register(
        VIEW_ZOOM_OUT,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.adjust_zoom(1.0 / ZOOM_STEP_FACTOR)),
    );
    registry.register(
        VIEW_ZOOM_RESET,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.reset_zoom()),
    );
    registry.register(
        VIEW_TOGGLE_WRAP,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_soft_wrap()),
    );
    registry.register(
        VIEW_TOGGLE_FILE_TREE,
        focused.clone(),
        Arc::new(|_args, ctx| {
            let Some(file_context) = ctx.file_context() else {
                return Err(crate::Error::UnsupportedContext("toggle_file_tree"));
            };
            file_context.toggle_file_tree()
        }),
    );
    registry.register(
        VIEW_SCROLL_PAGE_UP,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.scroll_page(-1.0)),
    );
    registry.register(
        VIEW_SCROLL_PAGE_DOWN,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.scroll_page(1.0)),
    );
    registry.register(
        VIEW_SCROLL_LINE_UP,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.scroll_lines(-1.0)),
    );
    registry.register(
        VIEW_SCROLL_LINE_DOWN,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.scroll_lines(1.0)),
    );
    registry.register(
        VIEW_SCROLL_DOC_START,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.scroll_doc_start()),
    );
    registry.register(
        VIEW_SCROLL_DOC_END,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.scroll_doc_end()),
    );

    // Phase 11: themes
    registry.register(
        VIEW_CYCLE_THEME,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.cycle_theme()),
    );
    // §E4: palette-mode theme picker.
    registry.register(
        VIEW_PICK_THEME,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.pick_theme()),
    );
    registry.register(
        THEME_RELOAD,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.reload_theme()),
    );

    // Phase 11: fonts
    registry.register(
        VIEW_SET_FONT_FAMILY,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.pick_font_family()),
    );
    // §E3: `view.pick_font` is the spec-targeted name for the palette-
    // mode font picker. It dispatches through the same Context method as
    // the legacy `view.set_font_family` chord so existing bindings keep
    // working.
    registry.register(
        VIEW_PICK_FONT,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.pick_font_family()),
    );
    registry.register(
        VIEW_SET_FONT_SIZE,
        focused.clone(),
        Arc::new(|args, ctx| {
            let size = parse_font_size(args).unwrap_or(14.0);
            ctx.set_font_size(size)
        }),
    );
    registry.register(
        VIEW_TOGGLE_LIGATURES,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_ligatures()),
    );

    // Phase 11: view toggles
    registry.register(
        VIEW_TOGGLE_LINE_NUMBERS,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_line_numbers()),
    );
    registry.register(
        VIEW_TOGGLE_RELATIVE_LINE_NUMBERS,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_relative_line_numbers()),
    );
    registry.set_description(
        VIEW_TOGGLE_RELATIVE_LINE_NUMBERS,
        "Toggle relative gutter line numbers",
    );
    registry.register(
        VIEW_TOGGLE_ALL_LINE_NUMBERS,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_all_line_numbers()),
    );
    registry.set_description(
        VIEW_TOGGLE_ALL_LINE_NUMBERS,
        "Toggle all visible gutter line numbers",
    );
    registry.register(
        VIEW_TOGGLE_CURRENT_LINE_HIGHLIGHT,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_current_line_highlight()),
    );
    registry.register(
        VIEW_TOGGLE_INDENT_GUIDES,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_indent_guides()),
    );
    registry.register(
        VIEW_TOGGLE_WHITESPACE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_whitespace_markers()),
    );
    registry.register(
        VIEW_TOGGLE_TRAILING_WHITESPACE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_trailing_whitespace()),
    );
    registry.register(
        VIEW_TOGGLE_MINIMAP,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_minimap()),
    );
    // Phase F1: sticky heading breadcrumb visibility toggle.
    registry.register(
        VIEW_TOGGLE_STICKY_BREADCRUMB,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_sticky_breadcrumb()),
    );
    // Phase F2: outline-sidebar visibility toggle.
    registry.register(
        VIEW_TOGGLE_OUTLINE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_outline()),
    );

    // Spec §H — focus modes, indent folding, slash commands, outline
    // manipulation, Ctrl+Tab overlay. Split into a sibling module to
    // keep this file under the 600-line cap.
    crate::view_modes::register_view_modes_commands(registry, &focused);
    // Spec §I — time-machine slider + named snapshots (I1),
    // metrics buffer (I2). Split into a sibling module for the same
    // reason.
    crate::view_timeline_metrics::register_view_timeline_metrics_commands(registry, &focused);
    registry.register(
        VIEW_SET_RULER_COLUMNS,
        focused.clone(),
        Arc::new(|args, ctx| ctx.set_ruler_columns(parse_ruler_columns(args))),
    );

    // Phase 11: caret style
    registry.register(
        VIEW_CYCLE_CARET_STYLE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.cycle_caret_style()),
    );
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::Value;

    use super::*;
    use crate::{Context, Error};

    struct CountingCtx {
        zoom_calls: Cell<u32>,
        wrap_calls: Cell<u32>,
        scroll_lines_total: Cell<f32>,
        scroll_pages_total: Cell<f32>,
        view_calls: Cell<u32>,
    }
    impl CountingCtx {
        fn new() -> Self {
            Self {
                zoom_calls: Cell::new(0),
                wrap_calls: Cell::new(0),
                scroll_lines_total: Cell::new(0.0),
                scroll_pages_total: Cell::new(0.0),
                view_calls: Cell::new(0),
            }
        }
    }
    impl crate::FindContext for CountingCtx {}
    impl Context for CountingCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }
        fn adjust_zoom(&mut self, _factor: f32) -> Result<(), Error> {
            self.zoom_calls.set(self.zoom_calls.get() + 1);
            Ok(())
        }
        fn reset_zoom(&mut self) -> Result<(), Error> {
            self.zoom_calls.set(self.zoom_calls.get() + 1);
            Ok(())
        }
        fn toggle_soft_wrap(&mut self) -> Result<(), Error> {
            self.wrap_calls.set(self.wrap_calls.get() + 1);
            Ok(())
        }
        fn scroll_lines(&mut self, lines: f32) -> Result<(), Error> {
            self.scroll_lines_total
                .set(self.scroll_lines_total.get() + lines);
            Ok(())
        }
        fn scroll_page(&mut self, dir: f32) -> Result<(), Error> {
            self.scroll_pages_total
                .set(self.scroll_pages_total.get() + dir);
            Ok(())
        }
        fn scroll_doc_start(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn scroll_doc_end(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }
    impl crate::ViewContext for CountingCtx {
        fn cycle_theme(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn reload_theme(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn pick_font_family(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn set_font_size(&mut self, _size_dip: f32) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn toggle_line_numbers(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn toggle_current_line_highlight(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn toggle_indent_guides(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn toggle_whitespace_markers(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn toggle_trailing_whitespace(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn toggle_minimap(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn toggle_sticky_breadcrumb(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn toggle_outline(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn set_ruler_columns(&mut self, _columns: Vec<u32>) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn cycle_caret_style(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
        fn toggle_ligatures(&mut self) -> Result<(), Error> {
            self.view_calls.set(self.view_calls.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn zoom_in_out_reset_dispatch_to_context() {
        let mut registry = Registry::new();
        register_view_commands(&mut registry);
        let mut ctx = CountingCtx::new();
        registry
            .dispatch(VIEW_ZOOM_IN, &Value::Null, &mut ctx)
            .unwrap();
        registry
            .dispatch(VIEW_ZOOM_OUT, &Value::Null, &mut ctx)
            .unwrap();
        registry
            .dispatch(VIEW_ZOOM_RESET, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.zoom_calls.get(), 3);
    }

    #[test]
    fn toggle_wrap_dispatches() {
        let mut registry = Registry::new();
        register_view_commands(&mut registry);
        let mut ctx = CountingCtx::new();
        registry
            .dispatch(VIEW_TOGGLE_WRAP, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.wrap_calls.get(), 1);
    }

    #[test]
    fn scroll_commands_dispatch() {
        let mut registry = Registry::new();
        register_view_commands(&mut registry);
        let mut ctx = CountingCtx::new();
        registry
            .dispatch(VIEW_SCROLL_LINE_UP, &Value::Null, &mut ctx)
            .unwrap();
        registry
            .dispatch(VIEW_SCROLL_LINE_DOWN, &Value::Null, &mut ctx)
            .unwrap();
        registry
            .dispatch(VIEW_SCROLL_PAGE_UP, &Value::Null, &mut ctx)
            .unwrap();
        registry
            .dispatch(VIEW_SCROLL_PAGE_DOWN, &Value::Null, &mut ctx)
            .unwrap();
        registry
            .dispatch(VIEW_SCROLL_DOC_START, &Value::Null, &mut ctx)
            .unwrap();
        registry
            .dispatch(VIEW_SCROLL_DOC_END, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.scroll_lines_total.get(), 0.0);
        assert_eq!(ctx.scroll_pages_total.get(), 0.0);
    }

    #[test]
    fn toggle_outline_command_is_registered() {
        let mut registry = Registry::new();
        register_view_commands(&mut registry);
        let mut ctx = CountingCtx::new();
        registry
            .dispatch(VIEW_TOGGLE_OUTLINE, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.view_calls.get(), 1);
    }

    #[test]
    fn toggle_sticky_breadcrumb_command_is_registered() {
        let mut registry = Registry::new();
        register_view_commands(&mut registry);
        let mut ctx = CountingCtx::new();
        registry
            .dispatch(VIEW_TOGGLE_STICKY_BREADCRUMB, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.view_calls.get(), 1);
    }

    #[test]
    fn view_toggles_dispatch_through_context() {
        let mut registry = Registry::new();
        register_view_commands(&mut registry);
        let mut ctx = CountingCtx::new();
        let ids = [
            VIEW_TOGGLE_LINE_NUMBERS,
            VIEW_TOGGLE_MINIMAP,
            VIEW_TOGGLE_WHITESPACE,
            VIEW_TOGGLE_INDENT_GUIDES,
            VIEW_TOGGLE_CURRENT_LINE_HIGHLIGHT,
            VIEW_TOGGLE_TRAILING_WHITESPACE,
            VIEW_TOGGLE_LIGATURES,
            VIEW_SET_FONT_FAMILY,
            VIEW_PICK_FONT,
            VIEW_CYCLE_THEME,
            VIEW_CYCLE_CARET_STYLE,
            THEME_RELOAD,
        ];
        for id in ids {
            registry.dispatch(id, &Value::Null, &mut ctx).unwrap();
        }
        // Each command increments view_calls exactly once.
        assert_eq!(ctx.view_calls.get(), ids.len() as u32);
    }

    #[test]
    fn set_font_size_parses_args_and_dispatches() {
        let mut registry = Registry::new();
        register_view_commands(&mut registry);
        let mut ctx = CountingCtx::new();
        let args = serde_json::json!({ "size": 16.0 });
        registry
            .dispatch(VIEW_SET_FONT_SIZE, &args, &mut ctx)
            .unwrap();
        assert_eq!(ctx.view_calls.get(), 1);
    }

    #[test]
    fn set_ruler_columns_parses_args_and_dispatches() {
        let mut registry = Registry::new();
        register_view_commands(&mut registry);
        let mut ctx = CountingCtx::new();
        let args = serde_json::json!({ "columns": [80, 100, 120] });
        registry
            .dispatch(VIEW_SET_RULER_COLUMNS, &args, &mut ctx)
            .unwrap();
        assert_eq!(ctx.view_calls.get(), 1);
    }
}
