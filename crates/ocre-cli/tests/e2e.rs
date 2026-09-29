//! End to end: a generated app built to WebAssembly and served by the real
//! `wrangler dev` (workerd + local D1). This is the test that exercises the
//! framework's runtime code (`crates/ocre/src/runtime/`), which only runs
//! inside workerd. Needs Node.js, the wasm32 target and network access:
//!
//!     cargo test -p ocre-cli --test e2e -- --ignored

mod common;

use std::{
    net::TcpListener,
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command},
    time::{Duration, Instant},
};

use common::Sandbox;
use ureq::Agent;

struct Server {
    child: Child,
    base: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        // ocre -> npx -> wrangler -> workerd share one process group.
        let _ = Command::new("kill").args(["-TERM", &format!("-{}", self.child.id())]).status();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn agent() -> Agent {
    Agent::config_builder().http_status_as_error(false).max_redirects(0).build().into()
}

fn start(sandbox: &Sandbox, root: &Path) -> Server {
    let port = free_port().to_string();
    let mut command = sandbox.command(&["dev", "--port", &port], root);
    // Under `cargo llvm-cov`, keep coverage flags away from the app's own
    // wasm32 build (the profiler runtime is native-only).
    for var in
        ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET_DIR", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"]
    {
        command.env_remove(var);
    }
    // One target dir for every run, so the wasm dependencies compile once.
    command.env("CARGO_TARGET_DIR", Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/e2e-app"));
    let log = sandbox.work.join("dev.log");
    let output = std::fs::File::create(&log).unwrap();
    let child = command.process_group(0).stdout(output.try_clone().unwrap()).stderr(output).spawn().unwrap();
    // wrangler dev listens on `localhost`, which is IPv6-only on some Linux hosts.
    let mut server = Server { child, base: format!("http://localhost:{port}") };
    let deadline = Instant::now() + Duration::from_secs(240);
    while agent().get(&server.base).call().is_err() {
        if let Some(status) = server.child.try_wait().unwrap() {
            panic!("`ocre dev` exited ({status}):\n{}", std::fs::read_to_string(&log).unwrap());
        }
        assert!(Instant::now() < deadline, "wrangler dev did not start:\n{}", std::fs::read_to_string(&log).unwrap());
        std::thread::sleep(Duration::from_secs(1));
    }
    server
}

struct Page {
    status: u16,
    location: String,
    body: String,
}

fn get(server: &Server, path: &str) -> Page {
    page(agent().get(&format!("{}{path}", server.base)).call().unwrap())
}

fn post(server: &Server, path: &str, form: &[(&str, &str)]) -> Page {
    page(agent().post(&format!("{}{path}", server.base)).send_form(form.iter().copied()).unwrap())
}

fn page(mut response: ureq::http::Response<ureq::Body>) -> Page {
    let location = response.headers().get("location").map_or("", |v| v.to_str().unwrap()).to_owned();
    let status = response.status().as_u16();
    // 204 has no body by definition; wrangler dev still labels it gzip, which
    // makes the client's decompressor fail on the empty stream.
    let body = if status == 204 { String::new() } else { response.body_mut().read_to_string().unwrap() };
    Page { status, location, body }
}

/// JSON request (`POST`, `PATCH`, `DELETE`); returns status and parsed body
/// (`null` for an empty body).
fn json(server: &Server, method: &str, path: &str, body: &str) -> (u16, serde_json::Value) {
    let url = format!("{}{path}", server.base);
    let agent = agent();
    let response = match method {
        "POST" => agent.post(&url).content_type("application/json").send(body),
        "PATCH" => agent.patch(&url).content_type("application/json").send(body),
        "DELETE" => agent.delete(&url).call(),
        _ => agent.get(&url).call(),
    };
    let page = page(response.unwrap());
    (
        page.status,
        if page.body.is_empty() { serde_json::Value::Null } else { serde_json::from_str(&page.body).unwrap() },
    )
}

#[test]
#[ignore = "builds WebAssembly and runs wrangler dev; run with --ignored"]
fn api_only_app_serves_rest_and_graphql_on_workerd() {
    let sandbox = Sandbox::new();
    sandbox.use_real_wrangler();
    let root = sandbox.new_app("e2e-api", &["--api", "--starter", "blog"]);
    let (report, ok) =
        sandbox.json(&["g", "api", "Book", "title:string", "pages:integer", "available:boolean", "--graphql"], &root);
    assert!(ok, "{report}");
    let server = start(&sandbox, &root);

    assert_eq!(json(&server, "GET", "/", ""), (200, serde_json::json!({"app": "e2e-api", "status": "ok"})));

    // REST.
    let (status, book) = json(&server, "POST", "/api/books", r#"{"title": "Dune", "pages": 412}"#);
    assert_eq!((status, book["id"].as_i64(), book["title"].as_str()), (201, Some(1), Some("Dune")));
    let (status, book) = json(&server, "PATCH", "/api/books/1", r#"{"pages": 500}"#);
    assert_eq!((status, book["pages"].as_i64(), book["title"].as_str()), (200, Some(500), Some("Dune")));
    let (status, list) = json(&server, "GET", "/api/books?limit=10", "");
    assert_eq!((status, list.as_array().map(Vec::len)), (200, Some(1)));
    assert_eq!(json(&server, "GET", "/api/books?limit=0", "").0, 400);
    assert_eq!(
        json(&server, "GET", "/api/books/9", ""),
        (404, serde_json::json!({"error": {"status": 404, "message": "Not found"}}))
    );
    assert_eq!(
        json(&server, "POST", "/api/books", r#"{"title": " ", "pages": 1}"#),
        (
            422,
            serde_json::json!({"error": {"status": 422, "message": "Validation failed", "fields": {"title": ["can't be blank"]}}})
        )
    );
    assert_eq!(json(&server, "POST", "/api/books", "{").0, 400);
    assert_eq!(json(&server, "POST", "/api/posts", r#"{"title": "Hi", "body": "there"}"#).0, 201, "starter resource");

    // GraphQL on the same data.
    let graphql = |query: &str| json(&server, "POST", "/graphql", &serde_json::json!({ "query": query }).to_string()).1;
    assert_eq!(
        graphql("{ books { title pages available } }"),
        serde_json::json!({"data": {"books": [{"title": "Dune", "pages": 500, "available": false}]}})
    );
    let created =
        graphql(r#"mutation { createBook(input: {title: "Emma", pages: 10, available: true}) { id available } }"#);
    assert_eq!(created["data"]["createBook"]["available"], true);
    assert_eq!(created["data"]["createBook"]["id"], 2);
    let missing = graphql("mutation { updateBook(id: 9, patch: {}) { id } }");
    assert_eq!(missing["errors"][0]["extensions"]["status"], 404);
    assert_eq!(get(&server, "/graphql").status, 200, "GraphiQL");

    // Delete.
    assert_eq!(json(&server, "DELETE", "/api/books/2", ""), (204, serde_json::Value::Null));
    assert_eq!(json(&server, "DELETE", "/api/books/2", "").0, 404);
}

#[test]
#[ignore = "builds WebAssembly and runs wrangler dev; run with --ignored"]
fn generated_app_serves_full_crud_on_workerd() {
    let sandbox = Sandbox::new();
    sandbox.use_real_wrangler();
    let root = sandbox.new_app("e2e", &["--starter", "blog"]);
    let (report, ok) =
        sandbox.json(&["g", "scaffold", "Book", "title:string", "pages:integer", "rating:float", "big:integer"], &root);
    assert!(ok, "{report}");
    let server = start(&sandbox, &root);

    let home = get(&server, "/");
    assert_eq!(home.status, 200);
    assert!(home.body.contains("<h1>e2e</h1>"));

    // Create, with HTML that must come back escaped.
    let created = post(&server, "/posts", &[("title", "Hello <b>edge</b>"), ("body", "First"), ("published", "true")]);
    assert_eq!((created.status, created.location.as_str()), (303, "/posts/1"));
    let shown = get(&server, "/posts/1");
    assert!(shown.body.contains("<dd>Hello &#60;b&#62;edge&#60;/b&#62;</dd>"), "{}", shown.body);
    assert!(shown.body.contains("<dt>Published</dt><dd>true</dd>"));

    // List, edit form, update (unchecked box = false).
    assert!(get(&server, "/posts").body.contains("<a href=\"/posts/1\">Show</a>"));
    assert!(get(&server, "/posts/1/edit").body.contains("value=\"true\" checked"));
    let updated = post(&server, "/posts/1", &[("title", "Renamed"), ("body", "Second")]);
    assert_eq!((updated.status, updated.location.as_str()), (303, "/posts/1"));
    let shown = get(&server, "/posts/1");
    assert!(shown.body.contains("<dd>Renamed</dd>") && shown.body.contains("<dd>false</dd>"));

    // Validation and missing records.
    let invalid = post(&server, "/posts", &[("title", "  "), ("body", "kept")]);
    assert_eq!(invalid.status, 422);
    assert!(invalid.body.contains("<li>Title can&#39;t be blank</li>"), "{}", invalid.body);
    assert!(invalid.body.contains(">kept</textarea>"), "form keeps what was typed: {}", invalid.body);
    assert_eq!(get(&server, "/posts/999").status, 404);
    assert_eq!(post(&server, "/posts/999", &[("title", "a"), ("body", "b")]).status, 404);
    assert_eq!(post(&server, "/posts/999/delete", &[]).status, 404);

    // Numbers: the largest integer D1 round-trips exactly, then one beyond it.
    let book =
        post(&server, "/books", &[("title", "Dune"), ("pages", "412"), ("rating", "4.5"), ("big", "9007199254740991")]);
    assert_eq!(book.status, 303);
    let shown = get(&server, &book.location);
    assert!(shown.body.contains("<dd>412</dd>") && shown.body.contains("<dd>4.5</dd>"), "{}", shown.body);
    assert!(shown.body.contains("<dd>9007199254740991</dd>"), "{}", shown.body);
    let too_big =
        post(&server, "/books", &[("title", "x"), ("pages", "1"), ("rating", "1"), ("big", "9007199254740992")]);
    assert_eq!(too_big.status, 422);
    assert!(too_big.body.contains("<li>Big must be less than or equal to 9007199254740991</li>"), "{}", too_big.body);
    let typo = post(&server, "/books", &[("title", "x"), ("pages", "many"), ("rating", "1"), ("big", "1")]);
    assert_eq!(typo.status, 422);
    assert!(typo.body.contains("<li>Pages is not a number</li>") && typo.body.contains("value=\"many\""));

    // Delete.
    let deleted = post(&server, "/posts/1/delete", &[]);
    assert_eq!((deleted.status, deleted.location.as_str()), (303, "/posts"));
    assert_eq!(get(&server, "/posts/1").status, 404);
}
