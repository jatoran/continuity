//! Desktop-facing access to the storage-neutral command resolver.

use continuity_host::EditorOperation;

/// Resolve a context-free editor command to its typed engine operation.
///
/// `None` means the command requires desktop/context mediation or is unknown.
#[must_use]
pub fn editor_operation_for_command(command: &str) -> Option<EditorOperation> {
    continuity_host::editor_operation_for_command(command)
}

#[cfg(test)]
mod tests {
    use super::editor_operation_for_command;
    use continuity_core::{EmphasisKind, SelectionEdit};
    use continuity_host::EditorOperation;

    #[test]
    fn desktop_registry_uses_the_shared_resolver() {
        assert!(matches!(
            editor_operation_for_command("markdown.toggle_bold"),
            Some(EditorOperation::ApplySelectionEdit(
                SelectionEdit::MarkdownToggleEmphasis(EmphasisKind::Bold)
            ))
        ));
        assert!(editor_operation_for_command("file.open").is_none());
    }
}
