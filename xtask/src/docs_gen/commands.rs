//! Command and default-keymap generated docs.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::docs_gen::{escape_md_cell, new_doc};
use crate::tutorial_gen::{KeyBinding, KeymapDoc, KEYMAP_PATH};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CommandDoc {
    pub(crate) id: String,
    pub(crate) family: String,
    pub(crate) label: String,
    pub(crate) registry_predicate: String,
    pub(crate) bindings: Vec<CommandBindingDoc>,
    pub(crate) palette_safe: bool,
    pub(crate) description: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CommandBindingDoc {
    pub(crate) keys: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) when: Option<String>,
}

pub(crate) fn write_commands(workspace: &Path) -> Result<String> {
    let commands = collect_commands(workspace)?;
    let mut by_family: BTreeMap<&str, Vec<&CommandDoc>> = BTreeMap::new();
    for command in &commands {
        by_family
            .entry(command.family.as_str())
            .or_default()
            .push(command);
    }

    let mut out = new_doc("Commands");
    out.push_str("Generated from `continuity_command::default_registry()` and `crates/keymap/assets/default.toml`.\n");
    out.push_str("Registry predicates are inferred by evaluating handlers against standard context probes.\n\n");
    for (family, commands) in by_family {
        out.push_str(&format!("## `{family}.*`\n\n"));
        out.push_str("| Command | Label | Registry predicate | Keys | Palette | Description |\n");
        out.push_str("|---|---|---|---|---|---|\n");
        for command in commands {
            let palette = if command.palette_safe { "yes" } else { "" };
            out.push_str(&format!(
                "| `{}` | {} | `{}` | {} | {} | {} |\n",
                command.id,
                escape_md_cell(&command.label),
                command.registry_predicate,
                format_bindings(&command.bindings),
                palette,
                escape_md_cell(&command.description)
            ));
        }
        out.push('\n');
    }
    Ok(out)
}

pub(crate) fn collect_commands(workspace: &Path) -> Result<Vec<CommandDoc>> {
    let keymap = read_keymap(workspace)?;
    let bindings = bindings_by_command(&keymap.binding);
    let registry = continuity_command::default_registry();
    let mut ids: Vec<continuity_command::CommandId> = registry.ids().collect();
    ids.sort_by(|a, b| a.0.cmp(b.0));

    let mut commands = Vec::new();
    for id in ids {
        let family = id.0.split_once('.').map_or(id.0, |(family, _)| family);
        commands.push(CommandDoc {
            id: id.0.to_string(),
            family: family.to_string(),
            label: command_label(id.0),
            registry_predicate: infer_registry_predicate(&registry, id.0).to_string(),
            bindings: bindings
                .get(id.0)
                .map(|items| {
                    items
                        .iter()
                        .map(|binding| CommandBindingDoc {
                            keys: binding.keys.clone(),
                            when: binding.when.clone().filter(|when| !when.trim().is_empty()),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            palette_safe: registry.is_palette_safe(id.0),
            description: registry.description(id.0).unwrap_or("").to_string(),
        });
    }
    Ok(commands)
}

fn read_keymap(workspace: &Path) -> Result<KeymapDoc> {
    let path = workspace.join(KEYMAP_PATH);
    let text =
        fs::read_to_string(&path).with_context(|| format!("reading keymap {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing keymap {}", path.display()))
}

fn bindings_by_command(bindings: &[KeyBinding]) -> BTreeMap<String, Vec<KeyBinding>> {
    let mut by_command: BTreeMap<String, Vec<KeyBinding>> = BTreeMap::new();
    for binding in bindings {
        by_command
            .entry(binding.command.clone())
            .or_default()
            .push(binding.clone());
    }
    for values in by_command.values_mut() {
        values.sort_by(|a, b| a.keys.cmp(&b.keys).then_with(|| a.when.cmp(&b.when)));
    }
    by_command
}

fn format_bindings(bindings: &[CommandBindingDoc]) -> String {
    if bindings.is_empty() {
        return String::new();
    }
    bindings
        .iter()
        .map(|binding| {
            let keys = binding.keys.join(" -> ");
            match &binding.when {
                Some(when) if !when.trim().is_empty() => {
                    format!("`{keys}` when `{}`", escape_md_cell(when))
                }
                _ => format!("`{keys}`"),
            }
        })
        .collect::<Vec<_>>()
        .join("<br>")
}

fn command_label(id: &str) -> String {
    let raw = id.split_once('.').map_or(id, |(_, name)| name);
    raw.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let mut word = String::new();
            word.push(first.to_ascii_uppercase());
            word.push_str(chars.as_str());
            word
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn infer_registry_predicate(registry: &continuity_command::Registry, id: &str) -> &'static str {
    for (label, fields) in predicate_probes() {
        let ctx = ProbeContext::new(fields);
        if registry.handler_for_name(id, &ctx).is_ok() {
            return label;
        }
    }
    "custom/unknown"
}

fn predicate_probes() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        ("true", vec![]),
        ("editor.focused", vec![("editor.focused", "true")]),
        ("find_bar.visible", vec![("find_bar.visible", "true")]),
        ("language == 'markdown'", vec![("language", "markdown")]),
        (
            "editor.focused && language == 'markdown'",
            vec![("editor.focused", "true"), ("language", "markdown")],
        ),
        (
            "editor.focused && find_bar.visible",
            vec![("editor.focused", "true"), ("find_bar.visible", "true")],
        ),
        (
            "editor.focused && editor.line_is_heading == 'true'",
            vec![
                ("editor.focused", "true"),
                ("editor.line_is_heading", "true"),
            ],
        ),
    ]
}

struct ProbeContext {
    fields: HashMap<&'static str, &'static str>,
}

impl ProbeContext {
    fn new(fields: Vec<(&'static str, &'static str)>) -> Self {
        Self {
            fields: fields.into_iter().collect(),
        }
    }
}

impl continuity_command::Context for ProbeContext {
    fn lookup(&self, key: &str) -> Option<&str> {
        self.fields.get(key).copied()
    }
}

impl continuity_command::FindContext for ProbeContext {}
impl continuity_command::ViewContext for ProbeContext {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_generated_from_command_id_tail() {
        assert_eq!(command_label("editor.insert_newline"), "Insert Newline");
    }

    #[test]
    fn true_predicate_is_inferred() {
        let registry = continuity_command::default_registry();
        assert_eq!(
            infer_registry_predicate(&registry, "window.new_window"),
            "true"
        );
    }
}
