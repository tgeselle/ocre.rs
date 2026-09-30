//! Many rows in one D1 statement (Rails' `insert_all`, `upsert_all`, and an
//! `update_all` with a value per row).
//!
//! The rows travel as one JSON array bound to a single parameter, and SQLite
//! unpacks it with `json_each`, so a thousand rows cost one query instead of
//! a thousand. It matters on Workers: the free plan allows 50 D1 queries
//! per invocation, and D1 binds at most 100 parameters per statement.
//!
//! ```no_run
//! use ocre::{Ctx, Result, bulk};
//! use serde::Serialize;
//!
//! #[derive(Serialize)]
//! struct Track {
//!     title: String,
//!     plays: i64,
//! }
//!
//! async fn import(ctx: &Ctx, tracks: &[Track]) -> Result<usize> {
//!     let insert = bulk::insert("tracks", &["title", "plays"], tracks)?;
//!     ctx.db()?.execute(&insert.sql, insert.params).await
//! }
//! ```
//!
//! Each builder returns a [`Statement`]: run it with `db.execute`, or put
//! several in one `db.batch` to apply them together. Rows are any serde
//! value serializing to an object (a struct, a map, `json!({...})`); the
//! listed columns are read from each one (a missing key is `NULL`), other
//! keys are ignored. Values are read with `->>`, so strings, numbers and
//! `NULL` are stored as SQLite values, booleans as `1`/`0`, and nested
//! arrays or objects as their JSON text (for `json` columns).
//!
//! # Limits
//!
//! A D1 statement and its parameters are capped (100 KB of SQL; a bound
//! string may be larger, but the whole request is limited too): send a few
//! thousand small rows per statement, `rows.chunks(500)` for larger sets.

use serde::Serialize;
use serde_json::Value;

use crate::{Error, IntoParam, Result, Statement};

/// `INSERT INTO table (columns) SELECT ... FROM json_each(?1)`: every row in one statement.
///
/// # Errors
///
/// [`Error::Internal`] when a name is not a plain SQL identifier, `columns`
/// is empty, or a row does not serialize to a JSON object.
///
/// # Examples
///
/// ```
/// use ocre::{bulk, serde_json::json};
///
/// let rows = [json!({"title": "A", "plays": 1}), json!({"title": "B", "plays": 2})];
/// let insert = bulk::insert("tracks", &["title", "plays"], &rows).unwrap();
/// assert_eq!(
///     insert.sql,
///     "INSERT INTO tracks (title, plays) SELECT value ->> '$.title', value ->> '$.plays' FROM json_each(?1)"
/// );
/// assert_eq!(insert.params.len(), 1);
/// ```
pub fn insert<T: Serialize>(table: &str, columns: &[&str], rows: &[T]) -> Result<Statement> {
    let (list, values) = columns_and_values(table, columns)?;
    let sql = format!("INSERT INTO {table} ({list}) SELECT {values} FROM json_each(?1)");
    Ok(Statement::new(sql, vec![rows_param(serde_json::to_value(rows))?]))
}

/// `INSERT ... ON CONFLICT (key) DO UPDATE SET ...`: inserts the rows, and
/// updates the listed columns of those whose `key` already exists (Rails'
/// `upsert_all`). `key` must have a unique index; it is one of `columns`.
///
/// # Errors
///
/// As [`insert`], and when `key` is not one of `columns`.
///
/// # Examples
///
/// ```
/// use ocre::{bulk, serde_json::json};
///
/// let rows = [json!({"isrc": "FR1", "plays": 10})];
/// let upsert = bulk::upsert("tracks", "isrc", &["isrc", "plays"], &rows).unwrap();
/// assert!(upsert.sql.ends_with("FROM json_each(?1) WHERE true ON CONFLICT (isrc) DO UPDATE SET plays = excluded.plays"));
/// ```
pub fn upsert<T: Serialize>(table: &str, key: &str, columns: &[&str], rows: &[T]) -> Result<Statement> {
    let (list, values) = columns_and_values(table, columns)?;
    if !columns.contains(&key) {
        return Err(Error::internal(format!("bulk::upsert: the key `{key}` must be one of the columns")));
    }
    let sets: Vec<String> =
        columns.iter().filter(|column| **column != key).map(|column| format!("{column} = excluded.{column}")).collect();
    let action = if sets.is_empty() { "NOTHING".to_owned() } else { format!("UPDATE SET {}", sets.join(", ")) };
    // `WHERE true` lets SQLite tell the upsert clause from a join condition.
    let sql = format!(
        "INSERT INTO {table} ({list}) SELECT {values} FROM json_each(?1) WHERE true ON CONFLICT ({key}) DO {action}"
    );
    Ok(Statement::new(sql, vec![rows_param(serde_json::to_value(rows))?]))
}

/// `UPDATE table SET ... FROM json_each(?1) WHERE table.key = ...`: a value
/// per row for many rows in one statement. Each row holds `key` (usually
/// `id`) and the `columns` to set; `updated_at` is set to now when `touch`.
///
/// # Errors
///
/// As [`insert`].
///
/// # Examples
///
/// ```
/// use ocre::{bulk, serde_json::json};
///
/// let rows = [json!({"id": 1, "plays": 11}), json!({"id": 2, "plays": 3})];
/// let update = bulk::update("tracks", "id", &["plays"], &rows, true).unwrap();
/// assert_eq!(
///     update.sql,
///     "UPDATE tracks SET plays = row.value ->> '$.plays', updated_at = datetime('now') \
///      FROM json_each(?1) AS row WHERE tracks.id = row.value ->> '$.id'"
/// );
/// ```
pub fn update<T: Serialize>(table: &str, key: &str, columns: &[&str], rows: &[T], touch: bool) -> Result<Statement> {
    columns_and_values(table, columns)?;
    identifier(key)?;
    let mut sets: Vec<String> = columns.iter().map(|column| format!("{column} = row.value ->> '$.{column}'")).collect();
    if touch {
        sets.push("updated_at = datetime('now')".to_owned());
    }
    let sql = format!(
        "UPDATE {table} SET {} FROM json_each(?1) AS row WHERE {table}.{key} = row.value ->> '$.{key}'",
        sets.join(", ")
    );
    Ok(Statement::new(sql, vec![rows_param(serde_json::to_value(rows))?]))
}

/// The column list and the `value ->> '$.column'` expressions, names checked.
fn columns_and_values(table: &str, columns: &[&str]) -> Result<(String, String)> {
    identifier(table)?;
    if columns.is_empty() {
        return Err(Error::internal("bulk: no columns to write"));
    }
    for column in columns {
        identifier(column)?;
    }
    let values: Vec<String> = columns.iter().map(|column| format!("value ->> '$.{column}'")).collect();
    Ok((columns.join(", "), values.join(", ")))
}

/// Names are written into the SQL: only plain identifiers, never user input.
fn identifier(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid { Ok(()) } else { Err(Error::internal(format!("bulk: `{name}` is not a plain SQL identifier"))) }
}

/// The rows (`serde_json::to_value(rows)`) as one JSON array parameter; not
/// generic, so each row type adds only the serialization.
fn rows_param(rows: serde_json::Result<Value>) -> Result<crate::Param> {
    let rows = rows.map_err(|err| Error::internal(format!("bulk: rows do not serialize: {err}")))?;
    let objects = rows.as_array().is_some_and(|rows| rows.iter().all(Value::is_object));
    if !objects {
        return Err(Error::internal("bulk: each row must serialize to a JSON object (a struct or a map)"));
    }
    Ok(rows.to_string().into_param())
}

#[cfg(test)]
#[path = "../tests/bulk.rs"]
mod tests;
