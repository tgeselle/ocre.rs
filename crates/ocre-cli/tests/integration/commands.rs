//! `ocre version|about|doctor|stats|notes|test|template` and the whole-database
//! tasks (`ocre db create|drop|version|truncate|prepare|seed --replant`),
//! against a fake wrangler, cargo and rustc.

#[path = "../support/mod.rs"]
mod support;

use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
};

use serde_json::{Value, json};
use support::{Sandbox, text};

fn ok(sandbox: &Sandbox, args: &[&str], root: &Path) -> Value {
    let (report, ok) = sandbox.json(args, root);
    assert!(ok, "{args:?}: {report}");
    report
}

fn fails(sandbox: &Sandbox, args: &[&str], root: &Path) -> Value {
    let (report, ok) = sandbox.json(args, root);
    assert!(!ok, "{args:?} should fail: {report}");
    report
}

/// A `rustc` whose sysroot has (or lacks) the wasm32 target.
fn fake_rustc(sandbox: &Sandbox, wasm: bool) {
    let sysroot = sandbox.work.join(if wasm { "sysroot" } else { "sysroot-native" });
    fs::create_dir_all(sysroot.join("lib/rustlib")).unwrap();
    if wasm {
        fs::create_dir_all(sysroot.join("lib/rustlib/wasm32-unknown-unknown")).unwrap();
    }
    sandbox.script("rustc", &format!("#!/bin/sh\necho {}\n", sysroot.display()));
}

/// A `cargo` that logs its arguments and fails for subcommands listed in
/// `$FAKE_WRANGLER_STATE/cargo_fails_<subcommand>`.
fn fake_cargo(sandbox: &Sandbox) {
    sandbox.script(
        "cargo",
        "#!/bin/sh\necho \"$*\" >> \"$FAKE_WRANGLER_STATE/cargo.log\"\necho \"cargo $1 output\"\nif [ -e \"$FAKE_WRANGLER_STATE/cargo_fails_$1\" ]; then exit 101; fi\n",
    );
}

// ---------- version, about ----------

#[test]
fn version_outside_and_inside_an_app() {
    let sandbox = Sandbox::new();
    let report = ok(&sandbox, &["version"], &sandbox.work);
    assert_eq!(report, json!({ "ok": true, "command": "version", "about": { "cli": env!("CARGO_PKG_VERSION") } }));
    let root = sandbox.new_app("shop", &[]);
    let about = &ok(&sandbox, &["version"], &root)["about"];
    assert_eq!((&about["app"], &about["app_version"]), (&json!("shop"), &json!("0.1.0")));
    assert!(about["ocre"].as_str().unwrap().starts_with("path /"), "{about}");
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let cargo = cargo.lines().map(|l| if l.starts_with("ocre = ") { "ocre = \"0.1\"" } else { l }).collect::<Vec<_>>();
    fs::write(root.join("Cargo.toml"), cargo.join("\n")).unwrap();
    assert_eq!(ok(&sandbox, &["version"], &root)["about"]["ocre"], "0.1");
    let (stdout, _) = text(&sandbox.ocre(&["version"], &root));
    assert!(stdout.contains("App version         0.1.0\n"), "{stdout}");
}

#[test]
fn about_lists_configuration() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &["--api"]);
    let mut wrangler = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    wrangler.push_str(
        "\n[[kv_namespaces]]\nbinding = \"CACHE\"\n\n[[r2_buckets]]\nbinding = \"STORAGE\"\nbucket_name = \"shop-files\"\n\n[[queues.producers]]\nbinding = \"JOBS\"\nqueue = \"shop-jobs\"\n\n[[queues.consumers]]\nqueue = \"shop-jobs\"\n\n[[durable_objects.bindings]]\nname = \"CHANNELS\"\nclass_name = \"OcreChannel\"\n\n[[send_email]]\nname = \"EMAIL\"\n\n[triggers]\ncrons = [\"0 3 * * *\"]\n",
    );
    fs::write(root.join("wrangler.toml"), wrangler).unwrap();
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let cargo = cargo
        .lines()
        .map(|l| if l.starts_with("ocre = ") { l.replace(" }", ", features = [\"cache\"] }") } else { l.to_owned() });
    let cargo = cargo.collect::<Vec<_>>().join("\n");
    fs::write(root.join("Cargo.toml"), cargo).unwrap();
    let about = ok(&sandbox, &["about"], &root)["about"].clone();
    assert_eq!(about["mode"], "api (JSON only)");
    assert_eq!(about["rust_toolchain"], "stable");
    assert_eq!(about["compatibility_date"], "2026-09-01");
    assert_eq!(about["vars"], json!(["MAIL_FROM"]));
    assert_eq!(about["features"], json!(["cache"]));
    assert_eq!(
        about["bindings"],
        json!([
            "D1 DB (shop)",
            "KV CACHE",
            "R2 STORAGE (shop-files)",
            "Queue JOBS (shop-jobs)",
            "Queue consumer (shop-jobs)",
            "Durable Object CHANNELS (OcreChannel)",
            "Email EMAIL",
            "cron 0 3 * * *",
            "Assets (public)"
        ])
    );
    let (stdout, _) = text(&sandbox.ocre(&["about"], &root));
    assert!(
        stdout.contains("Mode                api (JSON only)\n") && stdout.contains("Ocre features       cache\n"),
        "{stdout}"
    );
    let outside = fails(&sandbox, &["about"], &sandbox.work);
    assert_eq!(outside["error"], "no wrangler.toml found in this directory or its parents");
}

// ---------- doctor ----------

#[test]
fn doctor_passes_warns_and_fails() {
    let mut sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_rustc(&sandbox, true);
    sandbox.login_as(&[("acc", "Me")]);
    sandbox.set("has_secret");
    let report = ok(&sandbox, &["doctor"], &root);
    let names: Vec<(&str, &str)> = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].as_str().unwrap(), c["status"].as_str().unwrap()))
        .collect();
    assert_eq!(
        names,
        [
            ("rust", "ok"),
            ("node", "ok"),
            ("cloudflare login", "ok"),
            ("migrations", "ok"),
            ("local secrets", "ok"),
            ("production secrets", "ok")
        ]
    );
    assert_eq!(report["checks"][2]["detail"], "logged in as dev@example.com");

    // Warnings keep it passing.
    sandbox.write_state("pending_migrations", "0001_create_posts.sql\n");
    sandbox.write_state("secret_list.json", "[]");
    let report = ok(&sandbox, &["doctor"], &root);
    assert_eq!(report["checks"][3]["detail"], "pending locally: 0001_create_posts.sql");
    assert_eq!(report["checks"][5]["detail"], "missing on the deployed Worker: SECRET_KEY_BASE");

    // Failures: code using a binding wrangler.toml lacks, no local secret.
    fs::write(root.join("src/cached.rs"), "fn f() { ocre::cache::fetch }\n#[event(scheduled)]\n").unwrap();
    fs::write(root.join(".dev.vars"), "MAIL_ADAPTER=log\n").unwrap();
    fake_rustc(&sandbox, false);
    let report = fails(&sandbox, &["doctor"], &root);
    assert_eq!(report["error"], "4 check(s) failed: rust, cache binding, cron triggers, local secrets");
    assert_eq!(report["checks"][3]["hint"], "run `ocre g cache` (adds the CACHE KV binding)");
    let output = sandbox.ocre(&["doctor"], &root);
    let (stdout, stderr) = text(&output);
    assert!(
        stdout.contains("  FAIL  cache binding       the code uses it but wrangler.toml does not declare it\n"),
        "{stdout}"
    );
    assert!(stdout.contains("  ok    node                npx 10.9.0\n"), "{stdout}");
    assert!(stderr.starts_with("error: 4 check(s) failed"), "{stderr}");
    assert!(!output.status.success());

    // Bindings present: ok.
    let wrangler = fs::read_to_string(root.join("wrangler.toml"))
        .unwrap()
        .replace("[vars]\n", "[vars]\nMAIL_ADAPTER = \"resend\"\n")
        + "\n[[kv_namespaces]]\nbinding = \"CACHE\"\n\n[triggers]\ncrons = [\"0 3 * * *\"]\n";
    fs::write(root.join("wrangler.toml"), wrangler).unwrap();
    fs::write(root.join(".dev.vars"), "SECRET_KEY_BASE=x\n").unwrap();
    fake_rustc(&sandbox, true);
    sandbox.set("migrations_list_fails");
    fs::remove_file(root.join("../../state/secret_list.json")).ok();
    let report = ok(&sandbox, &["doctor"], &root);
    let by_name = |name: &str| report["checks"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap().clone();
    assert_eq!(by_name("cache binding")["status"], "ok");
    assert_eq!(by_name("cron triggers")["status"], "ok");
    assert_eq!(by_name("migrations")["status"], "warn");
    assert!(by_name("production secrets")["detail"].as_str().unwrap().contains("RESEND_API_KEY"), "{report}");

    // Undeployed Worker, then a failing secret list.
    sandbox.set("secret_list_fails");
    let report = ok(&sandbox, &["doctor"], &root);
    assert_eq!(report["checks"].as_array().unwrap().last().unwrap()["detail"], "not deployed yet");
    fs::remove_file(root.join("../../state/secret_list_fails")).ok();
    sandbox.set("secret_list_errors");
    let report = ok(&sandbox, &["doctor"], &root);
    assert_eq!(report["checks"].as_array().unwrap().last().unwrap()["status"], "warn");

    // Not logged in, then an unreadable session.
    sandbox.write_state("whoami.json", "not json");
    let report = fails(&sandbox, &["doctor"], &root);
    assert_eq!(report["checks"][2]["status"], "fail");
    fs::remove_file(root.join("../../state/logged_in")).ok();
    let report = ok(&sandbox, &["doctor"], &root);
    assert_eq!(report["checks"][2]["status"], "warn");
    assert_eq!(report["checks"].as_array().unwrap().len(), 7, "no production check when logged out");

    // Without Node.js inside an app: no migrations check (it needs wrangler).
    sandbox.isolate_path();
    fake_rustc(&sandbox, true);
    fs::remove_file(sandbox.work.join("../bin/npx")).unwrap();
    let report = fails(&sandbox, &["doctor"], &root);
    let names: Vec<&str> = report["checks"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["rust", "node", "cache binding", "cron triggers", "local secrets"]);
    assert_eq!(report["error"], "1 check(s) failed: node");

    // Outside an app, without Node.js: tool checks only.
    let report = fails(&sandbox, &["doctor"], &sandbox.work);
    assert_eq!(report["checks"].as_array().unwrap().len(), 2);
    assert_eq!(report["error"], "1 check(s) failed: node");
}

#[test]
fn doctor_checks_the_bindings_the_generated_code_uses() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_rustc(&sandbox, true);
    fs::write(root.join(".dev.vars"), "SECRET_KEY_BASE=x\n").unwrap();
    let bare = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    ok(&sandbox, &["g", "job", "SendWelcome", "user_id:integer"], &root);
    ok(&sandbox, &["g", "schedule", "nightly_cleanup", "every day at 3am"], &root);
    ok(&sandbox, &["g", "scaffold", "Post", "title:string", "cover:attachment?", "--realtime"], &root);
    fs::write(root.join("src/notes.txt"), "ocre::cache::fetch\n").unwrap();
    let binding_checks = |report: &Value| -> Vec<(String, String)> {
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| {
                c["name"].as_str().unwrap().contains("binding")
                    || ["jobs queue", "cron triggers"].contains(&c["name"].as_str().unwrap())
            })
            .map(|c| (c["name"].as_str().unwrap().to_owned(), c["status"].as_str().unwrap().to_owned()))
            .collect()
    };
    let report = ok(&sandbox, &["doctor"], &root);
    let names = ["storage binding", "jobs queue", "cron triggers", "realtime binding"];
    let all = |status: &str| names.map(|name| (name.to_owned(), status.to_owned())).to_vec();
    assert_eq!(binding_checks(&report), all("ok"), "only .rs files are code, so no cache check: {report}");

    let generated = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    fs::write(root.join("wrangler.toml"), &bare).unwrap();
    let report = fails(&sandbox, &["doctor"], &root);
    assert_eq!(report["error"], "4 check(s) failed: storage binding, jobs queue, cron triggers, realtime binding");
    assert_eq!(binding_checks(&report), all("fail"));

    // A queue producer without its consumer is still a broken jobs setup.
    let producer_only = generated.replace("[[queues.consumers]]", "[[queues.unused]]");
    fs::write(root.join("wrangler.toml"), producer_only).unwrap();
    assert_eq!(fails(&sandbox, &["doctor"], &root)["error"], "1 check(s) failed: jobs queue");
}

// ---------- stats, notes ----------

#[test]
fn stats_counts_code_per_part() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(root.join("tests/home.rs"), "// Home.\n#[test]\nfn home() {}\n\n").unwrap();
    fs::create_dir_all(root.join("lib/.hidden")).unwrap();
    fs::write(root.join("lib/util.rs"), "/* x */\n pub(crate) async fn a() {}\npub const fn b() {}\n").unwrap();
    fs::write(root.join("lib/.hidden/skip.rs"), "fn skipped() {}\n").unwrap();
    fs::write(root.join("lib/blob.bin"), [0xff, 0xfe]).unwrap();
    fs::create_dir_all(root.join("src/models")).unwrap();
    fs::write(root.join("src/models/post.rs"), "pub struct Post;\n").unwrap();
    let stats = ok(&sandbox, &["stats", "lib/", "src/models"], &root)["stats"].clone();
    let row = |name: &str| stats["rows"].as_array().unwrap().iter().find(|r| r["name"] == name).unwrap().clone();
    assert_eq!(row("Tests"), json!({ "name": "Tests", "files": 1, "lines": 4, "loc": 2, "functions": 1 }));
    assert_eq!(row("lib"), json!({ "name": "lib", "files": 1, "lines": 3, "loc": 2, "functions": 2 }));
    assert_eq!(row("Models")["files"], 1);
    assert_eq!(row("src/models")["files"], 0, "a file is counted in one row only");
    assert_eq!(stats["test_loc"], 2);
    assert!(stats["code_loc"].as_u64().unwrap() > 10);
    let (stdout, _) = text(&sandbox.ocre(&["stats"], &root));
    assert!(stdout.starts_with("Name                    Files   Lines     LOC Functions\n"), "{stdout}");
    assert!(stdout.contains("Code to test ratio: 1:"), "{stdout}");
    assert_eq!(fails(&sandbox, &["stats", "nope"], &root)["error"], "nope is not a directory of the app");
}

#[test]
fn stats_of_an_empty_app_has_no_ratio() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &["--api"]);
    fs::remove_dir_all(root.join("src")).unwrap();
    let (stdout, _) = text(&sandbox.ocre(&["stats"], &root));
    assert!(stdout.contains("Code to test ratio: 1:0.0"), "{stdout}");
}

#[test]
fn notes_lists_tagged_comments() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::write(
        root.join("src/a.rs"),
        "let todo = 1; // TODO: rename\n/* FIXME slow */\n// TODOS are not tags\n// NOTE_TODO neither\n",
    )
    .unwrap();
    fs::write(root.join("templates/x.html"), "{# OPTIMIZE: cache #}\n<!-- HACK: remove -->\n").unwrap();
    fs::create_dir_all(root.join("db")).unwrap();
    fs::write(root.join("db/seeds.sql"), "-- TODO more rows\n").unwrap();
    let notes = ok(&sandbox, &["notes"], &root)["notes"].clone();
    assert_eq!(
        notes,
        json!([
            { "path": "src/a.rs", "line": 1, "tag": "TODO", "text": "rename" },
            { "path": "src/a.rs", "line": 2, "tag": "FIXME", "text": "slow" },
            { "path": "templates/x.html", "line": 1, "tag": "OPTIMIZE", "text": "cache" },
            { "path": "db/seeds.sql", "line": 1, "tag": "TODO", "text": "more rows" }
        ])
    );
    let notes = ok(&sandbox, &["notes", "--annotations", "HACK,FIXME"], &root)["notes"].clone();
    assert_eq!(notes.as_array().unwrap().len(), 2);
    let (stdout, _) = text(&sandbox.ocre(&["notes", "--annotations", "HACK"], &root));
    assert_eq!(stdout, "templates/x.html:2: [HACK] remove\n");
}

// ---------- db create, drop, version, truncate, prepare, seed --replant ----------

const LOCAL_STATE: &str = ".wrangler/state/v3/d1";

#[test]
fn db_create_locally_and_on_cloudflare() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = ok(&sandbox, &["db", "create"], &root);
    assert_eq!(report["ran"], json!(["created local database shop"]));
    fs::create_dir_all(root.join(LOCAL_STATE)).unwrap();
    assert_eq!(ok(&sandbox, &["db", "create"], &root)["ran"], json!(["local database shop already exists"]));
    let report = ok(&sandbox, &["db", "create", "--remote"], &root);
    assert_eq!(report["provisioned"], json!(["D1 database shop"]));
    sandbox.write_state("d1_list.json", r#"[{"name":"shop"}]"#);
    assert_eq!(ok(&sandbox, &["db", "create", "--remote"], &root)["ran"], json!(["D1 database shop already exists"]));
    assert!(sandbox.calls().contains(&"d1 create shop".to_owned()));
}

#[test]
fn db_drop_is_local_only() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    assert_eq!(ok(&sandbox, &["db", "drop"], &root)["ran"], json!(["no local database to delete"]));
    fs::create_dir_all(root.join(LOCAL_STATE)).unwrap();
    assert_eq!(ok(&sandbox, &["db", "drop"], &root)["ran"], json!(["deleted .wrangler/state/v3/d1"]));
    assert!(!root.join(LOCAL_STATE).exists());
    let report = fails(&sandbox, &["db", "drop", "--remote"], &root);
    assert_eq!(report["error"], "`ocre db drop` only runs on the local database");
}

#[test]
fn db_version_reads_the_last_migration() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.write_state("execute.json", r#"[{"results":[{"name":"0002_add_slug.sql"}],"success":true}]"#);
    let report = ok(&sandbox, &["db", "version", "--remote"], &root);
    assert_eq!(report, json!({ "ok": true, "command": "db version", "version": "0002_add_slug.sql", "remote": true }));
    let (stdout, _) = text(&sandbox.ocre(&["db", "version", "--remote"], &root));
    assert_eq!(stdout, "0002_add_slug.sql\nTarget: remote D1 database on Cloudflare\n");
    sandbox.set("execute_no_table");
    assert_eq!(ok(&sandbox, &["db", "version"], &root)["version"], Value::Null);
    assert_eq!(text(&sandbox.ocre(&["db", "version"], &root)).0, "no migration applied\n");
    sandbox.set("execute_fails");
    assert!(fails(&sandbox, &["db", "version"], &root)["error"].as_str().unwrap().contains("execute_fails"));
    sandbox.write_state("execute.json", "not json");
    fs::remove_file(sandbox.work.join("../state/execute_fails")).unwrap();
    fs::remove_file(sandbox.work.join("../state/execute_no_table")).unwrap();
    assert!(
        fails(&sandbox, &["db", "version"], &root)["error"]
            .as_str()
            .unwrap()
            .starts_with("unexpected `wrangler d1 execute --json` output")
    );
}

#[test]
fn db_truncate_empties_tables_locally() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let tables = r#"[{"results":[{"name":"comments"},{"name":"posts"}],"success":true}]"#;
    let sequence = r#"[{"results":[{"name":"sqlite_sequence"}],"success":true}]"#;
    sandbox.queue_execute(&[tables, sequence, "[]"]);
    let report = ok(&sandbox, &["db", "truncate"], &root);
    assert_eq!(report["ran"], json!(["emptied comments, posts (--local)"]));
    let calls = sandbox.calls();
    assert!(calls.last().unwrap().contains(
        "PRAGMA defer_foreign_keys = on; DELETE FROM \"comments\"; DELETE FROM \"posts\"; DELETE FROM sqlite_sequence;"
    ), "{calls:?}");
    sandbox.queue_execute(&[tables, r#"[{"results":[],"success":true}]"#, "[]"]);
    ok(&sandbox, &["db", "truncate"], &root);
    assert!(!sandbox.calls().last().unwrap().contains("sqlite_sequence"));
    assert_eq!(ok(&sandbox, &["db", "truncate"], &root)["ran"], json!(["no tables to empty"]));
    assert_eq!(
        fails(&sandbox, &["db", "truncate", "--remote"], &root)["error"],
        "`ocre db truncate` only runs on the local database"
    );
}

#[test]
fn db_seed_replant_and_prepare() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    assert_eq!(fails(&sandbox, &["db", "seed", "--replant"], &root)["error"], "db/seeds.sql not found in the app");
    assert_eq!(
        fails(&sandbox, &["db", "seed", "--replant", "--remote"], &root)["error"],
        "`ocre db seed --replant` only runs on the local database"
    );
    fs::create_dir_all(root.join("db")).unwrap();
    fs::write(root.join("db/seeds.sql"), "INSERT INTO posts (title) VALUES ('a');\n").unwrap();
    let report = ok(&sandbox, &["db", "seed", "--replant"], &root);
    assert_eq!(report["ran"], json!(["no tables to empty", "loaded db/seeds.sql (--local)"]));

    let report = ok(&sandbox, &["db", "prepare"], &root);
    assert_eq!(report["ran"], json!(["applied migrations (--local)", "loaded db/seeds.sql (--local)"]));
    fs::create_dir_all(root.join(LOCAL_STATE)).unwrap();
    assert_eq!(ok(&sandbox, &["db", "prepare"], &root)["ran"], json!(["applied migrations (--local)"]));
}

// ---------- ocre test ----------

#[test]
fn test_runs_unit_tests_then_the_wasm_check() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_cargo(&sandbox);
    fake_rustc(&sandbox, true);
    let output = sandbox.ocre(&["test", "--json", "--", "models", "--nocapture"], &root);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["ran"], json!(["cargo test: ok", "cargo check --target wasm32-unknown-unknown: ok"]));
    let log = fs::read_to_string(sandbox.work.join("../state/cargo.log")).unwrap();
    assert_eq!(log, "test models --nocapture\ncheck --target wasm32-unknown-unknown\n");
    let output = sandbox.ocre(&["test", "--json"], &root);
    assert!(text(&output).1.contains("cargo test output"), "cargo's output goes to stderr with --json");

    sandbox.write_state("cargo_fails_check", "");
    let report = fails(&sandbox, &["test"], &root);
    assert_eq!(report["error"], "cargo check --target wasm32-unknown-unknown failed (exit status: 101)");
    assert_eq!(report["ran"], json!(["cargo test: ok"]));
    sandbox.write_state("cargo_fails_test", "");
    assert_eq!(fails(&sandbox, &["test"], &root)["ran"], Value::Null);

    fs::remove_file(sandbox.work.join("../state/cargo_fails_test")).unwrap();
    fake_rustc(&sandbox, false);
    let report = fails(&sandbox, &["test"], &root);
    assert!(report["error"].as_str().unwrap().contains("wasm32-unknown-unknown target is not installed"));
}

#[test]
fn test_e2e_runs_the_script_against_one_server() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_cargo(&sandbox);
    fake_rustc(&sandbox, true);
    let report = fails(&sandbox, &["test", "--e2e"], &root);
    assert_eq!(report["error"], "tests/e2e.sh not found");
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(root.join("tests/e2e.sh"), "echo \"checking $BASE_URL\"\n").unwrap();
    let report = ok(&sandbox, &["test", "--e2e", "--port", "9123"], &root);
    assert_eq!(report["ran"][2], "tests/e2e.sh against wrangler dev on port 9123: ok");
    let calls = sandbox.calls();
    assert!(calls.contains(&"dev --port 9123".to_owned()) && calls.contains(&"build --dev".to_owned()), "{calls:?}");
    let output = sandbox.ocre(&["test", "--e2e", "--json"], &root);
    assert!(text(&output).1.contains("checking http://localhost:8788"), "{}", text(&output).1);

    fs::write(root.join("tests/e2e.sh"), "exit 3\n").unwrap();
    assert_eq!(fails(&sandbox, &["test", "--e2e"], &root)["error"], "tests/e2e.sh failed (exit status: 3)");
    sandbox.set("dev_fails");
    assert_eq!(fails(&sandbox, &["test", "--e2e"], &root)["error"], "wrangler dev stopped or did not get ready");
}

#[test]
fn test_needs_cargo() {
    let mut sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.isolate_path();
    let report = fails(&sandbox, &["test"], &root);
    assert!(report["error"].as_str().unwrap().starts_with("could not run cargo"), "{report}");
}

// ---------- application templates ----------

#[test]
fn new_applies_a_template_file() {
    let sandbox = Sandbox::new();
    fake_cargo(&sandbox);
    fs::write(
        sandbox.work.join("blog.ocre"),
        "# Blog\n\nocre g scaffold Post title:string 'body:text'\ng controller Pages \"about\"\ncargo add slug\n",
    )
    .unwrap();
    let root = sandbox.new_app("blog", &["--template", "blog.ocre"]);
    assert!(root.join("src/posts.rs").exists() && root.join("src/pages.rs").exists());
    let log = fs::read_to_string(sandbox.work.join("../state/cargo.log")).unwrap();
    assert_eq!(log, "add slug\n");
    let (report, _) = sandbox.json(
        &["new", "blog2", "-m", "blog.ocre", "--ocre-path", support::ocre_crate().to_str().unwrap()],
        &sandbox.work,
    );
    assert_eq!(
        report["ran"],
        json!(["ocre g scaffold Post title:string body:text", "ocre g controller Pages about", "cargo add slug"])
    );
    assert!(report["created"].as_array().unwrap().contains(&json!("blog2/src/pages.rs")));
}

#[test]
fn template_lines_are_checked_before_anything_runs() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    for (line, why) in [
        ("deploy", "is not allowed in a template"),
        ("cargo build", "is not allowed in a template"),
        ("ocre", "is not allowed in a template"),
        ("migrate --remote", "targets production (--remote)"),
        ("g model Post 'title:string", "has an unclosed quote"),
    ] {
        fs::write(root.join("t.ocre"), format!("g controller Pages\n{line}\n")).unwrap();
        let report = fails(&sandbox, &["template", "t.ocre"], &root);
        assert_eq!(report["error"], format!("t.ocre line 2: `{line}` {why}"));
    }
    assert!(!root.join("src/pages.rs").exists());
    let report = fails(&sandbox, &["template", "missing.ocre"], &root);
    assert!(report["error"].as_str().unwrap().starts_with("could not read the template missing.ocre"));
    let report = fails(&sandbox, &["new", "x", "--template", "missing.ocre"], &sandbox.work);
    assert!(report["error"].as_str().unwrap().starts_with("could not read the template"));
    assert!(!sandbox.work.join("x").exists(), "nothing is created when the template cannot be read");
}

#[test]
fn template_stops_at_the_first_failing_line() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_cargo(&sandbox);
    fs::write(root.join("t.ocre"), "g controller Pages\ng controller Pages\n").unwrap();
    let report = fails(&sandbox, &["template", "t.ocre"], &root);
    assert_eq!(report["error"], "template line `ocre g controller Pages` failed: src/pages.rs already exists");
    assert!(report["hint"].as_str().unwrap().contains("--skip"));
    sandbox.write_state("cargo_fails_add", "");
    fs::write(root.join("t.ocre"), "cargo add nope\n").unwrap();
    assert_eq!(
        fails(&sandbox, &["template", "t.ocre"], &root)["error"],
        "template line `cargo add nope` failed (exit status: 101)"
    );
    fs::write(root.join("t.ocre"), "routes --bogus\n").unwrap();
    let report = fails(&sandbox, &["template", "t.ocre"], &root);
    assert_eq!(report["error"], "template line `ocre routes --bogus` failed: it exited with an error");
    assert_eq!(report["hint"], "run the line by hand to see the error");
}

#[test]
fn template_from_a_url() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).unwrap();
        let body = "g controller Pages about\ng model Tag name:string\n";
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let report = ok(&sandbox, &["template", &format!("http://127.0.0.1:{port}/blog.ocre")], &root);
    server.join().unwrap();
    assert_eq!(report["ran"], json!(["ocre g controller Pages about", "ocre g model Tag name:string"]));
    assert!(report["updated"].as_array().unwrap().contains(&json!("src/lib.rs")));
    let report = fails(&sandbox, &["template", &format!("http://127.0.0.1:{port}/gone")], &root);
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .starts_with(&format!("could not download the template http://127.0.0.1:{port}/gone"))
    );

    // The connection closes before the announced body length.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).unwrap();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\ng controller").unwrap();
    });
    let url = format!("http://127.0.0.1:{port}/cut.ocre");
    let report = fails(&sandbox, &["template", &url], &root);
    server.join().unwrap();
    assert!(report["error"].as_str().unwrap().starts_with(&format!("could not download the template {url}: ")));
}
