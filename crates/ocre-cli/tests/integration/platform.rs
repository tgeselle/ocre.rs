//! Deployment and platform commands: `ocre g ci`, `ocre ci`, `ocre g pwa`,
//! `ocre domains`, the production and custom checks of `ocre doctor`,
//! `ocre dev --no-cache`, `ocre secrets fetch` and registered `ocre stats`
//! directories, against fake cf, cargo, rustc and gh.

#[path = "../support/mod.rs"]
mod support;

use std::{fs, os::unix::fs::PermissionsExt, path::Path};

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

fn read(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap()
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

/// A `cargo` (and `gh`) that log their arguments and fail when
/// `$FAKE_CF_STATE/<tool>_fails_<first argument>` exists.
fn fake_tool(sandbox: &Sandbox, tool: &str) {
    sandbox.script(
        tool,
        &format!(
            "#!/bin/sh\necho \"{tool} $*\" >> \"$FAKE_CF_STATE/tools.log\"\necho \"{tool} $1 output\"\n\
             if [ -e \"$FAKE_CF_STATE/{tool}_fails_$1\" ]; then exit 101; fi\n"
        ),
    );
}

fn tools_log(sandbox: &Sandbox) -> Vec<String> {
    let log = fs::read_to_string(sandbox.work.join("../state/tools.log")).unwrap_or_default();
    let _ = fs::remove_file(sandbox.work.join("../state/tools.log"));
    log.lines().map(str::to_owned).collect()
}

fn unset(sandbox: &Sandbox, marker: &str) {
    fs::remove_file(sandbox.work.join("../state").join(marker)).unwrap();
}

// ---------- ocre g ci ----------

#[test]
fn ci_generator_writes_the_workflow_once() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = ok(&sandbox, &["g", "ci"], &root);
    assert_eq!(report["created"], json!([".github/workflows/ci.yml"]));
    assert!(report["next"][0].as_str().unwrap().contains("CLOUDFLARE_API_TOKEN"), "{report}");
    let workflow = read(&root, ".github/workflows/ci.yml");
    let steps = [
        "      - run: cargo fmt --check\n      - run: cargo clippy --all-targets -- -D warnings\n",
        "      - run: cargo test\n      - run: cargo check --target wasm32-unknown-unknown\n",
        "      - if: hashFiles('locales/*.yml') != ''\n        run: cargo install --git https://github.com/tgeselle/ocre.rs ocre-cli && ocre i18n missing\n",
        "    if: github.event_name == 'push' && github.ref == 'refs/heads/main'\n",
        "      - run: npm ci\n      - run: cargo install --git https://github.com/tgeselle/ocre.rs ocre-cli\n",
        "      - run: ocre deploy --json\n",
        "          CLOUDFLARE_API_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}\n          CLOUDFLARE_ACCOUNT_ID: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}\n",
        "node-version: 22",
    ];
    for step in steps {
        assert!(workflow.contains(step), "{step} missing from:\n{workflow}");
    }
    assert!(workflow.find("cargo fmt").unwrap() < workflow.find("cargo test").unwrap());

    let report = fails(&sandbox, &["g", "ci"], &root);
    assert_eq!(report["error"], ".github/workflows/ci.yml already exists");
    assert!(report["hint"].as_str().unwrap().contains("--skip"), "{report}");
    assert_eq!(read(&root, ".github/workflows/ci.yml"), workflow);

    let report = ok(&sandbox, &["destroy", "ci"], &root);
    assert_eq!(report["removed"][0], ".github/workflows/ci.yml");
    assert!(!root.join(".github/workflows/ci.yml").exists());
}

// ---------- ocre ci ----------

#[test]
fn ci_runs_the_steps_in_order_and_stops_at_the_first_failure() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_tool(&sandbox, "cargo");
    fake_rustc(&sandbox, true);
    let steps = [
        "cargo fmt --check: ok",
        "cargo clippy --all-targets -- -D warnings: ok",
        "cargo test: ok",
        "cargo check --target wasm32-unknown-unknown: ok",
    ];
    let report = ok(&sandbox, &["ci"], &root);
    assert_eq!(report["command"], "ci");
    assert_eq!(report["ran"], json!(steps));
    assert_eq!(
        tools_log(&sandbox),
        [
            "cargo fmt --check",
            "cargo clippy --all-targets -- -D warnings",
            "cargo test",
            "cargo check --target wasm32-unknown-unknown"
        ]
    );
    let output = sandbox.ocre(&["ci", "--json"], &root);
    assert!(text(&output).1.contains("cargo fmt output"), "tool output goes to stderr with --json");
    let output = sandbox.ocre(&["ci"], &root);
    assert!(text(&output).0.contains("cargo test: ok"), "{}", text(&output).0);
    tools_log(&sandbox);

    // Each failure stops the run and names the fix.
    for (tool, error, hint) in [
        ("cargo_fails_fmt", "cargo fmt --check failed (exit status: 101)", "run `cargo fmt`, then `ocre ci` again"),
        (
            "cargo_fails_clippy",
            "cargo clippy --all-targets -- -D warnings failed (exit status: 101)",
            "fix the warnings above (`rustup component add clippy` if cargo has no clippy command)",
        ),
        ("cargo_fails_test", "cargo test failed (exit status: 101)", "the cargo output above names the failure"),
    ] {
        sandbox.set(tool);
        let report = fails(&sandbox, &["ci"], &root);
        assert_eq!((report["error"].as_str().unwrap(), report["hint"].as_str().unwrap()), (error, hint));
        unset(&sandbox, tool);
    }
    sandbox.set("cargo_fails_test");
    let report = fails(&sandbox, &["ci"], &root);
    assert_eq!(report["ran"], json!(steps[..2]));
    assert!(!tools_log(&sandbox).iter().any(|call| call.starts_with("cargo check")));
    unset(&sandbox, "cargo_fails_test");

    fake_rustc(&sandbox, false);
    let report = fails(&sandbox, &["ci"], &root);
    assert!(report["error"].as_str().unwrap().contains("wasm32-unknown-unknown target is not installed"));
    assert_eq!(report["ran"], json!(steps[..3]));
}

#[test]
fn ci_checks_translations_when_the_app_has_locales() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_tool(&sandbox, "cargo");
    fake_rustc(&sandbox, true);
    ok(&sandbox, &["g", "locale", "en", "fr"], &root);
    let en = read(&root, "locales/en.yml");
    let fr = en.lines().map(|l| if l == "en:" { "fr:" } else { l }).collect::<Vec<_>>().join("\n");
    fs::write(root.join("locales/fr.yml"), fr).unwrap();
    let report = ok(&sandbox, &["ci"], &root);
    assert_eq!(report["ran"][4], "ocre i18n missing: ok");

    let en = read(&root, "locales/en.yml");
    fs::write(root.join("locales/en.yml"), format!("{en}\nonly_english: Hello\n")).unwrap();
    let report = fails(&sandbox, &["ci"], &root);
    assert!(report["error"].as_str().unwrap().contains("only_english"), "{report}");
    assert_eq!(report["ran"].as_array().unwrap().len(), 4);
}

#[test]
fn ci_signoff_runs_gh_after_a_green_run() {
    let mut sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_tool(&sandbox, "cargo");
    fake_rustc(&sandbox, true);
    fake_tool(&sandbox, "gh");
    let report = ok(&sandbox, &["ci", "--signoff"], &root);
    assert_eq!(report["ran"][4], "gh signoff: ok");
    assert_eq!(tools_log(&sandbox).last().unwrap(), "gh signoff");

    sandbox.set("gh_fails_signoff");
    let report = fails(&sandbox, &["ci", "--signoff"], &root);
    assert_eq!(report["error"], "gh signoff failed (exit status: 101)");
    assert!(report["hint"].as_str().unwrap().contains("gh extension install basecamp/gh-signoff"));
    tools_log(&sandbox);

    sandbox.set("cargo_fails_fmt");
    fails(&sandbox, &["ci", "--signoff"], &root);
    assert!(!tools_log(&sandbox).iter().any(|call| call.starts_with("gh")), "no signoff after a failure");
    unset(&sandbox, "cargo_fails_fmt");

    fs::remove_file(sandbox.work.join("../bin/gh")).unwrap();
    sandbox.isolate_path();
    let report = fails(&sandbox, &["ci", "--signoff"], &root);
    assert_eq!(report["error"], "gh not found");
    assert!(report["hint"].as_str().unwrap().contains("https://cli.github.com"));

    fs::remove_file(sandbox.work.join("../bin/cargo")).unwrap();
    let report = fails(&sandbox, &["ci"], &root);
    assert!(report["error"].as_str().unwrap().starts_with("could not run cargo"), "{report}");
}

// ---------- ocre g pwa ----------

#[test]
fn pwa_generator_adds_the_manifest_and_service_worker() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = ok(&sandbox, &["g", "pwa"], &root);
    assert_eq!(
        report["created"],
        json!(["public/manifest.webmanifest", "public/service-worker.js", "public/pwa.js", "public/icon.svg"])
    );
    assert_eq!(report["updated"], json!(["templates/layout.html"]));
    let manifest: Value = serde_json::from_str(&read(&root, "public/manifest.webmanifest")).unwrap();
    assert_eq!((&manifest["name"], &manifest["start_url"]), (&json!("shop"), &json!("/")));
    assert!(read(&root, "public/icon.svg").contains(">S</text>"));
    assert!(read(&root, "public/pwa.js").contains("register(\"/service-worker.js\")"));
    assert!(read(&root, "public/service-worker.js").contains("addEventListener(\"push\""));
    let layout = read(&root, "templates/layout.html");
    assert!(
        layout.contains("  <link rel=\"manifest\" href=\"/manifest.webmanifest\">\n")
            && layout.contains("  <script src=\"/pwa.js\" defer></script>\n</head>"),
        "{layout}"
    );

    let report = fails(&sandbox, &["g", "pwa"], &root);
    assert_eq!(report["error"], "templates/layout.html already links a web app manifest");

    ok(&sandbox, &["destroy", "pwa"], &root);
    assert!(!root.join("public/pwa.js").exists());
    assert!(!read(&root, "templates/layout.html").contains("manifest"));

    fs::write(root.join("templates/layout.html"), "<html><body></body></html>\n").unwrap();
    assert_eq!(fails(&sandbox, &["g", "pwa"], &root)["error"], "templates/layout.html has no </head>");
    fs::remove_file(root.join("templates/layout.html")).unwrap();
    assert_eq!(fails(&sandbox, &["g", "pwa"], &root)["error"], "templates/layout.html not found");
    assert!(!root.join("public/manifest.webmanifest").exists(), "nothing written on failure");

    let api = sandbox.new_app("api", &["--api"]);
    assert_eq!(fails(&sandbox, &["g", "pwa"], &api)["error"], "a PWA needs HTML pages; this app is API-only");
}

// ---------- ocre domains ----------

#[test]
fn domains_are_added_listed_and_removed_in_the_config() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    assert_eq!(ok(&sandbox, &["domains"], &root)["domains"], json!([]));
    let output = sandbox.ocre(&["domains"], &root);
    assert!(text(&output).0.contains("no custom domain: the Worker answers on workers.dev"));

    let report = ok(&sandbox, &["domains", "add", "shop.example.com"], &root);
    assert_eq!(report["domains"], json!(["shop.example.com"]));
    assert_eq!(report["updated"], json!(["cloudflare.config.ts"]));
    assert!(report["next"][0].as_str().unwrap().starts_with("ocre deploy (creates the DNS record"));
    let config = read(&root, "cloudflare.config.ts");
    assert!(config.contains("\t\tname: \"shop\",\n\t\t// Custom domains"), "{config}");
    assert!(config.contains("\t\tdomains: [\"shop.example.com\"],\n"), "{config}");

    ok(&sandbox, &["domains", "add", "www.shop.example.com"], &root);
    assert!(read(&root, "cloudflare.config.ts").contains("domains: [\"shop.example.com\", \"www.shop.example.com\"],"));
    let output = sandbox.ocre(&["domains"], &root);
    assert!(text(&output).0.contains("  www.shop.example.com\n"));

    let report = fails(&sandbox, &["domains", "add", "shop.example.com"], &root);
    assert_eq!(report["error"], "shop.example.com is already a custom domain of the Worker");
    for bad in ["https://shop.example.com", "*.example.com", "localhost", "Shop.example.com", "a-.example.com", "a..b"]
    {
        let report = fails(&sandbox, &["domains", "add", bad], &root);
        assert_eq!(report["error"], format!("`{bad}` is not a hostname Ocre can add as a custom domain"));
    }

    let report = ok(&sandbox, &["domains", "remove", "shop.example.com"], &root);
    assert_eq!(report["domains"], json!(["www.shop.example.com"]));
    assert_eq!(report["next"][0], "ocre deploy (detaches the domain from the Worker)");
    let report = fails(&sandbox, &["domains", "remove", "shop.example.com"], &root);
    assert_eq!(report["error"], "shop.example.com is not a custom domain of the Worker");

    // A list Ocre cannot read, then no `name` to add the entry after.
    let config = read(&root, "cloudflare.config.ts");
    fs::write(root.join("cloudflare.config.ts"), config.replace("[\"www.shop.example.com\"]", "hosts")).unwrap();
    let report = fails(&sandbox, &["domains"], &root);
    assert_eq!(report["error"], "cloudflare.config.ts has a `domains` entry Ocre cannot read");
    let without =
        config.lines().filter(|l| !l.contains("domains") && l.trim() != "name: \"shop\",").collect::<Vec<_>>();
    fs::write(root.join("cloudflare.config.ts"), without.join("\n")).unwrap();
    let report = fails(&sandbox, &["domains", "add", "shop.example.com"], &root);
    assert_eq!(report["error"], "cloudflare.config.ts has no `worker.name` Ocre can read");
}

// ---------- ocre doctor ----------

fn check(report: &Value, name: &str) -> Value {
    let found = report["checks"].as_array().unwrap().iter().find(|c| c["name"] == name);
    found.unwrap_or_else(|| panic!("no {name} check: {report}")).clone()
}

fn add_var(root: &Path, entry: &str) {
    let config = read(root, "cloudflare.config.ts").replacen("// ocre:env", &format!("{entry}\n\t\t\t// ocre:env"), 1);
    fs::write(root.join("cloudflare.config.ts"), config).unwrap();
}

#[test]
fn doctor_flags_settings_unsafe_in_production() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_rustc(&sandbox, true);
    let doctor = || sandbox.json(&["doctor"], &root).0;
    let production = check(&doctor(), "production config");
    assert_eq!(production["status"], "ok");

    add_var(&root, "MAIL_ADAPTER: bindings.text(\"log\"),\nLOG_LEVEL: bindings.text(\"debug\"),");
    let production = check(&doctor(), "production config");
    assert_eq!(production["status"], "warn");
    assert_eq!(
        production["detail"],
        "MAIL_ADAPTER = \"log\": production only logs emails instead of sending them; \
         LOG_LEVEL = \"debug\": production logs every debug line (Workers Logs: 200,000 events a day)"
    );

    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    let report = fails(&sandbox, &["doctor"], &root);
    assert_eq!(
        check(&report, "production config")["detail"],
        ".dev.vars is not in .gitignore: local secrets would be committed"
    );

    add_var(&root, "STRIPE_API_KEY: bindings.text(\"sk_live\"),");
    let production = check(&fails(&sandbox, &["doctor"], &root), "production config");
    assert_eq!(production["status"], "fail");
    assert_eq!(
        production["detail"],
        "STRIPE_API_KEY is a plain-text variable in cloudflare.config.ts: committed and readable"
    );
}

#[test]
fn doctor_runs_the_apps_own_checks() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fake_rustc(&sandbox, true);
    let dir = root.join(".ocre/doctor");
    fs::create_dir_all(dir.join("helpers")).unwrap();
    let script = |name: &str, body: &str, mode: u32| {
        fs::write(dir.join(name), body).unwrap();
        fs::set_permissions(dir.join(name), fs::Permissions::from_mode(mode)).unwrap();
    };
    script("1-stripe", "#!/bin/sh\necho \"stripe webhook secret set in $(basename \"$PWD\")\"\n", 0o755);
    script("2-backups", "#!/bin/sh\necho 'no backup in 8 days'\necho 'run ./backup.sh' >&2\nexit 2\n", 0o755);
    script("3-broken", "#!/bin/sh\nexit 1\n", 0o755);
    script("4-not-executable", "echo hi\n", 0o644);
    let report = fails(&sandbox, &["doctor"], &root);
    assert_eq!(
        check(&report, "1-stripe"),
        json!({ "name": "1-stripe", "status": "ok", "detail": "stripe webhook secret set in shop" })
    );
    assert_eq!(
        check(&report, "2-backups"),
        json!({ "name": "2-backups", "status": "warn", "detail": "no backup in 8 days", "hint": "run ./backup.sh" })
    );
    assert_eq!(
        check(&report, "3-broken"),
        json!({ "name": "3-broken", "status": "fail", "detail": ".ocre/doctor/3-broken: exit status: 1" })
    );
    let not_executable = check(&report, "4-not-executable");
    assert!(not_executable["detail"].as_str().unwrap().starts_with("could not run .ocre/doctor/4-not-executable"));
    assert_eq!(
        not_executable["hint"],
        "make it executable (`chmod +x .ocre/doctor/4-not-executable`) with a `#!` line"
    );
    assert!(report["error"].as_str().unwrap().ends_with("3-broken, 4-not-executable"), "{report}");
    assert!(!report["checks"].as_array().unwrap().iter().any(|c| c["name"] == "helpers"));
}

// ---------- ocre dev --no-cache, ocre secrets fetch, ocre stats ----------

#[test]
fn dev_turns_the_cache_off_and_on() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let before = read(&root, ".dev.vars");
    let report = ok(&sandbox, &["dev", "--no-cache"], &root);
    assert_eq!(report["ran"], json!(["caching off: CACHE_STORE=null in .dev.vars"]));
    assert_eq!(read(&root, ".dev.vars"), format!("{before}CACHE_STORE=null\n"));
    ok(&sandbox, &["dev", "--no-cache"], &root);
    assert_eq!(read(&root, ".dev.vars").matches("CACHE_STORE").count(), 1);
    ok(&sandbox, &["dev"], &root);
    assert!(read(&root, ".dev.vars").contains("CACHE_STORE=null"), "plain `ocre dev` keeps the choice");
    let report = ok(&sandbox, &["dev", "--cache"], &root);
    assert_eq!(report["ran"], json!(["caching on: CACHE_STORE removed from .dev.vars"]));
    assert_eq!(read(&root, ".dev.vars"), before);

    // Development only: never pushed to production from .dev.vars.
    ok(&sandbox, &["dev", "--no-cache"], &root);
    let report = fails(&sandbox, &["secrets", "push", "CACHE_STORE"], &root);
    assert_eq!(report["error"], "CACHE_STORE in .dev.vars is a development value");
}

#[test]
fn secrets_fetch_prints_one_local_value() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::write(root.join(".prod.vars"), "STRIPE_KEY=\"sk_live_1\"\n").unwrap();
    let report = ok(&sandbox, &["secrets", "fetch", "STRIPE_KEY", "--file", ".prod.vars"], &root);
    assert_eq!(report["secret"], "sk_live_1");
    let output = sandbox.ocre(&["secrets", "fetch", "STRIPE_KEY", "--file", ".prod.vars"], &root);
    assert_eq!(text(&output).0, "sk_live_1\n");
    assert!(ok(&sandbox, &["secrets", "fetch", "SECRET_KEY_BASE"], &root)["secret"].as_str().unwrap().len() > 20);
    let report = fails(&sandbox, &["secrets", "fetch", "STRIPE_KEY"], &root);
    assert_eq!(report["error"], "STRIPE_KEY is not set in .dev.vars");
    assert!(report["hint"].as_str().unwrap().contains("cannot be read back"));
    assert!(sandbox.calls().is_empty(), "never calls Cloudflare");
}

#[test]
fn stats_counts_directories_registered_in_cargo_toml() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::create_dir_all(root.join("lib")).unwrap();
    fs::write(root.join("lib/util.rs"), "pub fn util() {}\n").unwrap();
    let cargo = read(&root, "Cargo.toml");
    fs::write(root.join("Cargo.toml"), format!("{cargo}\n[package.metadata.ocre]\nstats = [\"lib/\"]\n")).unwrap();
    let rows = |report: &Value| -> Vec<String> {
        report["stats"]["rows"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_owned()).collect()
    };
    let report = ok(&sandbox, &["stats"], &root);
    assert_eq!(rows(&report).last().unwrap(), "lib");
    assert_eq!(report["stats"]["rows"].as_array().unwrap().last().unwrap()["functions"], 1);
    let report = ok(&sandbox, &["stats", "lib"], &root);
    assert_eq!(rows(&report).iter().filter(|name| *name == "lib").count(), 1, "counted once");
}
