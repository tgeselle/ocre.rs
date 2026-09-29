//! Whole-database tasks on the app's D1 database: `ocre db create`, `drop`,
//! `version`, `truncate`, `prepare` and `seed --replant`.
//!
//! Destructive tasks (drop, truncate, replant) only touch the local
//! database: Ocre never deletes production data; the Cloudflare dashboard
//! or `npx wrangler d1 delete` does, on purpose.

use serde::Deserialize;

use crate::{
    CliResult,
    db::{LOCAL_STATE, SEEDS, load_seeds},
    output::{CliError, Report},
    project::Project,
    wrangler::{Echo, Wrangler, target},
};

/// App tables: everything but SQLite's and D1's own.
const TABLES_QUERY: &str = "SELECT name FROM sqlite_master WHERE type = 'table' \
     AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND name NOT LIKE '\\_cf\\_%' ESCAPE '\\' AND name != 'd1_migrations' \
     ORDER BY name";

#[derive(Deserialize)]
struct Rows<T> {
    results: Vec<T>,
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

/// Runs one query with `--json` and returns the rows of its single statement.
fn query<T: for<'de> Deserialize<'de>>(
    wrangler: &Wrangler,
    database: &str,
    sql: &str,
    remote: bool,
) -> Result<Vec<T>, CliError> {
    let output = wrangler.run(&["d1", "execute", database, "--command", sql, target(remote), "--json"])?;
    let mut statements: Vec<Rows<T>> = serde_json::from_str(&output)
        .map_err(|err| CliError::new(format!("unexpected `wrangler d1 execute --json` output: {err}")))?;
    Ok(statements.pop().map(|rows| rows.results).unwrap_or_default())
}

fn local_only(task: &str) -> CliError {
    CliError::new(format!("`ocre db {task}` only runs on the local database")).hint(
        "Ocre never deletes production data; use the Cloudflare dashboard or `npx wrangler d1 ...` for that on purpose",
    )
}

/// `ocre db create`: the local database (wrangler creates its file on first
/// use), or with `--remote` the D1 database on Cloudflare when missing.
pub fn create(remote: bool, json: bool) -> CliResult {
    let project = Project::find()?;
    let wrangler = Wrangler::new(&project.root, Echo::for_json(json));
    let database = &project.database_name;
    let mut report = Report { remote, ..Report::new("db create") };
    if remote {
        if wrangler.database_exists(database)? {
            report.ran.push(format!("D1 database {database} already exists"));
        } else {
            wrangler.run(&["d1", "create", database])?;
            report.provisioned.push(format!("D1 database {database}"));
        }
    } else {
        let existed = project.root.join(LOCAL_STATE).exists();
        wrangler.run(&["d1", "execute", database, "--command", "SELECT 1", "--local"])?;
        report.ran.push(if existed {
            format!("local database {database} already exists")
        } else {
            format!("created local database {database}")
        });
        report.next = vec!["ocre migrate".to_owned()];
    }
    Ok(report)
}

/// `ocre db drop`: deletes the local database.
pub fn drop(remote: bool) -> CliResult {
    if remote {
        return Err(local_only("drop"));
    }
    let project = Project::find()?;
    let state = project.root.join(LOCAL_STATE);
    let ran = if state.exists() {
        std::fs::remove_dir_all(&state)?;
        format!("deleted {LOCAL_STATE}")
    } else {
        "no local database to delete".to_owned()
    };
    Ok(Report { ran: vec![ran], next: vec!["ocre db prepare".to_owned()], ..Report::new("db drop") })
}

/// `ocre db version`: the last migration applied, `null` when none is.
pub fn version(remote: bool) -> CliResult {
    let project = Project::find()?;
    let wrangler = Wrangler::new(&project.root, Echo::Capture);
    let sql = "SELECT name FROM d1_migrations ORDER BY id DESC LIMIT 1";
    let version = match query::<Name>(&wrangler, &project.database_name, sql, remote) {
        Ok(rows) => rows.into_iter().next().map(|row| row.name),
        Err(err) if err.message.contains("no such table") => None,
        Err(err) => return Err(err),
    };
    Ok(Report { version: Some(version), remote, ..Report::new("db version") })
}

/// `ocre db truncate`: empties every app table of the local database,
/// keeping the tables and the applied migrations.
pub fn truncate(remote: bool, json: bool) -> CliResult {
    if remote {
        return Err(local_only("truncate"));
    }
    let project = Project::find()?;
    let ran = empty_tables(&project, json)?;
    Ok(Report { ran: vec![ran], ..Report::new("db truncate") })
}

fn empty_tables(project: &Project, json: bool) -> Result<String, CliError> {
    let database = &project.database_name;
    let tables = query::<Name>(&Wrangler::new(&project.root, Echo::Capture), database, TABLES_QUERY, false)?;
    if tables.is_empty() {
        return Ok("no tables to empty".to_owned());
    }
    let sqlite_sequence = "SELECT name FROM sqlite_master WHERE name = 'sqlite_sequence'";
    let has_sequence =
        !query::<Name>(&Wrangler::new(&project.root, Echo::Capture), database, sqlite_sequence, false)?.is_empty();
    // Deferred foreign keys: rows can go in any order within the batch.
    let mut sql = String::from("PRAGMA defer_foreign_keys = on;");
    for table in &tables {
        sql.push_str(&format!(" DELETE FROM \"{}\";", table.name));
    }
    if has_sequence {
        // AUTOINCREMENT counters start again at 1.
        sql.push_str(" DELETE FROM sqlite_sequence;");
    }
    Wrangler::new(&project.root, Echo::for_json(json)).run(&[
        "d1",
        "execute",
        database,
        "--command",
        &sql,
        "--local",
    ])?;
    let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
    Ok(format!("emptied {} (--local)", names.join(", ")))
}

/// `ocre db seed --replant`: empties the local tables, then loads the seeds.
pub fn replant(remote: bool, json: bool) -> CliResult {
    if remote {
        return Err(local_only("seed --replant"));
    }
    let project = Project::find()?;
    if !project.root.join(SEEDS).is_file() {
        return Err(CliError::new(format!("{SEEDS} not found in the app"))
            .hint(format!("create {SEEDS} with INSERT statements, then run `ocre db seed --replant`")));
    }
    let emptied = empty_tables(&project, json)?;
    let seeded = load_seeds(&Wrangler::new(&project.root, Echo::for_json(json)), &project.database_name, false)?;
    Ok(Report { ran: vec![emptied, seeded], ..Report::new("db seed") })
}

/// `ocre db prepare`: safe to run any time. Applies pending migrations to
/// the local database, and loads the seeds when it was just created.
pub fn prepare(json: bool) -> CliResult {
    let project = Project::find()?;
    let wrangler = Wrangler::new(&project.root, Echo::for_json(json));
    let fresh = !project.root.join(LOCAL_STATE).exists();
    wrangler.migrate(&project.database_name, false)?;
    let mut ran = vec!["applied migrations (--local)".to_owned()];
    if fresh && project.root.join(SEEDS).is_file() {
        ran.push(load_seeds(&wrangler, &project.database_name, false)?);
    }
    Ok(Report { ran, ..Report::new("db prepare") })
}
