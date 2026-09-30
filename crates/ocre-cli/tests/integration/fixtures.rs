//! `ocre db seed` with fixture files (`db/fixtures`, `--from`) and
//! `ocre db dump`: the local database through the app's wrangler, the remote
//! one through cf (fakes).

#[path = "../support/mod.rs"]
mod support;

use std::{fs, path::Path};

use serde_json::json;
use support::{Sandbox, local_d1, text};

const FIXTURES_SQL: &str = "d1 execute DB --local --file .wrangler/ocre-fixtures.sql --yes";
const SEEDS_SQL: &str = "d1 execute DB --local --file db/seeds.sql --yes";

fn write(root: &Path, path: &str, contents: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn write_fixtures(root: &Path) {
    write(root, "db/fixtures/users.yml", "ada:\n  name: Ada\n");
    write(root, "db/fixtures/posts.yml", "hello:\n  title: Hello\n");
}

// ---------- ocre db seed ----------

#[test]
fn db_seed_loads_fixtures_then_the_seeds_file() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    write_fixtures(&root);
    let (report, ok) = sandbox.json(&["db", "seed"], &root);
    assert!(ok, "{report}");
    assert_eq!(
        report,
        json!({ "ok": true, "command": "db seed", "ran": ["loaded db/fixtures: posts, users (--local)"] })
    );
    assert_eq!(sandbox.calls(), [local_d1(FIXTURES_SQL)]);
    assert!(!root.join(".wrangler/ocre-fixtures.sql").exists(), "the SQL file is deleted");

    write(&root, "db/seeds.sql", "INSERT INTO posts (title) VALUES ('Seeded');\n");
    sandbox.clear_calls();
    let output = sandbox.ocre(&["db", "seed"], &root);
    assert_eq!(
        text(&output).0,
        "Executed .wrangler/ocre-fixtures.sql on DB (--local)\nExecuted db/seeds.sql on DB (--local)\n  \
         loaded db/fixtures: posts, users (--local)\n  loaded db/seeds.sql (--local)\n"
    );
    assert_eq!(sandbox.calls(), [local_d1(FIXTURES_SQL), local_d1(SEEDS_SQL)]);
}

#[test]
fn db_seed_from_another_directory() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    write(&root, "test/fixtures/posts.json", r#"{"hello": {"title": "Hello"}}"#);
    let (report, ok) = sandbox.json(&["db", "seed", "--from", "test/fixtures"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["ran"], json!(["loaded test/fixtures: posts (--local)"]));

    let (report, ok) = sandbox.json(&["db", "seed", "--from", "nowhere"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "nowhere not found in the app");
    assert_eq!(report["hint"], "pass the directory of the fixture files, relative to the app root");
}

#[test]
fn db_seed_refuses_fixtures_on_the_remote_database() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    write_fixtures(&root);
    write(&root, "db/seeds.sql", "INSERT INTO posts (title) VALUES ('Seeded');\n");
    sandbox.remote_database("shop");
    let (report, ok) = sandbox.json(&["db", "seed", "--remote"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "the fixtures of db/fixtures only load into the local database");
    assert_eq!(report["hint"], "fixtures replace table rows: local only; use db/seeds.sql for remote data");
    assert_eq!(sandbox.calls(), ["cf d1 list --name shop"], "nothing ran on the remote database");

    // An empty fixture directory loads nothing: the seeds still run remotely.
    fs::remove_dir_all(root.join("db/fixtures")).unwrap();
    fs::create_dir_all(root.join("db/fixtures")).unwrap();
    let (report, ok) = sandbox.json(&["db", "seed", "--remote"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["ran"], json!(["loaded db/seeds.sql (--remote)"]));
}

#[test]
fn db_seed_reports_fixture_and_tool_errors() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    write(&root, "db/fixtures/posts.yml", "hello: 3\n");
    let (report, ok) = sandbox.json(&["db", "seed"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "db/fixtures/posts.yml: `hello`: expected a mapping of `column: value`");
    assert_eq!(report["hint"], "indent the row's columns below its label");
    assert!(sandbox.calls().is_empty());

    write_fixtures(&root);
    sandbox.set("execute_fails");
    let (report, ok) = sandbox.json(&["db", "seed"], &root);
    assert!(!ok);
    let error = report["error"].as_str().unwrap();
    assert!(error.starts_with("`wrangler d1 execute DB --local --file .wrangler/ocre-fixtures.sql --yes` failed"));
    assert!(!root.join(".wrangler/ocre-fixtures.sql").exists(), "the SQL file is deleted on failure too");
}

#[test]
fn db_seed_replant_reset_and_prepare_load_fixtures() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    write_fixtures(&root);
    let report = sandbox.json(&["db", "seed", "--replant"], &root).0;
    assert_eq!(report["ran"], json!(["no tables to empty", "loaded db/fixtures: posts, users (--local)"]));

    let report = sandbox.json(&["db", "reset"], &root).0;
    assert_eq!(report["ran"], json!(["applied migrations (--local)", "loaded db/fixtures: posts, users (--local)"]));

    let report = sandbox.json(&["db", "prepare"], &root).0;
    assert_eq!(report["ran"], json!(["applied migrations (--local)", "loaded db/fixtures: posts, users (--local)"]));
}

// ---------- ocre db dump ----------

const TABLES: &str = r#"[{"results":[{"name":"posts"},{"name":"users"}],"success":true}]"#;
const ROWS: &str = r#"[{"results":[{"id":1,"title":"Hello","author_id":7},{"id":2,"title":"It's","author_id":null}],"success":true},{"results":[{"id":7,"name":"Ada"}],"success":true}]"#;

#[test]
fn db_dump_writes_fixture_files_that_seed_loads_back() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.queue_execute(&[TABLES, ROWS]);
    let (report, ok) = sandbox.json(&["db", "dump"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["created"], json!(["db/fixtures/posts.yml", "db/fixtures/users.yml"]));
    let calls = sandbox.calls();
    assert!(calls[0].starts_with("wrangler d1 execute DB --local --command SELECT name FROM sqlite_master"));
    assert_eq!(
        calls[1],
        local_d1(r#"d1 execute DB --local --command SELECT * FROM "posts"; SELECT * FROM "users"; --json"#)
    );
    assert_eq!(
        fs::read_to_string(root.join("db/fixtures/posts.yml")).unwrap(),
        "# Rows of the local D1 database, written by `ocre db dump`; `ocre db seed` loads them.\n\
         posts_1:\n  id: 1\n  title: \"Hello\"\n  author_id: 7\nposts_2:\n  id: 2\n  title: \"It's\"\n  author_id: null\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("db/fixtures/users.yml")).unwrap(),
        "# Rows of the local D1 database, written by `ocre db dump`; `ocre db seed` loads them.\n\
         users_7:\n  id: 7\n  name: \"Ada\"\n"
    );
    let (report, ok) = sandbox.json(&["db", "seed"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["ran"], json!(["loaded db/fixtures: posts, users (--local)"]));

    let (report, ok) = sandbox.json(&["db", "dump", "--tables", "posts,users"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "db/fixtures/posts.yml, db/fixtures/users.yml already exist");
    assert_eq!(report["hint"], "pass --force to overwrite them, or --dir <dir> to dump elsewhere");

    sandbox.clear_calls();
    sandbox.write_state("execute.json", r#"[{"results":[{"id":7,"name":"Grace"}],"success":true}]"#);
    let (report, ok) = sandbox.json(&["db", "dump", "--tables", "users", "--force"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["updated"], json!(["db/fixtures/users.yml"]));
    assert_eq!(sandbox.calls(), [local_d1(r#"d1 execute DB --local --command SELECT * FROM "users"; --json"#)]);
    assert!(fs::read_to_string(root.join("db/fixtures/users.yml")).unwrap().contains("name: \"Grace\""));
}

#[test]
fn db_dump_from_the_remote_database_into_another_directory() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.remote_database("shop");
    sandbox.write_state("execute.json", r#"[{"results":[{"name":"Ada"}],"success":true}]"#);
    let output = sandbox.ocre(&["db", "dump", "--remote", "--tables", "users", "--dir", "tmp/prod"], &root);
    assert!(output.status.success(), "{:?}", text(&output));
    assert_eq!(
        fs::read_to_string(root.join("tmp/prod/users.yml")).unwrap(),
        "# Rows of the remote D1 database, written by `ocre db dump`; `ocre db seed` loads them.\n\
         users_1:\n  name: \"Ada\"\n"
    );
    assert_eq!(
        sandbox.calls(),
        [
            "cf d1 list --name shop".to_owned(),
            "cf d1 query uuid-shop --batch @.wrangler/ocre-batch.json".to_owned(),
            format!("batch {}", json!([{ "sql": "SELECT * FROM \"users\";" }])),
        ]
    );
}

#[test]
fn db_dump_without_tables_or_with_unexpected_output() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["db", "dump"], &root);
    assert!(ok, "{report}");
    assert_eq!(report, json!({ "ok": true, "command": "db dump", "ran": ["no tables to dump"] }));

    sandbox.write_state("execute.json", "not json");
    let (report, ok) = sandbox.json(&["db", "dump", "--tables", "posts"], &root);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with("unexpected D1 query output"));
    assert!(!root.join("db/fixtures/posts.yml").exists());
}
