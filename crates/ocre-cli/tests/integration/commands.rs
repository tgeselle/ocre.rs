//! `ocre version|about|doctor|stats|notes|test|template` and the whole-database
//! tasks (`ocre db create|drop|version|truncate|prepare|seed --replant`),
//! against fake cf, wrangler, node, cargo and rustc.

#[path = "../support/mod.rs"]
mod support;

use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
};

use serde_json::{Value, json};
use support::{Sandbox, local_d1, text};

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
/// `$FAKE_CF_STATE/cargo_fails_<subcommand>`.
fn fake_cargo(sandbox: &Sandbox) {
    sandbox.script(
        "cargo",
        "#!/bin/sh\necho \"$*\" >> \"$FAKE_CF_STATE/cargo.log\"\necho \"cargo $1 output\"\nif [ -e \"$FAKE_CF_STATE/cargo_fails_$1\" ]; then exit 101; fi\n",
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

/// `entries` inserted right after the `marker` line of cloudflare.config.ts.
fn add_to_config(root: &Path, marker: &str, entries: &str) {
    let path = root.join("cloudflare.config.ts");
    let config = fs::read_to_string(&path).unwrap();
    assert!(config.contains(marker), "{config}");
    fs::write(&path, config.replacen(marker, &format!("{marker}\n{entries}"), 1)).unwrap();
}

#[test]
fn about_lists_configuration() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &["--api"]);
    add_to_config(
        &root,
        "// ocre:env",
        "CACHE: bindings.kv(),\nSTORAGE: bindings.r2({ name: \"shop-files\" }),\nJOBS: bindings.queue({ name: \"shop-jobs\" }),\n\
         CHANNELS: bindings.durableObject({ worker: \"shop\", exportName: \"OcreChannel\" }),\nEMAIL: bindings.sendEmail(),\n\
         LIMITER: bindings.rateLimit({ limit: 10, period: 60 }),\nMAIL_ADAPTER: bindings.text(\"resend\"),",
    );
    add_to_config(
        &root,
        "// ocre:triggers",
        "triggers.queue({ name: \"shop-jobs\" }),\ntriggers.scheduled({ schedule: \"0 3 * * *\" }),",
    );
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
    assert_eq!(about["vars"], json!(["MAIL_FROM", "MAIL_ADAPTER"]), "commented-out entries are not read");
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
            "Rate limit LIMITER",
            "cron 0 3 * * *",
            "Assets (public)"
        ])
    );
    let (stdout, _) = text(&sandbox.ocre(&["about"], &root));
    assert!(
        stdout.contains("Mode                api (JSON only)\n") && stdout.contains("Ocre features       cache\n"),
        "{stdout}"
    );

    // No assets directory in wrangler.config.ts: no Assets line.
    fs::write(root.join("wrangler.config.ts"), "export default {};\n").unwrap();
    let bindings = ok(&sandbox, &["about"], &root)["about"]["bindings"].clone();
    assert_eq!(bindings.as_array().unwrap().last().unwrap(), "cron 0 3 * * *");

    let outside = fails(&sandbox, &["about"], &sandbox.work);
    assert_eq!(outside["error"], "no cloudflare.config.ts found in this directory or its parents");
}

// ---------- doctor ----------

/// (name, status) of each check.
fn statuses(report: &Value) -> Vec<(String, String)> {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].as_str().unwrap().to_owned(), c["status"].as_str().unwrap().to_owned()))
        .collect()
}

fn check(report: &Value, name: &str) -> Value {
    let found = report["checks"].as_array().unwrap().iter().find(|c| c["name"] == name);
    found.unwrap_or_else(|| panic!("no {name} check: {report}")).clone()
}

fn has_check(report: &Value, name: &str) -> bool {
    report["checks"].as_array().unwrap().iter().any(|c| c["name"] == name)
}

fn state(sandbox: &Sandbox, name: &str) -> std::path::PathBuf {
    sandbox.work.join("../state").join(name)
}

fn unset(sandbox: &Sandbox, marker: &str) {
    fs::remove_file(state(sandbox, marker)).unwrap();
}

#[test]
fn doctor_passes_warns_and_fails() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_rustc(&sandbox, true);
    sandbox.login_as(&[("acc", "Me")]);
    sandbox.set("has_secret");
    let report = ok(&sandbox, &["doctor"], &root);
    let expected = [
        ("rust", "ok"),
        ("node", "ok"),
        ("cloudflare login", "ok"),
        ("npm packages", "ok"),
        ("config", "ok"),
        ("migrations", "ok"),
        ("local secrets", "ok"),
        ("production secrets", "ok"),
    ];
    assert_eq!(statuses(&report), expected.map(|(n, s)| (n.to_owned(), s.to_owned())));
    assert_eq!(check(&report, "cloudflare login")["detail"], "logged in as dev@example.com");
    assert_eq!(check(&report, "npm packages")["detail"], "cf 1.0.0-beta.5, wrangler 4.144.0");
    assert_eq!(check(&report, "config")["detail"], "cloudflare.config.ts is valid");
    assert_eq!(check(&report, "production secrets")["detail"], "set on the deployed Worker");
    assert!(sandbox.calls().contains(&"cf workers secrets list --worker shop".to_owned()), "{:?}", sandbox.calls());
    assert!(sandbox.calls().contains(&local_d1("d1 migrations list DB --local")), "{:?}", sandbox.calls());

    // Warnings keep it passing.
    sandbox.write_state("pending_migrations", "0001_create_posts.sql\n");
    sandbox.write_state("secret_list.json", "[]");
    let report = ok(&sandbox, &["doctor"], &root);
    assert_eq!(check(&report, "migrations")["detail"], "pending locally: 0001_create_posts.sql");
    assert_eq!(check(&report, "migrations")["hint"], "run `ocre migrate`");
    assert_eq!(check(&report, "production secrets")["detail"], "missing on the deployed Worker: SECRET_KEY_BASE");
    unset(&sandbox, "pending_migrations");

    // Failures: code using a binding cloudflare.config.ts lacks, no local secret.
    fs::write(root.join("src/cached.rs"), "fn f() { ocre::cache::fetch }\n#[event(scheduled)]\n").unwrap();
    fs::write(root.join(".dev.vars"), "MAIL_ADAPTER=log\n").unwrap();
    fake_rustc(&sandbox, false);
    let report = fails(&sandbox, &["doctor"], &root);
    assert_eq!(report["error"], "4 check(s) failed: rust, cache binding, cron triggers, local secrets");
    assert_eq!(
        check(&report, "cache binding")["hint"],
        "run `ocre g cache` (adds `CACHE: bindings.kv(),` to cloudflare.config.ts)"
    );
    let output = sandbox.ocre(&["doctor"], &root);
    let (stdout, stderr) = text(&output);
    assert!(
        stdout.contains("  FAIL  cache binding       the code uses it but cloudflare.config.ts does not declare it\n"),
        "{stdout}"
    );
    assert!(stdout.contains("  ok    node                node v22.23.2\n"), "{stdout}");
    assert!(stderr.starts_with("error: 4 check(s) failed"), "{stderr}");
    assert!(!output.status.success());

    // Bindings present: ok. MAIL_ADAPTER "resend" needs RESEND_API_KEY in production.
    add_to_config(&root, "// ocre:env", "CACHE: bindings.kv(),\nMAIL_ADAPTER: bindings.text(\"resend\"),");
    add_to_config(&root, "// ocre:triggers", "triggers.scheduled({ schedule: \"0 3 * * *\" }),");
    fs::write(root.join(".dev.vars"), "SECRET_KEY_BASE=x\n").unwrap();
    fake_rustc(&sandbox, true);
    sandbox.set("migrations_list_fails");
    unset(&sandbox, "secret_list.json");
    let report = ok(&sandbox, &["doctor"], &root);
    assert_eq!(check(&report, "cache binding")["status"], "ok");
    assert_eq!(check(&report, "cache binding")["detail"], "configured in cloudflare.config.ts");
    assert_eq!(check(&report, "cron triggers")["status"], "ok");
    let migrations = check(&report, "migrations");
    assert_eq!(migrations["status"], "warn");
    assert!(migrations["detail"].as_str().unwrap().starts_with("`wrangler d1 migrations list DB --local` failed"));
    assert_eq!(migrations["hint"], "run `ocre migrate --status` to see the error");
    assert_eq!(check(&report, "production secrets")["detail"], "missing on the deployed Worker: RESEND_API_KEY");
    sandbox.write_state(
        "secret_list.json",
        r#"[{"name":"SECRET_KEY_BASE","type":"secret_text"},{"name":"RESEND_API_KEY","type":"secret_text"}]"#,
    );
    assert_eq!(check(&ok(&sandbox, &["doctor"], &root), "production secrets")["status"], "ok");
    unset(&sandbox, "secret_list.json");

    // Undeployed Worker, then a failing secret list.
    sandbox.set("secret_list_fails");
    let report = ok(&sandbox, &["doctor"], &root);
    assert_eq!(
        check(&report, "production secrets"),
        json!({ "name": "production secrets", "status": "ok", "detail": "not deployed yet" })
    );
    unset(&sandbox, "secret_list_fails");
    sandbox.set("secret_list_errors");
    let report = ok(&sandbox, &["doctor"], &root);
    let secrets = check(&report, "production secrets");
    assert_eq!(secrets["status"], "warn");
    assert!(secrets["detail"].as_str().unwrap().contains("Authentication error"), "{secrets}");
    assert_eq!(secrets["hint"], "run `ocre secrets list` to see the error");

    // An unreadable session fails.
    sandbox.write_state("whoami.json", "not json");
    let report = fails(&sandbox, &["doctor"], &root);
    assert_eq!(check(&report, "cloudflare login")["status"], "fail");
    assert!(
        check(&report, "cloudflare login")["detail"]
            .as_str()
            .unwrap()
            .starts_with("unexpected `cf auth whoami` output")
    );
    assert!(!has_check(&report, "production secrets"));
}

/// `ocre doctor --json` with HOME set to `home`.
fn doctor_at_home(sandbox: &Sandbox, root: &Path, home: &Path) -> Value {
    let output = sandbox.command(&["doctor", "--json"], root).env("HOME", home).output().unwrap();
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn doctor_warns_when_logged_out_and_names_the_separate_cf_login() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_rustc(&sandbox, true);
    let home = sandbox.work.join("home");
    fs::create_dir_all(&home).unwrap();
    let report = doctor_at_home(&sandbox, &root, &home);
    assert_eq!(report["ok"], true, "{report}");
    let login = check(&report, "cloudflare login");
    assert_eq!(
        login,
        json!({ "name": "cloudflare login", "status": "warn", "detail": "not logged in (needed by ocre deploy)", "hint": "run `ocre login`" })
    );
    assert!(!has_check(&report, "production secrets"), "no production check when logged out");
    assert!(!sandbox.calls().iter().any(|call| call.starts_with("cf workers")), "{:?}", sandbox.calls());

    // Logged in with wrangler: cf needs its own login.
    fs::create_dir_all(home.join(".wrangler/config")).unwrap();
    fs::write(home.join(".wrangler/config/default.toml"), "oauth_token = \"x\"\n").unwrap();
    let report = doctor_at_home(&sandbox, &root, &home);
    assert_eq!(
        check(&report, "cloudflare login")["hint"],
        "run `ocre login`: cf keeps its own login, separate from wrangler's, so log in once more"
    );
}

#[test]
fn doctor_checks_node() {
    let mut sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_rustc(&sandbox, true);
    sandbox.login_as(&[("acc", "Me")]);

    sandbox.write_state("node_version", "v20.1.0\n");
    let report = fails(&sandbox, &["doctor"], &root);
    assert_eq!(report["error"], "1 check(s) failed: node");
    let node = check(&report, "node");
    assert_eq!(node["detail"], "node v20.1.0 is too old");
    assert_eq!(node["hint"], "install Node.js 22 or newer (cf needs it; it provides npm)");
    let names: Vec<String> = statuses(&report).into_iter().map(|(name, _)| name).collect();
    assert_eq!(
        names,
        ["rust", "node", "npm packages", "local secrets"],
        "no cf, config or wrangler checks without node"
    );
    sandbox.write_state("node_version", "garbage\n");
    assert_eq!(check(&fails(&sandbox, &["doctor"], &root), "node")["detail"], "node garbage is too old");

    // Without Node.js at all (nor any real one further on PATH), inside and outside an app.
    unset(&sandbox, "node_version");
    sandbox.remove_tool("node");
    sandbox.isolate_path();
    let report = fails(&sandbox, &["doctor"], &root);
    assert_eq!(check(&report, "node")["detail"], "node not found");
    assert_eq!(report["error"], "1 check(s) failed: node");
    let report = fails(&sandbox, &["doctor"], &sandbox.work);
    let names: Vec<String> = statuses(&report).into_iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["rust", "node"]);
}

#[test]
fn doctor_checks_the_npm_packages() {
    let sandbox = Sandbox::new();
    fake_rustc(&sandbox, true);
    sandbox.write_state("installed_cf", "1.0.0-beta.4");
    let root = sandbox.new_app("shop", &[]);
    let report = ok(&sandbox, &["doctor"], &root);
    let packages = check(&report, "npm packages");
    assert_eq!(packages["status"], "warn");
    assert_eq!(
        packages["detail"],
        "cf 1.0.0-beta.4, wrangler 4.144.0; this Ocre CLI is tested with cf 1.0.0-beta.5, wrangler 4.144.0"
    );
    assert_eq!(packages["hint"], "run `npm install --save-dev --save-exact cf@1.0.0-beta.5 wrangler@4.144.0`");

    // A wrangler older than cf's `cf-wrangler` entry point fails.
    unset(&sandbox, "installed_cf");
    sandbox.write_state("installed_wrangler", "4.100.0");
    let old = sandbox.new_app("old", &[]);
    let report = fails(&sandbox, &["doctor"], &old);
    assert_eq!(report["error"], "1 check(s) failed: npm packages");
    let packages = check(&report, "npm packages");
    assert_eq!(packages["detail"], "wrangler 4.100.0 is older than 4.136, which cf delegates builds to");
    assert_eq!(packages["hint"], "run `npm install --save-dev --save-exact wrangler@4.144.0`");
    fs::write(old.join("node_modules/wrangler/package.json"), r#"{"version":"next"}"#).unwrap();
    let report = fails(&sandbox, &["doctor"], &old);
    assert_eq!(
        check(&report, "npm packages")["detail"],
        "wrangler next is older than 4.136, which cf delegates builds to"
    );

    // Not installed: no config or migrations check (they need the packages).
    fs::remove_dir_all(old.join("node_modules")).unwrap();
    let report = fails(&sandbox, &["doctor"], &old);
    let packages = check(&report, "npm packages");
    assert_eq!(packages["detail"], "cf and wrangler are not installed in the app");
    assert_eq!(packages["hint"], "run `npm install` in the app");
    assert!(!has_check(&report, "config") && !has_check(&report, "migrations"), "{report}");
}

#[test]
fn doctor_validates_the_config_with_cf_loader_and_tsc() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_rustc(&sandbox, true);
    sandbox.set("config_invalid");
    let report = fails(&sandbox, &["doctor"], &root);
    assert_eq!(report["error"], "1 check(s) failed: config");
    let config = check(&report, "config");
    assert_eq!(
        config["detail"],
        r#"cloudflare.config.ts is invalid: [{"path":["worker","env","CACHE"],"message":"Invalid input"}]"#
    );
    assert!(config["hint"].as_str().unwrap().contains("npx tsc -p ."), "{config}");

    unset(&sandbox, "config_invalid");
    sandbox.set("tsc_fails");
    let config = check(&fails(&sandbox, &["doctor"], &root), "config");
    assert_eq!(
        config["detail"],
        "`tsc -p .` failed:\ncloudflare.config.ts(12,4): error TS2322: Type '30' is not assignable to type '10 | 60'."
    );
    assert_eq!(config["hint"], "fix the type errors in cloudflare.config.ts or wrangler.config.ts");

    // Without the app's TypeScript, cf's loader is the whole check.
    fs::remove_file(root.join("node_modules/.bin/tsc")).unwrap();
    assert_eq!(check(&ok(&sandbox, &["doctor"], &root), "config")["status"], "ok");
}

#[test]
fn doctor_checks_the_bindings_the_generated_code_uses() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_rustc(&sandbox, true);
    fs::write(root.join(".dev.vars"), "SECRET_KEY_BASE=x\n").unwrap();
    let bare = fs::read_to_string(root.join("cloudflare.config.ts")).unwrap();
    ok(&sandbox, &["g", "job", "SendWelcome", "user_id:integer"], &root);
    ok(&sandbox, &["g", "schedule", "nightly_cleanup", "every day at 3am"], &root);
    ok(&sandbox, &["g", "scaffold", "Post", "title:string", "cover:attachment?", "--realtime"], &root);
    fs::write(root.join("src/notes.txt"), "ocre::cache::fetch\n").unwrap();
    let names = ["storage binding", "jobs queue", "cron triggers", "realtime binding"];
    let binding_checks = |report: &Value| -> Vec<(String, String)> {
        statuses(report).into_iter().filter(|(name, _)| names.contains(&name.as_str())).collect()
    };
    let all = |status: &str| names.map(|name| (name.to_owned(), status.to_owned())).to_vec();
    let report = ok(&sandbox, &["doctor"], &root);
    assert_eq!(binding_checks(&report), all("ok"), "{report}");
    assert!(!has_check(&report, "cache binding"), "only .rs files are code");

    let generated = fs::read_to_string(root.join("cloudflare.config.ts")).unwrap();
    fs::write(root.join("cloudflare.config.ts"), &bare).unwrap();
    let report = fails(&sandbox, &["doctor"], &root);
    assert_eq!(report["error"], "4 check(s) failed: storage binding, jobs queue, cron triggers, realtime binding");
    assert_eq!(binding_checks(&report), all("fail"));

    // A queue producer without its consumer is still a broken jobs setup.
    fs::write(root.join("cloudflare.config.ts"), generated.replace("triggers.queue(", "triggers.unused(")).unwrap();
    assert_eq!(fails(&sandbox, &["doctor"], &root)["error"], "1 check(s) failed: jobs queue");
    // So is a realtime binding without its exported class.
    let unexported = generated.replace("OcreChannel: exports.", "Other: exports.");
    fs::write(root.join("cloudflare.config.ts"), unexported).unwrap();
    assert_eq!(fails(&sandbox, &["doctor"], &root)["error"], "1 check(s) failed: realtime binding");
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
    assert_eq!(sandbox.calls(), [local_d1("d1 execute DB --local --command SELECT 1")]);
    fs::create_dir_all(root.join(LOCAL_STATE)).unwrap();
    assert_eq!(ok(&sandbox, &["db", "create"], &root)["ran"], json!(["local database shop already exists"]));

    sandbox.clear_calls();
    let report = ok(&sandbox, &["db", "create", "--remote"], &root);
    assert_eq!(report["provisioned"], json!(["D1 database shop"]));
    assert_eq!(sandbox.calls(), ["cf d1 list --name shop", "cf d1 create --name shop"]);
    let (stdout, _) = text(&sandbox.ocre(&["db", "create", "--remote"], &root));
    assert_eq!(stdout, "  D1 database shop already exists\nTarget: remote D1 database on Cloudflare\n");
    assert_eq!(sandbox.calls().len(), 3, "an existing database is not created again");

    let other = sandbox.new_app("other", &[]);
    sandbox.set("d1_create_no_uuid");
    let report = fails(&sandbox, &["db", "create", "--remote"], &other);
    let error = report["error"].as_str().unwrap();
    assert!(error.starts_with("`cf d1 create --name other` returned no uuid: {\"created_at\""), "{error}");
    sandbox.set("d1_create_fails");
    let report = fails(&sandbox, &["db", "create", "--remote"], &sandbox.new_app("third", &[]));
    assert_eq!(report["error"], "`cf d1 create --name third` failed: ┌ Error\n│ d1_create_fails\n└");
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
    sandbox.remote_database("shop");
    sandbox.write_state("execute.json", r#"[{"results":[{"name":"0002_add_slug.sql"}],"success":true}]"#);
    let report = ok(&sandbox, &["db", "version", "--remote"], &root);
    assert_eq!(report, json!({ "ok": true, "command": "db version", "version": "0002_add_slug.sql", "remote": true }));
    let sql = "SELECT name FROM d1_migrations ORDER BY id DESC LIMIT 1";
    assert_eq!(
        sandbox.calls(),
        [
            "cf d1 list --name shop".to_owned(),
            "cf d1 query uuid-shop --batch @.wrangler/ocre-batch.json".to_owned(),
            format!("batch {}", json!([{ "sql": sql }])),
        ]
    );
    let (stdout, _) = text(&sandbox.ocre(&["db", "version", "--remote"], &root));
    assert_eq!(stdout, "0002_add_slug.sql\nTarget: remote D1 database on Cloudflare\n");
    assert_eq!(ok(&sandbox, &["db", "version"], &root)["version"], "0002_add_slug.sql");
    assert!(sandbox.calls().contains(&local_d1(&format!("d1 execute DB --local --command {sql} --json"))));

    // No migrations table yet, locally or remotely: no version.
    sandbox.set("execute_no_table");
    assert_eq!(ok(&sandbox, &["db", "version", "--remote"], &root)["version"], Value::Null);
    assert_eq!(ok(&sandbox, &["db", "version"], &root)["version"], Value::Null);
    assert_eq!(text(&sandbox.ocre(&["db", "version"], &root)).0, "no migration applied\n");
    sandbox.set("execute_fails");
    assert!(fails(&sandbox, &["db", "version"], &root)["error"].as_str().unwrap().contains("execute_fails"));
    sandbox.write_state("execute.json", "not json");
    fs::remove_file(sandbox.work.join("../state/execute_fails")).unwrap();
    fs::remove_file(sandbox.work.join("../state/execute_no_table")).unwrap();
    assert!(
        fails(&sandbox, &["db", "version"], &root)["error"].as_str().unwrap().starts_with("unexpected D1 query output")
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
    assert_eq!(report["ran"][2], "tests/e2e.sh against cf dev on port 9123: ok");
    assert_eq!(
        sandbox.calls(),
        [local_d1("d1 migrations apply DB --local"), "cf dev --port 9123".to_owned(), "build --dev".to_owned()]
    );
    let output = sandbox.ocre(&["test", "--e2e", "--json"], &root);
    assert!(text(&output).1.contains("checking http://localhost:8788"), "{}", text(&output).1);

    fs::write(root.join("tests/e2e.sh"), "exit 3\n").unwrap();
    assert_eq!(fails(&sandbox, &["test", "--e2e"], &root)["error"], "tests/e2e.sh failed (exit status: 3)");
    sandbox.set("dev_fails");
    assert_eq!(fails(&sandbox, &["test", "--e2e"], &root)["error"], "cf dev stopped or did not get ready");
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
        json!([
            "ocre g scaffold Post title:string body:text",
            "ocre g controller Pages about",
            "cargo add slug",
            "npm install (cf 1.0.0-beta.5, wrangler 4.144.0)"
        ])
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
