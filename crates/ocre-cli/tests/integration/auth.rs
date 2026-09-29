//! `ocre g auth`, run through the real binary. The generated code itself is
//! compiled and exercised on workerd by the e2e test.

#[path = "../support/mod.rs"]
mod support;

use std::{collections::BTreeMap, fs, path::Path};

use support::{Sandbox, text};

/// Every file under `root` (except build output) with its contents.
fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let name = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                files.insert(name, fs::read(&path).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

fn read(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap()
}

#[test]
fn auth_generates_pages_json_api_and_models_in_a_full_stack_app() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &["--starter", "blog"]);
    let (report, ok) = sandbox.json(&["g", "auth"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["command"], "generate auth");
    assert_eq!(
        report["created"],
        serde_json::json!([
            "migrations/0002_create_users.sql",
            "migrations/0003_create_auth_tokens.sql",
            "migrations/0004_create_api_keys.sql",
            "src/models/user.rs",
            "src/models/api_key.rs",
            "src/models/auth_token.rs",
            "src/auth_api.rs",
            "src/auth.rs",
            "src/registrations.rs",
            "src/sessions.rs",
            "src/passwords.rs",
            "src/confirmations.rs",
            "templates/auth/signup.html",
            "templates/auth/login.html",
            "templates/auth/account.html",
            "templates/auth/magic_link_new.html",
            "templates/auth/magic_link_show.html",
            "templates/auth/password_new.html",
            "templates/auth/password_edit.html",
            "templates/auth/confirmation_show.html",
        ]),
        "numbered after the starter's migration"
    );
    assert_eq!(report["updated"], serde_json::json!(["src/models/mod.rs", "src/lib.rs", "cloudflare.config.ts"]));
    assert_eq!(report["next"], serde_json::json!(["ocre migrate", "ocre dev", "open http://localhost:8787/signup"]));

    let lib = read(&root, "src/lib.rs");
    for line in ["mod auth;", "mod auth_api;", "mod registrations;", "mod sessions;", "mod passwords;"] {
        assert!(lib.contains(&format!("\n{line}\n")), "{line} in {lib}");
    }
    for module in ["auth_api", "registrations", "sessions", "passwords", "confirmations"] {
        assert!(lib.contains(&format!(".merge({module}::routes())")), "{module} in {lib}");
    }
    assert!(!lib.contains("auth::routes"), "src/auth.rs has extractors, no routes");
    let models = read(&root, "src/models/mod.rs");
    for module in ["post", "user", "api_key", "auth_token"] {
        assert!(models.contains(&format!("pub mod {module};")), "{module} in {models}");
    }
    assert!(read(&root, "migrations/0002_create_users.sql").contains("email TEXT NOT NULL COLLATE NOCASE"));
    assert!(read(&root, "src/auth.rs").contains("pub struct CurrentUser(pub User);"));
    assert!(read(&root, "src/auth_api.rs").contains("pub struct BearerUser(pub User);"));
    assert!(read(&root, "src/sessions.rs").contains("mail::send(&ctx, Email::new(&user.email"));
    assert!(read(&root, "src/auth.rs").contains("pub const OAUTH_PROVIDERS: &[&str] = &[];"));
    let account = read(&root, "templates/auth/account.html");
    assert!(!account.contains("ocre:account-links") && !account.contains("/account/sessions"), "{account}");
    let config = read(&root, "cloudflare.config.ts");
    let entry = config.lines().find(|line| line.contains("AUTH_RATE_LIMITER:")).unwrap_or_else(|| panic!("{config}"));
    assert!(entry.trim().starts_with("AUTH_RATE_LIMITER: bindings.rateLimit({ namespace: \""), "{entry}");
    assert!(entry.ends_with("\", simple: { limit: 10, period: 60 } }),"), "{entry}");

    let (stdout, _) = text(&sandbox.ocre(&["routes", "login"], &root));
    assert!(
        stdout.contains("GET     /login  sessions::new") && stdout.contains("POST    /login  sessions::create"),
        "{stdout}"
    );
}

#[test]
fn auth_generates_only_the_json_api_in_an_api_only_app() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("svc", &["--api"]);
    for options in [&["--db-sessions"][..], &["--oauth", "github"]] {
        let before = snapshot(&root);
        let (report, ok) = sandbox.json(&[&["g", "auth"][..], options].concat(), &root);
        assert!(!ok);
        assert_eq!(report["error"], "--db-sessions and --oauth need HTML pages, and this app is API-only");
        assert!(snapshot(&root) == before, "nothing written on failure");
    }
    let output = sandbox.ocre(&["g", "auth"], &root);
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stderr}");
    for file in [
        "migrations/0001_create_users.sql",
        "migrations/0002_create_api_keys.sql",
        "src/models/mod.rs",
        "src/models/user.rs",
        "src/models/api_key.rs",
        "src/auth_api.rs",
    ] {
        assert!(stdout.contains(&format!("  create  {file}\n")), "{file} in {stdout}");
    }
    assert!(stdout.contains("  update  src/lib.rs\n"), "{stdout}");
    assert!(stdout.contains("curl -X POST http://localhost:8787/api/auth/signup"), "{stdout}");
    for missing in ["src/auth.rs", "src/sessions.rs", "src/models/auth_token.rs", "templates"] {
        assert!(!root.join(missing).exists(), "{missing}");
    }
    let lib = read(&root, "src/lib.rs");
    assert!(lib.contains("\nmod models;\n") && lib.contains(".merge(auth_api::routes())"), "{lib}");
    assert!(!lib.contains("mod auth;"), "{lib}");
}

#[test]
fn auth_with_database_sessions_and_oauth() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "auth", "--oauth", "GitHub, google", "--oauth", "github"], &root);
    assert!(ok, "{report}");
    let auth = read(&root, "src/auth.rs");
    assert!(auth.contains("pub const OAUTH_PROVIDERS: &[&str] = &[\"github\", \"google\"];"), "once each: {auth}");
    assert!(auth.contains("pub const USER_ID: &str"), "the cookie sessions variant");
    let root2 = sandbox.new_app("club", &[]);
    let (report, ok) = sandbox.json(&["g", "auth", "--db-sessions", "--oauth", "google,github"], &root2);
    assert!(ok, "{report}");
    let created: Vec<&str> = report["created"].as_array().unwrap().iter().map(|path| path.as_str().unwrap()).collect();
    for file in [
        "migrations/0004_create_user_sessions.sql",
        "migrations/0005_create_identities.sql",
        "src/models/user_session.rs",
        "src/models/identity.rs",
        "src/user_sessions.rs",
        "src/oauth.rs",
        "templates/auth/user_sessions.html",
    ] {
        assert!(created.contains(&file), "{file} in {created:?}");
    }
    assert!(read(&root2, "src/auth.rs").contains("pub const OAUTH_PROVIDERS: &[&str] = &[\"google\", \"github\"];"));
    assert!(read(&root2, "src/auth.rs").contains("user_session::start(ctx, new)"), "the D1 sessions variant");
    assert!(
        read(&root2, "templates/auth/account.html").contains("<a href=\"/account/sessions\">Signed-in devices</a>")
    );
    let dev_vars = read(&root2, ".dev.vars");
    for name in ["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET", "GITHUB_CLIENT_ID", "GITHUB_CLIENT_SECRET"] {
        assert!(dev_vars.contains(&format!("# {name}=...\n")), "{name} in {dev_vars}");
    }
    let next = report["next"].as_array().unwrap();
    assert!(next[3].as_str().unwrap().starts_with("register an OAuth app with google"), "{next:?}");
    let lib = read(&root2, "src/lib.rs");
    assert!(lib.contains(".merge(user_sessions::routes())") && lib.contains(".merge(oauth::routes())"), "{lib}");

    // Unknown providers fail before anything is written.
    let other = sandbox.new_app("guild", &[]);
    let before = snapshot(&other);
    let (report, ok) = sandbox.json(&["g", "auth", "--oauth", "github,myspace"], &other);
    assert!(!ok);
    assert_eq!(report["error"], "unknown OAuth provider `myspace`");
    assert_eq!(report["hint"], "--oauth accepts github, google (comma-separated)");
    assert!(snapshot(&other) == before);

    // An existing rate limiter binding is kept; no .dev.vars is fine.
    fs::remove_file(other.join(".dev.vars")).unwrap();
    let config = read(&other, "cloudflare.config.ts").replace(
        "// ocre:env",
        "AUTH_RATE_LIMITER: bindings.rateLimit({ namespace: \"7\", simple: { limit: 5, period: 10 } }),\n\t\t\t// ocre:env",
    );
    fs::write(other.join("cloudflare.config.ts"), &config).unwrap();
    let (report, ok) = sandbox.json(&["g", "auth", "--oauth", "github"], &other);
    assert!(ok, "{report}");
    assert_eq!(read(&other, "cloudflare.config.ts"), config);
    assert!(!other.join(".dev.vars").exists());
}

#[test]
fn auth_runs_once_and_never_with_an_existing_user_model() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "auth"], &root);
    assert!(ok, "{report}");
    let before = snapshot(&root);
    let (report, ok) = sandbox.json(&["g", "auth"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "this app already has a User model or a users table");
    assert!(report["hint"].as_str().unwrap().contains("runs once per app"), "{report}");
    assert!(snapshot(&root) == before, "nothing written on failure");

    let other = sandbox.new_app("club", &[]);
    let (report, ok) = sandbox.json(&["g", "model", "User", "name:string"], &other);
    assert!(ok, "{report}");
    fs::remove_file(other.join("src/models/user.rs")).unwrap();
    let before = snapshot(&other);
    let (report, ok) = sandbox.json(&["g", "auth"], &other);
    assert!(!ok, "a users table without the model still blocks");
    assert_eq!(report["error"], "this app already has a User model or a users table");
    assert!(snapshot(&other) == before);
}

#[test]
fn auth_writes_nothing_when_a_file_is_in_the_way() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::create_dir_all(root.join("templates/auth")).unwrap();
    fs::write(root.join("templates/auth/login.html"), "mine").unwrap();
    let before = snapshot(&root);
    let (report, ok) = sandbox.json(&["g", "auth"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "templates/auth/login.html already exists");
    assert!(report["hint"].is_string());
    assert!(snapshot(&root) == before, "nothing written on failure");

    let (report, ok) = sandbox.json(&["g", "auth"], &sandbox.work);
    assert!(!ok);
    assert_eq!(report["error"], "no cloudflare.config.ts found in this directory or its parents");
}

#[test]
fn auth_needs_the_lib_markers() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::write(root.join("src/lib.rs"), "fn main() {}\n").unwrap();
    let before = snapshot(&root);
    let (report, ok) = sandbox.json(&["g", "auth"], &root);
    assert!(!ok);
    assert!(report["hint"].as_str().unwrap().contains("// ocre:modules"), "{report}");
    assert!(snapshot(&root) == before, "nothing written on failure");
}

#[test]
fn models_can_reference_the_generated_user() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    assert!(sandbox.json(&["g", "auth"], &root).1);
    let (report, ok) = sandbox.json(&["g", "model", "Note", "title:string", "user:references"], &root);
    assert!(ok, "{report}");
    assert!(read(&root, "src/models/user.rs").contains("pub async fn notes(&self, ctx: &Ctx, page: ocre::Page)"));
}
