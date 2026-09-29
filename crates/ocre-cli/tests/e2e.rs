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
    let mut server = Server { child, base: format!("http://127.0.0.1:{port}") };
    let deadline = Instant::now() + Duration::from_secs(900); // first build compiles every dependency
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
    Page { status: response.status().as_u16(), location, body: response.body_mut().read_to_string().unwrap() }
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
    let invalid = post(&server, "/posts", &[("title", "  "), ("body", "x")]);
    assert_eq!((invalid.status, invalid.body.as_str()), (400, "<h1>400</h1><p>Title is required.</p>"));
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
    assert_eq!((too_big.status, too_big.body.as_str()), (400, "<h1>400</h1><p>Big is out of range.</p>"));
    assert_eq!(
        post(&server, "/books", &[("title", "x"), ("pages", "many"), ("rating", "1"), ("big", "1")]).status,
        422
    );

    // Delete.
    let deleted = post(&server, "/posts/1/delete", &[]);
    assert_eq!((deleted.status, deleted.location.as_str()), (303, "/posts"));
    assert_eq!(get(&server, "/posts/1").status, 404);
}
