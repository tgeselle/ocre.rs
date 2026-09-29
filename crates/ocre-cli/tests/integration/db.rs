//! `ocre migrate --status`, `ocre db seed|reset` and `ocre sql` against a fake wrangler.

#[path = "../support/mod.rs"]
mod support;

use std::fs;

use serde_json::json;
use support::{Sandbox, text};

fn write_seeds(root: &std::path::Path) {
    fs::create_dir_all(root.join("db")).unwrap();
    fs::write(root.join("db/seeds.sql"), "INSERT INTO posts (title) VALUES ('Hello');\n").unwrap();
}

// ---------- ocre migrate --status ----------

#[test]
fn migrate_status_shows_the_table_and_reports_pending_migrations() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.write_state("pending_migrations", "0001_create_posts.sql\n0002_add_slug.sql\n");
    let output = sandbox.ocre(&["migrate", "--status"], &root);
    let (stdout, _) = text(&output);
    assert!(output.status.success());
    assert!(stdout.contains("│ 0002_add_slug.sql │\n"), "wrangler's table is shown: {stdout}");
    assert!(stdout.ends_with("└───────────┘\n\nNext:\n  ocre migrate\n"), "{stdout}");

    let (report, ok) = sandbox.json(&["migrate", "--status", "--remote"], &root);
    assert!(ok, "{report}");
    assert_eq!(
        report,
        json!({
            "ok": true,
            "command": "migrate",
            "pending": ["0001_create_posts.sql", "0002_add_slug.sql"],
            "remote": true,
            "next": ["ocre migrate --remote"],
        })
    );
    assert_eq!(sandbox.calls(), ["d1 migrations list shop --local", "d1 migrations list shop --remote"]);
}

#[test]
fn migrate_status_with_nothing_pending_suggests_nothing() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["migrate", "--status"], &root);
    assert!(ok, "{report}");
    assert_eq!(report, json!({ "ok": true, "command": "migrate" }));
}

#[test]
fn migrate_status_failure_points_at_wrangler_output() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("migrations_list_fails");
    let (report, ok) = sandbox.json(&["migrate", "--status"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "`wrangler d1 migrations list shop --local` failed (exit status: 1)");
    assert!(report["hint"].as_str().unwrap().contains("wrangler output"));
}

// ---------- ocre db seed ----------

#[test]
fn db_seed_runs_the_seeds_file_locally_or_remotely() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    write_seeds(&root);
    let output = sandbox.ocre(&["db", "seed"], &root.join("src"));
    assert_eq!(text(&output).0, "Executed db/seeds.sql on shop (--local)\n  loaded db/seeds.sql (--local)\n");

    let (report, ok) = sandbox.json(&["db", "seed", "--remote"], &root);
    assert!(ok, "{report}");
    assert_eq!(
        report,
        json!({ "ok": true, "command": "db seed", "ran": ["loaded db/seeds.sql (--remote)"], "remote": true })
    );
    let output = sandbox.ocre(&["db", "seed", "--remote"], &root);
    assert!(text(&output).0.ends_with("Target: remote D1 database on Cloudflare\n"));
    assert_eq!(
        sandbox.calls()[..2],
        ["d1 execute shop --file db/seeds.sql --local --yes", "d1 execute shop --file db/seeds.sql --remote --yes"]
    );
}

#[test]
fn db_seed_without_a_seeds_file_names_the_fix() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["db", "seed"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "db/seeds.sql not found in the app");
    assert_eq!(report["hint"], "create db/seeds.sql with INSERT statements, then run `ocre db seed`");
    assert!(sandbox.calls().is_empty());
}

#[test]
fn db_seed_failure_points_at_wrangler_output() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    write_seeds(&root);
    sandbox.set("execute_fails");
    let (report, ok) = sandbox.json(&["db", "seed"], &root);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with("`wrangler d1 execute shop --file db/seeds.sql"));
}

// ---------- ocre db reset ----------

#[test]
fn db_reset_deletes_the_local_database_then_migrates_and_seeds() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    write_seeds(&root);
    let state = root.join(".wrangler/state/v3/d1/miniflare-D1DatabaseObject");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("db.sqlite"), "old").unwrap();
    fs::write(root.join(".wrangler/state/v3/keep"), "other state").unwrap();

    let (report, ok) = sandbox.json(&["db", "reset"], &root);
    assert!(ok, "{report}");
    assert_eq!(
        report["ran"],
        json!(["deleted .wrangler/state/v3/d1", "applied migrations (--local)", "loaded db/seeds.sql (--local)"])
    );
    assert!(report.get("remote").is_none());
    assert!(!root.join(".wrangler/state/v3/d1").exists());
    assert!(root.join(".wrangler/state/v3/keep").exists(), "only the D1 state is deleted");
    assert_eq!(
        sandbox.calls(),
        ["d1 migrations apply shop --local", "d1 execute shop --file db/seeds.sql --local --yes"]
    );
}

#[test]
fn db_reset_without_local_state_or_seeds_only_migrates() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let output = sandbox.ocre(&["db", "reset"], &root);
    assert_eq!(text(&output).0, "Migrations applied to shop (--local)\n  applied migrations (--local)\n");
    assert_eq!(sandbox.calls(), ["d1 migrations apply shop --local"]);
}

#[test]
fn db_reset_is_local_only() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let output = sandbox.ocre(&["db", "reset", "--remote"], &root);
    assert!(!output.status.success());
    assert!(text(&output).1.contains("unexpected argument '--remote'"));
    assert!(sandbox.calls().is_empty());
}

// ---------- ocre sql ----------

const ROWS: &str =
    r#"[{"results":[{"title":"Hello","id":1},{"title":null,"id":2}],"success":true,"meta":{"duration":0}}]"#;

#[test]
fn sql_prints_rows_as_a_table() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.write_state("execute.json", ROWS);
    let output = sandbox.ocre(&["sql", "SELECT title, id FROM posts"], &root);
    assert!(output.status.success());
    assert_eq!(text(&output).0, "title | id\n------+---\nHello | 1\nNULL  | 2\n(2 rows)\n");
    assert_eq!(sandbox.calls(), ["d1 execute shop --command SELECT title, id FROM posts --local --json"]);
}

#[test]
fn sql_json_returns_wrangler_results_and_flags_remote() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.write_state("execute.json", ROWS);
    let (report, ok) = sandbox.json(&["sql", "SELECT 1", "--remote"], &root);
    assert!(ok, "{report}");
    assert_eq!(
        report,
        json!({
            "ok": true,
            "command": "sql",
            "rows": serde_json::from_str::<serde_json::Value>(ROWS).unwrap(),
            "remote": true,
        })
    );
    let output = sandbox.ocre(&["sql", "DELETE FROM posts", "--remote"], &root);
    assert!(text(&output).0.ends_with("(2 rows)\nTarget: remote D1 database on Cloudflare\n"));
    assert_eq!(sandbox.calls()[0], "d1 execute shop --command SELECT 1 --remote --json");
}

#[test]
fn sql_failure_includes_wrangler_error() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("execute_fails");
    let (report, ok) = sandbox.json(&["sql", "SELECT nope"], &root);
    assert!(!ok);
    let error = report["error"].as_str().unwrap();
    assert!(error.starts_with("`wrangler d1 execute shop --command SELECT nope --local --json` failed"), "{error}");
    assert!(error.contains("[ERROR] execute_fails"), "{error}");
}

#[test]
fn sql_rejects_unexpected_wrangler_output() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.write_state("execute.json", "Not JSON");
    let (report, ok) = sandbox.json(&["sql", "SELECT 1"], &root);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with("unexpected `wrangler d1 execute --json` output"));
}

// ---------- ocre db schema ----------

const SCHEMA_ROWS: &str = r#"[{"results":[{"sql":"CREATE TABLE posts (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    title TEXT\n)"},{"sql":"CREATE INDEX index_posts_on_title ON posts (title)"},{"sql":null}],"success":true}]"#;

#[test]
fn db_schema_dumps_create_statements_for_rebuild_migrations() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.write_state("execute.json", SCHEMA_ROWS);
    let (report, ok) = sandbox.json(&["db", "schema"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["created"], json!(["db/schema.sql"]));
    let call = &sandbox.calls()[0];
    assert!(
        call.starts_with("d1 execute shop --command SELECT sql FROM sqlite_master WHERE sql IS NOT NULL"),
        "{call}"
    );
    assert!(call.ends_with("--local --json"), "{call}");
    let schema = fs::read_to_string(root.join("db/schema.sql")).unwrap();
    assert!(schema.starts_with("-- Schema of the local D1 database, written by `ocre db schema`"), "{schema}");
    assert!(
        schema.ends_with(
            "\n\nCREATE TABLE posts (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    title TEXT\n);\n\n\
             CREATE INDEX index_posts_on_title ON posts (title);\n"
        ),
        "{schema}"
    );

    let (report, ok) = sandbox.json(&["g", "migration", "rebuild_posts"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["next"], json!(["ocre migrate", "update the model in src/models/ to match the new columns"]));
    let migration = fs::read_to_string(root.join("migrations/0001_rebuild_posts.sql")).unwrap();
    assert!(migration.contains("INSERT INTO posts_new (id, title) SELECT id, title FROM posts;\n"), "{migration}");
    assert!(migration.contains("CREATE INDEX index_posts_on_title ON posts (title);\n"), "{migration}");

    let (report, ok) = sandbox.json(&["db", "schema", "--remote"], &root);
    assert!(ok, "{report}");
    assert_eq!((&report["updated"], &report["remote"]), (&json!(["db/schema.sql"]), &json!(true)));
    let schema = fs::read_to_string(root.join("db/schema.sql")).unwrap();
    assert!(schema.starts_with("-- Schema of the remote D1 database"), "{schema}");

    sandbox.write_state("execute.json", "not json");
    let (report, ok) = sandbox.json(&["db", "schema"], &root);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with("unexpected `wrangler d1 execute --json` output"));
}

#[test]
fn migrations_rename_drop_and_index_from_their_name() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "migration", "add_index_to_posts", "author_id", "created_at"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["next"], json!(["ocre migrate"]));
    let sql = fs::read_to_string(root.join("migrations/0001_add_index_to_posts.sql")).unwrap();
    assert!(sql.ends_with("CREATE INDEX index_posts_on_author_id_and_created_at ON posts (author_id, created_at);\n"));
    let (report, _) = sandbox.json(&["g", "migration", "rebuild_posts"], &root);
    assert_eq!(report["error"], "db/schema.sql not found");
    let (report, ok) = sandbox.json(&["g", "migration", "rename_title_to_headline_in_posts"], &root);
    assert!(ok, "{report}");
    let sql = fs::read_to_string(root.join("migrations/0002_rename_title_to_headline_in_posts.sql")).unwrap();
    assert!(sql.ends_with("ALTER TABLE posts RENAME COLUMN title TO headline;\n"), "{sql}");
}
