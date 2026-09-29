//! Non-interactive CLI behaviour, run through the real binary with a fake
//! wrangler. Covers the `--json` contract, human output and every failure hint.

#[path = "../support/mod.rs"]
mod support;

use std::fs;

use support::{Sandbox, ocre_crate, text};

// ---------- ocre new ----------

#[test]
fn new_creates_an_app_without_touching_cloudflare_by_default() {
    let sandbox = Sandbox::new();
    let ocre = ocre_crate();
    let (report, ok) = sandbox.json(&["new", "shop", "--ocre-path", ocre.to_str().unwrap()], &sandbox.work);
    assert!(ok, "{report}");
    assert_eq!(report["command"], "new");
    assert_eq!(report["next"], serde_json::json!(["cd shop", "ocre dev", "ocre deploy"]));
    let created = report["created"].as_array().unwrap();
    for file in ["shop/AGENTS.md", "shop/public/robots.txt", "shop/.dev.vars"] {
        assert!(created.contains(&file.into()), "{file} in {created:?}");
    }
    let root = sandbox.work.join("shop");
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("name = \"shop\""));
    assert!(cargo.contains(&format!("ocre = {{ path = {:?} }}", ocre.display().to_string())));
    let wrangler = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    assert!(wrangler.contains("database_name = \"shop\"") && !wrangler.contains("account_id"));
    assert!(!root.join(".git").exists());
    assert_dev_secret(&root);
    let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains(".route(\"/up\", get(up))\n        // ocre:routes"), "{lib}");
    assert!(wrangler.contains("[assets]\ndirectory = \"public\""), "{wrangler}");
    assert!(fs::read_to_string(root.join("public/robots.txt")).unwrap().contains("User-agent: *"));
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(gitignore.contains("\n.dev.vars\n.dev.vars.*\n"), "{gitignore}");
    let agents = fs::read_to_string(root.join("AGENTS.md")).unwrap();
    assert!(agents.starts_with("# shop\n") && agents.contains("`https://ocre.rs/llms.txt`"), "{agents}");
    assert!(!agents.contains("__"), "no template placeholder left: {agents}");
    assert!(sandbox.calls().is_empty(), "no wrangler call without --login/--deploy");
}

/// `.dev.vars` holds a fresh 128-hex SECRET_KEY_BASE and MAIL_ADAPTER=log for `wrangler dev`.
fn assert_dev_secret(root: &std::path::Path) {
    let vars = fs::read_to_string(root.join(".dev.vars")).unwrap();
    let secret =
        vars.strip_prefix("SECRET_KEY_BASE=").and_then(|rest| rest.strip_suffix("\nMAIL_ADAPTER=log\n")).unwrap();
    assert!(secret.len() == 128 && secret.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')), "{vars}");
    let wrangler = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    assert!(wrangler.contains("[vars]\n# Sender") && wrangler.contains("\nMAIL_FROM = \""), "{wrangler}");
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
    assert_eq!(report["secret_created"], true, "a new Worker gets SECRET_KEY_BASE");
    assert_eq!(
        sandbox.calls(),
        [
            "whoami --json",
            "secret list --format json",
            "d1 list --json",
            "deploy --secrets-file .wrangler/ocre-secrets.env",
            "secrets file ok",
            "build --release",
            "d1 migrations apply shop --remote"
        ]
    );
    assert!(!sandbox.work.join("shop/.wrangler/ocre-secrets.env").exists(), "secrets file deleted");
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
        sandbox.json(&["g", "scaffold", "Product", "name:string^", "price:float", "stock:integer?"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["updated"], serde_json::json!(["src/lib.rs"]));
    assert_eq!(
        report["created"],
        serde_json::json!([
            "src/models/mod.rs",
            "migrations/0001_create_products.sql",
            "src/models/product.rs",
            "src/products.rs",
            "templates/products/index.html",
            "templates/products/show.html",
            "templates/products/new.html",
            "templates/products/edit.html",
            "templates/products/_form.html",
        ])
    );
    let sql = fs::read_to_string(root.join("migrations/0001_create_products.sql")).unwrap();
    assert!(sql.contains("price REAL NOT NULL") && sql.contains("stock INTEGER,"), "{sql}");
    assert!(sql.contains("CREATE UNIQUE INDEX index_products_on_name ON products (name);"), "{sql}");
    let model = fs::read_to_string(root.join("src/models/product.rs")).unwrap();
    assert!(model.contains("\"INSERT INTO products (name, price, stock) VALUES (?1, ?2, ?3) RETURNING *\""));
    assert!(model.contains("\"has already been taken\""));
    let module = fs::read_to_string(root.join("src/products.rs")).unwrap();
    assert!(module.contains("Generated by `ocre g scaffold Product name:string^ price:float stock:integer?`"));
    assert!(module.contains("product::create(&ctx, new).await"), "handlers call the model");
    let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains("mod models;") && lib.contains("mod products;") && lib.contains(".merge(products::routes())"));

    // Human output, from a subdirectory of the app; the name gives the SQL.
    let output = sandbox.ocre(&["generate", "migration", "add_sku_to_products", "sku:string?"], &root.join("src"));
    assert_eq!(
        text(&output).0,
        "  create  migrations/0002_add_sku_to_products.sql\n\nNext:\n  ocre migrate\n  update the model in src/models/ to match the new columns\n"
    );
    let sql = fs::read_to_string(root.join("migrations/0002_add_sku_to_products.sql")).unwrap();
    assert!(sql.ends_with("\nALTER TABLE products ADD COLUMN sku TEXT;\n"), "{sql}");
}

#[test]
fn model_links_associations_both_ways() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "model", "Author", "name:string"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["next"], serde_json::json!(["ocre migrate", "cargo check --target wasm32-unknown-unknown"]));
    let (report, ok) = sandbox.json(&["g", "model", "Book", "title:string", "author:references"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["updated"], serde_json::json!(["src/models/author.rs", "src/models/mod.rs"]));
    let author = fs::read_to_string(root.join("src/models/author.rs")).unwrap();
    assert!(author.contains("pub async fn books(&self, ctx: &Ctx, page: ocre::Page)"), "{author}");
    let book = fs::read_to_string(root.join("src/models/book.rs")).unwrap();
    assert!(book.contains("pub async fn author(&self, ctx: &Ctx)"), "{book}");
    let sql = fs::read_to_string(root.join("migrations/0002_create_books.sql")).unwrap();
    assert!(sql.contains("author_id INTEGER NOT NULL REFERENCES authors(id) ON DELETE CASCADE"), "{sql}");

    let (report, _) = sandbox.json(&["g", "model", "Book", "title:string"], &root);
    assert_eq!(report["error"], "src/models/book.rs already exists");
}

#[test]
fn model_references_need_the_target_model_and_its_marker() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, _) = sandbox.json(&["g", "model", "Book", "author:references"], &root);
    assert_eq!(report["error"], "src/models/author.rs does not exist");
    assert_eq!(report["hint"], "generate the referenced model first, e.g. `ocre g model Author name:string`");

    sandbox.json(&["g", "model", "Author", "name:string"], &root);
    fs::write(root.join("src/models/author.rs"), "impl Author {}\n").unwrap();
    let (report, _) = sandbox.json(&["g", "model", "Book", "author:references"], &root);
    assert_eq!(report["error"], "src/models/author.rs is missing the `// ocre:associations` marker");

    fs::write(root.join("src/models/mod.rs"), "pub mod author;\n").unwrap();
    let (report, _) = sandbox.json(&["g", "model", "Tag", "label:string"], &root);
    assert_eq!(report["error"], "src/models/mod.rs is missing the `// ocre:models` marker");
}

#[test]
fn scaffold_converts_form_text_for_every_type() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.json(&["g", "model", "Venue", "name:string"], &root);
    let fields = [
        "title:string",
        "note:text?",
        "seats:integer",
        "price:float?",
        "day:date",
        "at:datetime",
        "venue:references?",
        "settings:json",
        "extra:json?",
    ];
    let (report, ok) = sandbox.json(&[&["g", "scaffold", "Event"][..], &fields].concat(), &root);
    assert!(ok, "{report}");
    let module = fs::read_to_string(root.join("src/events.rs")).unwrap();
    for expected in [
        "            title: self.title.clone(),",
        "            note: (!self.note.trim().is_empty()).then(|| self.note.clone()),",
        "            seats: v.number(\"seats\", &self.seats).unwrap_or_default(),",
        "            price: v.optional_number(\"price\", &self.price),",
        "            note: record.note.clone().unwrap_or_default(),",
        "            seats: record.seats.to_string(),",
        "            price: record.price.map(|value| value.to_string()).unwrap_or_default(),",
        "            settings: v.json(\"settings\", &self.settings).unwrap_or_default(),",
        "            extra: v.optional_json(\"extra\", &self.extra),",
        "            settings: record.settings.to_string(),",
        "            extra: record.extra.as_ref().map(|value| value.to_string()).unwrap_or_default(),",
    ] {
        assert!(module.contains(expected), "missing {expected:?} in\n{module}");
    }
    let form = fs::read_to_string(root.join("templates/events/_form.html")).unwrap();
    for expected in [
        r#"<input type="number" step="1" name="seats" value="{{ form.seats }}" required>"#,
        r#"<input type="number" step="any" name="price" value="{{ form.price }}">"#,
        r#"<input type="date" name="day" value="{{ form.day }}" required>"#,
        r#"<input type="datetime-local" name="at" value="{{ form.at }}" required>"#,
        r#"<label>Venue <input type="number" step="1" name="venue_id" value="{{ form.venue_id }}">"#,
        r#"<textarea name="settings" rows="5" spellcheck="false" placeholder="{}" required>{{ form.settings }}</textarea>"#,
        r#"<textarea name="extra" rows="5" spellcheck="false" placeholder="{}">{{ form.extra }}</textarea>"#,
    ] {
        assert!(form.contains(expected), "missing {expected:?} in\n{form}");
    }
    let model = fs::read_to_string(root.join("src/models/event.rs")).unwrap();
    assert!(model.contains("Some(id) => crate::models::venue::find(ctx, id).await,\n            None => Ok(None),"));
    for expected in [
        "    #[serde(deserialize_with = \"ocre::json_from_sql\")]\n    pub settings: ocre::serde_json::Value,",
        "    #[serde(deserialize_with = \"ocre::optional_json_from_sql\")]\n    pub extra: Option<ocre::serde_json::Value>,",
        "    #[serde(default)]\n    pub extra: Option<ocre::serde_json::Value>,",
        "    #[serde(default, deserialize_with = \"ocre::patch_json\")]\n    pub extra: Option<Option<ocre::serde_json::Value>>,",
    ] {
        assert!(model.contains(expected), "missing {expected:?} in\n{model}");
    }
    let migration = fs::read_to_string(root.join("migrations/0002_create_events.sql")).unwrap();
    assert!(migration.contains("    settings TEXT NOT NULL CHECK (json_valid(settings)),\n"), "{migration}");
}

#[test]
fn scaffold_human_output_lists_created_and_updated_files() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::remove_dir_all(root.join("migrations")).unwrap();
    let output = sandbox.ocre(&["g", "scaffold", "Tag", "label:string"], &root);
    let (stdout, _) = text(&output);
    assert!(stdout.starts_with("  create  src/models/mod.rs\n  create  migrations/0001_create_tags.sql\n"), "{stdout}");
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
        (&["g", "migration", "backfill", "x:string"], "cannot tell which table `backfill` changes"),
    ];
    for (args, error) in cases {
        let (report, ok) = sandbox.json(args, &root);
        assert!(!ok);
        assert_eq!(report["error"], *error, "{args:?}");
    }
}

#[test]
fn scaffold_never_overwrites_a_template() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::create_dir_all(root.join("templates/items")).unwrap();
    fs::write(root.join("templates/items/show.html"), "mine").unwrap();
    let (report, _) = sandbox.json(&["g", "scaffold", "Item", "name:string"], &root);
    assert_eq!(report["error"], "templates/items/show.html already exists");
    assert!(!root.join("src/models").exists(), "nothing written on failure");
}

#[test]
fn scaffold_needs_the_lib_markers() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::write(root.join("src/lib.rs"), "fn main() {}\n").unwrap();
    let (report, ok) = sandbox.json(&["g", "scaffold", "Item", "name:string"], &root);
    assert!(!ok);
    assert!(report["hint"].as_str().unwrap().contains("// ocre:modules"), "{report}");
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

// ---------- API mode and ocre generate api ----------

#[test]
fn new_api_creates_a_json_only_app() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("svc", &["--api", "--starter", "blog"]);
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains(", default-features = false }"), "html feature off: {cargo}");
    assert!(!cargo.contains("askama"));
    assert!(cargo.ends_with("[package.metadata.ocre]\n# JSON only: `ocre g scaffold` generates APIs, Ocre's `html` feature is off.\nmode = \"api\"\n"));
    assert!(
        fs::read_to_string(root.join("src/lib.rs")).unwrap().contains("Json(Status { app: \"svc\", status: \"ok\" })")
    );
    let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains(".route(\"/up\", get(up))") && lib.contains("async fn up() -> &'static str"), "{lib}");
    assert_dev_secret(&root);
    assert!(root.join("public/robots.txt").is_file());
    assert!(root.join("src/posts_api.rs").is_file() && !root.join("templates").exists());

    // In an API-only app, scaffold generates the JSON API.
    let (report, ok) = sandbox.json(&["g", "scaffold", "Tag", "label:string"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["command"], "generate api");
    assert!(root.join("src/tags_api.rs").is_file() && !root.join("templates/tags").exists());
}

#[test]
fn api_generates_a_rest_resource() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "api", "Product", "name:string", "stock:integer", "active:boolean"], &root);
    assert!(ok, "{report}");
    assert_eq!(
        report["created"],
        serde_json::json!([
            "src/models/mod.rs",
            "migrations/0001_create_products.sql",
            "src/models/product.rs",
            "src/products_api.rs"
        ])
    );
    assert_eq!(report["updated"], serde_json::json!(["src/lib.rs"]));
    let module = fs::read_to_string(root.join("src/products_api.rs")).unwrap();
    assert!(module.starts_with(
        "//! Products JSON API. Generated by `ocre g api Product name:string stock:integer active:boolean`."
    ));
    assert!(module.contains(".route(\"/api/products/{id}\", get(show).patch(update).delete(delete))"));
    assert!(module.contains("Ok(Created(product::create(&ctx, new).await?))"), "handlers call the model");
    assert!(!module.contains("async_graphql") && !root.join("src/graphql.rs").exists());
    let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains("mod products_api;") && lib.contains(".merge(products_api::routes())"));

    let (report, _) = sandbox.json(&["g", "api", "Product", "name:string"], &root);
    assert_eq!(report["error"], "src/products_api.rs already exists");
}

#[test]
fn api_after_scaffold_reuses_the_table() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("blog", &["--starter", "blog"]);
    let (report, ok) = sandbox.json(&["g", "api", "Post", "title:string", "body:text", "published:boolean"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["created"], serde_json::json!(["src/posts_api.rs"]), "reuses the Post model and table");
}

#[test]
fn api_with_graphql_wires_the_schema_and_dependency() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(
        &["g", "api", "Product", "name:string", "active:boolean", "note:text?", "tags:json?", "--graphql"],
        &root,
    );
    assert!(ok, "{report}");
    assert_eq!(
        report["created"],
        serde_json::json!([
            "src/models/mod.rs",
            "migrations/0001_create_products.sql",
            "src/models/product.rs",
            "src/products_api.rs",
            "src/graphql.rs"
        ])
    );
    assert_eq!(report["updated"], serde_json::json!(["src/lib.rs", "Cargo.toml"]));
    assert_eq!(report["next"][3], "open http://localhost:8787/graphql");
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("features = [\"graphql\"] }\nasync-graphql = "), "{cargo}");
    let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains("mod graphql;") && lib.contains(".merge(graphql::routes())"));
    let module = fs::read_to_string(root.join("src/products_api.rs")).unwrap();
    assert!(module.contains("async fn create_product(&self, ctx: &Context<'_>, input: NewProductInput)"));
    assert!(module.contains("    #[graphql(default)]\n    pub active: bool,"), "unchecked = false");
    assert!(module.contains("    pub note: MaybeUndefined<String>,"), "null clears, missing keeps");
    assert!(module.contains("    pub tags: MaybeUndefined<ocre::serde_json::Value>,"), "the JSON scalar");
    assert!(module.contains("MaybeUndefined::Null => Some(None),"));

    // A second GraphQL resource joins the existing schema.
    let (report, ok) = sandbox.json(&["g", "api", "Order", "total:float", "--graphql"], &root);
    assert!(ok, "{report}");
    assert_eq!(
        report["updated"],
        serde_json::json!(["src/models/mod.rs", "src/lib.rs", "Cargo.toml", "src/graphql.rs"])
    );
    let schema = fs::read_to_string(root.join("src/graphql.rs")).unwrap();
    assert!(schema.contains("crate::orders_api::OrderQuery,\n    crate::products_api::ProductQuery,"));
    assert_eq!(fs::read_to_string(root.join("Cargo.toml")).unwrap(), cargo, "dependency added once");
}

#[test]
fn api_graphql_refuses_what_it_cannot_edit() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(root.join("Cargo.toml"), cargo.replace("ocre = {", "ocre.git = \"x\" # {")).unwrap();
    let (report, _) = sandbox.json(&["g", "api", "Product", "name:string", "--graphql"], &root);
    assert_eq!(report["error"], "Cargo.toml has no one-line `ocre = { ... }` dependency");
    assert!(!root.join("src/products_api.rs").exists(), "nothing written on failure");

    fs::write(root.join("Cargo.toml"), cargo).unwrap();
    fs::write(root.join("src/graphql.rs"), "pub struct Query();\n").unwrap();
    let (report, _) = sandbox.json(&["g", "api", "Product", "name:string", "--graphql"], &root);
    assert!(report["error"].as_str().unwrap().starts_with("src/graphql.rs is missing"));
}

#[test]
fn api_human_output_and_full_stack_without_cargo_metadata() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::remove_file(root.join("Cargo.toml")).unwrap();
    // No Cargo.toml: treated as full-stack, so scaffold makes HTML pages.
    let (report, ok) = sandbox.json(&["g", "scaffold", "Tag", "label:string"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["command"], "generate scaffold");
    let output = sandbox.ocre(&["g", "api", "Item", "name:string"], &root);
    assert!(text(&output).0.ends_with("Next:\n  ocre migrate\n  ocre dev\n  curl http://localhost:8787/api/items\n"));
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
    sandbox.set("has_secret");
    let output = sandbox.ocre(&["deploy"], &root);
    let (stdout, _) = text(&output);
    assert!(output.status.success());
    assert!(
        stdout.ends_with("Uploaded app\n  https://app.example.workers.dev\n\nhttps://app.example.workers.dev\n"),
        "{stdout}"
    );
    assert_eq!(
        sandbox.calls(),
        [
            "secret list --format json",
            "d1 list --json",
            "d1 migrations apply shop --remote",
            "deploy",
            "build --release"
        ]
    );
}

#[test]
fn deploy_creates_secret_key_base_only_when_the_worker_has_none() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let secrets_file = root.join(".wrangler/ocre-secrets.env");
    let deploy_calls = || sandbox.calls().into_iter().filter(|call| call.starts_with("deploy")).collect::<Vec<_>>();

    // Deployed Worker without the secret; a stale file from a killed deploy is replaced.
    fs::create_dir_all(secrets_file.parent().unwrap()).unwrap();
    fs::write(&secrets_file, "SECRET_KEY_BASE=stale\n").unwrap();
    let output = sandbox.ocre(&["deploy"], &root);
    let (stdout, _) = text(&output);
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.ends_with("Created the SECRET_KEY_BASE secret on Cloudflare\n\nhttps://app.example.workers.dev\n"),
        "{stdout}"
    );
    assert!(sandbox.calls().contains(&"secrets file ok".to_owned()));
    assert_eq!(deploy_calls(), ["deploy --secrets-file .wrangler/ocre-secrets.env"]);
    assert!(!secrets_file.exists(), "deleted after the deploy");

    // No Worker yet: `secret list` fails with "not found".
    sandbox.set("secret_list_fails");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["secret_created"], true);
    assert_eq!(report.as_object().unwrap().len(), 4, "the secret itself is never reported: {report}");

    // The Worker has one: never replaced.
    fs::remove_file(sandbox.work.join("../state/secret_list_fails")).unwrap();
    sandbox.set("has_secret");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok, "{report}");
    assert!(report.get("secret_created").is_none(), "{report}");
    assert_eq!(deploy_calls().last().unwrap(), "deploy");
}

#[test]
fn secret_prints_a_new_random_secret() {
    let sandbox = Sandbox::new();
    let output = sandbox.ocre(&["secret"], &sandbox.work);
    let (stdout, _) = text(&output);
    assert!(output.status.success());
    let secret = stdout.strip_suffix('\n').unwrap();
    assert!(secret.len() == 128 && secret.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')), "{stdout}");

    let (report, ok) = sandbox.json(&["secret"], &sandbox.work);
    assert!(ok);
    assert_eq!(report["command"], "secret");
    assert_eq!(report["secret"].as_str().unwrap().len(), 128);
    assert_ne!(report["secret"], secret, "a new secret every time");
}

#[test]
fn deploy_without_a_workers_dev_url_reports_none() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.script(
        "npx",
        &include_str!("../support/fake_npx.sh")
            .replace("https://app.example.workers.dev", "https://custom.example.com"),
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
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .starts_with("`wrangler deploy --secrets-file .wrangler/ocre-secrets.env` failed")
    );
    assert!(!root.join(".wrangler/ocre-secrets.env").exists(), "deleted after a failed deploy");

    // Only a missing Worker means "no secret yet"; other failures stop the deploy.
    sandbox.set("secret_list_errors");
    let (report, _) = sandbox.json(&["deploy"], &root);
    assert_eq!(report["error"], "`wrangler secret list` failed: ✘ [ERROR] secret_list_errors");
    assert!(report["hint"].as_str().unwrap().contains("only creates SECRET_KEY_BASE when sure"));

    fs::remove_file(sandbox.work.join("../state/secret_list_errors")).unwrap();
    sandbox.write_state("secret_list.json", "oops");
    let (report, _) = sandbox.json(&["deploy"], &root);
    assert!(report["error"].as_str().unwrap().starts_with("unexpected `wrangler secret list` output"));
    assert_eq!(sandbox.calls().last().unwrap(), "secret list --format json", "nothing ran after the failed check");
}
