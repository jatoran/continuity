//! Compact repository and symbol-localization generated docs.

use crate::docs_gen::structured::{CrateDoc, GeneratedIndex, PublicApiIndexDoc};
use crate::docs_gen::{escape_md_cell, new_doc};

pub(crate) fn write_repo_map(index: &GeneratedIndex) -> String {
    let mut out = new_doc("Repository Map");
    out.push_str("Generated from workspace manifests, public API items, tests, settings, commands, and persistence schema.\n");
    out.push_str("Use `index.json` for the complete machine-readable surface and `symbols/<crate>.md` for per-crate symbol localization.\n\n");
    out.push_str("## Crates\n\n");
    out.push_str("| Crate | Source | Generated docs | Tests | Localization hints |\n");
    out.push_str("|---|---|---|---|---|\n");
    for krate in &index.crates {
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} |\n",
            krate.member,
            krate.path,
            generated_docs(krate),
            krate
                .test_command
                .as_ref()
                .map(|command| format!("`{}`", escape_md_cell(command)))
                .unwrap_or_default(),
            crate_hints(index, krate)
        ));
    }

    out.push_str("\n## Command Families\n\n");
    out.push_str("| Family | Commands | Docs |\n");
    out.push_str("|---|---:|---|\n");
    for (family, count) in command_family_counts(index) {
        out.push_str(&format!("| `{}` | {} | `COMMANDS.md` |\n", family, count));
    }

    out.push_str("\n## Settings Sections\n\n");
    out.push_str("| Section | Keys | Source |\n");
    out.push_str("|---|---:|---|\n");
    for (section, keys, source) in settings_sections(index) {
        out.push_str(&format!("| `{section}` | {keys} | `{source}` |\n"));
    }

    out.push_str("\n## Persistence Tables\n\n");
    out.push_str("| Table | Introduced | Columns | Source |\n");
    out.push_str("|---|---:|---|---|\n");
    for table in &index.persist_schema.tables {
        out.push_str(&format!(
            "| `{}` | {} | {} | `PERSIST_SCHEMA.md` |\n",
            table.name,
            table.introduced,
            format_code_list(&table.columns, 8)
        ));
    }
    out
}

pub(crate) fn write_symbols_for_crate(index: &GeneratedIndex, crate_member: &str) -> String {
    let mut out = new_doc(&format!("Symbols: {crate_member}"));
    out.push_str(
        "Generated from top-level public API items and deterministic localization heuristics.\n",
    );
    out.push_str(
        "The JSON manifest keeps the same rows in tool-readable form at `../index.json`.\n\n",
    );
    out.push_str("| Symbol | Kind | Source | Tests | Settings | Commands | Schema |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    let mut rows = 0usize;
    for item in index
        .public_api
        .iter()
        .filter(|item| item.crate_member == crate_member)
    {
        rows += 1;
        out.push_str(&format!(
            "| `{}` | `{}` | `{}`:{} | {} | {} | {} | {} |\n",
            escape_md_cell(&item.qualified_name),
            item.kind,
            item.source_path,
            item.line,
            tests_cell(item),
            format_code_list(&item.related_settings, 6),
            format_code_list(&item.related_commands, 6),
            format_code_list(&item.related_schema_tables, 6)
        ));
    }
    if rows == 0 {
        out.push_str("| None |  |  |  |  |  |  |\n");
    }
    out
}

fn generated_docs(krate: &CrateDoc) -> String {
    [
        krate.modules_doc.as_deref(),
        krate.api_doc.as_deref(),
        krate.symbols_doc.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(|path| format!("`{}`", escape_md_cell(path)))
    .collect::<Vec<_>>()
    .join("<br>")
}

fn crate_hints(index: &GeneratedIndex, krate: &CrateDoc) -> String {
    let mut hints = Vec::new();
    let api_count = index
        .public_api
        .iter()
        .filter(|item| item.crate_member == krate.member)
        .count();
    let module_count = index
        .modules
        .iter()
        .filter(|module| module.crate_member == krate.member)
        .count();
    if module_count > 0 {
        hints.push(format!("{module_count} modules"));
    }
    if api_count > 0 {
        hints.push(format!("{api_count} public items"));
    }
    let setting_count = index
        .settings
        .iter()
        .filter(|setting| setting.source_path.starts_with(&krate.path))
        .count();
    if setting_count > 0 {
        hints.push(format!("{setting_count} settings"));
    }
    if krate.member == "command" {
        hints.push(format!("{} commands", index.commands.len()));
    }
    if krate.member == "persist" {
        hints.push(format!(
            "{} schema tables",
            index.persist_schema.tables.len()
        ));
    }
    escape_md_cell(&hints.join(", "))
}

fn command_family_counts(index: &GeneratedIndex) -> Vec<(String, usize)> {
    let mut counts = std::collections::BTreeMap::new();
    for command in &index.commands {
        *counts.entry(command.family.clone()).or_insert(0) += 1;
    }
    counts.into_iter().collect()
}

fn settings_sections(index: &GeneratedIndex) -> Vec<(String, usize, String)> {
    let mut sections = std::collections::BTreeMap::<String, (usize, String)>::new();
    for setting in &index.settings {
        let entry = sections
            .entry(setting.section.clone())
            .or_insert_with(|| (0, setting.source_path.clone()));
        entry.0 += 1;
    }
    sections
        .into_iter()
        .map(|(section, (keys, source))| (section, keys, source))
        .collect()
}

fn tests_cell(item: &PublicApiIndexDoc) -> String {
    if item.related_tests.is_empty() {
        format!("`{}`", escape_md_cell(&item.test_command))
    } else {
        format_code_list(&item.related_tests, 3)
    }
}

fn format_code_list(items: &[String], limit: usize) -> String {
    if items.is_empty() {
        return String::new();
    }
    let mut out = items
        .iter()
        .take(limit)
        .map(|item| format!("`{}`", escape_md_cell(item)))
        .collect::<Vec<_>>();
    if items.len() > limit {
        out.push(format!("+{}", items.len() - limit));
    }
    out.join("<br>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_code_list_limits_long_lists() {
        let items = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(format_code_list(&items, 2), "`a`<br>`b`<br>+1");
    }
}
