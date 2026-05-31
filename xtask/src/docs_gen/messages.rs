//! Message and event generated docs.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::docs_gen::rust_source::parse_enum_variants;
use crate::docs_gen::{escape_md_cell, new_doc};

const MESSAGE_ENUMS: &[MessageEnum] = &[
    MessageEnum {
        label: "app::RegistryEvent",
        enum_name: "RegistryEvent",
        path: "crates/app/src/registry.rs",
    },
    MessageEnum {
        label: "config::ConfigEvent",
        enum_name: "ConfigEvent",
        path: "crates/config/src/watcher.rs",
    },
    MessageEnum {
        label: "core::EditorMessage",
        enum_name: "EditorMessage",
        path: "crates/core/src/message.rs",
    },
    MessageEnum {
        label: "core::EditEvent",
        enum_name: "EditEvent",
        path: "crates/core/src/message.rs",
    },
    MessageEnum {
        label: "persist::PersistMessage",
        enum_name: "PersistMessage",
        path: "crates/persist/src/message.rs",
    },
    MessageEnum {
        label: "persist::PersistEvent",
        enum_name: "PersistEvent",
        path: "crates/persist/src/events.rs",
    },
    MessageEnum {
        label: "persist::PersistOperation",
        enum_name: "PersistOperation",
        path: "crates/persist/src/events.rs",
    },
    MessageEnum {
        label: "ui::WindowControl",
        enum_name: "WindowControl",
        path: "crates/ui/src/window_control.rs",
    },
    MessageEnum {
        label: "ui::FileIoEvent",
        enum_name: "FileIoEvent",
        path: "crates/ui/src/file_io.rs",
    },
];

pub(crate) fn write_messages(workspace: &Path) -> Result<String> {
    let mut out = new_doc("Messages");
    out.push_str("Generated from typed message/event/control enums.\n\n");
    for entry in MESSAGE_ENUMS {
        let path = workspace.join(entry.path);
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let variants = parse_enum_variants(&text, entry.enum_name);
        out.push_str(&format!("## `{}`\n\n", entry.label));
        out.push_str("| Variant | Payload | Source | First doc line |\n");
        out.push_str("|---|---|---|---|\n");
        for variant in variants {
            out.push_str(&format!(
                "| `{}` | {} | `{}`:{} | {} |\n",
                variant.name,
                format_payload(&variant.payload),
                entry.path,
                variant.line,
                escape_md_cell(&variant.doc)
            ));
        }
        out.push('\n');
    }
    Ok(out)
}

struct MessageEnum {
    label: &'static str,
    enum_name: &'static str,
    path: &'static str,
}

fn format_payload(payload: &str) -> String {
    if payload.is_empty() {
        String::new()
    } else {
        format!("`{}`", escape_md_cell(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_empty_payload_as_empty_cell() {
        assert_eq!(format_payload(""), "");
        assert_eq!(format_payload("String"), "`String`");
    }
}
