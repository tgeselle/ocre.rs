//! Non-interactive CLI behaviour, run through the real binary with a fake
//! cf, wrangler and npm. Covers the `--json` contract, human output and every failure hint.

#[path = "../support/mod.rs"]
mod support;

use std::fs;

use support::{Sandbox, local_d1, ocre_crate, text};

// ---------- ocre new ----------

#[test]
fn new_creates_an_app_and_installs_its_npm_packages() {
    let sandbox = Sandbox::new();
    let ocre = ocre_crate();
    let (report, ok) = sandbox.json(&["new", "shop", "--ocre-path", ocre.to_str().unwrap()], &sandbox.work);
    assert!(ok, "{report}");
    assert_eq!(report["command"], "new");
    assert_eq!(report["next"], serde_json::json!(["cd shop", "ocre dev", "ocre deploy"]));
    let created = report["created"].as_array().unwrap();
    for file in [
        "shop/cloudflare.config.ts",
        "shop/wrangler.config.ts",
        "shop/package.json",
        "shop/tsconfig.json",
        "shop/AGENTS.md",
        "shop/public/robots.txt",
        "shop/.dev.vars",
    ] {
        assert!(created.contains(&file.into()), "{file} in {created:?}");
    }
    let root = sandbox.work.join("shop");
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("name = \"shop\""));
    assert!(cargo.contains(&format!("ocre = {{ path = {:?} }}", ocre.display().to_string())));
    assert!(!root.join("wrangler.toml").exists());
    let config = fs::read_to_string(root.join("cloudflare.config.ts")).unwrap();
    assert!(config.contains("\t\tname: \"shop\",\n"), "{config}");
    assert!(config.contains("DB: bindings.d1({ name: \"shop\" }),") && !config.contains("accountId"), "{config}");
    let build = fs::read_to_string(root.join("wrangler.config.ts")).unwrap();
    assert!(build.contains("assetsDirectory: \"public\""), "{build}");
    let package: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("package.json")).unwrap()).unwrap();
    assert_eq!((&package["name"], &package["type"]), (&"shop".into(), &"module".into()), "{package}");
    for dependency in ["cf", "wrangler", "typescript"] {
        let version = package["devDependencies"][dependency].as_str().unwrap();
        assert!(version.starts_with(|c: char| c.is_ascii_digit()), "exact pin for {dependency}: {package}");
    }
    assert!(!root.join(".git").exists());
    assert_dev_secret(&root);
    let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains(".route(\"/up\", get(up))\n        // ocre:routes"), "{lib}");
    assert!(fs::read_to_string(root.join("public/robots.txt")).unwrap().contains("User-agent: *"));
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(gitignore.contains("\n.dev.vars\n.dev.vars.*\n"), "{gitignore}");
    assert!(gitignore.contains("\nnode_modules/\n") && gitignore.contains("\n.cloudflare/\n"), "{gitignore}");
    assert!(gitignore.contains("\n.env\n.env.*\n"), "cf's credentials: {gitignore}");
    let agents = fs::read_to_string(root.join("AGENTS.md")).unwrap();
    assert!(
        agents.starts_with("# shop\n") && agents.contains("`https://ocre-docs.raitomm.workers.dev/llms.txt`"),
        "{agents}"
    );
    for file in ["AGENTS.md", "cloudflare.config.ts", "package.json"] {
        let text = fs::read_to_string(root.join(file)).unwrap();
        assert!(!text.contains("__"), "no template placeholder left in {file}: {text}");
    }
    // The pinned cf and wrangler are installed in the app.
    assert!(root.join("node_modules/.bin/cf").is_file() && root.join("package-lock.json").is_file());
    assert!(report["ran"][0].as_str().unwrap().starts_with("npm install (cf "), "{report}");
    assert_eq!(sandbox.calls(), ["npm install"], "no cf call without --login/--deploy");
}

/// `.dev.vars` holds a fresh 128-hex SECRET_KEY_BASE and MAIL_ADAPTER=log for `ocre dev`.
fn assert_dev_secret(root: &std::path::Path) {
    let vars = fs::read_to_string(root.join(".dev.vars")).unwrap();
    let secret =
        vars.strip_prefix("SECRET_KEY_BASE=").and_then(|rest| rest.strip_suffix("\nMAIL_ADAPTER=log\n")).unwrap();
    assert!(secret.len() == 128 && secret.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')), "{vars}");
    let config = fs::read_to_string(root.join("cloudflare.config.ts")).unwrap();
    assert!(config.contains("\n\t\t\tMAIL_FROM: bindings.text(\""), "{config}");
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
    assert!(stdout.contains("  create  shop/cloudflare.config.ts"), "{stdout}");
    assert!(stdout.contains("\n  npm install (cf "), "{stdout}");
    assert!(stdout.contains("Next:\n  cd shop\n  ocre dev\n  ocre deploy"), "{stdout}");
}

#[test]
fn new_needs_npm_unless_told_not_to_install() {
    let mut sandbox = Sandbox::new();
    sandbox.remove_tool("npm");
    sandbox.isolate_path();
    let (report, ok) = sandbox.json(&["new", "shop"], &sandbox.work);
    assert!(!ok);
    assert_eq!(report["error"], "npm not found");
    assert!(report["hint"].as_str().unwrap().contains("--no-install"), "{report}");
    assert!(!sandbox.work.join("shop").exists(), "nothing written");

    let (report, ok) = sandbox.json(&["new", "shop", "--no-install"], &sandbox.work);
    assert!(ok, "{report}");
    assert!(report.get("ran").is_none(), "npm did not run: {report}");
    let root = sandbox.work.join("shop");
    assert!(root.join("package.json").is_file() && !root.join("node_modules").exists());
    assert!(sandbox.calls().is_empty());
}

#[test]
fn new_reports_a_failing_npm_install() {
    let sandbox = Sandbox::new();
    sandbox.set("npm_install_fails");
    let (report, ok) = sandbox.json(&["new", "shop"], &sandbox.work);
    assert!(!ok);
    let error = report["error"].as_str().unwrap();
    assert!(error.starts_with("`npm install` failed in ") && error.contains("npm_install_fails"), "{error}");
    assert!(report["hint"].as_str().unwrap().starts_with("the app is created"), "{report}");
    assert!(sandbox.work.join("shop/package.json").is_file(), "kept for a later `npm install`");
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

    // Deploying needs the npm packages: refused before logging in or writing.
    let (report, _) = sandbox.json(&["new", "shop", "--deploy", "--no-install"], &sandbox.work);
    assert_eq!(report["error"], "--deploy needs the app's npm packages");
    assert!(report["hint"].as_str().unwrap().contains("npm install && ocre deploy"), "{report}");
    assert!(!sandbox.work.join("shop").exists());
    assert!(sandbox.calls().is_empty());
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
    assert_eq!(sandbox.calls(), ["cf auth whoami", "cf auth login", "cf auth whoami", "npm install"]);
}

#[test]
fn new_with_login_skips_the_login_when_already_logged_in() {
    let sandbox = Sandbox::new();
    sandbox.login_as(&[("acc1", "Main")]);
    let (report, ok) = sandbox.json(&["new", "shop", "--login"], &sandbox.work);
    assert!(ok, "{report}");
    assert_eq!(sandbox.calls(), ["cf auth whoami", "npm install"]);
}

#[test]
fn new_with_login_fails_when_the_login_does_not_complete() {
    let sandbox = Sandbox::new();
    sandbox.set("login_does_nothing");
    let (report, ok) = sandbox.json(&["new", "shop", "--login"], &sandbox.work);
    assert!(!ok);
    assert_eq!(report["error"], "Cloudflare login did not complete");
    assert!(!sandbox.work.join("shop").exists(), "nothing written");

    sandbox.set("login_fails");
    let (report, _) = sandbox.json(&["new", "shop", "--login"], &sandbox.work);
    assert!(report["error"].as_str().unwrap().starts_with("`cf auth login` failed"), "{report}");
    assert!(report["hint"].as_str().unwrap().contains("cf output above"));
}

#[test]
fn new_needs_an_account_id_when_the_login_has_several_accounts() {
    let sandbox = Sandbox::new();
    sandbox.login_as(&[("acc1", "Main"), ("acc2", "Side")]);
    let (report, ok) = sandbox.json(&["new", "shop", "--login"], &sandbox.work);
    assert!(!ok);
    assert_eq!(report["hint"], "pass --account-id with one of: acc1 (Main), acc2 (Side)");

    let (report, _) = sandbox.json(&["new", "shop", "--login", "--account-id", "acc3"], &sandbox.work);
    assert_eq!(report["error"], "account `acc3` is not available to this Cloudflare login");
    assert!(!sandbox.work.join("shop").exists());

    let (report, ok) = sandbox.json(&["new", "shop", "--login", "--account-id", "acc2"], &sandbox.work);
    assert!(ok, "{report}");
    let config = fs::read_to_string(sandbox.work.join("shop/cloudflare.config.ts")).unwrap();
    assert!(config.contains("defineConfig({\n\taccountId: \"acc2\",\n\tworker: {\n"), "{config}");
}

#[test]
fn new_keeps_an_account_id_given_without_login() {
    let sandbox = Sandbox::new();
    sandbox.new_app("shop", &["--account-id", "acc9"]);
    let config = fs::read_to_string(sandbox.work.join("shop/cloudflare.config.ts")).unwrap();
    assert!(config.contains("\taccountId: \"acc9\",\n\tworker: {\n"), "{config}");
}

#[test]
fn new_with_deploy_logs_in_deploys_and_returns_the_url() {
    let sandbox = Sandbox::new();
    sandbox.login_as(&[("acc1", "Main")]);
    let (report, ok) =
        sandbox.json(&["new", "shop", "--deploy", "--ocre-path", ocre_crate().to_str().unwrap()], &sandbox.work);
    assert!(ok, "{report}");
    assert_eq!(report["url"], "https://shop.example.workers.dev");
    assert_eq!(report["next"], serde_json::json!(["cd shop", "ocre dev"]));
    assert_eq!(report["secret_created"], true, "a new Worker gets SECRET_KEY_BASE");
    assert_eq!(report["provisioned"], serde_json::json!(["D1 database shop"]));
    assert_eq!(
        sandbox.calls(),
        [
            "cf auth whoami",
            "npm install",
            "cf d1 list --name shop",
            "cf d1 create --name shop",
            "cf workers secrets list --worker shop",
            "cf d1 migrations apply uuid-shop",
            "cf deploy --secrets-file .wrangler/ocre-secrets.env",
            "secrets file ok",
            "build --release",
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
    assert_eq!(sandbox.calls(), ["cf auth whoami"]);
}

#[test]
fn login_runs_cf_auth_login_and_streams_its_output() {
    let sandbox = Sandbox::new();
    sandbox.accounts_after_login(&[("acc1", "Main")]);
    let output = sandbox.ocre(&["login"], &sandbox.work);
    let (stdout, _) = text(&output);
    assert!(stdout.starts_with("Successfully logged in.\n"), "{stdout}");
    assert_eq!(sandbox.calls(), ["cf auth whoami", "cf auth login", "cf auth whoami"]);

    // With --json, cf's output moves to stderr.
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
    assert!(stderr.starts_with("error: unexpected `cf auth whoami` output"), "{stderr}");
    assert!(!stderr.contains("hint:"));
}

#[test]
fn a_logged_out_or_failing_whoami_means_no_session() {
    // cf answers `{"authenticated": false, ...}` with exit 0 when logged out.
    let sandbox = Sandbox::new();
    sandbox.set("login_does_nothing");
    let (report, _) = sandbox.json(&["login"], &sandbox.work);
    assert_eq!(report["error"], "Cloudflare login did not complete");
    assert_eq!(sandbox.calls(), ["cf auth whoami", "cf auth login", "cf auth whoami"]);

    sandbox.set("whoami_fails");
    let (report, _) = sandbox.json(&["login"], &sandbox.work);
    assert_eq!(report["error"], "Cloudflare login did not complete");
}

#[test]
fn commands_explain_a_missing_node() {
    let mut sandbox = Sandbox::new();
    sandbox.remove_tool("npx");
    sandbox.isolate_path();
    let (report, ok) = sandbox.json(&["login"], &sandbox.work);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with("could not run cf"), "{report}");
    assert_eq!(report["hint"], "install Node.js 22 or newer, then run `npm install` in the app");
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
            "tests/factories/mod.rs",
            "tests/factories/product.rs",
            "src/products.rs",
            "templates/products/index.html",
            "templates/products/show.html",
            "templates/products/new.html",
            "templates/products/edit.html",
            "templates/products/_form.html",
            "tests/products.rs",
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
    assert_eq!(
        report["updated"],
        serde_json::json!(["src/models/author.rs", "src/models/mod.rs", "tests/factories/mod.rs"])
    );
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
        (&["g", "scaffold", "Item", "lock_version:string"], "`lock_version` must be `lock_version:integer`"),
        (&["g", "scaffold", "Item", "body:rich_text^"], "rich_text field `body` cannot be unique"),
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
fn commands_explain_a_missing_legacy_or_broken_config() {
    let sandbox = Sandbox::new();
    let (report, _) = sandbox.json(&["g", "migration", "x"], &sandbox.work);
    assert_eq!(report["error"], "no cloudflare.config.ts found in this directory or its parents");
    assert!(report["hint"].as_str().unwrap().contains("ocre new <name>"));

    // An app from before cf: converted with `cf migrate`, never read half-way.
    fs::write(sandbox.work.join("wrangler.toml"), "name = \"old\"\n").unwrap();
    let (report, _) = sandbox.json(&["g", "migration", "x"], &sandbox.work);
    assert!(
        report["error"].as_str().unwrap().ends_with(" uses wrangler.toml; Ocre now reads cloudflare.config.ts"),
        "{report}"
    );
    assert!(report["hint"].as_str().unwrap().contains("`npx cf migrate --no-install`"), "{report}");
    assert!(!sandbox.work.join("migrations").exists(), "nothing written");

    let config = "export default defineConfig({\n\tworker: {\n\t\tname: \"x\",\n\t\tenv: {},\n\t},\n});\n";
    fs::write(sandbox.work.join("cloudflare.config.ts"), config).unwrap();
    let (report, _) = sandbox.json(&["g", "migration", "x"], &sandbox.work);
    assert_eq!(report["error"], "cloudflare.config.ts has no D1 database bound to `DB`");
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
            "tests/factories/mod.rs",
            "tests/factories/product.rs",
            "src/products_api.rs",
            "tests/api_products.rs"
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
    assert_eq!(
        report["created"],
        serde_json::json!(["src/posts_api.rs", "tests/api_posts.rs"]),
        "reuses the Post model and table"
    );
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
            "tests/factories/mod.rs",
            "tests/factories/product.rs",
            "src/products_api.rs",
            "tests/api_products.rs",
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
        serde_json::json!(["src/models/mod.rs", "tests/factories/mod.rs", "src/lib.rs", "src/graphql.rs"]),
        "Cargo.toml already has the dependency: unchanged, not reported"
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
    assert_eq!(text(&output).0, "Migrations applied to DB (--local)\n");
    // wrangler reads the local database from a config derived from cloudflare.config.ts.
    let derived: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(".wrangler/ocre-d1.json")).unwrap()).unwrap();
    assert_eq!(
        derived["d1_databases"],
        serde_json::json!([{"binding": "DB", "database_name": "shop", "migrations_dir": "../migrations"}])
    );

    // The remote database is created by the first deploy, not by migrate.
    let (report, ok) = sandbox.json(&["migrate", "--remote"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "the D1 database shop does not exist on Cloudflare yet");
    assert!(report["hint"].as_str().unwrap().contains("ocre deploy"), "{report}");

    sandbox.remote_database("shop");
    let (report, ok) = sandbox.json(&["migrate", "--remote"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["remote"], true);
    assert_eq!(
        sandbox.calls(),
        [
            local_d1("d1 migrations apply DB --local"),
            "cf d1 list --name shop".to_owned(),
            "cf d1 list --name shop".to_owned(),
            "cf d1 migrations apply uuid-shop".to_owned(),
        ]
    );
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
    assert!(stderr.contains("error: `wrangler d1 migrations apply DB --local` failed"), "{stderr}");
    assert!(stderr.contains("hint: read the wrangler output above"));
}

#[test]
fn dev_migrates_then_serves() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["dev", "--port", "9123"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["url"], "http://localhost:9123");
    assert_eq!(
        sandbox.calls(),
        [local_d1("d1 migrations apply DB --local"), "cf dev --port 9123".to_owned(), "build --dev".to_owned()]
    );
}

#[test]
fn dev_and_deploy_need_the_npm_packages() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::remove_dir_all(root.join("node_modules")).unwrap();
    for command in ["dev", "deploy"] {
        let (report, ok) = sandbox.json(&[command], &root);
        assert!(!ok);
        assert_eq!(
            report["error"],
            "the app's npm packages are not installed (node_modules/.bin/cf, node_modules/.bin/wrangler)"
        );
        assert!(report["hint"].as_str().unwrap().starts_with("run `npm install` in "), "{report}");
    }
    assert!(sandbox.calls().is_empty());
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
    // rustup is installed, but another rustc comes first in PATH.
    sandbox.script("rustup", "#!/bin/sh\necho 'rustup 1.28.2'\n");
    let (report, _) = sandbox.json(&["dev"], &root);
    assert!(
        report["hint"].as_str().unwrap().starts_with("this rustc is not rustup's but comes first in PATH"),
        "{report}"
    );
    sandbox.script("rustup", "#!/bin/sh\nexit 1\n");
    let (report, _) = sandbox.json(&["dev"], &root);
    assert!(report["hint"].as_str().unwrap().starts_with("use a rustup toolchain"), "{report}");
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
    sandbox.remote_database("shop");
    sandbox.set("has_secret");
    let output = sandbox.ocre(&["deploy"], &root);
    let (stdout, _) = text(&output);
    assert!(output.status.success());
    assert!(
        stdout.contains("│    https://shop.example.workers.dev\n")
            && stdout.ends_with("\n\nhttps://shop.example.workers.dev\n"),
        "{stdout}"
    );
    assert_eq!(
        sandbox.calls(),
        [
            "cf d1 list --name shop",
            "cf workers secrets list --worker shop",
            "cf d1 migrations apply uuid-shop",
            "cf deploy",
            "build --release"
        ]
    );

    // A failed migration stops the deploy: the old code keeps running.
    sandbox.clear_calls();
    sandbox.set("migrate_fails");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with("`cf d1 migrations apply uuid-shop` failed"), "{report}");
    assert_eq!(sandbox.calls().last().unwrap(), "cf d1 migrations apply uuid-shop");
}

#[test]
fn deploy_creates_secret_key_base_only_when_the_worker_has_none() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let secrets_file = root.join(".wrangler/ocre-secrets.env");
    let deploy_calls = || sandbox.calls().into_iter().filter(|call| call.starts_with("cf deploy")).collect::<Vec<_>>();

    // Deployed Worker without the secret, no database yet; a stale file
    // from a killed deploy is replaced.
    fs::create_dir_all(secrets_file.parent().unwrap()).unwrap();
    fs::write(&secrets_file, "SECRET_KEY_BASE=stale\n").unwrap();
    let output = sandbox.ocre(&["deploy"], &root);
    let (stdout, _) = text(&output);
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.ends_with(
            "Created the SECRET_KEY_BASE secret on Cloudflare\nSaved it in .prod.vars (git-ignored): back it up, Cloudflare never gives it back\nCreated D1 database shop on Cloudflare\n\nhttps://shop.example.workers.dev\n"
        ),
        "{stdout}"
    );
    assert!(sandbox.calls().contains(&"secrets file ok".to_owned()));
    assert_eq!(deploy_calls(), ["cf deploy --secrets-file .wrangler/ocre-secrets.env"]);
    assert!(!secrets_file.exists(), "deleted after the deploy");
    // The only copy: kept in .prod.vars, readable by its owner only.
    let uploaded = fs::read_to_string(sandbox.work.join("../state/uploaded_secrets")).unwrap();
    let prod_vars = fs::read_to_string(root.join(".prod.vars")).unwrap();
    assert!(prod_vars.starts_with("# Created by `ocre deploy`") && prod_vars.ends_with(&uploaded), "{prod_vars}");
    let mode = std::os::unix::fs::PermissionsExt::mode(&fs::metadata(root.join(".prod.vars")).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);

    // No Worker yet: `cf workers secrets list` fails with API code 10007.
    sandbox.set("secret_list_fails");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["secret_created"], true);
    assert!(report.get("provisioned").is_none(), "the database exists now: {report}");
    assert_eq!(report.as_object().unwrap().len(), 4, "the secret itself is never reported: {report}");
    assert!(report.get("secret_saved").is_none(), "the one of .prod.vars is uploaded again: {report}");
    assert_eq!(fs::read_to_string(sandbox.work.join("../state/uploaded_secrets")).unwrap(), uploaded);

    // The Worker has one: never replaced.
    fs::remove_file(sandbox.work.join("../state/secret_list_fails")).unwrap();
    sandbox.set("has_secret");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok, "{report}");
    assert!(report.get("secret_created").is_none(), "{report}");
    assert_eq!(deploy_calls().last().unwrap(), "cf deploy");
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
    sandbox.set("deploy_no_url");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok, "{report}");
    assert!(report.get("url").is_none());
}

#[test]
fn deploy_failures_carry_hints() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("d1_list_fails");
    let (report, _) = sandbox.json(&["deploy"], &root);
    assert_eq!(report["error"], "`cf d1 list --name shop` failed: ┌ Error\n│ d1_list_fails\n└");
    assert!(report["hint"].as_str().unwrap().contains("ocre login"));

    fs::remove_file(sandbox.work.join("../state/d1_list_fails")).unwrap();
    sandbox.set("d1_create_no_uuid");
    let (report, _) = sandbox.json(&["deploy"], &root);
    assert!(
        report["error"].as_str().unwrap().starts_with("`cf d1 create --name shop` returned no uuid: {\"created_at\""),
        "{report}"
    );
    assert!(report["hint"].as_str().unwrap().contains("run `ocre deploy` again"), "{report}");

    // The database exists now; the deploy itself fails.
    sandbox.set("deploy_fails");
    let (report, _) = sandbox.json(&["deploy"], &root);
    assert!(
        report["error"].as_str().unwrap().starts_with("`cf deploy --secrets-file .wrangler/ocre-secrets.env` failed")
    );
    assert!(!root.join(".wrangler/ocre-secrets.env").exists(), "deleted after a failed deploy");

    // Only a missing Worker means "no secret yet"; other failures stop the deploy.
    sandbox.clear_calls();
    sandbox.set("secret_list_errors");
    let (report, _) = sandbox.json(&["deploy"], &root);
    assert_eq!(
        report["error"],
        "`cf workers secrets list --worker shop` failed: ┌ APIError\n│ [10000] Authentication error\n│ 401 Unauthorized\n└"
    );
    assert!(report["hint"].as_str().unwrap().contains("only creates SECRET_KEY_BASE when sure"));
    assert_eq!(sandbox.calls(), ["cf d1 list --name shop", "cf workers secrets list --worker shop"]);

    fs::remove_file(sandbox.work.join("../state/secret_list_errors")).unwrap();
    sandbox.write_state("secret_list.json", "oops");
    let (report, _) = sandbox.json(&["deploy"], &root);
    assert!(report["error"].as_str().unwrap().starts_with("unexpected `cf workers secrets list` output"));
    let last = sandbox.calls().pop().unwrap();
    assert_eq!(last, "cf workers secrets list --worker shop", "nothing ran after the failed check");
}

#[test]
fn time_zones_lists_the_iana_names() {
    let sandbox = Sandbox::new();
    let (report, ok) = sandbox.json(&["time-zones"], &sandbox.work);
    assert!(ok, "{report}");
    let zones = report["time_zones"].as_array().unwrap();
    assert!(zones.len() > 400 && zones.contains(&serde_json::json!("Europe/Paris")));
    let output = sandbox.ocre(&["time-zones"], &sandbox.work);
    assert!(String::from_utf8(output.stdout).unwrap().lines().any(|line| line == "America/New_York"));
}
