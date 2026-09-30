//! Fixtures: Rails-style YAML (or JSON) files of named rows, one file per
//! table, loaded by `ocre db seed` (from `db/fixtures`) and `ocre test --e2e`,
//! written by `ocre db dump`.
//!
//! ```yaml
//! # db/fixtures/posts.yml
//! DEFAULTS: &defaults
//!   published: true
//! hello:
//!   <<: *defaults
//!   title: Hello $LABEL   # $LABEL: the row's label, "hello"
//!   author: ada           # author_id = the id of the fixture labelled `ada`
//! ```
//!
//! Loading turns the files into plain SQL (no database introspection): each
//! fixture table is emptied, then gets one `INSERT` per row. A row's id is
//! its `id` value, else Rails' `identify(label)`, stable across runs.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use serde_json::Value;
use yaml_rust2::{Yaml, YamlLoader};

use crate::{names::pluralize, output::CliError};

/// Default fixture directory, relative to the app root (Loco's `src/fixtures`).
pub const FIXTURES: &str = "db/fixtures";

/// The label of the row holding shared values (`<<: *defaults`), never inserted.
const DEFAULTS: &str = "DEFAULTS";

/// The fixtures of `dir` (relative to `project_root`) as SQL: foreign keys
/// deferred, every fixture table emptied, then one `INSERT` per row with
/// explicit columns. A missing directory gives an empty string.
pub fn to_sql(project_root: &Path, dir: &str) -> Result<String, CliError> {
    Ok(load(project_root, dir)?.sql)
}

/// SQL of a fixture directory, with the tables it fills.
pub struct Fixtures {
    /// Table names, in file-name order.
    pub tables: Vec<String>,
    /// Empty when there are no fixture files.
    pub sql: String,
}

/// [`to_sql`], keeping the table names for reports.
pub fn load(project_root: &Path, dir: &str) -> Result<Fixtures, CliError> {
    let path = project_root.join(dir);
    if !path.is_dir() {
        return Ok(Fixtures { tables: Vec::new(), sql: String::new() });
    }
    let mut files: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(&path)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if let Some((table, "yml" | "yaml" | "json")) = name.rsplit_once('.') {
            files.push((name.clone(), table.to_owned()));
        }
    }
    files.sort();
    let mut tables = Vec::new();
    for (name, table) in files {
        let file = format!("{dir}/{name}");
        let source = std::fs::read_to_string(path.join(&name))?;
        tables.push(parse(&file, table, &source)?);
    }
    let sql = if tables.is_empty() { String::new() } else { render(&tables, &foreign_keys(project_root)?)? };
    Ok(Fixtures { tables: tables.into_iter().map(|table| table.name).collect(), sql })
}

/// Rails' `ActiveRecord::FixtureSet.identify`: the id of a row labelled
/// `label` without an explicit one, `crc32(label) % (2^30 - 1)`.
pub fn identify(label: &str) -> u32 {
    crc32(label.as_bytes()) % ((1 << 30) - 1)
}

/// CRC-32 (IEEE, zlib's), bit by bit: labels are short.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

struct Table {
    name: String,
    file: String,
    rows: Vec<Row>,
}

struct Row {
    label: String,
    /// SQL literal.
    id: String,
    /// Without `id`, `<<` merged, `$LABEL` replaced.
    columns: Vec<(String, Yaml)>,
}

fn parse(file: &str, table: String, source: &str) -> Result<Table, CliError> {
    let docs = YamlLoader::load_from_str(source).map_err(|err| {
        CliError::new(format!("{file}: invalid YAML: {err}"))
            .hint("a fixture file maps labels to rows of `column: value` (YAML or JSON)")
    })?;
    let mut rows = Vec::new();
    let top = match docs.into_iter().next() {
        None | Some(Yaml::Null) => return Ok(Table { name: table, file: file.to_owned(), rows }),
        Some(Yaml::Hash(top)) => top,
        Some(_) => {
            return Err(CliError::new(format!("{file}: expected a mapping of labels to rows"))
                .hint("write one `label:` per row, its columns indented below it"));
        }
    };
    for (label, value) in top {
        let Yaml::String(label) = label else {
            return Err(CliError::new(format!("{file}: labels must be strings, found {label:?}"))
                .hint("quote the label, e.g. `\"1\":`"));
        };
        if label == DEFAULTS {
            continue;
        }
        let at = |err: CliError| CliError { message: format!("{file}: `{label}`: {}", err.message), hint: err.hint };
        let mut id = identify(&label).to_string();
        let mut columns = Vec::new();
        for (column, value) in row_columns(value).map_err(at)? {
            let value = match value {
                Yaml::String(text) => Yaml::String(text.replace("$LABEL", &label)),
                other => other,
            };
            if column == "id" {
                id = sql_value(&value).map_err(at)?;
            } else {
                columns.push((column, value));
            }
        }
        rows.push(Row { label, id, columns });
    }
    Ok(Table { name: table, file: file.to_owned(), rows })
}

/// The columns of a row: its own, then those merged with `<<` (a mapping or
/// a list of them, earlier ones first) that it does not set itself.
fn row_columns(row: Yaml) -> Result<Vec<(String, Yaml)>, CliError> {
    let hash = match row {
        Yaml::Null => return Ok(Vec::new()),
        Yaml::Hash(hash) => hash,
        _ => {
            return Err(
                CliError::new("expected a mapping of `column: value`").hint("indent the row's columns below its label")
            );
        }
    };
    let mut own = Vec::new();
    let mut merged = Vec::new();
    for (column, value) in hash {
        let Yaml::String(column) = column else {
            return Err(
                CliError::new(format!("column names must be strings, found {column:?}")).hint("quote the column name")
            );
        };
        if column != "<<" {
            own.push((column, value));
            continue;
        }
        let sources = match value {
            Yaml::Array(sources) => sources,
            source => vec![source],
        };
        for source in sources {
            if !matches!(source, Yaml::Hash(_)) {
                return Err(CliError::new("`<<` merges a mapping or a list of mappings")
                    .hint("merge an anchored row, e.g. `<<: *defaults`"));
            }
            merged.extend(row_columns(source)?);
        }
    }
    for (column, value) in merged {
        if !own.iter().any(|(name, _)| *name == column) {
            own.push((column, value));
        }
    }
    Ok(own)
}

/// Every `<word>_id` of `migrations/*.sql`: `author: ada` becomes
/// `author_id = <id of ada>` when `author_id` is one.
fn foreign_keys(project_root: &Path) -> Result<HashSet<String>, CliError> {
    let mut keys = HashSet::new();
    let Ok(entries) = std::fs::read_dir(project_root.join("migrations")) else {
        return Ok(keys);
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "sql") {
            let sql = std::fs::read_to_string(&path)?;
            let words = sql.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
            keys.extend(words.filter(|word| word.ends_with("_id")).map(str::to_owned));
        }
    }
    Ok(keys)
}

fn render(tables: &[Table], foreign_keys: &HashSet<String>) -> Result<String, CliError> {
    let mut labels: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    for table in tables {
        for row in &table.rows {
            labels.entry(&row.label).or_default().push((&table.name, &row.id));
        }
    }
    let mut sql = String::from("PRAGMA defer_foreign_keys = on;\n");
    for table in tables {
        sql.push_str(&format!("DELETE FROM {};\n", ident(&table.name)));
    }
    for table in tables {
        for row in &table.rows {
            let at = |message: String| format!("{}: `{}`: {message}", table.file, row.label);
            let mut columns = vec![ident("id")];
            let mut values = vec![row.id.clone()];
            for (column, value) in &row.columns {
                let key = format!("{column}_id");
                if !foreign_keys.contains(&key) {
                    columns.push(ident(column));
                    values.push(sql_value(value).map_err(|err| CliError { message: at(err.message), ..err })?);
                    continue;
                }
                let Yaml::String(target) = value else {
                    return Err(CliError::new(at(format!("`{column}` names a fixture label (`{key}` is a column)")))
                        .hint(format!("write `{column}: <label>`, or set `{key}` directly")));
                };
                let found = labels.get(target.as_str()).map(Vec::as_slice).unwrap_or_default();
                let plural = pluralize(column);
                let id = match found.iter().find(|(name, _)| *name == plural) {
                    Some((_, id)) => id,
                    None => match found {
                        [(_, id)] => id,
                        [] => {
                            return Err(CliError::new(at(format!("no fixture labelled `{target}` for `{column}`")))
                                .hint(format!("add a `{target}:` row to a fixture file, or set `{key}` to an id")));
                        }
                        _ => {
                            return Err(CliError::new(at(format!(
                                "`{target}` labels rows of several tables, none of them `{plural}`"
                            )))
                            .hint(format!("set `{key}` to the id instead")));
                        }
                    },
                };
                columns.push(ident(&key));
                values.push((*id).to_owned());
            }
            sql.push_str(&format!(
                "INSERT INTO {} ({}) VALUES ({});\n",
                ident(&table.name),
                columns.join(", "),
                values.join(", ")
            ));
        }
    }
    Ok(sql)
}

/// A double-quoted SQL identifier.
pub(crate) fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A single-quoted SQL string.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The SQL literal of a YAML value: sequences and mappings as JSON text.
fn sql_value(value: &Yaml) -> Result<String, CliError> {
    Ok(match value {
        Yaml::String(text) => quote(text),
        Yaml::Integer(number) => number.to_string(),
        Yaml::Real(number) => number.clone(),
        Yaml::Boolean(flag) => u8::from(*flag).to_string(),
        Yaml::Null => "NULL".to_owned(),
        Yaml::Array(_) | Yaml::Hash(_) => quote(&json(value)?.to_string()),
        Yaml::Alias(_) | Yaml::BadValue => return Err(invalid()),
    })
}

fn json(value: &Yaml) -> Result<Value, CliError> {
    Ok(match value {
        Yaml::String(text) => Value::String(text.clone()),
        Yaml::Integer(number) => Value::from(*number),
        Yaml::Real(number) => number.parse::<f64>().map(Value::from).unwrap_or_else(|_| Value::String(number.clone())),
        Yaml::Boolean(flag) => Value::Bool(*flag),
        Yaml::Null => Value::Null,
        Yaml::Array(items) => Value::Array(items.iter().map(json).collect::<Result<_, _>>()?),
        Yaml::Hash(hash) => {
            let mut object = serde_json::Map::new();
            for (key, item) in hash {
                let Yaml::String(key) = key else {
                    return Err(
                        CliError::new(format!("JSON keys must be strings, found {key:?}")).hint("quote the key")
                    );
                };
                object.insert(key.clone(), json(item)?);
            }
            Value::Object(object)
        }
        Yaml::Alias(_) | Yaml::BadValue => return Err(invalid()),
    })
}

fn invalid() -> CliError {
    CliError::new("invalid value").hint("check the value's `!!tag`, or quote it to store it as text")
}

/// A fixture file of `rows` (columns in query order) for `ocre db dump`:
/// labels `<table>_<id>` (`<table>_<n>`, 1-based, without an `id` column),
/// strings double-quoted, every value as D1 returned it.
pub fn to_yaml(table: &str, rows: &[&[(String, Value)]], header: &str) -> String {
    let mut out = format!("# {header}\n");
    for (index, row) in rows.iter().enumerate() {
        let id = row.iter().find(|(column, _)| column == "id").map(|(_, id)| id);
        let label = match id {
            Some(Value::String(id)) => format!("{table}_{id}"),
            Some(id @ Value::Number(_)) => format!("{table}_{id}"),
            _ => format!("{table}_{}", index + 1),
        };
        out.push_str(&format!("{}:\n", key(&label)));
        for (column, value) in row.iter() {
            out.push_str(&format!("  {}: {value}\n", key(column)));
        }
    }
    out
}

/// A mapping key: plain when it reads back as the same string, else JSON-quoted.
fn key(name: &str) -> String {
    let plain = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !["true", "false", "null"].contains(&name.to_ascii_lowercase().as_str());
    if plain { name.to_owned() } else { Value::from(name).to_string() }
}

#[cfg(test)]
#[path = "../tests/fixtures.rs"]
mod tests;
