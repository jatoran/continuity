//! Structured generated documentation index.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::docs_gen::commands::{self, CommandDoc};
use crate::docs_gen::modules;
use crate::docs_gen::persist_schema;
use crate::docs_gen::rust_source::{PublicItem, WorkspaceCrate};
use crate::docs_gen::settings;
use crate::docs_gen::test_index::{self, TestKind};

pub(crate) const JSON_GENERATED_BY: &str = "cargo xtask docs";

#[derive(Clone, Debug, Serialize)]
pub(crate) struct GeneratedIndex {
    pub(crate) generated_by: &'static str,
    pub(crate) do_not_edit: bool,
    pub(crate) schema_version: u32,
    pub(crate) surfaces: Vec<SurfaceDoc>,
    pub(crate) crates: Vec<CrateDoc>,
    pub(crate) modules: Vec<ModuleIndexDoc>,
    pub(crate) public_api: Vec<PublicApiIndexDoc>,
    pub(crate) commands: Vec<CommandDoc>,
    pub(crate) settings: Vec<SettingIndexDoc>,
    pub(crate) tests: Vec<TestCrateIndexDoc>,
    pub(crate) persist_schema: PersistSchemaIndexDoc,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SurfaceDoc {
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) description: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CrateDoc {
    pub(crate) member: String,
    pub(crate) package_name: String,
    pub(crate) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) readme_path: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) direct_workspace_deps: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) modules_doc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) api_doc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) symbols_doc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) test_command: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ModuleIndexDoc {
    pub(crate) crate_member: String,
    pub(crate) module_path: String,
    pub(crate) visibility: String,
    pub(crate) source_path: String,
    pub(crate) lines: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) first_doc_line: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicApiIndexDoc {
    pub(crate) crate_member: String,
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) qualified_name: String,
    pub(crate) source_path: String,
    pub(crate) line: usize,
    pub(crate) signature: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) first_doc_line: String,
    pub(crate) test_command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) related_tests: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) related_settings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) related_commands: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) related_schema_tables: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SettingIndexDoc {
    pub(crate) section: String,
    pub(crate) key: String,
    pub(crate) rust_field: String,
    pub(crate) ty: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) default: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) validation_hint: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) description: String,
    pub(crate) source_path: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TestCrateIndexDoc {
    pub(crate) crate_member: String,
    pub(crate) package_name: String,
    pub(crate) unit_test_files: usize,
    pub(crate) unit_test_functions: usize,
    pub(crate) ignored_tests: usize,
    pub(crate) test_command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) integration_files: Vec<TestFileIndexDoc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) bench_files: Vec<TestFileIndexDoc>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TestFileIndexDoc {
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) command: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PersistSchemaIndexDoc {
    pub(crate) current_version: u32,
    pub(crate) migrations: Vec<SchemaMigrationIndexDoc>,
    pub(crate) tables: Vec<TableIndexDoc>,
    pub(crate) indexes: Vec<SchemaIndexDoc>,
    pub(crate) alters: Vec<AlterIndexDoc>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SchemaMigrationIndexDoc {
    pub(crate) version: u32,
    pub(crate) source_path: String,
    pub(crate) line: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) tables: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) indexes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) alters: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TableIndexDoc {
    pub(crate) name: String,
    pub(crate) introduced: u32,
    pub(crate) columns: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SchemaIndexDoc {
    pub(crate) name: String,
    pub(crate) table: String,
    pub(crate) introduced: u32,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AlterIndexDoc {
    pub(crate) table: String,
    pub(crate) column: String,
    pub(crate) introduced: u32,
}

pub(crate) fn build(workspace: &Path, crates: &[WorkspaceCrate]) -> Result<GeneratedIndex> {
    let commands = commands::collect_commands(workspace)?;
    let settings = settings::collect_settings(workspace)?
        .into_iter()
        .map(|setting| SettingIndexDoc {
            section: setting.section,
            key: setting.key,
            rust_field: setting.rust_field,
            ty: setting.ty,
            default: setting.default,
            validation_hint: setting.validation_hint,
            description: setting.description,
            source_path: setting.source_path,
        })
        .collect::<Vec<_>>();
    let tests = collect_tests(workspace)?;
    let schema = collect_schema(workspace)?;
    let mut crate_docs = Vec::new();
    let mut module_docs = Vec::new();
    let mut public_api = Vec::new();
    let test_commands = tests
        .iter()
        .map(|tests| (tests.crate_member.clone(), tests.test_command.clone()))
        .collect::<BTreeMap<_, _>>();

    for krate in crates {
        let has_generated_crate_docs = krate.path.starts_with("crates/");
        crate_docs.push(CrateDoc {
            member: krate.member.clone(),
            package_name: krate.package_name.clone(),
            path: krate.path.clone(),
            readme_path: workspace
                .join(&krate.path)
                .join("README.md")
                .exists()
                .then(|| format!("{}/README.md", krate.path)),
            direct_workspace_deps: direct_workspace_deps(workspace, krate)?,
            modules_doc: has_generated_crate_docs.then(|| format!("modules/{}.md", krate.member)),
            api_doc: has_generated_crate_docs.then(|| format!("api/{}.md", krate.member)),
            symbols_doc: has_generated_crate_docs.then(|| format!("symbols/{}.md", krate.member)),
            test_command: test_commands.get(&krate.member).cloned(),
        });

        if !has_generated_crate_docs {
            continue;
        }
        for module in modules::collect_modules(workspace, krate)? {
            module_docs.push(ModuleIndexDoc {
                crate_member: krate.member.clone(),
                module_path: module.module_path,
                visibility: module.visibility,
                source_path: module.source_path,
                lines: module.lines,
                first_doc_line: module.first_doc_line,
            });
        }
        for item in crate::docs_gen::api::collect_api_items(workspace, krate)? {
            public_api.push(public_api_doc(
                krate, item, &commands, &settings, &tests, &schema,
            ));
        }
    }

    Ok(GeneratedIndex {
        generated_by: JSON_GENERATED_BY,
        do_not_edit: true,
        schema_version: 1,
        surfaces: surfaces(),
        crates: crate_docs,
        modules: module_docs,
        public_api,
        commands,
        settings,
        tests,
        persist_schema: schema,
    })
}

pub(crate) fn write_index_json(index: &GeneratedIndex) -> Result<String> {
    let mut out = serde_json::to_string(index).context("serializing generated index")?;
    out.push('\n');
    Ok(out)
}

fn public_api_doc(
    krate: &WorkspaceCrate,
    item: PublicItem,
    commands: &[CommandDoc],
    settings: &[SettingIndexDoc],
    tests: &[TestCrateIndexDoc],
    schema: &PersistSchemaIndexDoc,
) -> PublicApiIndexDoc {
    let test_command = tests
        .iter()
        .find(|tests| tests.crate_member == krate.member)
        .map(|tests| tests.test_command.clone())
        .unwrap_or_else(|| format!("cargo test -p {}", krate.package_name));
    PublicApiIndexDoc {
        qualified_name: qualified_name(&krate.member, &item),
        related_tests: related_tests(&krate.member, &item.name, tests),
        related_settings: related_settings(&item.path, settings),
        related_commands: related_commands(&item.name, commands),
        related_schema_tables: related_schema_tables(&item.path, schema),
        crate_member: krate.member.clone(),
        kind: item.kind,
        name: item.name,
        source_path: item.path,
        line: item.line,
        signature: item.signature,
        first_doc_line: item.doc,
        test_command,
    }
}

fn collect_tests(workspace: &Path) -> Result<Vec<TestCrateIndexDoc>> {
    Ok(test_index::collect_crate_tests(workspace)?
        .into_iter()
        .map(|tests| TestCrateIndexDoc {
            test_command: format!("cargo test -p {}", tests.package_name),
            integration_files: tests
                .integration_files
                .iter()
                .map(|file| test_file_doc(&tests.package_name, file))
                .collect(),
            bench_files: tests
                .bench_files
                .iter()
                .map(|file| test_file_doc(&tests.package_name, file))
                .collect(),
            crate_member: tests.member,
            package_name: tests.package_name,
            unit_test_files: tests.unit_test_files,
            unit_test_functions: tests.unit_test_functions,
            ignored_tests: tests.ignored_tests,
        })
        .collect())
}

fn collect_schema(workspace: &Path) -> Result<PersistSchemaIndexDoc> {
    let schema = persist_schema::collect_persist_schema(workspace)?;
    Ok(PersistSchemaIndexDoc {
        current_version: schema.current_version,
        migrations: schema
            .migrations
            .into_iter()
            .map(|migration| SchemaMigrationIndexDoc {
                version: migration.version,
                source_path: migration.source_path,
                line: migration.line,
                summary: migration.summary,
                tables: migration.tables,
                indexes: migration.indexes,
                alters: migration.alters,
            })
            .collect(),
        tables: schema
            .tables
            .into_iter()
            .map(|table| TableIndexDoc {
                name: table.name,
                introduced: table.introduced,
                columns: table.columns,
            })
            .collect(),
        indexes: schema
            .indexes
            .into_iter()
            .map(|index| SchemaIndexDoc {
                name: index.name,
                table: index.table,
                introduced: index.introduced,
            })
            .collect(),
        alters: schema
            .alters
            .into_iter()
            .map(|alter| AlterIndexDoc {
                table: alter.table,
                column: alter.column,
                introduced: alter.introduced,
            })
            .collect(),
    })
}

fn test_file_doc(package_name: &str, file: &test_index::TestFile) -> TestFileIndexDoc {
    let command = match file.kind {
        TestKind::Bench => format!("cargo bench -p {} --bench {}", package_name, file.stem),
        _ => format!("cargo test -p {} --test {}", package_name, file.stem),
    };
    TestFileIndexDoc {
        path: file.path.clone(),
        kind: file.kind.as_str().to_string(),
        command,
    }
}

fn qualified_name(crate_member: &str, item: &PublicItem) -> String {
    let module = module_path_from_source_path(&item.path);
    if module.is_empty() {
        format!("{crate_member}::{}", item.name)
    } else {
        format!("{crate_member}::{module}::{}", item.name)
    }
}

fn related_tests(
    crate_member: &str,
    symbol_name: &str,
    tests: &[TestCrateIndexDoc],
) -> Vec<String> {
    let token = comparable_token(symbol_name);
    tests
        .iter()
        .find(|tests| tests.crate_member == crate_member)
        .into_iter()
        .flat_map(|tests| {
            tests
                .integration_files
                .iter()
                .chain(tests.bench_files.iter())
        })
        .filter(|file| comparable_token(&file.path).contains(&token))
        .map(|file| file.path.clone())
        .take(8)
        .collect()
}

fn related_settings(source_path: &str, settings: &[SettingIndexDoc]) -> Vec<String> {
    settings
        .iter()
        .filter(|setting| setting.source_path == source_path)
        .map(|setting| format!("{}.{}", setting.section, setting.key))
        .take(12)
        .collect()
}

fn related_commands(symbol_name: &str, commands: &[CommandDoc]) -> Vec<String> {
    let token = comparable_token(symbol_name);
    if token.len() < 4 {
        return Vec::new();
    }
    commands
        .iter()
        .filter(|command| {
            comparable_token(&command.id).contains(&token)
                || comparable_token(&command.label).contains(&token)
                || comparable_token(&command.description).contains(&token)
        })
        .map(|command| command.id.clone())
        .take(12)
        .collect()
}

fn related_schema_tables(source_path: &str, schema: &PersistSchemaIndexDoc) -> Vec<String> {
    if source_path != "crates/persist/src/schema.rs" {
        return Vec::new();
    }
    schema
        .tables
        .iter()
        .map(|table| table.name.clone())
        .collect()
}

fn direct_workspace_deps(workspace: &Path, krate: &WorkspaceCrate) -> Result<Vec<String>> {
    let path = workspace.join(&krate.path).join("Cargo.toml");
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let mut deps = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(table) = manifest.get(section).and_then(toml::Value::as_table) else {
            continue;
        };
        for (name, value) in table {
            let is_workspace = value
                .as_table()
                .and_then(|table| table.get("workspace"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            if is_workspace && name.starts_with("continuity-") {
                deps.push(name.clone());
            }
        }
    }
    deps.sort();
    deps.dedup();
    Ok(deps)
}

fn surfaces() -> Vec<SurfaceDoc> {
    [
        ("README.md", "markdown", "generated-doc routing"),
        ("index.json", "json", "machine-readable generated manifest"),
        (
            "REPO_MAP.md",
            "markdown",
            "compact crate and localization map",
        ),
        ("CRATES.md", "markdown", "workspace crate inventory"),
        ("COMMANDS.md", "markdown", "command and keymap inventory"),
        ("SETTINGS.md", "markdown", "settings inventory"),
        ("TEST_INDEX.md", "markdown", "test inventory"),
        (
            "PERSIST_SCHEMA.md",
            "markdown",
            "persistence schema inventory",
        ),
        (
            "modules/<crate>.md",
            "markdown",
            "per-crate module inventory",
        ),
        (
            "api/<crate>.md",
            "markdown",
            "per-crate public API inventory",
        ),
        (
            "symbols/<crate>.md",
            "markdown",
            "per-crate symbol localization",
        ),
    ]
    .into_iter()
    .map(|(path, kind, description)| SurfaceDoc {
        path: path.into(),
        kind: kind.into(),
        description: description.into(),
    })
    .collect()
}

fn comparable_token(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn module_path_from_source_path(path: &str) -> String {
    let Some((_, rest)) = path.split_once("/src/") else {
        return String::new();
    };
    if rest == "lib.rs" || rest == "main.rs" {
        String::new()
    } else {
        rest.trim_end_matches(".rs").replace('/', "::")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparable_token_drops_separators() {
        assert_eq!(comparable_token("ApplySelectionEdit"), "applyselectionedit");
        assert_eq!(
            comparable_token("apply_selection_edit"),
            "applyselectionedit"
        );
    }

    #[test]
    fn index_json_contains_generated_marker() {
        let index = GeneratedIndex {
            generated_by: JSON_GENERATED_BY,
            do_not_edit: true,
            schema_version: 1,
            surfaces: Vec::new(),
            crates: Vec::new(),
            modules: Vec::new(),
            public_api: Vec::new(),
            commands: Vec::new(),
            settings: Vec::new(),
            tests: Vec::new(),
            persist_schema: PersistSchemaIndexDoc {
                current_version: 0,
                migrations: Vec::new(),
                tables: Vec::new(),
                indexes: Vec::new(),
                alters: Vec::new(),
            },
        };
        let json = write_index_json(&index).expect("json");
        assert!(json.contains("\"generated_by\":\"cargo xtask docs\""));
    }
}
