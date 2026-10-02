//! Generator infrastructure: --pretend/--force/--skip, generation records and
//! `ocre destroy`, `ocre g controller|resource|override|generator`, app
//! generators, and the field types they share.

#[path = "../support/mod.rs"]
mod support;

use std::{fs, path::Path};

use serde_json::{Value, json};
use support::{Sandbox, text};

fn read(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("{path}: {err}"))
}

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

fn records(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root.join(".ocre/generated"))
        .map(|dir| dir.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

// ---------- --pretend, --force, --skip ----------

#[test]
fn pretend_reports_without_writing_anything() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let lib = read(&root, "src/lib.rs");
    let report = ok(&sandbox, &["g", "--pretend", "controller", "Pages", "about"], &root);
    assert_eq!(report["pretend"], true);
    assert_eq!(report["created"], json!(["src/pages.rs", "templates/pages/about.html"]));
    assert_eq!(report["updated"], json!(["src/lib.rs"]));
    assert!(!root.join("src/pages.rs").exists() && !root.join(".ocre").exists());
    assert_eq!(read(&root, "src/lib.rs"), lib);
    let output = sandbox.ocre(&["g", "controller", "Pages", "about", "--pretend"], &root);
    assert!(text(&output).0.contains("(--pretend: nothing was written)"), "{}", text(&output).0);
}

#[test]
fn existing_files_fail_unless_forced_or_skipped() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    ok(&sandbox, &["g", "controller", "Pages", "about"], &root);
    fs::write(root.join("templates/pages/about.html"), "mine").unwrap();
    let report = fails(&sandbox, &["g", "controller", "Pages", "about", "contact"], &root);
    assert_eq!(report["error"], "src/pages.rs already exists");
    assert!(report["hint"].as_str().unwrap().contains("--skip"));

    let report = ok(&sandbox, &["g", "controller", "Pages", "about", "contact", "--skip"], &root);
    assert_eq!(report["skipped"], json!(["src/pages.rs", "templates/pages/about.html"]));
    assert_eq!(report["created"], json!(["templates/pages/contact.html"]));
    assert_eq!(read(&root, "templates/pages/about.html"), "mine");

    let report = ok(&sandbox, &["g", "controller", "Pages", "about", "contact", "--force"], &root);
    assert_eq!(
        report["updated"],
        json!(["src/pages.rs", "templates/pages/about.html"]),
        "unchanged files are not rewritten"
    );
    assert!(read(&root, "src/pages.rs").contains("fn contact()"));
    assert!(read(&root, "templates/pages/about.html").contains("<h1>About</h1>"));
    let lib = read(&root, "src/lib.rs");
    assert_eq!(lib.matches("mod pages;").count(), 1, "markers are not inserted twice: {lib}");
    assert_eq!(lib.matches(".merge(pages::routes())").count(), 1);
    let (stdout, _) = text(&sandbox.ocre(&["g", "controller", "Pages", "about", "--skip"], &root));
    assert!(stdout.starts_with("  skip    src/pages.rs\n  skip    templates/pages/about.html\n\nNext:"), "{stdout}");

    let (_, stderr) = text(&sandbox.ocre(&["g", "controller", "Pages", "--force", "--skip"], &root));
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

// ---------- records and ocre destroy ----------

#[test]
fn every_run_is_recorded_and_destroy_takes_it_back() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let lib = read(&root, "src/lib.rs");
    ok(&sandbox, &["g", "model", "Author", "name:string"], &root);
    ok(&sandbox, &["g", "scaffold", "BlogPost", "title:string", "author:references"], &root);
    assert_eq!(records(&root), ["0001_model_author.json", "0002_scaffold_blogpost.json"]);
    let record: Value = serde_json::from_str(&read(&root, ".ocre/generated/0002_scaffold_blogpost.json")).unwrap();
    assert_eq!(record["command"], "ocre g scaffold BlogPost title:string author:references");
    assert_eq!((&record["generator"], &record["name"]), (&json!("scaffold"), &json!("BlogPost")));
    assert_eq!(record["created"][0]["path"], "migrations/0002_create_blog_posts.sql");
    assert_eq!(record["created"][0]["sha256"].as_str().unwrap().len(), 64);
    let author = &record["updated"][0];
    assert_eq!(author["path"], "src/models/author.rs");
    assert_eq!(author["hunks"][0]["after"], "    // ocre:associations");

    let report = ok(&sandbox, &["destroy", "scaffold", "blog-post", "--pretend"], &root);
    assert_eq!(report["pretend"], true);
    assert!(root.join("src/blog_posts.rs").exists());

    let report = ok(&sandbox, &["d", "scaffold", "blog_post"], &root);
    assert_eq!(
        report["updated"],
        json!(["src/models/author.rs", "src/models/mod.rs", "tests/factories/mod.rs", "src/lib.rs"])
    );
    assert!(report["removed"].as_array().unwrap().contains(&json!("templates/blog_posts/index.html")));
    assert_eq!(report["removed"].as_array().unwrap().last().unwrap(), ".ocre/generated/0002_scaffold_blogpost.json");
    assert!(report["next"][0].as_str().unwrap().contains("migrations/0002_create_blog_posts.sql"));
    assert!(!root.join("templates/blog_posts").exists(), "empty directories go too");
    assert!(!read(&root, "src/models/author.rs").contains("blog_posts"));

    ok(&sandbox, &["destroy", "model"], &root);
    assert_eq!(read(&root, "src/lib.rs"), lib, "lib.rs is back to what `ocre new` wrote");
    assert!(!root.join("src/models").exists() && !root.join(".ocre").exists());
}

#[test]
fn destroy_refuses_changed_files_unless_forced() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    ok(&sandbox, &["g", "controller", "Pages", "about"], &root);
    ok(&sandbox, &["g", "controller", "Help"], &root);
    fs::write(root.join("templates/pages/about.html"), "edited").unwrap();
    let lib = read(&root, "src/lib.rs").replace(".merge(pages::routes())", ".merge(pages::routes()) // edited");
    fs::write(root.join("src/lib.rs"), lib).unwrap();
    let report = fails(&sandbox, &["destroy", "controller", "Pages"], &root);
    assert_eq!(
        report["error"],
        "cannot destroy `ocre g controller Pages about`: templates/pages/about.html changed since it was generated; src/lib.rs: the generated `.merge(pages::routes())` changed"
    );
    assert!(root.join("src/pages.rs").exists());

    let report = ok(&sandbox, &["destroy", "controller", "Pages", "--force"], &root);
    assert_eq!(report["skipped"], json!(["src/lib.rs"]));
    assert!(!root.join("templates/pages").exists());
    let lib = read(&root, "src/lib.rs");
    assert!(!lib.contains("mod pages;") && lib.contains("// edited") && lib.contains("mod help;"), "{lib}");

    fs::remove_file(root.join("src/lib.rs")).unwrap();
    let report = fails(&sandbox, &["destroy", "controller"], &root);
    assert!(report["error"].as_str().unwrap().ends_with("src/lib.rs no longer exists"), "{report}");
    fs::remove_file(root.join("src/help.rs")).unwrap();
    ok(&sandbox, &["destroy", "controller", "--force"], &root);
}

#[test]
fn destroy_keeps_cloudflare_config_changes() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    ok(&sandbox, &["g", "cache"], &root);
    let (cargo, config) = (read(&root, "Cargo.toml"), read(&root, "cloudflare.config.ts"));
    assert!(config.contains("CACHE: bindings.kv(),"), "{config}");
    let report = ok(&sandbox, &["destroy", "cache"], &root);
    assert_eq!(report["skipped"], json!(["cloudflare.config.ts"]));
    assert_eq!((read(&root, "Cargo.toml"), read(&root, "cloudflare.config.ts")), (cargo, config));
}

#[test]
fn destroy_names_what_it_can_undo() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = fails(&sandbox, &["destroy", "scaffold", "Post"], &root);
    assert_eq!(report["error"], "no recorded `ocre g scaffold Post` run to destroy");
    assert!(report["hint"].as_str().unwrap().contains("this app has none"));
    ok(&sandbox, &["g", "controller", "Pages"], &root);
    let report = fails(&sandbox, &["destroy", "controller", "Help"], &root);
    assert_eq!(report["hint"], "recorded runs: ocre g controller Pages");
    fs::write(root.join(".ocre/generated/0002_broken.json"), "{").unwrap();
    fs::write(root.join(".ocre/generated/notes.txt"), "").unwrap();
    fs::write(root.join(".ocre/generated/0003_notes.txt"), "").unwrap();
    let report = fails(&sandbox, &["destroy", "controller", "Pages"], &root);
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .starts_with(".ocre/generated/0002_broken.json is not a valid generation record")
    );
}

#[test]
fn a_forced_overwrite_is_restored_by_destroy() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::create_dir_all(root.join("templates/pages")).unwrap();
    fs::write(root.join("templates/pages/index.html"), "old page\n").unwrap();
    ok(&sandbox, &["g", "controller", "Pages", "--force"], &root);
    assert!(read(&root, "templates/pages/index.html").contains("<h1>Index</h1>"));
    let (stdout, _) = text(&sandbox.ocre(&["destroy", "controller", "Pages"], &root));
    assert!(
        stdout.starts_with("  update  templates/pages/index.html\n  update  src/lib.rs\n  remove  src/pages.rs\n"),
        "{stdout}"
    );
    assert_eq!(read(&root, "templates/pages/index.html"), "old page\n");
}

#[test]
fn the_blog_starter_records_its_scaffold() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &["--starter", "blog"]);
    let record: Value = serde_json::from_str(&read(&root, ".ocre/generated/0001_scaffold_post.json")).unwrap();
    assert_eq!(record["command"], "ocre g scaffold Post title:string body:text published:boolean");
    ok(&sandbox, &["destroy", "scaffold", "Post"], &root);
    assert!(!root.join("src/posts.rs").exists());
}

// ---------- ocre g controller ----------

#[test]
fn controller_generates_html_pages_with_paths() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = ok(&sandbox, &["g", "controller", "PagesController", "index", "AboutUs"], &root);
    assert_eq!(
        report["created"],
        json!(["src/pages.rs", "templates/pages/index.html", "templates/pages/about_us.html"])
    );
    assert_eq!(report["next"], json!(["ocre dev", "open http://localhost:8787/pages"]));
    let code = read(&root, "src/pages.rs");
    assert!(
        code.starts_with("//! Pages pages (HTML). Generated by `ocre g controller PagesController index about_us`.")
    );
    assert!(code.contains(".route(\"/pages\", get(index))\n        .route(\"/pages/about_us\", get(about_us))\n}"));
    assert!(code.contains("    pub fn index() -> &'static str {\n        \"/pages\"\n    }\n\n    pub fn about_us()"));
    assert!(code.contains("async fn about_us() -> Result<Html<String>> {\n    render(&AboutUsView)\n}\n"));
    assert!(!code.contains("CurrentUser"));
    assert!(read(&root, "templates/pages/about_us.html").contains("{% block title %}About us{% endblock %}"));
    let lib = read(&root, "src/lib.rs");
    assert!(lib.contains("mod pages;") && lib.contains(".merge(pages::routes())"));

    let report = ok(&sandbox, &["g", "controller", "Reports", "--api"], &root);
    assert_eq!(report["created"], json!(["src/reports_api.rs"]));
    assert_eq!(report["next"][1], "curl http://localhost:8787/api/reports");
    let code = read(&root, "src/reports_api.rs");
    assert!(code.contains("//! GET /api/reports\n"));
    assert!(code.contains("Ok(Json(IndexResponse { message: \"Edit index in src/reports_api.rs\" }))"));
}

#[test]
fn controller_actions_need_valid_names() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    for (args, error) in [
        (&["g", "controller", "fn"][..], "invalid controller name `fn`"),
        (&["g", "controller", "Stats_api"], "invalid controller name `Stats_api`"),
        (&["g", "controller", "Pages", "routes"], "invalid action name `routes`"),
        (&["g", "controller", "Pages", "9lives"], "invalid action name `9lives`"),
        (&["g", "controller", "Pages", "about", "About"], "action `about` is listed twice"),
    ] {
        assert_eq!(fails(&sandbox, args, &root)["error"], error);
    }
}

#[test]
fn controller_auth_needs_the_auth_generator() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = fails(&sandbox, &["g", "controller", "Dashboard", "--auth"], &root);
    assert_eq!(report["error"], "--auth needs src/auth.rs, which `ocre g auth` creates");
    ok(&sandbox, &["g", "auth"], &root);
    ok(&sandbox, &["g", "controller", "Dashboard", "--auth"], &root);
    let code = read(&root, "src/dashboard.rs");
    assert!(code.contains("use crate::auth::CurrentUser;"));
    assert!(code.contains("async fn index(CurrentUser(_user): CurrentUser) -> Result<Html<String>> {"));
    ok(&sandbox, &["g", "controller", "Stats", "--api", "--auth"], &root);
    let code = read(&root, "src/stats_api.rs");
    assert!(code.contains("use crate::auth_api::BearerUser;"));
    assert!(code.contains("async fn index(BearerUser(_user): BearerUser) -> ApiResult<Json<IndexResponse>> {"));
}

#[test]
fn controller_in_an_api_app_is_json() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("api", &["--api"]);
    let report = ok(&sandbox, &["g", "controller", "Health", "check"], &root);
    assert_eq!(report["created"], json!(["src/health_api.rs"]));
    assert!(read(&root, "src/health_api.rs").contains(".route(\"/api/health/check\", get(check))"));
    let report = fails(&sandbox, &["g", "controller", "Admin", "--auth"], &root);
    assert_eq!(report["error"], "--auth needs src/auth_api.rs, which `ocre g auth` creates");
}

// ---------- ocre g resource ----------

#[test]
fn resource_generates_model_index_and_show() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = ok(
        &sandbox,
        &["g", "resource", "Product", "name:string", "photo:attachment?", "sku:uuid?", "manual:attachment"],
        &root,
    );
    assert!(report["created"].as_array().unwrap().contains(&json!("src/models/product.rs")));
    assert_eq!(report["next"][2], "open http://localhost:8787/products");
    let code = read(&root, "src/products.rs");
    assert!(code.contains(".route(\"/products/{id}\", get(show))"));
    assert!(code.contains("render(&ShowView { product: product::find(&ctx, id).await?.or_404()? })"));
    let show = read(&root, "templates/products/show.html");
    assert!(show.contains("  <dt>Name</dt><dd>{{ product.name }}</dd>\n"));
    assert!(show.contains("<dd>{% if let Some(file) = product.photo() %}{{ file.filename }}{% endif %}</dd>"));
    assert!(show.contains("<dd>{% if let Some(value) = product.sku %}{{ value }}{% endif %}</dd>"));
    assert!(show.contains("<dd>{{ product.manual_filename }}</dd>"));
    assert!(
        read(&root, "templates/products/index.html")
            .contains("<a href=\"{{ paths::show(product.id) }}\">Product {{ product.id }}</a>")
    );

    let report = ok(&sandbox, &["g", "resource", "Order", "total:decimal", "--api"], &root);
    assert!(report["created"].as_array().unwrap().contains(&json!("src/orders_api.rs")));
    assert_eq!(report["next"][2], "curl http://localhost:8787/api/orders");
    assert!(read(&root, "src/orders_api.rs").contains("Ok((page.links(\"/api/orders\", orders.len()), Json(orders)))"));
}

// ---------- template overrides ----------

#[test]
fn override_lists_copies_and_uses_templates() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = ok(&sandbox, &["g", "override"], &root);
    let templates = report["templates"].as_array().unwrap();
    assert!(templates.contains(&json!({ "path": "scaffold/index.html", "overridden": false })));
    let report = ok(&sandbox, &["g", "override", "scaffold/", "controller/view.html"], &root);
    assert_eq!(report["created"].as_array().unwrap().len(), 7);
    assert!(
        ok(&sandbox, &["g", "override"], &root)["templates"]
            .as_array()
            .unwrap()
            .contains(&json!({ "path": "controller/view.html", "overridden": true }))
    );
    let output = sandbox.ocre(&["g", "override"], &root);
    assert!(text(&output).0.contains("  scaffold/show.html  (overridden in .ocre/templates/)\n"));

    fs::write(
        root.join(".ocre/templates/controller/view.html"),
        "<p><%= action.human | upper %> in <%= module %></p>\n",
    )
    .unwrap();
    ok(&sandbox, &["g", "controller", "Pages", "about"], &root);
    assert_eq!(read(&root, "templates/pages/about.html"), "<p>ABOUT in pages</p>\n");

    let index = read(&root, ".ocre/templates/scaffold/index.html").replace("<h1>", "<h1 class=\"title\">");
    fs::write(root.join(".ocre/templates/scaffold/index.html"), index).unwrap();
    ok(&sandbox, &["g", "scaffold", "Post", "title:string"], &root);
    assert!(read(&root, "templates/posts/index.html").contains("<h1 class=\"title\">Posts</h1>"));

    fs::write(root.join(".ocre/templates/scaffold/show.html"), "<%= missing %>").unwrap();
    let report = fails(&sandbox, &["g", "scaffold", "Tag", "name:string"], &root);
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .starts_with(".ocre/templates/scaffold/show.html failed to render: undefined value"),
        "{report}"
    );
    assert_eq!(
        report["hint"],
        "fix .ocre/templates/scaffold/show.html, or delete it to use the built-in template again"
    );

    let report = fails(&sandbox, &["g", "override", "mailer"], &root);
    assert_eq!(report["error"], "no generator template `mailer`");
}

// ---------- app generators ----------

#[test]
fn app_generators_render_files_and_insert_lines() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = ok(&sandbox, &["g", "generator", "Service"], &root);
    assert_eq!(
        report["created"],
        json!([".ocre/generators/service/generator.toml", ".ocre/generators/service/src/services/<%= singular %>.rs"])
    );
    let report = ok(&sandbox, &["g", "service", "Billing", "amount:decimal", "note:text?", "--pretend"], &root);
    assert_eq!(report["created"], json!(["src/services/billing.rs"]));
    ok(&sandbox, &["g", "service", "Billing", "amount:decimal", "note:text?"], &root);
    assert_eq!(
        read(&root, "src/services/billing.rs"),
        "//! Billing. Generated by `ocre g service Billing`.\n\npub struct Billing {\n    pub amount: String,\n    pub note: Option<String>,\n}\n"
    );

    fs::write(
        root.join(".ocre/generators/service/generator.toml"),
        "description = \"x\"\n[[insert]]\nfile = \"src/lib.rs\"\nafter = \"// ocre:modules\"\nline = \"mod <%= plural %>; // <%= args | join(',') %> <%= options.api %> <%= options.dry_run %>\"\n",
    )
    .unwrap();
    fs::write(root.join(".ocre/generators/service/src/services/<%= singular %>.rs"), "// <%= fields | length %>\n")
        .unwrap();
    let report = ok(&sandbox, &["g", "service", "Invoice", "a", "b", "--api=v2", "--dry-run"], &root);
    assert_eq!(report["updated"], json!(["src/lib.rs"]));
    assert_eq!(read(&root, "src/services/invoice.rs"), "// 0\n");
    assert!(read(&root, "src/lib.rs").contains("mod invoices; // a,b v2 true\n"));
    ok(&sandbox, &["destroy", "service", "Invoice"], &root);
    assert!(!read(&root, "src/lib.rs").contains("mod invoices;"));
}

#[test]
fn app_generators_report_their_mistakes() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = fails(&sandbox, &["g", "service", "X"], &root);
    assert_eq!(report["error"], "unknown generator `service`");
    ok(&sandbox, &["g", "generator", "service"], &root);
    let dir = root.join(".ocre/generators/service");
    assert_eq!(fails(&sandbox, &["g", "service"], &root)["error"], "`ocre g service` needs a name");
    fs::write(dir.join("generator.toml"), "descriptin = 1\n").unwrap();
    assert!(
        fails(&sandbox, &["g", "service", "X"], &root)["error"]
            .as_str()
            .unwrap()
            .starts_with(".ocre/generators/service/generator.toml is invalid")
    );
    fs::write(dir.join("generator.toml"), "[[insert]]\nfile = \"src/missing.rs\"\nafter = \"// m\"\nline = \"x\"\n")
        .unwrap();
    assert_eq!(fails(&sandbox, &["g", "service", "X"], &root)["error"], "src/missing.rs does not exist");
    fs::write(dir.join("generator.toml"), "[[insert]]\nfile = \"src/lib.rs\"\nafter = \"// m\"\nline = \"x\"\n")
        .unwrap();
    assert_eq!(fails(&sandbox, &["g", "service", "X"], &root)["error"], "src/lib.rs is missing the `// m` marker");
    fs::remove_file(dir.join("generator.toml")).unwrap();
    fs::write(dir.join("src/services/<%= singular %>.rs"), "<% if %>").unwrap();
    let report = fails(&sandbox, &["g", "service", "X"], &root);
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .starts_with(".ocre/generators/service/src/services/<%= singular %>.rs failed to render"),
        "{report}"
    );
    assert_eq!(report["hint"], "fix .ocre/generators/service/src/services/<%= singular %>.rs");
    let report = fails(&sandbox, &["g", "generator", "service"], &root);
    assert_eq!(report["error"], ".ocre/generators/service/src/services/<%= singular %>.rs already exists");
    assert!(!root.join(".ocre/generators/service/generator.toml").exists(), "nothing written on failure");
}

// ---------- field types ----------

#[test]
fn field_type_aliases_and_new_types() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    ok(&sandbox, &["g", "model", "Author", "name:string"], &root);
    ok(
        &sandbox,
        &[
            "g",
            "scaffold",
            "Book",
            "pages:small_int",
            "views:big_int",
            "stock:int",
            "weight:double",
            "price:decimal",
            "opens:time?",
            "isbn:uuid^",
            "done:bool",
            "at:date_time",
            "meta:jsonb?",
            "author:references:writer_id?",
        ],
        &root,
    );
    let sql = read(&root, "migrations/0002_create_books.sql");
    for column in [
        "pages INTEGER NOT NULL",
        "views INTEGER NOT NULL",
        "weight REAL NOT NULL",
        "price TEXT NOT NULL",
        "opens TEXT,",
        "isbn TEXT NOT NULL",
        "done INTEGER NOT NULL DEFAULT 0",
        "meta TEXT CHECK (json_valid(meta))",
        "writer_id INTEGER REFERENCES authors(id)",
        "CREATE UNIQUE INDEX index_books_on_isbn ON books (isbn);",
    ] {
        assert!(sql.contains(column), "{column} in {sql}");
    }
    let model = read(&root, "src/models/book.rs");
    for check in ["v.decimal(\"price\", &self.price);", "v.time(\"opens\", opens);", "v.uuid(\"isbn\", &self.isbn);"] {
        assert!(model.contains(check), "{check} in {model}");
    }
    assert!(model.contains("pub async fn writer(&self, ctx: &Ctx)"), "{model}");
    let form = read(&root, "templates/books/_form.html");
    assert!(form.contains("<label>Opens <input type=\"time\" name=\"opens\" value=\"{{ form.opens }}\"></label>"));
    assert!(form.contains("<input inputmode=\"decimal\" name=\"price\" value=\"{{ form.price }}\" required>"));
    assert!(form.contains("<label>Writer <input type=\"number\""), "{form}");
    assert!(read(&root, "src/models/author.rs").contains("books"));
}

#[test]
fn field_type_arguments_are_checked() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    ok(&sandbox, &["g", "model", "Author", "name:string"], &root);
    for (field, error) in [
        ("author:references:writer", "invalid foreign key column `writer` for `author`"),
        ("author:references:Writer_id?", "invalid foreign key column `Writer_id` for `author`"),
        ("title:string:long", "type `string` of `title` takes no `:long`"),
        ("size:tiny", "unknown field type `tiny` for `size`"),
        ("done:boolean?", "boolean field `done` cannot be optional"),
        ("cover:attachment^", "attachment `cover` cannot be unique"),
        ("data:json^", "json field `data` cannot be unique"),
        ("edit:attachment", "attachment name `edit` clashes with a scaffold route"),
        ("status:enum:draft,draft", "invalid values `draft,draft` for enum `status`"),
        ("status:enum:draft,published^", "enum `status` cannot be unique"),
        ("status:enum", "enum `status` has no values"),
    ] {
        let report = fails(&sandbox, &["g", "model", "Book", field], &root);
        assert_eq!(report["error"], error, "{field}");
    }
    let report = fails(&sandbox, &["g", "model", "Book", "size:tiny"], &root);
    assert!(report["hint"].as_str().unwrap().contains("decimal, boolean (bool), date, time"));
}

#[test]
fn model_factories_bring_the_test_dependency_to_older_apps() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    // An app made before generators wrote tests: no [dev-dependencies] at all.
    let cargo = read(&root, "Cargo.toml");
    let start = cargo.find("\n[dev-dependencies]").unwrap();
    let end = cargo[start + 1..].find("\n[").map_or(cargo.len(), |at| start + 1 + at);
    let old = format!("{}{}", &cargo[..start], &cargo[end..]);
    fs::write(root.join("Cargo.toml"), &old).unwrap();
    ok(&sandbox, &["g", "model", "Tag", "label:string"], &root);
    let cargo = read(&root, "Cargo.toml");
    let dev = &cargo[cargo.find("[dev-dependencies]").expect("section added")..];
    assert!(dev.lines().nth(1).unwrap().ends_with("features = [\"testing\"] }"), "{cargo}");
    ok(&sandbox, &["g", "model", "Label", "name:string"], &root);
    assert_eq!(read(&root, "Cargo.toml").matches("\"testing\"").count(), 1, "added once");

    // [dev-dependencies] present, without Ocre: the line goes under it.
    fs::write(root.join("Cargo.toml"), format!("{old}\n[dev-dependencies]\npretty_assertions = \"1\"\n")).unwrap();
    ok(&sandbox, &["g", "model", "Badge", "name:string"], &root);
    assert!(read(&root, "Cargo.toml").contains("[dev-dependencies]\nocre = {"));
}

#[test]
fn model_factories_need_a_one_line_ocre_dependency_and_the_marker() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let cargo = read(&root, "Cargo.toml");
    let broken: String = cargo.lines().filter(|l| !l.starts_with("ocre = ")).map(|l| format!("{l}\n")).collect();
    fs::write(root.join("Cargo.toml"), broken).unwrap();
    let report = fails(&sandbox, &["g", "model", "Tag", "label:string"], &root);
    assert_eq!(report["error"], "Cargo.toml has no one-line `ocre = { ... }` dependency");

    fs::write(root.join("Cargo.toml"), cargo).unwrap();
    fs::create_dir_all(root.join("tests/factories")).unwrap();
    fs::write(root.join("tests/factories/mod.rs"), "pub mod tag;\n").unwrap();
    let report = fails(&sandbox, &["g", "model", "Tag", "label:string"], &root);
    assert_eq!(report["error"], "tests/factories/mod.rs is missing the `// ocre:factories` marker");
    assert!(!root.join("src/models/tag.rs").exists(), "nothing written");
}

#[test]
fn data_loaders_compile_a_json_file_into_the_worker() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "data", "countries"], &root);
    assert!(ok, "{report}");
    assert_eq!(
        report["created"],
        serde_json::json!(["src/data/mod.rs", "data/countries.json", "src/data/countries.rs"])
    );
    assert!(fs::read_to_string(root.join("src/lib.rs")).unwrap().contains("mod data;"));
    let loader = fs::read_to_string(root.join("src/data/countries.rs")).unwrap();
    assert!(loader.contains("include_str!(\"../../data/countries.json\")"), "{loader}");
    let (report, ok) = sandbox.json(&["g", "data", "currencies"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["updated"], serde_json::json!(["src/data/mod.rs"]));
    assert!(
        fs::read_to_string(root.join("src/data/mod.rs")).unwrap().contains("pub mod currencies;\npub mod countries;")
    );

    let (report, ok) = sandbox.json(&["g", "data", "Bad-Name"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "invalid data name `Bad-Name`");
    fs::write(root.join("src/data/mod.rs"), "").unwrap();
    assert_eq!(
        sandbox.json(&["g", "data", "rates"], &root).0["error"],
        "src/data/mod.rs is missing the `// ocre:data` marker"
    );
    fs::remove_file(root.join("src/data/mod.rs")).unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();
    assert_eq!(
        sandbox.json(&["g", "data", "rates"], &root).0["error"],
        "src/lib.rs is missing the `// ocre:modules` marker"
    );
}

#[test]
fn generated_rust_is_formatted_by_rustfmt_when_available() {
    let mut sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.remove_tool("rustfmt");
    let (report, ok) = sandbox.json(&["g", "scaffold", "Post", "title:string", "body:text?"], &root);
    assert!(ok, "{report}");
    for file in ["src/models/post.rs", "src/posts.rs", "src/lib.rs", "tests/posts.rs"] {
        let status = std::process::Command::new("rustfmt")
            .args(["--edition", "2024", "--check", file])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success(), "{file} is not rustfmt-clean");
    }
    // A rustfmt that fails, or none at all: the text is written as generated.
    sandbox.script("rustfmt", "#!/bin/sh\nexit 1\n");
    assert!(sandbox.json(&["g", "model", "Tag", "name:string"], &root).1);
    sandbox.remove_tool("rustfmt");
    sandbox.isolate_path();
    assert!(sandbox.json(&["g", "model", "Label", "name:string"], &root).1);
    assert!(fs::read_to_string(root.join("src/models/label.rs")).unwrap().contains("pub fn query() -> Query<Label>"));
}

#[test]
fn system_tests_add_playwright_once() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "system_test", "signing_up"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["created"], json!(["playwright.config.ts", "tests/system/signing_up.spec.ts"]));
    assert_eq!(report["updated"], json!(["package.json", ".gitignore"]));
    assert!(
        fs::read_to_string(root.join("package.json"))
            .unwrap()
            .contains("\"devDependencies\": {\n    \"@playwright/test\": \"1.63.0\",\n")
    );
    assert!(fs::read_to_string(root.join("tests/system/signing_up.spec.ts")).unwrap().contains("test(\"Signing up\""));
    let (report, ok) = sandbox.json(&["g", "system-test", "checkout"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["created"], json!(["tests/system/checkout.spec.ts"]), "config and package once");

    assert_eq!(sandbox.json(&["g", "system_test", "Bad"], &root).0["error"], "invalid system test name `Bad`");
    assert_eq!(
        sandbox.json(&["g", "system_test", "checkout"], &root).0["error"],
        "tests/system/checkout.spec.ts already exists"
    );
    let other = sandbox.new_app("other", &[]);
    fs::write(other.join("package.json"), "{}\n").unwrap();
    assert_eq!(
        sandbox.json(&["g", "system_test", "x"], &other).0["error"],
        "package.json has no `\"devDependencies\": {` block"
    );
    fs::write(other.join("package.json"), "{\n  \"devDependencies\": {\n  }\n}\n").unwrap();
    fs::write(other.join(".gitignore"), "target/\ntest-results/\n").unwrap();
    let (report, _) = sandbox.json(&["g", "system_test", "x"], &other);
    assert_eq!(report["updated"], json!(["package.json"]), "test-results/ already ignored");
}

#[test]
fn webhook_adds_an_endpoint_its_secret_and_the_events_table_once() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = ok(&sandbox, &["g", "webhook", "payments_webhook"], &root);
    let migration = report["created"][2].as_str().unwrap().to_owned();
    assert_eq!(report["created"], json!(["src/payments_webhook.rs", "tests/payments_webhook.rs", migration]));
    assert!(migration.ends_with("_create_webhook_events.sql"), "{migration}");
    assert_eq!(report["updated"], json!([".dev.vars", "src/lib.rs"]));
    let secret = |name: &str| {
        let vars = read(&root, ".dev.vars");
        vars.lines().find_map(|line| line.strip_prefix(&format!("{name}=")).map(str::to_owned)).unwrap()
    };
    assert_eq!(secret("PAYMENTS_WEBHOOK_SECRET").len(), 128);
    assert!(read(&root, "src/lib.rs").contains(".merge(payments_webhook::routes())"));

    // A second webhook shares the table; a Standard Webhooks one gets a whsec_ secret.
    let report = ok(&sandbox, &["g", "webhook", "gpu", "--standard"], &root);
    assert_eq!(report["created"], json!(["src/gpu_webhook.rs", "tests/gpu_webhook.rs"]));
    assert!(report["next"][0].as_str().unwrap().starts_with("write the effect"), "no migration to run");
    assert!(secret("GPU_WEBHOOK_SECRET").starts_with("whsec_"));

    for (name, error) in [("Payments", "invalid webhook name `Payments`"), ("fn", "invalid webhook name `fn`")] {
        assert_eq!(fails(&sandbox, &["g", "webhook", name], &root)["error"], error);
    }
    assert_eq!(fails(&sandbox, &["g", "webhook", "gpu"], &root)["error"], "src/gpu_webhook.rs already exists");

    // A fresh clone has no .dev.vars: the secret is left to the developer.
    fs::remove_file(root.join(".dev.vars")).unwrap();
    let report = ok(&sandbox, &["g", "webhook", "mail"], &root);
    assert_eq!(report["updated"], json!(["src/lib.rs"]));
    assert!(!root.join(".dev.vars").exists());
}

#[test]
fn stepped_jobs_run_their_steps_in_order_with_an_optional_lock() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = ok(
        &sandbox,
        &["g", "job", "ProcessVideo", "video_id:integer", "--steps", "fetch,split", "--lock", "video_id"],
        &root,
    );
    let migration = report["created"][1].as_str().unwrap().to_owned();
    assert!(migration.ends_with("_create_job_locks.sql"), "{report}");
    assert_eq!(report["next"][0], "ocre migrate");
    assert_eq!(
        report["next"][1],
        "enqueue it from a handler: jobs::ProcessVideo::new(video_id).perform_later(&ctx).await?"
    );
    let code = read(&root, "src/jobs/process_video.rs");
    assert!(code.contains("pub const STEPS: [&str; 2] = [\"fetch\", \"split\"];"), "{code}");
    assert!(code.contains("jobs::lock(&db, &key, &self.run, LOCK_TTL)"), "{code}");

    // A lock alone is one `work` step; the table exists already. Named queues work too.
    let report = ok(
        &sandbox,
        &["g", "job", "Import", "account_id:integer", "--lock", "account_id", "--queue", "imports"],
        &root,
    );
    assert!(
        !report["created"].as_array().unwrap().iter().any(|path| path.as_str().unwrap().ends_with(".sql")),
        "{report}"
    );
    let code = read(&root, "src/jobs/import.rs");
    assert!(
        code.contains("0 => self.work(ctx).await?,") && code.contains("queue(ctx, \"imports\").enqueue_in("),
        "{code}"
    );
    // Steps without a lock, no fields.
    ok(&sandbox, &["g", "job", "Nightly", "--steps", "a,b"], &root);
    assert!(!read(&root, "src/jobs/nightly.rs").contains("jobs::lock"));

    for (args, error) in [
        (&["g", "job", "X", "a:integer", "--lock", "b"][..], "--lock b is not a field of the job"),
        (&["g", "job", "X", "--steps", "One"], "invalid step name `One`"),
        (&["g", "job", "X", "--steps", "perform"], "invalid step name `perform`"),
        (&["g", "job", "X", "--steps", "a,a"], "step `a` is listed twice"),
        (&["g", "job", "X", "a:integer", "--steps", "a"], "step `a` has the name of a field"),
    ] {
        assert_eq!(fails(&sandbox, args, &root)["error"], error);
    }
}

#[test]
fn external_job_adds_its_table_webhook_sweep_and_settings() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let report = ok(
        &sandbox,
        &["g", "external_job", "upscale_jobs", "video_id:integer", "hdr:boolean", "meta:json?", "spec:json"],
        &root,
    );
    assert_eq!(report["created"][0], "src/upscale_jobs.rs");
    assert_eq!(report["created"][2], "src/schedules/upscale_jobs_sweep.rs");
    let migration = read(&root, report["created"][1].as_str().unwrap());
    assert!(migration.contains("video_id INTEGER NOT NULL,\n  hdr INTEGER NOT NULL DEFAULT 0,\n  meta TEXT CHECK (json_valid(meta)),\n  spec TEXT NOT NULL CHECK (json_valid(spec)),\n  progress INTEGER"), "{migration}");
    assert!(read(&root, "cloudflare.config.ts").contains("triggers.scheduled({ schedule: \"*/5 * * * *\" }),"));
    let vars = read(&root, ".dev.vars");
    assert!(
        vars.contains("UPSCALE_URL=http://localhost:8787/fake\n") && vars.contains("APP_URL=http://localhost:8787\n"),
        "{vars}"
    );

    // A second one: its own sweep time; APP_URL is not added twice.
    ok(&sandbox, &["g", "external_job", "transcribe", "--sweep", "every 10 minutes"], &root);
    assert_eq!(read(&root, ".dev.vars").matches("APP_URL=").count(), 1);
    assert!(read(&root, "src/transcribe_jobs.rs").contains("pub async fn start(ctx: &Ctx) -> Result<TranscribeJob>"));

    ok(&sandbox, &["g", "webhook", "payments"], &root);
    fs::remove_file(root.join(".dev.vars")).unwrap();
    for (args, error) in [
        (&["g", "external_job", "Upscale"][..], "invalid external job name `Upscale`"),
        (&["g", "external_job", "x", "photo:attachment"], "external job field `photo` cannot be of that type"),
        (
            &["g", "external_job", "x", "progress:integer"],
            "external job field `progress` is a column of the jobs table",
        ),
        (&["g", "external_job", "payments"], "src/payments_webhook.rs already answers /webhooks/payments"),
        (
            &["g", "external_job", "x", "--sweep", "*/5 * * * *"],
            "cron `*/5 * * * *` is already scheduled in cloudflare.config.ts",
        ),
    ] {
        assert_eq!(fails(&sandbox, args, &root)["error"], error);
    }
    let report = ok(&sandbox, &["g", "external_job", "ocr", "--sweep", "every 15 minutes"], &root);
    assert!(!report["updated"].as_array().unwrap().iter().any(|path| path == ".dev.vars"), "{report}");
    ok(&sandbox, &["g", "external_job", "a", "--sweep", "every 20 minutes"], &root);
    ok(&sandbox, &["g", "external_job", "b", "--sweep", "every 30 minutes"], &root);
    let report = ok(&sandbox, &["g", "external_job", "c", "--sweep", "every hour"], &root);
    assert!(
        report["next"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .as_str()
            .unwrap()
            .contains("6 crons; the free plan allows 5"),
        "{report}"
    );
}
