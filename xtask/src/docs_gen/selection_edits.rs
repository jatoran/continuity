//! Selection-edit generated docs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::docs_gen::rust_source::{brace_delta, parse_enum_variants};
use crate::docs_gen::{escape_md_cell, new_doc};

const SOURCE_PATH: &str = "crates/core/src/selection_edit.rs";
const HELPER_ENUMS: &[&str] = &[
    "SortKind",
    "CaseKind",
    "IndentUnit",
    "LineEnding",
    "EmphasisKind",
];

pub(crate) fn write_selection_edits(workspace: &Path) -> Result<String> {
    let path = workspace.join(SOURCE_PATH);
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let variants = parse_enum_variants(&text, "SelectionEdit");
    let planners = planner_map(&text);

    let mut out = new_doc("Selection Edits");
    out.push_str("Generated from `crates/core/src/selection_edit.rs`.\n\n");
    out.push_str("## `SelectionEdit`\n\n");
    out.push_str("| Variant | Payload | Planner(s) | Source | First doc line |\n");
    out.push_str("|---|---|---|---|---|\n");
    for variant in variants {
        let planner = planners
            .get(&variant.name)
            .map(|items| items.join("<br>"))
            .unwrap_or_default();
        out.push_str(&format!(
            "| `{}` | {} | {} | `{}`:{} | {} |\n",
            variant.name,
            format_payload(&variant.payload),
            planner,
            SOURCE_PATH,
            variant.line,
            escape_md_cell(&variant.doc)
        ));
    }

    for enum_name in HELPER_ENUMS {
        let variants = parse_enum_variants(&text, enum_name);
        out.push_str(&format!("\n## `{enum_name}`\n\n"));
        out.push_str("| Variant | Payload | Source | First doc line |\n");
        out.push_str("|---|---|---|---|\n");
        for variant in variants {
            out.push_str(&format!(
                "| `{}` | {} | `{}`:{} | {} |\n",
                variant.name,
                format_payload(&variant.payload),
                SOURCE_PATH,
                variant.line,
                escape_md_cell(&variant.doc)
            ));
        }
    }
    Ok(out)
}

fn planner_map(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut in_match = false;
    let mut match_depth = 0isize;
    let mut current: Option<String> = None;
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.contains("match edit") {
            in_match = true;
            match_depth = brace_delta(line).max(0);
            continue;
        }
        if !in_match {
            continue;
        }
        if let Some(name) = selection_edit_name(line) {
            if let Some(previous) = current.replace(name) {
                out.insert(previous, planners_for_arm(&lines));
                lines.clear();
            }
        }
        if current.is_some() {
            lines.push(line.to_string());
        }
        match_depth += brace_delta(line);
        if match_depth <= 0 {
            if let Some(name) = current.take() {
                out.insert(name, planners_for_arm(&lines));
            }
            break;
        }
    }
    out
}

fn selection_edit_name(line: &str) -> Option<String> {
    let after = line.split_once("SelectionEdit::")?.1;
    let name = after
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>();
    (!name.is_empty()).then_some(name)
}

fn planners_for_arm(lines: &[String]) -> Vec<String> {
    let mut planners = BTreeSet::new();
    for line in lines {
        let mut rest = line.as_str();
        while let Some(idx) = rest.find("plan_") {
            let after = &rest[idx..];
            let name = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>();
            if !name.is_empty() {
                planners.insert(format!("`{name}`"));
            }
            rest = &after[name.len()..];
        }
    }
    planners.into_iter().collect()
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
    fn maps_match_arm_to_planner_names() {
        let text = r#"
pub fn plan(edit: &SelectionEdit) {
    match edit {
        SelectionEdit::InsertText(text) => plan_insert_text(text),
        SelectionEdit::Normalize => {
            crate::edit_normalize::plan_normalize()
        }
    }
}
"#;
        let map = planner_map(text);
        assert_eq!(map["InsertText"], vec!["`plan_insert_text`"]);
        assert_eq!(map["Normalize"], vec!["`plan_normalize`"]);
    }
}
