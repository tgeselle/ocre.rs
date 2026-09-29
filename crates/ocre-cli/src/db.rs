//! Database commands on the app's D1 database: migration status, seeds,
//! local reset and ad-hoc SQL. All of them go through wrangler.

use std::fmt;

use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_json::Value;

use crate::{
    CliResult,
    output::{CliError, Report},
    project::Project,
    wrangler::{Echo, Wrangler, target},
};

/// Seed data, relative to the app root.
pub const SEEDS: &str = "db/seeds.sql";

/// Schema dump written by `ocre db schema`, relative to the app root.
pub const SCHEMA: &str = "db/schema.sql";

/// Every table, index, view and trigger of the app, without SQLite's and
/// D1's own (`sqlite_*`, `_cf_*`, `d1_migrations`).
const SCHEMA_QUERY: &str = "SELECT sql FROM sqlite_master WHERE sql IS NOT NULL \
     AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND name NOT LIKE '\\_cf\\_%' ESCAPE '\\' AND name != 'd1_migrations' \
     ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 ELSE 2 END, tbl_name, name";

/// Wrangler's local D1 databases, relative to the app root.
pub(crate) const LOCAL_STATE: &str = ".wrangler/state/v3/d1";

/// `ocre migrate --status`: wrangler's table on screen, pending names in the report.
pub fn status(remote: bool, json: bool) -> CliResult {
    let project = Project::find()?;
    let output = Wrangler::new(&project.root, Echo::for_json(json)).run(&[
        "d1",
        "migrations",
        "list",
        &project.database_name,
        target(remote),
    ])?;
    let pending = pending_migrations(&output);
    let next = match (pending.is_empty(), remote) {
        (true, _) => vec![],
        (false, false) => vec!["ocre migrate".to_owned()],
        (false, true) => vec!["ocre migrate --remote".to_owned()],
    };
    Ok(Report { pending, next, remote, ..Report::new("migrate") })
}

pub fn seed(remote: bool, json: bool) -> CliResult {
    let project = Project::find()?;
    if !project.root.join(SEEDS).is_file() {
        return Err(CliError::new(format!("{SEEDS} not found in the app"))
            .hint(format!("create {SEEDS} with INSERT statements, then run `ocre db seed`")));
    }
    let step = load_seeds(&Wrangler::new(&project.root, Echo::for_json(json)), &project.database_name, remote)?;
    Ok(Report { ran: vec![step], remote, ..Report::new("db seed") })
}

/// Local only: drops the local database, re-applies every migration, then
/// loads the seeds when the app has them.
pub fn reset(json: bool) -> CliResult {
    let project = Project::find()?;
    let wrangler = Wrangler::new(&project.root, Echo::for_json(json));
    let mut ran = Vec::new();
    let state = project.root.join(LOCAL_STATE);
    if state.exists() {
        std::fs::remove_dir_all(&state)?;
        ran.push(format!("deleted {LOCAL_STATE}"));
    }
    wrangler.migrate(&project.database_name, false)?;
    ran.push("applied migrations (--local)".to_owned());
    if project.root.join(SEEDS).is_file() {
        ran.push(load_seeds(&wrangler, &project.database_name, false)?);
    }
    Ok(Report { ran, ..Report::new("db reset") })
}

/// `--yes` answers wrangler's "database unavailable during import" prompt,
/// which would otherwise hang agents on remote imports.
pub(crate) fn load_seeds(wrangler: &Wrangler, database: &str, remote: bool) -> Result<String, CliError> {
    wrangler.run(&["d1", "execute", database, "--file", SEEDS, target(remote), "--yes"])?;
    Ok(format!("loaded {SEEDS} ({})", target(remote)))
}

/// Runs `query`; human mode prints each statement's rows as a table.
pub fn sql(query: &str, remote: bool, json: bool) -> CliResult {
    let project = Project::find()?;
    // Captured: stdout is wrangler's JSON, rendered below; failures include it.
    let output = Wrangler::new(&project.root, Echo::Capture).run(&[
        "d1",
        "execute",
        &project.database_name,
        "--command",
        query,
        target(remote),
        "--json",
    ])?;
    let unexpected =
        |err: serde_json::Error| CliError::new(format!("unexpected `wrangler d1 execute --json` output: {err}"));
    let statements: Vec<Statement> = serde_json::from_str(&output).map_err(unexpected)?;
    if !json {
        print!("{}", render(&statements));
    }
    let rows = serde_json::from_str(&output).map_err(unexpected)?;
    Ok(Report { rows: Some(rows), remote, ..Report::new("sql") })
}

/// `ocre db schema`: the database's current `CREATE` statements in
/// `db/schema.sql` (Rails' `structure.sql`), for reading and for
/// `ocre g migration rebuild_<table>`. Migrations stay the source of truth.
pub fn schema(remote: bool, _json: bool) -> CliResult {
    let project = Project::find()?;
    let output = Wrangler::new(&project.root, Echo::Capture).run(&[
        "d1",
        "execute",
        &project.database_name,
        "--command",
        SCHEMA_QUERY,
        target(remote),
        "--json",
    ])?;
    let statements: Vec<Statement> = serde_json::from_str(&output)
        .map_err(|err| CliError::new(format!("unexpected `wrangler d1 execute --json` output: {err}")))?;
    let path = project.root.join(SCHEMA);
    let existed = path.exists();
    std::fs::create_dir_all(project.root.join("db"))?;
    std::fs::write(&path, schema_sql(&statements, remote))?;
    let (created, updated) =
        if existed { (vec![], vec![SCHEMA.to_owned()]) } else { (vec![SCHEMA.to_owned()], vec![]) };
    Ok(Report { created, updated, remote, ..Report::new("db schema") })
}

/// The dump: a header, then one statement per paragraph.
fn schema_sql(statements: &[Statement], remote: bool) -> String {
    let source = if remote { "remote" } else { "local" };
    let mut out = format!(
        "-- Schema of the {source} D1 database, written by `ocre db schema` from sqlite_master.\n\
         -- A snapshot for reading: migrations/ are the source of truth. Run `ocre db schema` again after `ocre migrate`.\n"
    );
    for row in statements.iter().flat_map(|statement| &statement.results) {
        if let Some((_, Value::String(sql))) = row.0.first() {
            out.push('\n');
            out.push_str(sql);
            out.push_str(";\n");
        }
    }
    out
}

/// Names from the "Migrations to be applied" table of `wrangler d1 migrations list`.
pub(crate) fn pending_migrations(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.trim().strip_prefix('│')?.strip_suffix('│'))
        .map(str::trim)
        .filter(|name| name.ends_with(".sql"))
        .map(str::to_owned)
        .collect()
}

/// One statement's result in `wrangler d1 execute --json`.
#[derive(Deserialize)]
struct Statement {
    #[serde(default)]
    results: Vec<Row>,
}

/// Columns in query order (a `serde_json::Map` would sort them by name).
struct Row(Vec<(String, Value)>);

impl<'de> Deserialize<'de> for Row {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowVisitor;

        impl<'de> Visitor<'de> for RowVisitor {
            type Value = Row;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a result row object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Row, A::Error> {
                let mut columns = Vec::new();
                while let Some(column) = map.next_entry()? {
                    columns.push(column);
                }
                Ok(Row(columns))
            }
        }

        deserializer.deserialize_map(RowVisitor)
    }
}

/// One table per statement, each followed by its row count.
fn render(statements: &[Statement]) -> String {
    statements.iter().map(|statement| table(&statement.results)).collect::<Vec<_>>().join("\n")
}

fn table(rows: &[Row]) -> String {
    let count = match rows.len() {
        1 => "(1 row)\n".to_owned(),
        n => format!("({n} rows)\n"),
    };
    let Some(first) = rows.first() else {
        return count;
    };
    let header: Vec<String> = first.0.iter().map(|(name, _)| name.clone()).collect();
    let cells: Vec<Vec<String>> = rows.iter().map(|row| row.0.iter().map(|(_, value)| cell(value)).collect()).collect();
    let widths: Vec<usize> = (0..header.len())
        .map(|i| {
            std::iter::once(&header)
                .chain(&cells)
                .filter_map(|line| line.get(i))
                .map(|text| text.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |values: &[String]| {
        let padded: Vec<String> = values.iter().zip(&widths).map(|(value, width)| format!("{value:width$}")).collect();
        format!("{}\n", padded.join(" | ").trim_end())
    };
    let rule: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    let mut out = line(&header);
    out.push_str(&format!("{}\n", rule.join("-+-")));
    for row in &cells {
        out.push_str(&line(row));
    }
    out.push_str(&count);
    out
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_owned(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
#[path = "../tests/db.rs"]
mod tests;
