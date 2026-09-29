//! `ocre routes`: reads the app's router from its source.

#[path = "../support/mod.rs"]
mod support;

use std::{fs, path::Path};

use serde_json::{Value, json};
use support::{Sandbox, text};

fn route(method: &str, path: &str, handler: &str) -> Value {
    json!({ "method": method, "path": path, "handler": handler })
}

/// `GET /up` when the app template has the health check.
fn up_route(root: &Path) -> Vec<Value> {
    let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    if lib.contains(".route(\"/up\", get(up))") { vec![route("GET", "/up", "up")] } else { Vec::new() }
}

fn routes(sandbox: &Sandbox, args: &[&str], root: &Path) -> Vec<Value> {
    let mut all = vec!["routes"];
    all.extend_from_slice(args);
    let (report, ok) = sandbox.json(&all, root);
    assert!(ok, "{report}");
    assert_eq!(report["command"], "routes");
    report["routes"].as_array().unwrap().clone()
}

#[test]
fn routes_lists_the_blog_starter_rest_and_graphql_routes() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("blog", &["--starter", "blog"]);
    let (report, ok) = sandbox.json(&["g", "api", "Comment", "body:text", "--graphql"], &root);
    assert!(ok, "{report}");

    let mut expected = vec![
        route("GET", "/", "home"),
        route("GET", "/api/comments", "comments_api::index"),
        route("POST", "/api/comments", "comments_api::create"),
        route("GET", "/api/comments/{id}", "comments_api::show"),
        route("PATCH", "/api/comments/{id}", "comments_api::update"),
        route("DELETE", "/api/comments/{id}", "comments_api::delete"),
        route("GET", "/graphql", "ocre::graphql::graphiql"),
        route("POST", "/graphql", "ocre::graphql::respond"),
        route("GET", "/posts", "posts::index"),
        route("POST", "/posts", "posts::create"),
        route("GET", "/posts/new", "posts::new"),
        route("GET", "/posts/{id}", "posts::show"),
        route("POST", "/posts/{id}", "posts::update"),
        route("POST", "/posts/{id}/delete", "posts::delete"),
        route("GET", "/posts/{id}/edit", "posts::edit"),
    ];
    expected.extend(up_route(&root));
    expected.sort_by_key(|r| r["path"].as_str().unwrap().to_owned());
    assert_eq!(routes(&sandbox, &[], &root), expected);

    // From a subdirectory, filtered on method, path or handler, case-insensitively.
    let filtered = routes(&sandbox, &["patch"], &root.join("src"));
    assert_eq!(filtered, [route("PATCH", "/api/comments/{id}", "comments_api::update")]);
    let filtered = routes(&sandbox, &["GraphiQL"], &root);
    assert_eq!(filtered, [route("GET", "/graphql", "ocre::graphql::graphiql")]);
    assert_eq!(routes(&sandbox, &["/posts/new"], &root), [route("GET", "/posts/new", "posts::new")]);
}

#[test]
fn routes_lists_an_api_app() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("feed", &["--api", "--starter", "blog"]);
    let mut expected = vec![
        route("GET", "/", "status"),
        route("GET", "/api/posts", "posts_api::index"),
        route("POST", "/api/posts", "posts_api::create"),
        route("GET", "/api/posts/{id}", "posts_api::show"),
        route("PATCH", "/api/posts/{id}", "posts_api::update"),
        route("DELETE", "/api/posts/{id}", "posts_api::delete"),
    ];
    expected.extend(up_route(&root));
    assert_eq!(routes(&sandbox, &[], &root), expected);
}

#[test]
fn routes_prints_an_aligned_table() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("blog", &["--starter", "blog"]);
    let output = sandbox.ocre(&["routes", "posts"], &root);
    assert!(output.status.success());
    let (stdout, _) = text(&output);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "METHOD  PATH                HANDLER");
    assert_eq!(lines[1], "GET     /posts              posts::index");
    assert_eq!(lines[3], "GET     /posts/new          posts::new");
    assert_eq!(lines[6], "POST    /posts/{id}/delete  posts::delete");
    assert_eq!(lines.len(), 8);

    let output = sandbox.ocre(&["routes", "nothing-matches"], &root);
    assert_eq!(text(&output).0, "No routes.\n");
}

/// Hand-written routers: comments, literals, nested and `crate::` modules,
/// closures, cycles and constructs the scanner does not know.
#[test]
fn routes_tolerates_hand_written_routers() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let src = root.join("src");
    fs::write(
        src.join("lib.rs"),
        r#"
mod admin;
mod shop;
// .route("/commented", get(nope))
/* .route("/block", get(nope)) */
const QUOTE: char = '"';
const APOSTROPHE: char = '\'';
const UNICODE: char = '\u{e9}';
fn label<'a>(text: &'a str) -> &'a str { text }
fn routes() -> Router<Ctx> {
    Router::new()
        .route("/", axum::routing::get(crate::pages::home).head(home))
        .route(
            "/multi\"line",
            get(show)
                .post(|| async { "closure" })
                .on(MethodFilter::PUT, put_it)
                .put(put_it),
        )
        .route(PATH, get(constant))
        .route("/no-chain")
        .route("/brackets", get(list[0]))
        .merge(admin::routes())
        .merge(crate::shop::routes())
        .merge(super::other::routes())
        .merge(missing::routes())
        .merge(Router::new().route("/inline", options(inline)))
        .route("/unterminated", get(x
}
"#,
    )
    .unwrap();
    fs::create_dir_all(src.join("admin")).unwrap();
    fs::write(
        src.join("admin/mod.rs"),
        r#"pub fn routes() -> Router<Ctx> {
    Router::new().route("/admin", delete(crate::admin::purge)).merge(users::routes()).merge(crate::shop::routes())
}"#,
    )
    .unwrap();
    fs::write(
        src.join("admin/users.rs"),
        r#"pub fn routes() -> Router { Router::new().route("/admin/users", get(list)) }"#,
    )
    .unwrap();
    fs::write(src.join("shop.rs"), r#"pub fn routes() -> Router { Router::new().merge(crate::admin::routes()) }"#)
        .unwrap();

    assert_eq!(
        routes(&sandbox, &[], &root),
        [
            route("GET", "/", "pages::home"),
            route("HEAD", "/", "home"),
            route("DELETE", "/admin", "admin::purge"),
            route("GET", "/admin/users", "admin::users::list"),
            route("OPTIONS", "/inline", "inline"),
            route("GET", "/multi\\\"line", "show"),
            route("PUT", "/multi\\\"line", "put_it"),
        ]
    );
}

#[test]
fn routes_needs_an_app_with_a_lib_rs() {
    let sandbox = Sandbox::new();
    let (report, ok) = sandbox.json(&["routes"], &sandbox.work);
    assert!(!ok);
    assert!(report["hint"].as_str().unwrap().contains("ocre new"), "{report}");

    let root = sandbox.new_app("shop", &[]);
    fs::remove_file(root.join("src/lib.rs")).unwrap();
    let (report, ok) = sandbox.json(&["routes"], &root);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with("cannot read src/lib.rs"), "{report}");
    assert!(report["hint"].as_str().unwrap().contains("src/lib.rs"), "{report}");
}
