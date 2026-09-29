//! Non-interactive CLI behaviour, run through the real binary with a fake
//! wrangler. Covers the `--json` contract, human output and every failure hint.

mod common;

use std::fs;

use common::{Sandbox, ocre_crate, text};

// ---------- ocre new ----------

#[test]
fn new_creates_an_app_without_touching_cloudflare_by_default() {
    let sandbox = Sandbox::new();
    let ocre = ocre_crate();
    let (report, ok) = sandbox.json(&["new", "shop", "--ocre-path", ocre.to_str().unwrap()], &sandbox.work);
    assert!(ok, "{report}");
    assert_eq!(report["command"], "new");
    assert_eq!(report["next"], serde_json::json!(["cd shop", "ocre dev", "ocre deploy"]));
    assert!(report["created"].as_array().unwrap().contains(&"shop/AGENTS.md".into()));
    let root = sandbox.work.join("shop");
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("name = \"shop\""));
    assert!(cargo.contains(&format!("ocre = {{ path = {:?} }}", ocre.display().to_string())));
    let wrangler = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    assert!(wrangler.contains("database_name = \"shop\"") && !wrangler.contains("account_id"));
    assert!(!root.join(".git").exists());
    assert!(sandbox.calls().is_empty(), "no wrangler call without --login/--deploy");
}

#[test]
fn new_defaults_to_the_git_dependency() {
    let sandbox = Sandbox::new();
    let (report, ok) = sandbox.json(&["new", "shop"], &sandbox.work);
    assert!(ok, "{report}");
    let cargo = fs::read_to_string(sandbox.work.join("shop/Cargo.toml")).unwrap();
    assert!(cargo.contains("ocre = { git = \"https://github.com/tgeselle/ocre.rs\" }"));
}

#[test]
fn new_prints_a_human_summary_without_json() {
    let sandbox = Sandbox::new();
    let output = sandbox.ocre(&["new", "shop", "--yes"], &sandbox.work);
    let (stdout, _) = text(&output);
    assert!(output.status.success());
    assert!(stdout.contains("  create  shop/wrangler.toml"), "{stdout}");
    assert!(stdout.contains("Next:\n  cd shop\n  ocre dev\n  ocre deploy"), "{stdout}");
}

#[test]
fn new_rejects_bad_input_before_writing_anything() {
    let sandbox = Sandbox::new();
    let (report, ok) = sandbox.json(&["new"], &sandbox.work);
    assert!(!ok);
    assert_eq!(report["error"], "missing app name");

    let (report, _) = sandbox.json(&["new", "Bad_Name"], &sandbox.work);
    assert_eq!(report["error"], "invalid app name `Bad_Name`");
    assert!(report["hint"].as_str().unwrap().contains("lowercase"));

    fs::create_dir(sandbox.work.join("taken")).unwrap();
    let (report, _) = sandbox.json(&["new", "taken"], &sandbox.work);
    assert!(report["error"].as_str().unwrap().ends_with("taken` already exists"));

    let (report, _) = sandbox.json(&["new", "shop", "--ocre-path", "/does/not/exist"], &sandbox.work);
    assert!(report["error"].as_str().unwrap().starts_with("--ocre-path /does/not/exist:"));
    assert!(!sandbox.work.join("shop").exists());
}

#[test]
fn new_with_git_initializes_a_repository() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &["--git"]);
    assert!(root.join(".git").is_dir());
}

#[test]
fn new_with_git_requires_git() {
    let mut sandbox = Sandbox::new();
    sandbox.isolate_path();
    let (report, ok) = sandbox.json(&["new", "shop", "--git"], &sandbox.work);
    assert!(!ok);
    assert_eq!(report["error"], "git is not installed");
    assert!(!sandbox.work.join("shop").exists());
}

#[test]
fn new_reports_a_failing_git_init() {
    let sandbox = Sandbox::new();
    sandbox.script("git", "#!/bin/sh\n[ \"$1\" = --version ] && exit 0\nexit 3\n");
    let (report, ok) = sandbox.json(&["new", "shop", "--git"], &sandbox.work);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with("`git init` failed"), "{report}");
}

#[test]
fn new_with_the_blog_starter_scaffolds_posts() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("blog", &["--starter", "blog"]);
    assert!(root.join("src/posts.rs").is_file());
    assert!(root.join("templates/posts/edit.html").is_file());
    let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains("mod posts;") && lib.contains(".merge(posts::routes())"));
}

#[test]
fn new_with_login_runs_the_browser_login_when_needed() {
    let sandbox = Sandbox::new();
    sandbox.accounts_after_login(&[("acc1", "Main")]);
    let (report, ok) = sandbox.json(&["new", "shop", "--login"], &sandbox.work);
    assert!(ok, "{report}");
    assert_eq!(report["email"], "dev@example.com");
    assert_eq!(sandbox.calls(), ["whoami --json", "login", "whoami --json"]);
}

#[test]
fn new_with_login_skips_the_login_when_already_logged_in() {
    let sandbox = Sandbox::new();
    sandbox.login_as(&[("acc1", "Main")]);
    let (report, ok) = sandbox.json(&["new", "shop", "--login"], &sandbox.work);
    assert!(ok, "{report}");
    assert_eq!(sandbox.calls(), ["whoami --json"]);
}

#[test]
fn new_with_login_fails_when_the_login_does_not_complete() {
    let sandbox = Sandbox::new();
    sandbox.set("login_does_nothing");
    let (report, ok) = sandbox.json(&["new", "shop", "--login"], &sandbox.work);
    assert!(!ok);
    assert_eq!(report["error"], "Cloudflare login did not complete");

    sandbox.set("login_fails");
    let (report, _) = sandbox.json(&["new", "shop", "--login"], &sandbox.work);
    assert!(report["error"].as_str().unwrap().starts_with("`wrangler login` failed"), "{report}");
    assert!(report["hint"].as_str().unwrap().contains("wrangler output above"));
}

#[test]
fn new_needs_an_account_id_when_the_login_has_several_accounts() {
    let sandbox = Sandbox::new();
    sandbox.login_as(&[("acc1", "Main"), ("acc2", "Side")]);
    let (report, ok) = sandbox.json(&["new", "shop", "--login"], &sandbox.work);
    assert!(!ok);
    assert_eq!(report["hint"], "pass --account-id with one of: acc1 (Main), acc2 (Side)");

    let (report, ok) = sandbox.json(&["new", "shop", "--login", "--account-id", "acc2"], &sandbox.work);
    assert!(ok, "{report}");
    let wrangler = fs::read_to_string(sandbox.work.join("shop/wrangler.toml")).unwrap();
    assert!(wrangler.starts_with("name = \"shop\"\naccount_id = \"acc2\"\n"), "{wrangler}");
}

#[test]
fn new_keeps_an_account_id_given_without_login() {
    let sandbox = Sandbox::new();
    sandbox.new_app("shop", &["--account-id", "acc9"]);
    let wrangler = fs::read_to_string(sandbox.work.join("shop/wrangler.toml")).unwrap();
    assert!(wrangler.contains("account_id = \"acc9\""));
}

#[test]
fn new_with_deploy_logs_in_deploys_and_returns_the_url() {
    let sandbox = Sandbox::new();
    sandbox.login_as(&[("acc1", "Main")]);
    let (report, ok) =
        sandbox.json(&["new", "shop", "--deploy", "--ocre-path", ocre_crate().to_str().unwrap()], &sandbox.work);
    assert!(ok, "{report}");
    assert_eq!(report["url"], "https://app.example.workers.dev");
    assert_eq!(report["next"], serde_json::json!(["cd shop", "ocre dev"]));
    assert_eq!(
        sandbox.calls(),
        ["whoami --json", "d1 list --json", "deploy", "build --release", "d1 migrations apply shop --remote"]
    );
}

// ---------- ocre login ----------

#[test]
fn login_reports_the_existing_session() {
    let sandbox = Sandbox::new();
    sandbox.login_as(&[("acc1", "Main")]);
    let output = sandbox.ocre(&["login"], &sandbox.work);
    assert!(output.status.success());
    assert_eq!(text(&output).0, "Logged in to Cloudflare as dev@example.com\n");
}

#[test]
fn login_runs_wrangler_login_and_streams_its_output() {
    let sandbox = Sandbox::new();
    sandbox.accounts_after_login(&[("acc1", "Main")]);
    let output = sandbox.ocre(&["login"], &sandbox.work);
    let (stdout, _) = text(&output);
    assert!(stdout.starts_with("Successfully logged in.\n"), "{stdout}");

    // With --json, wrangler's output moves to stderr.
    let sandbox = Sandbox::new();
    sandbox.accounts_after_login(&[("acc1", "Main")]);
    let output = sandbox.ocre(&["login", "--json"], &sandbox.work);
    let (stdout, stderr) = text(&output);
    assert!(stderr.contains("Successfully logged in."));
    assert_eq!(serde_json::from_str::<serde_json::Value>(&stdout).unwrap()["email"], "dev@example.com");
}

#[test]
fn login_rejects_unexpected_whoami_output() {
    let sandbox = Sandbox::new();
    sandbox.set("logged_in");
    sandbox.write_state("whoami.json", "not json");
    let output = sandbox.ocre(&["login"], &sandbox.work);
    let (_, stderr) = text(&output);
    assert!(!output.status.success());
    assert!(stderr.starts_with("error: unexpected `wrangler whoami --json` output"), "{stderr}");
    assert!(!stderr.contains("hint:"));
}

#[test]
fn logged_out_whoami_json_means_no_session() {
    let sandbox = Sandbox::new();
    sandbox.set("logged_in");
    sandbox.write_state("whoami.json", r#"{"loggedIn": false}"#);
    sandbox.set("login_does_nothing");
    let (report, _) = sandbox.json(&["login"], &sandbox.work);
    assert_eq!(report["error"], "Cloudflare login did not complete");
}

#[test]
fn commands_explain_a_missing_node() {
    let mut sandbox = Sandbox::new();
    fs::remove_file(sandbox.work.join("../bin/npx")).unwrap();
    sandbox.isolate_path();
    let (report, ok) = sandbox.json(&["login"], &sandbox.work);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with("could not run npx"));
    assert_eq!(report["hint"], "install Node.js 20 or newer (it provides npx)");
}

// ---------- ocre generate ----------

#[test]
fn scaffold_generates_and_registers_a_resource() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) =
        sandbox.json(&["g", "scaffold", "Product", "name:string", "price:float", "stock:integer"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["updated"], serde_json::json!(["src/lib.rs"]));
    assert_eq!(report["created"][0], "migrations/0001_create_products.sql");
    let sql = fs::read_to_string(root.join("migrations/0001_create_products.sql")).unwrap();
    assert!(sql.contains("price REAL NOT NULL") && sql.contains("stock INTEGER NOT NULL"));
    let module = fs::read_to_string(root.join("src/products.rs")).unwrap();
    assert!(module.contains("Generated by `ocre g scaffold Product name:string price:float stock:integer`"));
    assert!(module.contains("\"INSERT INTO products (name, price, stock) VALUES (?1, ?2, ?3) RETURNING *\""));
    assert!(
        module.contains(
            "UPDATE products SET name = ?1, price = ?2, stock = ?3, updated_at = datetime('now') WHERE id = ?4"
        )
    );

    // Human output, from a subdirectory of the app.
    let output = sandbox.ocre(&["generate", "migration", "add_sku_to_products"], &root.join("src"));
    assert_eq!(text(&output).0, "  create  migrations/0002_add_sku_to_products.sql\n\nNext:\n  ocre migrate\n");
}

#[test]
fn scaffold_human_output_lists_created_and_updated_files() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::remove_dir_all(root.join("migrations")).unwrap();
    let output = sandbox.ocre(&["g", "scaffold", "Tag", "label:string"], &root);
    let (stdout, _) = text(&output);
    assert!(stdout.starts_with("  create  migrations/0001_create_tags.sql\n"), "{stdout}");
    assert!(stdout.contains("  update  src/lib.rs\n"), "{stdout}");
    assert!(stdout.ends_with("Next:\n  ocre migrate\n  ocre dev\n  open http://localhost:8787/tags\n"), "{stdout}");
}

#[test]
fn scaffold_refuses_to_overwrite_or_guess() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.json(&["g", "scaffold", "Product", "name:string"], &root);
    let cases: &[(&[&str], &str)] = &[
        (&["g", "scaffold", "Product", "name:string"], "src/products.rs already exists"),
        (&["g", "scaffold", "2Fast", "name:string"], "invalid model name `2Fast`"),
        (&["g", "scaffold", "Item", "title"], "field `title` has no type"),
        (&["g", "scaffold", "Item", "Title:string"], "invalid field name `Title`"),
        (&["g", "scaffold", "Item", "type:string"], "field name `type` is reserved"),
        (&["g", "scaffold", "Item", "size:huge"], "unknown field type `huge` for `size`"),
        (&["g", "scaffold", "Item", "a:string", "a:text"], "field `a` is listed twice"),
        (&["g", "migration", "Add-Thing"], "invalid migration name `Add-Thing`"),
    ];
    for (args, error) in cases {
        let (report, ok) = sandbox.json(args, &root);
        assert!(!ok);
        assert_eq!(report["error"], *error, "{args:?}");
    }
}

#[test]
fn scaffold_templates_exists_check_covers_the_template_directory() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::create_dir_all(root.join("templates/items")).unwrap();
    let (report, _) = sandbox.json(&["g", "scaffold", "Item", "name:string"], &root);
    assert_eq!(report["error"], "templates/items already exists");
}

#[test]
fn scaffold_needs_the_lib_markers() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::write(root.join("src/lib.rs"), "fn main() {}\n").unwrap();
    let (report, ok) = sandbox.json(&["g", "scaffold", "Item", "name:string"], &root);
    assert!(!ok);
    assert!(report["hint"].as_str().unwrap().contains("// ocre:routes"));
    assert!(!root.join("src/items.rs").exists(), "nothing written on failure");
    assert!(!root.join("migrations/0001_create_items.sql").exists());
}

#[test]
fn commands_explain_a_missing_or_broken_wrangler_toml() {
    let sandbox = Sandbox::new();
    let (report, _) = sandbox.json(&["g", "migration", "x"], &sandbox.work);
    assert_eq!(report["error"], "no wrangler.toml found in this directory or its parents");

    fs::write(sandbox.work.join("wrangler.toml"), "name = ").unwrap();
    let (report, _) = sandbox.json(&["g", "migration", "x"], &sandbox.work);
    assert!(report["error"].as_str().unwrap().starts_with("wrangler.toml is not valid TOML"));

    fs::write(sandbox.work.join("wrangler.toml"), "name = \"x\"\n[[d1_databases]]\nbinding = \"OTHER\"\n").unwrap();
    let (report, _) = sandbox.json(&["g", "migration", "x"], &sandbox.work);
    assert_eq!(report["error"], "wrangler.toml has no D1 database with binding \"DB\"");
}

// ---------- ocre migrate / dev / deploy ----------

#[test]
fn migrate_applies_locally_or_remotely() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let output = sandbox.ocre(&["migrate"], &root);
    assert_eq!(text(&output).0, "Migrations applied to shop (--local)\n");
    let (report, ok) = sandbox.json(&["migrate", "--remote"], &root);
    assert!(ok, "{report}");
    assert_eq!(sandbox.calls(), ["d1 migrations apply shop --local", "d1 migrations apply shop --remote"]);
}

#[test]
fn migrate_failure_points_at_wrangler_output() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("migrate_fails");
    let output = sandbox.ocre(&["migrate"], &root);
    let (stdout, stderr) = text(&output);
    assert!(!output.status.success());
    assert!(stdout.contains("stdout before failure"));
    assert!(stderr.contains("error: `wrangler d1 migrations apply shop --local` failed"), "{stderr}");
    assert!(stderr.contains("hint: read the wrangler output above"));
}

#[test]
fn dev_migrates_then_serves() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["dev", "--port", "9123"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["url"], "http://localhost:9123");
    assert_eq!(sandbox.calls(), ["d1 migrations apply shop --local", "dev --port 9123", "build --dev"]);
}

#[test]
fn dev_and_deploy_need_the_wasm_target() {
    let mut sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.script("rustc", "#!/bin/sh\necho /nonexistent/sysroot\n");
    for command in ["dev", "deploy"] {
        let (report, ok) = sandbox.json(&[command], &root);
        assert!(!ok);
        assert_eq!(
            report["error"],
            "the wasm32-unknown-unknown target is not installed for rustc at /nonexistent/sysroot"
        );
    }
    sandbox.isolate_path();
    fs::remove_file(sandbox.work.join("../bin/rustc")).unwrap();
    let (report, _) = sandbox.json(&["dev"], &root);
    assert_eq!(report["error"], "rustc not found");
    assert!(sandbox.calls().is_empty());
}

#[test]
fn deploy_migrates_an_existing_database_before_the_code_goes_live() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.write_state("d1_list.json", r#"[{"name": "other"}, {"name": "shop"}]"#);
    let output = sandbox.ocre(&["deploy"], &root);
    let (stdout, _) = text(&output);
    assert!(output.status.success());
    assert!(stdout.ends_with("\nhttps://app.example.workers.dev\n"), "{stdout}");
    assert_eq!(sandbox.calls(), ["d1 list --json", "d1 migrations apply shop --remote", "deploy", "build --release"]);
}

#[test]
fn deploy_without_a_workers_dev_url_reports_none() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.script(
        "npx",
        &include_str!("common/fake_npx.sh").replace("https://app.example.workers.dev", "https://custom.example.com"),
    );
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok, "{report}");
    assert!(report.get("url").is_none());
}

#[test]
fn deploy_failures_carry_hints() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.write_state("d1_list.json", "oops");
    let (report, _) = sandbox.json(&["deploy"], &root);
    assert!(report["error"].as_str().unwrap().starts_with("unexpected `wrangler d1 list --json` output"));

    sandbox.set("d1_list_fails");
    let (report, _) = sandbox.json(&["deploy"], &root);
    assert_eq!(report["error"], "`wrangler d1 list` failed: ✘ [ERROR] d1_list_fails");
    assert!(report["hint"].as_str().unwrap().contains("ocre login"));

    fs::remove_file(sandbox.work.join("../state/d1_list_fails")).unwrap();
    sandbox.write_state("d1_list.json", "[]");
    sandbox.set("deploy_fails");
    let (report, _) = sandbox.json(&["deploy"], &root);
    assert!(report["error"].as_str().unwrap().starts_with("`wrangler deploy` failed"));
}
