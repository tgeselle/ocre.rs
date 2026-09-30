//! `ocre generate migration`: numbered SQL files, with the SQL inferred from
//! Rails-style names (`create_tags`, `add_slug_to_posts`, `remove_slug_from_posts`,
//! `add_index_to_posts`, `rename_title_to_headline_in_posts`, `drop_tags`,
//! `rebuild_posts`).

use std::fmt::Write as _;

use super::{
    Edits,
    fields::{Field, FieldType, parse_fields},
    model::{index_sql, table_sql},
    next_migration_path,
};
use crate::{CliResult, db::SCHEMA, names::is_identifier, output::CliError, project::Project};

const HEADER: &str = "-- Applied once, in file-name order. Never edit a migration after it has been applied.\n";

pub fn migration(project: &Project, name: &str, specs: &[String]) -> CliResult {
    if !is_identifier(name) {
        return Err(
            CliError::new(format!("invalid migration name `{name}`")).hint("use snake_case, e.g. `add_slug_to_posts`")
        );
    }
    let mut edits = Edits::new(project);
    let (sql, model_changes) = match structural(name, specs, &|| edits.read(SCHEMA))? {
        Some(inferred) => inferred,
        None => {
            let fields = parse_fields(specs)?;
            (infer(name, &fields)?, !fields.is_empty())
        }
    };
    let path = next_migration_path(&edits, name)?;
    edits.create(&path, format!("-- Migration: {name}\n{HEADER}{sql}"))?;
    let mut report = edits.apply("generate migration")?;
    report.next = vec!["ocre migrate".to_owned()];
    if model_changes {
        report.next.push("update the model in src/models/ to match the new columns".to_owned());
    }
    Ok(report)
}

/// SQL for names whose arguments are column names, not fields: indexes,
/// renames, drops and table rebuilds. `None` for every other name. The
/// boolean says whether the model's Rust code must follow. `schema` reads
/// db/schema.sql, only for rebuilds (not generic: one copy to cover).
fn structural(
    name: &str,
    specs: &[String],
    schema: &dyn Fn() -> Result<Option<String>, CliError>,
) -> Result<Option<(String, bool)>, CliError> {
    let index = [("add_unique_index_to_", "CREATE UNIQUE INDEX"), ("add_index_to_", "CREATE INDEX")]
        .into_iter()
        .find_map(|(prefix, create)| Some((name.strip_prefix(prefix)?, create)));
    if let Some((table, create)) = index {
        let columns = column_names(name, specs)?;
        let index = format!("index_{table}_on_{}", columns.join("_and_"));
        return Ok(Some((format!("{create} {index} ON {table} ({});\n", columns.join(", ")), false)));
    }
    if let Some(table) = name.strip_prefix("remove_index_from_") {
        let columns = column_names(name, specs)?;
        return Ok(Some((format!("DROP INDEX IF EXISTS index_{table}_on_{};\n", columns.join("_and_")), false)));
    }
    let simple = name.starts_with("rename_") || name.starts_with("drop_") || name.starts_with("rebuild_");
    if simple && !specs.is_empty() {
        return Err(CliError::new(format!("`{name}` takes no fields"))
            .hint("the name says it all, e.g. `rename_title_to_headline_in_posts`, `drop_tags`, `rebuild_posts`"));
    }
    if let Some(rest) = name.strip_prefix("rename_") {
        let (renamed, table) = match rest.rsplit_once("_in_") {
            Some((columns, table)) => (columns, Some(table)),
            None => (rest, None),
        };
        let (old, new) = renamed.split_once("_to_").ok_or_else(|| {
            CliError::new(format!("cannot tell what `{name}` renames"))
                .hint("name it `rename_<column>_to_<new>_in_<table>` or `rename_<table>_to_<new>`")
        })?;
        let sql = match table {
            Some(table) => format!("ALTER TABLE {table} RENAME COLUMN {old} TO {new};\n"),
            None => format!("ALTER TABLE {old} RENAME TO {new};\n"),
        };
        return Ok(Some((sql, true)));
    }
    if let Some(table) = name.strip_prefix("drop_") {
        return Ok(Some((format!("DROP TABLE {table};\n"), true)));
    }
    if let Some(table) = name.strip_prefix("rebuild_") {
        let schema = schema()?.ok_or_else(|| {
            CliError::new(format!("{SCHEMA} not found"))
                .hint("run `ocre migrate` then `ocre db schema`: the rebuild copies the table's current definition")
        })?;
        return rebuild(table, &schema).map(|sql| Some((sql, true)));
    }
    Ok(None)
}

/// Column names given after an index migration's name.
fn column_names<'a>(name: &str, specs: &'a [String]) -> Result<Vec<&'a str>, CliError> {
    if specs.is_empty() || specs.iter().any(|column| !is_identifier(column)) {
        return Err(CliError::new(format!("`{name}` needs the indexed column names")).hint(
            "list them in index order, without types, e.g. `ocre g migration add_index_to_posts author_id created_at`",
        ));
    }
    Ok(specs.iter().map(String::as_str).collect())
}

/// SQLite's table rebuild, from the definition in `db/schema.sql`: create the
/// new table, copy the rows, drop the old one, rename, recreate the indexes.
fn rebuild(table: &str, schema: &str) -> Result<String, CliError> {
    let statements: Vec<String> = schema
        .split(";\n")
        .map(|statement| statement.lines().filter(|line| !line.starts_with("--")).collect::<Vec<_>>().join("\n"))
        .map(|statement| statement.trim().to_owned())
        .collect();
    let create_prefix = format!("CREATE TABLE {table} (");
    let create = statements.iter().find(|statement| statement.starts_with(&create_prefix)).ok_or_else(|| {
        CliError::new(format!("table `{table}` is not in {SCHEMA}"))
            .hint("run `ocre migrate` then `ocre db schema` to refresh it, and check the table name")
    })?;
    let referenced = format!("REFERENCES {table}(");
    if let Some(child) = statements.iter().find(|s| s.starts_with("CREATE TABLE") && s.contains(&referenced)) {
        let child = child.trim_start_matches("CREATE TABLE ").split([' ', '(']).next().unwrap_or_default();
        return Err(CliError::new(format!("`{child}` references `{table}`: rebuilding it would delete or clear their rows"))
            .hint(format!(
                "D1 enforces foreign keys, so dropping `{table}` runs ON DELETE on `{child}`; add a new column and backfill it instead"
            )));
    }
    let body = &create[create_prefix.len()..create.rfind(')').unwrap_or(create.len())];
    let columns: Vec<&str> = top_level_items(body)
        .into_iter()
        .filter_map(|item| item.split_whitespace().next())
        .filter(|word| {
            !["CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN"].contains(&word.to_ascii_uppercase().as_str())
        })
        .collect();
    let columns = columns.join(", ");
    let indexes: String = statements
        .iter()
        .filter(|s| s.starts_with("CREATE") && s.contains(" INDEX ") && s.contains(&format!(" ON {table} (")))
        .map(|s| format!("{s};\n"))
        .collect();
    let new_table = create.replacen(&create_prefix, &format!("CREATE TABLE {table}_new ("), 1);
    Ok(format!(
        "-- Rebuilds `{table}` to change what ALTER TABLE cannot (a column's type, NOT NULL,\n\
         -- DEFAULT, CHECK or REFERENCES): edit the CREATE TABLE below, and keep both\n\
         -- column lists of the INSERT in step with it.\n\
         PRAGMA defer_foreign_keys = true;\n\
         {new_table};\n\
         INSERT INTO {table}_new ({columns}) SELECT {columns} FROM {table};\n\
         DROP TABLE {table};\n\
         ALTER TABLE {table}_new RENAME TO {table};\n\
         {indexes}\
         PRAGMA defer_foreign_keys = false;\n"
    ))
}

/// Splits a column list on the commas outside parentheses and quotes.
fn top_level_items(body: &str) -> Vec<&str> {
    let (mut items, mut depth, mut quote, mut start) = (Vec::new(), 0_i32, None, 0);
    for (i, c) in body.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, ',') if depth == 0 => {
                items.push(body[start..i].trim());
                start = i + 1;
            }
            (None, _) => {}
        }
    }
    items.push(body[start..].trim());
    items
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
        // SQLite refuses to drop an indexed column: drop its generated index first.
        let (columns, indexed): (Vec<String>, Vec<String>) = if fields.is_empty() {
            (vec![column.to_owned()], vec![column.to_owned()])
        } else {
            let columns = fields.iter().flat_map(|f| f.columns()).map(|(name, _)| name).collect();
            let indexed = fields.iter().filter(|f| f.unique || f.target.is_some()).map(|f| f.name.clone()).collect();
            (columns, indexed)
        };
        let mut sql = String::new();
        for column in indexed {
            writeln!(sql, "DROP INDEX IF EXISTS index_{table}_on_{column};").expect("writing to a String");
        }
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
        if field.is_attachment() && !field.optional {
            return Err(CliError::new(format!("`{}` must be optional when added to an existing table", field.name))
                .hint("existing rows have no file: use `name:attachment?`"));
        }
        // Existing rows need a value for NOT NULL columns.
        let default = match (&field.enumeration, field.ty) {
            _ if field.optional || matches!(field.ty, FieldType::Boolean | FieldType::LockVersion) => String::new(),
            (Some(enumeration), _) => format!(" DEFAULT '{}'", enumeration.values[0]),
            _ if field.ty.is_textual() => " DEFAULT ''".to_owned(),
            (_, FieldType::Json) => " DEFAULT '{}'".to_owned(),
            _ => " DEFAULT 0".to_owned(),
        };
        for column in field.sql_columns() {
            writeln!(sql, "ALTER TABLE {table} ADD COLUMN {column}{default};").expect("writing to a String");
        }
    }
    for field in fields {
        sql.push_str(&index_sql(table, field));
    }
    Ok(sql)
}

#[cfg(test)]
#[path = "../../tests/generate/migration.rs"]
mod tests;
