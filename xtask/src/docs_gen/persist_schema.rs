//! Persistence schema generated docs.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::docs_gen::rust_source::clean_doc_text;
use crate::docs_gen::{escape_md_cell, new_doc};

#[derive(Clone, Debug)]
pub(crate) struct PersistSchemaDoc {
    pub(crate) current_version: u32,
    pub(crate) migrations: Vec<SchemaMigrationDoc>,
    pub(crate) tables: Vec<TableDoc>,
    pub(crate) indexes: Vec<IndexDoc>,
    pub(crate) alters: Vec<AlterDoc>,
}

#[derive(Clone, Debug)]
pub(crate) struct SchemaMigrationDoc {
    pub(crate) version: u32,
    pub(crate) source_path: String,
    pub(crate) line: usize,
    pub(crate) summary: String,
    pub(crate) tables: Vec<String>,
    pub(crate) indexes: Vec<String>,
    pub(crate) alters: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct TableDoc {
    pub(crate) name: String,
    pub(crate) introduced: u32,
    pub(crate) columns: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct IndexDoc {
    pub(crate) name: String,
    pub(crate) table: String,
    pub(crate) introduced: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct AlterDoc {
    pub(crate) table: String,
    pub(crate) column: String,
    pub(crate) introduced: u32,
}

const SCHEMA_PATH: &str = "crates/persist/src/schema.rs";

pub(crate) fn write_persist_schema(workspace: &Path) -> Result<String> {
    let schema = collect_persist_schema(workspace)?;
    let mut out = new_doc("Persistence Schema");
    out.push_str("Generated from `crates/persist/src/schema.rs`.\n\n");
    out.push_str(&format!(
        "- Current schema version: `{}`\n\n",
        schema.current_version
    ));
    out.push_str("## Migrations\n\n");
    out.push_str("| Version | Source | Summary | Tables | Indexes | Alters |\n");
    out.push_str("|---:|---|---|---|---|---|\n");
    for migration in &schema.migrations {
        out.push_str(&format!(
            "| {} | `{}`:{} | {} | {} | {} | {} |\n",
            migration.version,
            migration.source_path,
            migration.line,
            escape_md_cell(&migration.summary),
            format_code_list(&migration.tables),
            format_code_list(&migration.indexes),
            format_code_list(&migration.alters)
        ));
    }

    out.push_str("\n## Tables\n\n");
    out.push_str("| Table | Introduced | Columns |\n");
    out.push_str("|---|---:|---|\n");
    for table in &schema.tables {
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            table.name,
            table.introduced,
            format_code_list(&table.columns)
        ));
    }

    out.push_str("\n## Indexes\n\n");
    out.push_str("| Index | Table | Introduced |\n");
    out.push_str("|---|---|---:|\n");
    for index in &schema.indexes {
        out.push_str(&format!(
            "| `{}` | `{}` | {} |\n",
            index.name, index.table, index.introduced
        ));
    }

    if !schema.alters.is_empty() {
        out.push_str("\n## Alters\n\n");
        out.push_str("| Table | Column | Introduced |\n");
        out.push_str("|---|---|---:|\n");
        for alter in &schema.alters {
            out.push_str(&format!(
                "| `{}` | `{}` | {} |\n",
                alter.table, alter.column, alter.introduced
            ));
        }
    }
    Ok(out)
}

pub(crate) fn collect_persist_schema(workspace: &Path) -> Result<PersistSchemaDoc> {
    let path = workspace.join("crates/persist/src/schema.rs");
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let current_version = parse_current_version(&text).unwrap_or_default();
    let blocks = parse_schema_blocks(&text);
    let mut migrations = Vec::new();
    let mut tables = Vec::new();
    let mut indexes = Vec::new();
    let mut alters = Vec::new();
    for block in &blocks {
        let statements = parse_statements(block);
        let migration_tables = statements
            .iter()
            .filter_map(|statement| match statement {
                Statement::Table(table) => Some(table.name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let migration_indexes = statements
            .iter()
            .filter_map(|statement| match statement {
                Statement::Index(index) => Some(index.name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let migration_alters = statements
            .iter()
            .filter_map(|statement| match statement {
                Statement::Alter(alter) => Some(alter.column.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        for statement in statements {
            match statement {
                Statement::Table(table) => tables.push(TableDoc {
                    name: table.name,
                    introduced: block.version,
                    columns: table.columns,
                }),
                Statement::Index(index) => indexes.push(IndexDoc {
                    name: index.name,
                    table: index.table,
                    introduced: block.version,
                }),
                Statement::Alter(alter) => alters.push(AlterDoc {
                    table: alter.table,
                    column: alter.column,
                    introduced: block.version,
                }),
            }
        }
        migrations.push(SchemaMigrationDoc {
            version: block.version,
            source_path: SCHEMA_PATH.into(),
            line: block.line,
            summary: block.doc.clone(),
            tables: migration_tables,
            indexes: migration_indexes,
            alters: migration_alters,
        });
    }
    Ok(PersistSchemaDoc {
        current_version,
        migrations,
        tables,
        indexes,
        alters,
    })
}

#[derive(Clone, Debug)]
struct SchemaBlock {
    version: u32,
    line: usize,
    doc: String,
    sql: String,
}

#[derive(Clone, Debug)]
struct TableStatement {
    name: String,
    columns: Vec<String>,
}

#[derive(Clone, Debug)]
struct IndexStatement {
    name: String,
    table: String,
}

#[derive(Clone, Debug)]
struct AlterStatement {
    table: String,
    column: String,
}

enum Statement {
    Table(TableStatement),
    Index(IndexStatement),
    Alter(AlterStatement),
}

fn parse_current_version(text: &str) -> Option<u32> {
    text.lines().find_map(|line| {
        let rest = line
            .trim()
            .strip_prefix("pub const CURRENT_VERSION: u32 = ")?;
        rest.trim_end_matches(';').parse().ok()
    })
}

fn parse_schema_blocks(text: &str) -> Vec<SchemaBlock> {
    let mut out = Vec::new();
    let mut docs = Vec::new();
    let mut lines = text.lines().enumerate().peekable();
    while let Some((idx, line)) = lines.next() {
        let trimmed = line.trim();
        if let Some(doc) = trimmed.strip_prefix("///") {
            docs.push(clean_doc_text(doc));
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("const SCHEMA_V") else {
            if !trimmed.is_empty() && !trimmed.starts_with("///") {
                docs.clear();
            }
            continue;
        };
        let Some((version_text, _)) = rest.split_once(':') else {
            continue;
        };
        let Some(version) = version_text.parse::<u32>().ok() else {
            continue;
        };
        let mut sql = String::new();
        while let Some((_, sql_line)) = lines.peek().copied() {
            lines.next();
            if sql_line.trim() == "\"#;" {
                break;
            }
            sql.push_str(sql_line);
            sql.push('\n');
        }
        out.push(SchemaBlock {
            version,
            line: idx + 1,
            doc: docs
                .iter()
                .find(|line| !line.is_empty())
                .cloned()
                .unwrap_or_default(),
            sql,
        });
        docs.clear();
    }
    out
}

fn parse_statements(block: &SchemaBlock) -> Vec<Statement> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in block.sql.lines() {
        current.push_str(line);
        current.push('\n');
        if line.trim_end().ends_with(';') {
            if let Some(statement) = parse_statement(&current) {
                out.push(statement);
            }
            current.clear();
        }
    }
    out
}

fn parse_statement(statement: &str) -> Option<Statement> {
    let normalized = statement.trim();
    if let Some(table) = parse_table(normalized) {
        return Some(Statement::Table(table));
    }
    if let Some(index) = parse_index(normalized) {
        return Some(Statement::Index(index));
    }
    if let Some(alter) = parse_alter(normalized) {
        return Some(Statement::Alter(alter));
    }
    None
}

fn parse_table(statement: &str) -> Option<TableStatement> {
    let rest = statement.strip_prefix("CREATE TABLE IF NOT EXISTS ")?;
    let (name, body) = rest.split_once('(')?;
    let columns = body
        .trim_end_matches(';')
        .trim_end_matches(')')
        .lines()
        .filter_map(column_name)
        .collect::<Vec<_>>();
    Some(TableStatement {
        name: name.trim().to_string(),
        columns,
    })
}

fn parse_index(statement: &str) -> Option<IndexStatement> {
    let parts = statement.split_whitespace().collect::<Vec<_>>();
    if parts.get(0..5)? != ["CREATE", "INDEX", "IF", "NOT", "EXISTS"] {
        return None;
    }
    let name = parts.get(5)?;
    let on_idx = parts.iter().position(|part| *part == "ON")?;
    let table_part = parts.get(on_idx + 1)?;
    let table = table_part
        .split_once('(')
        .map_or(*table_part, |(table, _)| table);
    Some(IndexStatement {
        name: (*name).to_string(),
        table: table.to_string(),
    })
}

fn parse_alter(statement: &str) -> Option<AlterStatement> {
    let rest = statement.strip_prefix("ALTER TABLE ")?;
    let (table, after_table) = rest.split_once(" ADD COLUMN ")?;
    let column = after_table.split_whitespace().next()?.trim_end_matches(';');
    Some(AlterStatement {
        table: table.trim().to_string(),
        column: column.to_string(),
    })
}

fn column_name(line: &str) -> Option<String> {
    let trimmed = line.trim().trim_end_matches(',');
    if trimmed.is_empty()
        || trimmed.starts_with("PRIMARY ")
        || trimmed.starts_with("FOREIGN ")
        || trimmed.starts_with("UNIQUE ")
        || trimmed.starts_with("CHECK ")
    {
        return None;
    }
    trimmed.split_whitespace().next().map(str::to_string)
}

fn format_code_list(items: &[String]) -> String {
    if items.is_empty() {
        return String::new();
    }
    items
        .iter()
        .map(|item| format!("`{item}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_table_statement_columns() {
        let table = parse_table(
            "CREATE TABLE IF NOT EXISTS demo (
                id BLOB PRIMARY KEY,
                name TEXT,
                PRIMARY KEY(id)
            );",
        )
        .expect("table");
        assert_eq!(table.name, "demo");
        assert_eq!(table.columns, vec!["id", "name"]);
    }

    #[test]
    fn parses_alter_statement() {
        let alter = parse_alter("ALTER TABLE demo ADD COLUMN label TEXT;").expect("alter");
        assert_eq!(alter.table, "demo");
        assert_eq!(alter.column, "label");
    }
}
