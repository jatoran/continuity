//! Markdown command registration.
//!
//! Each handler builds a [`SelectionEdit`] and dispatches it through
//! [`Context::apply_selection_edit`]; the actual planning/mutation lives
//! on the core thread.

use std::sync::Arc;

use continuity_core::{EmphasisKind, SelectionEdit};

use crate::{CommandId, ContextPredicate, Registry};

macro_rules! markdown_id {
    ($name:ident, $id:literal) => {
        #[doc = concat!("Markdown command id `", $id, "`.")]
        pub const $name: CommandId = CommandId($id);
    };
}

markdown_id!(MARKDOWN_TOGGLE_BOLD, "markdown.toggle_bold");
markdown_id!(MARKDOWN_TOGGLE_ITALIC, "markdown.toggle_italic");
markdown_id!(
    MARKDOWN_TOGGLE_STRIKETHROUGH,
    "markdown.toggle_strikethrough"
);
markdown_id!(MARKDOWN_TOGGLE_INLINE_CODE, "markdown.toggle_inline_code");
markdown_id!(MARKDOWN_SET_HEADING_1, "markdown.set_heading_1");
markdown_id!(MARKDOWN_SET_HEADING_2, "markdown.set_heading_2");
markdown_id!(MARKDOWN_SET_HEADING_3, "markdown.set_heading_3");
markdown_id!(MARKDOWN_SET_HEADING_4, "markdown.set_heading_4");
markdown_id!(MARKDOWN_SET_HEADING_5, "markdown.set_heading_5");
markdown_id!(MARKDOWN_SET_HEADING_6, "markdown.set_heading_6");
markdown_id!(MARKDOWN_REMOVE_HEADING, "markdown.remove_heading");
markdown_id!(MARKDOWN_CYCLE_HEADING_UP, "markdown.cycle_heading_up");
markdown_id!(MARKDOWN_CYCLE_HEADING_DOWN, "markdown.cycle_heading_down");
markdown_id!(MARKDOWN_PROMOTE_SECTION, "markdown.promote_section");
markdown_id!(MARKDOWN_DEMOTE_SECTION, "markdown.demote_section");
markdown_id!(MARKDOWN_MOVE_SECTION_UP, "markdown.move_section_up");
markdown_id!(MARKDOWN_MOVE_SECTION_DOWN, "markdown.move_section_down");
markdown_id!(MARKDOWN_TOGGLE_BULLET, "markdown.toggle_bullet");
markdown_id!(MARKDOWN_TOGGLE_NUMBERED, "markdown.toggle_numbered");
markdown_id!(MARKDOWN_TOGGLE_CHECKBOX, "markdown.toggle_checkbox");
markdown_id!(MARKDOWN_TOGGLE_TASK, "markdown.toggle_task");
markdown_id!(MARKDOWN_CYCLE_LIST_MARKER, "markdown.cycle_list_marker");
markdown_id!(MARKDOWN_RENUMBER_LIST, "markdown.renumber_list");
markdown_id!(MARKDOWN_WRAP_IN_BLOCKQUOTE, "markdown.wrap_in_blockquote");
markdown_id!(MARKDOWN_STRIP_FORMATTING, "markdown.strip_formatting");
markdown_id!(MARKDOWN_INSERT_CODE_FENCE, "markdown.insert_code_fence");
markdown_id!(MARKDOWN_INSERT_LINK, "markdown.insert_link");
markdown_id!(MARKDOWN_INSERT_IMAGE_REF, "markdown.insert_image_ref");
markdown_id!(MARKDOWN_INSERT_TOC, "markdown.insert_toc");
markdown_id!(MARKDOWN_REFRESH_TOC, "markdown.refresh_toc");
markdown_id!(MARKDOWN_HIGHLIGHT_SELECTION, "markdown.highlight_selection");
markdown_id!(MARKDOWN_COLOR_SELECTION, "markdown.color_selection");
markdown_id!(MARKDOWN_CLEAR_INLINE_COLOR, "markdown.clear_inline_color");
markdown_id!(MARKDOWN_INSERT_TABLE, "markdown.insert_table");
markdown_id!(
    MARKDOWN_TABLE_INSERT_ROW_ABOVE,
    "markdown.table.insert_row_above"
);
markdown_id!(
    MARKDOWN_TABLE_INSERT_ROW_BELOW,
    "markdown.table.insert_row_below"
);
markdown_id!(
    MARKDOWN_TABLE_INSERT_COL_LEFT,
    "markdown.table.insert_col_left"
);
markdown_id!(
    MARKDOWN_TABLE_INSERT_COL_RIGHT,
    "markdown.table.insert_col_right"
);
markdown_id!(MARKDOWN_TABLE_DELETE_ROW, "markdown.table.delete_row");
markdown_id!(MARKDOWN_TABLE_DELETE_COL, "markdown.table.delete_col");
markdown_id!(MARKDOWN_TABLE_DELETE_TABLE, "markdown.table.delete_table");
markdown_id!(MARKDOWN_TABLE_SELECT_CELL, "markdown.table.select_cell");
markdown_id!(
    MARKDOWN_TABLE_CARET_CELL_START,
    "markdown.table.caret_cell_start"
);
markdown_id!(
    MARKDOWN_TABLE_CARET_CELL_END,
    "markdown.table.caret_cell_end"
);
markdown_id!(
    MARKDOWN_TABLE_EXTEND_CELL_START,
    "markdown.table.extend_cell_start"
);
markdown_id!(
    MARKDOWN_TABLE_EXTEND_CELL_END,
    "markdown.table.extend_cell_end"
);
markdown_id!(MARKDOWN_TABLE_TAB_NEXT, "markdown.table.tab_next");
markdown_id!(MARKDOWN_TABLE_TAB_PREV, "markdown.table.tab_prev");
markdown_id!(MARKDOWN_TABLE_ENTER, "markdown.table.enter");
markdown_id!(MARKDOWN_TABLE_INSERT_BREAK, "markdown.table.insert_break");
markdown_id!(MARKDOWN_TABLE_MOVE_UP, "markdown.table.move_up");
markdown_id!(MARKDOWN_TABLE_MOVE_DOWN, "markdown.table.move_down");
markdown_id!(MARKDOWN_TABLE_CELL_UP, "markdown.table.cell_up");

fn handler<F>(f: F) -> crate::registry::Handler
where
    F: Fn() -> SelectionEdit + Send + Sync + 'static,
{
    Arc::new(move |_, ctx| ctx.apply_selection_edit(f()))
}

/// Register every Phase 6 markdown command.
pub fn register_markdown_commands(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");
    let bind = |registry: &mut Registry, id: CommandId, h: crate::registry::Handler| {
        registry.register(id, focused.clone(), h);
    };

    bind(
        registry,
        MARKDOWN_TOGGLE_BOLD,
        handler(|| SelectionEdit::MarkdownToggleEmphasis(EmphasisKind::Bold)),
    );
    bind(
        registry,
        MARKDOWN_TOGGLE_ITALIC,
        handler(|| SelectionEdit::MarkdownToggleEmphasis(EmphasisKind::Italic)),
    );
    bind(
        registry,
        MARKDOWN_TOGGLE_STRIKETHROUGH,
        handler(|| SelectionEdit::MarkdownToggleEmphasis(EmphasisKind::Strikethrough)),
    );
    bind(
        registry,
        MARKDOWN_TOGGLE_INLINE_CODE,
        handler(|| SelectionEdit::MarkdownToggleEmphasis(EmphasisKind::InlineCode)),
    );
    for (id, level) in [
        (MARKDOWN_SET_HEADING_1, 1_u8),
        (MARKDOWN_SET_HEADING_2, 2),
        (MARKDOWN_SET_HEADING_3, 3),
        (MARKDOWN_SET_HEADING_4, 4),
        (MARKDOWN_SET_HEADING_5, 5),
        (MARKDOWN_SET_HEADING_6, 6),
    ] {
        bind(
            registry,
            id,
            handler(move || SelectionEdit::MarkdownSetHeading(level)),
        );
    }
    bind(
        registry,
        MARKDOWN_REMOVE_HEADING,
        handler(|| SelectionEdit::MarkdownSetHeading(0)),
    );
    bind(
        registry,
        MARKDOWN_CYCLE_HEADING_UP,
        handler(|| SelectionEdit::MarkdownCycleHeading(1)),
    );
    bind(
        registry,
        MARKDOWN_CYCLE_HEADING_DOWN,
        handler(|| SelectionEdit::MarkdownCycleHeading(-1)),
    );
    bind(
        registry,
        MARKDOWN_PROMOTE_SECTION,
        handler(|| SelectionEdit::MarkdownPromoteSection),
    );
    bind(
        registry,
        MARKDOWN_DEMOTE_SECTION,
        handler(|| SelectionEdit::MarkdownDemoteSection),
    );
    bind(
        registry,
        MARKDOWN_MOVE_SECTION_UP,
        handler(|| SelectionEdit::MarkdownMoveSectionUp),
    );
    bind(
        registry,
        MARKDOWN_MOVE_SECTION_DOWN,
        handler(|| SelectionEdit::MarkdownMoveSectionDown),
    );
    bind(
        registry,
        MARKDOWN_TOGGLE_BULLET,
        handler(|| SelectionEdit::MarkdownToggleBullet),
    );
    bind(
        registry,
        MARKDOWN_TOGGLE_NUMBERED,
        handler(|| SelectionEdit::MarkdownToggleNumbered),
    );
    bind(
        registry,
        MARKDOWN_TOGGLE_CHECKBOX,
        handler(|| SelectionEdit::MarkdownToggleCheckbox),
    );
    bind(
        registry,
        MARKDOWN_TOGGLE_TASK,
        handler(|| SelectionEdit::MarkdownToggleTask),
    );
    bind(
        registry,
        MARKDOWN_STRIP_FORMATTING,
        handler(|| SelectionEdit::MarkdownStripFormatting),
    );
    bind(
        registry,
        MARKDOWN_CYCLE_LIST_MARKER,
        handler(|| SelectionEdit::MarkdownCycleListMarker),
    );
    bind(
        registry,
        MARKDOWN_RENUMBER_LIST,
        handler(|| SelectionEdit::MarkdownRenumberList),
    );
    bind(
        registry,
        MARKDOWN_WRAP_IN_BLOCKQUOTE,
        handler(|| SelectionEdit::MarkdownWrapInBlockquote),
    );
    bind(
        registry,
        MARKDOWN_INSERT_CODE_FENCE,
        handler(|| SelectionEdit::MarkdownInsertCodeFence),
    );
    bind(
        registry,
        MARKDOWN_INSERT_LINK,
        handler(|| SelectionEdit::MarkdownInsertLink),
    );
    bind(
        registry,
        MARKDOWN_INSERT_IMAGE_REF,
        handler(|| SelectionEdit::MarkdownInsertImageRef),
    );
    // Phase F2 — TOC insertion / refresh. These bypass the
    // `apply_selection_edit` shape because the planner needs access to
    // both the rope and the decoration cache; the `Context` impl on
    // `ui::Window` does the work directly.
    registry.register(
        MARKDOWN_INSERT_TOC,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.markdown_insert_toc()),
    );
    registry.register(
        MARKDOWN_REFRESH_TOC,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.markdown_refresh_toc()),
    );
    // Phase F3 — inline color / highlight markup.
    registry.register(
        MARKDOWN_HIGHLIGHT_SELECTION,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.markdown_highlight_selection()),
    );
    registry.register(
        MARKDOWN_COLOR_SELECTION,
        focused.clone(),
        Arc::new(|args, ctx| {
            let prefill = args.get("hex").and_then(|v| v.as_str());
            ctx.markdown_color_selection(prefill)
        }),
    );
    registry.register(
        MARKDOWN_CLEAR_INLINE_COLOR,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.markdown_clear_inline_color()),
    );
    // Phase F4 — markdown.insert_table opens a rows × cols prompt. The
    // palette mode posts back `{ "rows": N, "cols": M }` on commit;
    // direct invocation can also pre-supply both args.
    registry.register(
        MARKDOWN_INSERT_TABLE,
        focused.clone(),
        Arc::new(|args, ctx| {
            let rows = args.get("rows").and_then(|v| v.as_u64()).unwrap_or(3) as u32;
            let cols = args.get("cols").and_then(|v| v.as_u64()).unwrap_or(3) as u32;
            ctx.markdown_insert_table(rows.max(1), cols.max(1))
        }),
    );
    registry.register(
        MARKDOWN_TABLE_INSERT_ROW_ABOVE,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_insert_row(true)),
    );
    registry.register(
        MARKDOWN_TABLE_INSERT_ROW_BELOW,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_insert_row(false)),
    );
    registry.register(
        MARKDOWN_TABLE_INSERT_COL_LEFT,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_insert_column(true)),
    );
    registry.register(
        MARKDOWN_TABLE_INSERT_COL_RIGHT,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_insert_column(false)),
    );
    registry.register(
        MARKDOWN_TABLE_DELETE_ROW,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_delete_row()),
    );
    registry.register(
        MARKDOWN_TABLE_DELETE_COL,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_delete_column()),
    );
    registry.register(
        MARKDOWN_TABLE_DELETE_TABLE,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_delete_table()),
    );
    registry.register(
        MARKDOWN_TABLE_SELECT_CELL,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_select_cell()),
    );
    registry.register(
        MARKDOWN_TABLE_CARET_CELL_START,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_caret_cell_edge(true, false)),
    );
    registry.register(
        MARKDOWN_TABLE_CARET_CELL_END,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_caret_cell_edge(false, false)),
    );
    registry.register(
        MARKDOWN_TABLE_EXTEND_CELL_START,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_caret_cell_edge(true, true)),
    );
    registry.register(
        MARKDOWN_TABLE_EXTEND_CELL_END,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_caret_cell_edge(false, true)),
    );
    registry.register(
        MARKDOWN_TABLE_TAB_NEXT,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_tab_next()),
    );
    registry.register(
        MARKDOWN_TABLE_TAB_PREV,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_tab_prev()),
    );
    registry.register(
        MARKDOWN_TABLE_ENTER,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_enter()),
    );
    registry.register(
        MARKDOWN_TABLE_INSERT_BREAK,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_insert_break()),
    );
    registry.register(
        MARKDOWN_TABLE_MOVE_UP,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_move_vertical(false)),
    );
    registry.register(
        MARKDOWN_TABLE_MOVE_DOWN,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_move_vertical(true)),
    );
    registry.register(
        MARKDOWN_TABLE_CELL_UP,
        focused.clone(),
        Arc::new(|_, ctx| ctx.markdown_table_cell_up()),
    );
    // §H5 — flag the existing insertion commands as palette_safe so
    // the slash-command palette picks them up. These five all produce
    // a single rope `Insert` op (one inserts a code fence, one a
    // link, one an image ref, one a TOC, one a table); none mutate
    // settings, buffers, or selections outside the active document.
    for id in [
        MARKDOWN_INSERT_CODE_FENCE,
        MARKDOWN_INSERT_LINK,
        MARKDOWN_INSERT_IMAGE_REF,
        MARKDOWN_INSERT_TOC,
        MARKDOWN_INSERT_TABLE,
    ] {
        registry.mark_palette_safe(id);
    }
    // §H5 — six additional inserters (footnote, callout, timestamp,
    // date, UUID, horizontal rule). Each is a single `Context::insert_text`
    // call registered with `register_palette_safe`.
    crate::markdown_inserters::register_markdown_inserters(registry, &focused);
    // Folding (`markdown.fold_section` / `markdown.unfold_section` /
    // `markdown.fold_all`) is intentionally not registered. The display
    // map already accepts fold ranges as input but `FrameDisplay::build`
    // doesn't yet plumb them, section-detection from tree-sitter-md
    // headings isn't wired, and there is no fold-gutter UI. Re-add the
    // commands when those three pieces land together.
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::{Context, Error};

    #[derive(Default)]
    struct Captor {
        last: Option<SelectionEdit>,
    }

    impl crate::FindContext for Captor {}
    impl crate::EditConfigContext for Captor {}
    impl Context for Captor {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }
        fn apply_selection_edit(&mut self, edit: SelectionEdit) -> Result<(), Error> {
            self.last = Some(edit);
            Ok(())
        }
    }
    impl crate::ViewContext for Captor {
        fn markdown_insert_toc(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_refresh_toc(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_highlight_selection(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_color_selection(&mut self, _prefill: Option<&str>) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_clear_inline_color(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_insert_table(&mut self, _rows: u32, _cols: u32) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_table_insert_row(&mut self, _above: bool) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_table_insert_column(&mut self, _before: bool) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_table_delete_row(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_table_delete_column(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_table_delete_table(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_table_select_cell(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn markdown_table_caret_cell_edge(
            &mut self,
            _to_start: bool,
            _extend: bool,
        ) -> Result<(), Error> {
            Ok(())
        }
    }

    fn dispatch(registry: &Registry, id: CommandId, ctx: &mut Captor) {
        registry
            .dispatch(id, &Value::Null, ctx)
            .expect("dispatch ok");
    }

    #[test]
    fn each_markdown_command_emits_expected_edit() {
        let mut registry = Registry::new();
        register_markdown_commands(&mut registry);
        let mut ctx = Captor::default();

        dispatch(&registry, MARKDOWN_TOGGLE_BOLD, &mut ctx);
        assert!(matches!(
            ctx.last,
            Some(SelectionEdit::MarkdownToggleEmphasis(EmphasisKind::Bold))
        ));
        dispatch(&registry, MARKDOWN_SET_HEADING_3, &mut ctx);
        assert!(matches!(
            ctx.last,
            Some(SelectionEdit::MarkdownSetHeading(3))
        ));
        dispatch(&registry, MARKDOWN_REMOVE_HEADING, &mut ctx);
        assert!(matches!(
            ctx.last,
            Some(SelectionEdit::MarkdownSetHeading(0))
        ));
        dispatch(&registry, MARKDOWN_TOGGLE_NUMBERED, &mut ctx);
        assert!(matches!(
            ctx.last,
            Some(SelectionEdit::MarkdownToggleNumbered)
        ));
        dispatch(&registry, MARKDOWN_INSERT_LINK, &mut ctx);
        assert!(matches!(ctx.last, Some(SelectionEdit::MarkdownInsertLink)));
    }

    #[test]
    fn insert_table_command_dispatches_with_default_args() {
        // §F4 — insert_table reads rows/cols from JSON args; defaults
        // when args are missing.
        let mut registry = Registry::new();
        register_markdown_commands(&mut registry);
        let mut ctx = Captor::default();
        registry
            .dispatch(MARKDOWN_INSERT_TABLE, &Value::Null, &mut ctx)
            .expect("dispatch ok");
        assert!(ctx.last.is_none());
    }

    #[test]
    fn inline_color_commands_dispatch_without_apply_edit() {
        // §F3 — highlight / color / clear all bypass SelectionEdit
        // because they need rope+caret context to wrap or unwrap the
        // surrounding markup span.
        let mut registry = Registry::new();
        register_markdown_commands(&mut registry);
        let mut ctx = Captor::default();
        dispatch(&registry, MARKDOWN_HIGHLIGHT_SELECTION, &mut ctx);
        dispatch(&registry, MARKDOWN_COLOR_SELECTION, &mut ctx);
        dispatch(&registry, MARKDOWN_CLEAR_INLINE_COLOR, &mut ctx);
        assert!(
            ctx.last.is_none(),
            "inline-color commands must bypass SelectionEdit"
        );
    }

    #[test]
    fn existing_inserters_are_palette_safe() {
        // The five pre-existing insertion commands must surface in
        // the slash-command palette. Destructive commands like
        // `markdown.promote_section` must not.
        let mut registry = Registry::new();
        register_markdown_commands(&mut registry);
        for id in [
            MARKDOWN_INSERT_CODE_FENCE,
            MARKDOWN_INSERT_LINK,
            MARKDOWN_INSERT_IMAGE_REF,
            MARKDOWN_INSERT_TOC,
            MARKDOWN_INSERT_TABLE,
        ] {
            assert!(
                registry.is_palette_safe(id.as_str()),
                "expected {} to be palette_safe",
                id.as_str()
            );
        }
        // Spot-check a non-insertion command — heading promotion
        // rewrites the leading `#` markers and must not appear.
        assert!(
            !registry.is_palette_safe(MARKDOWN_PROMOTE_SECTION.as_str()),
            "promote_section must not be palette_safe"
        );
    }

    #[test]
    fn toc_commands_dispatch_without_apply_edit() {
        // §F2 — insert_toc / refresh_toc go through `ViewContext` methods
        // because the planner needs rope + decoration access. Dispatch
        // must succeed but `apply_selection_edit` is never called.
        let mut registry = Registry::new();
        register_markdown_commands(&mut registry);
        let mut ctx = Captor::default();
        dispatch(&registry, MARKDOWN_INSERT_TOC, &mut ctx);
        dispatch(&registry, MARKDOWN_REFRESH_TOC, &mut ctx);
        assert!(ctx.last.is_none(), "TOC commands must bypass SelectionEdit");
    }
}
