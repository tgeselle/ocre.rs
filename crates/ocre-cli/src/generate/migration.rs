//! `ocre generate migration`: numbered SQL files, with the SQL inferred from
//! Rails-style names (`create_tags`, `add_slug_to_posts`, `remove_slug_from_posts`).

use std::fmt::Write as _;

use super::{
    Edits,
    fields::{Field, FieldType, parse_fields},
    model::{index_sql, table_sql},
    next_migration_path,
};
use crate::{CliResult, names::is_identifier, output::CliError, project::Project};

const HEADER: &str = "-- Applied once, in file-name order. Never edit a migration after it has been applied.\n";

pub fn migration(project: &Project, name: &str, specs: &[String]) -> CliResult {
    if !is_identifier(name) {
        return Err(
            CliError::new(format!("invalid migration name `{name}`")).hint("use snake_case, e.g. `add_slug_to_posts`")
        );
    }
    let fields = parse_fields(specs)?;
    let sql = infer(name, &fields)?;
    let mut edits = Edits::new(project);
    let path = next_migration_path(&edits, name)?;
    edits.create(&path, format!("-- Migration: {name}\n{HEADER}{sql}"))?;
    let mut report = edits.apply("generate migration")?;
    report.next = vec!["ocre migrate".to_owned()];
    if !fields.is_empty() {
        report.next.push("update the model in src/models/ to match the new columns".to_owned());
    }
    Ok(report)
}

/// SQL for a migration named like Rails: `create_<table>`,
/// `add_<anything>_to_<table>`, `remove_<anything>_from_<table>`. Other names
/// give an empty migration to fill in.
fn infer(name: &str, fields: &[Field]) -> Result<String, CliError> {
    if let Some(table) = name.strip_prefix("create_") {
        return Ok(table_sql(table, fields));
    }
    if let Some((_, table)) = name.strip_prefix("add_").and_then(|rest| rest.rsplit_once("_to_")) {
        return add_columns(table, fields);
    }
    if let Some((column, table)) = name.strip_prefix("remove_").and_then(|rest| rest.rsplit_once("_from_")) {
        let mut sql = String::new();
        let columns: Vec<&str> =
            if fields.is_empty() { vec![column] } else { fields.iter().map(|f| f.name.as_str()).collect() };
        for column in columns {
            writeln!(sql, "ALTER TABLE {table} DROP COLUMN {column};").expect("writing to a String");
        }
        return Ok(sql);
    }
    if fields.is_empty() {
        Ok(String::new())
    } else {
        Err(CliError::new(format!("cannot tell which table `{name}` changes"))
            .hint("name it `create_<table>`, `add_<columns>_to_<table>` or `remove_<columns>_from_<table>`"))
    }
}

fn add_columns(table: &str, fields: &[Field]) -> Result<String, CliError> {
    if fields.is_empty() {
        return Err(CliError::new("add_..._to_... needs the columns to add")
            .hint("list them like the model fields, e.g. `ocre g migration add_slug_to_posts slug:string^`"));
    }
    let mut sql = String::new();
    for field in fields {
        if field.target.is_some() && !field.optional {
            return Err(CliError::new(format!("`{}` must be optional when added to an existing table", field.name))
                .hint("SQLite adds reference columns as NULL for existing rows: use `name:references?`"));
        }
        // Existing rows need a value for NOT NULL columns.
        let default = match field.ty {
            _ if field.optional || field.ty == FieldType::Boolean => "",
            _ if field.ty.is_textual() => " DEFAULT ''",
            _ => " DEFAULT 0",
        };
        writeln!(sql, "ALTER TABLE {table} ADD COLUMN {}{default};", field.sql_column()).expect("writing to a String");
    }
    for field in fields {
        sql.push_str(&index_sql(table, field));
    }
    Ok(sql)
}

#[cfg(test)]
#[path = "../../tests/generate/migration.rs"]
mod tests;
