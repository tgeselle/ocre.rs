//! `ocre g auth`, run through the real binary. The generated code itself is
//! compiled and exercised on workerd by the e2e test.

mod common;

use std::{collections::BTreeMap, fs, path::Path};

use common::{Sandbox, text};

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
            "templates/auth/signup.html",
            "templates/auth/login.html",
            "templates/auth/account.html",
            "templates/auth/magic_link_new.html",
            "templates/auth/magic_link_show.html",
            "templates/auth/password_new.html",
            "templates/auth/password_edit.html",
        ]),
        "numbered after the starter's migration"
    );
    assert_eq!(report["updated"], serde_json::json!(["src/models/mod.rs", "src/lib.rs"]));
    assert_eq!(report["next"], serde_json::json!(["ocre migrate", "ocre dev", "open http://localhost:8787/signup"]));

    let lib = read(&root, "src/lib.rs");
    for line in ["mod auth;", "mod auth_api;", "mod registrations;", "mod sessions;", "mod passwords;"] {
        assert!(lib.contains(&format!("\n{line}\n")), "{line} in {lib}");
    }
    for module in ["auth_api", "registrations", "sessions", "passwords"] {
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
    assert_eq!(report["error"], "no wrangler.toml found in this directory or its parents");
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
