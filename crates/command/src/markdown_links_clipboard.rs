//! Markdown link-opening + clipboard-format commands.
//!
//! These sit in their own module to avoid pushing `command::markdown` past
//! the 600-line cap. Each command name is exposed as a stable
//! `&'static str` so keymap and palette references survive future moves.

use serde_json::Value;

use crate::id::CommandId;
use crate::registry::Registry;
use crate::Context;

/// Open the link the caret is currently inside (Ctrl+click semantics from
/// the spec, surfaced as a discoverable command + keymap binding).
pub const EDITOR_OPEN_LINK_AT_CARET: CommandId = CommandId("editor.open_link_at_caret");

/// `Ctrl+Shift+C` — copy the rendered (decoration-flattened) plain text of
/// the current selection.
pub const EDITOR_COPY_RENDERED_TEXT: CommandId = CommandId("editor.copy_rendered_text");

/// `Ctrl+C` analogue that copies the source markdown (already the default
/// for plain-text selections; this command makes the explicit "source"
/// path discoverable in the palette).
pub const EDITOR_COPY_SOURCE_TEXT: CommandId = CommandId("editor.copy_source_text");

/// `markdown.copy_as_html` — render the current buffer to HTML via
/// `pulldown-cmark` and place it on the clipboard.
pub const MARKDOWN_COPY_AS_HTML: CommandId = CommandId("markdown.copy_as_html");

/// Register the markdown link + clipboard commands with `registry`. Call
/// this once at app startup, after `register_markdown_commands`.
pub fn register_markdown_links_clipboard(registry: &mut Registry) {
    registry.register(
        EDITOR_OPEN_LINK_AT_CARET,
        crate::ContextPredicate::always(),
        std::sync::Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.open_link_at_caret()),
    );
    registry.register(
        EDITOR_COPY_RENDERED_TEXT,
        crate::ContextPredicate::always(),
        std::sync::Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.copy_rendered_text()),
    );
    registry.register(
        EDITOR_COPY_SOURCE_TEXT,
        crate::ContextPredicate::always(),
        std::sync::Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.copy_source_text()),
    );
    registry.register(
        MARKDOWN_COPY_AS_HTML,
        crate::ContextPredicate::always(),
        std::sync::Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.copy_as_html()),
    );
}

/// Diagnostic command-id list for tests / palette discovery.
pub const MARKDOWN_LINKS_CLIPBOARD_COMMANDS: [&str; 4] = [
    EDITOR_OPEN_LINK_AT_CARET.as_str(),
    EDITOR_COPY_RENDERED_TEXT.as_str(),
    EDITOR_COPY_SOURCE_TEXT.as_str(),
    MARKDOWN_COPY_AS_HTML.as_str(),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable() {
        let ids = MARKDOWN_LINKS_CLIPBOARD_COMMANDS;
        assert_eq!(ids.len(), 4);
        assert!(ids.contains(&"editor.open_link_at_caret"));
        assert!(ids.contains(&"markdown.copy_as_html"));
    }
}
