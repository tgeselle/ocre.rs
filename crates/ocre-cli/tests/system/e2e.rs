//! End to end: a generated app built to WebAssembly and served by the real
//! `wrangler dev` (workerd + local D1). This is the test that exercises the
//! framework's runtime code (`crates/ocre/src/runtime/`), which only runs
//! inside workerd. Needs Node.js, the wasm32 target and network access:
//!
//!     cargo test -p ocre-cli --test e2e -- --ignored

#[path = "../support/mod.rs"]
mod support;

use std::{
    net::TcpListener,
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command},
    time::{Duration, Instant},
};

use support::Sandbox;
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
    /// `name=value` of the first Set-Cookie, as a browser would send it back.
    cookie: String,
    headers: ureq::http::HeaderMap,
    body: String,
}

fn get(server: &Server, path: &str) -> Page {
    send(server, "GET", path, &[], &[])
}

fn post(server: &Server, path: &str, form: &[(&str, &str)]) -> Page {
    send(server, "POST", path, &[], form)
}

/// GET, or POST of a form, with extra request headers.
fn send(server: &Server, method: &str, path: &str, headers: &[(&str, &str)], form: &[(&str, &str)]) -> Page {
    let url = format!("{}{path}", server.base);
    let response = if method == "GET" {
        let mut request = agent().get(&url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.call()
    } else {
        let mut request = agent().post(&url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send_form(form.iter().copied())
    };
    page(response.unwrap())
}

fn page(mut response: ureq::http::Response<ureq::Body>) -> Page {
    let location = response.headers().get("location").map_or("", |v| v.to_str().unwrap()).to_owned();
    let cookie =
        response.headers().get("set-cookie").map_or("", |v| v.to_str().unwrap().split(';').next().unwrap()).to_owned();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    // 204 and 304 have no body by definition; wrangler dev still labels them
    // gzip, which makes the client's decompressor fail on the empty stream.
    let empty = status == 204 || status == 304;
    let body = if empty { String::new() } else { response.body_mut().read_to_string().unwrap() };
    Page { status, location, cookie, headers, body }
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
    let (report, ok) = sandbox.json(
        &["g", "api", "Book", "title:string", "pages:integer", "available:boolean", "meta:json?", "--graphql"],
        &root,
    );
    assert!(ok, "{report}");
    let (report, ok) = sandbox.json(&["g", "auth"], &root);
    assert!(ok, "{report}");
    let server = start(&sandbox, &root);

    // Authentication, JSON only: sign up, then a JWT.
    let credentials = r#"{"email": "Ada@Example.com", "password": "correct horse"}"#;
    let (status, user) = json(&server, "POST", "/api/auth/signup", credentials);
    assert_eq!((status, user["email"].as_str()), (201, Some("ada@example.com")), "{user}");
    assert_eq!(json(&server, "POST", "/api/auth/signup", credentials).0, 422, "email taken");
    let (status, token) = json(&server, "POST", "/api/auth/token", credentials);
    assert_eq!(status, 200, "{token}");
    let (status, me) = bearer_json(&server, "GET", "/api/auth/me", token["token"].as_str().unwrap(), "");
    assert_eq!((status, me["id"].as_i64()), (200, user["id"].as_i64()));
    assert_eq!(get(&server, "/login").status, 404, "no HTML pages in API-only apps");
    assert_eq!(
        json(&server, "GET", "/no/such/path", ""),
        (404, serde_json::json!({"error": {"status": 404, "message": "Not found"}})),
        "unmatched paths answer JSON"
    );

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

    // JSON fields: any JSON value in, the same value out; strings stay strings, null clears.
    let body = r#"{"title": "Meta", "pages": 1, "meta": {"tags": ["sf", 1.5], "a": null}}"#;
    let (status, book) = json(&server, "POST", "/api/books", body);
    assert_eq!((status, &book["meta"]), (201, &serde_json::json!({"tags": ["sf", 1.5], "a": null})), "{book}");
    let path = format!("/api/books/{}", book["id"]);
    assert_eq!(json(&server, "GET", &path, "").1["meta"], book["meta"], "read back from D1");
    let (status, book) = json(&server, "PATCH", &path, r#"{"meta": "{not json"}"#);
    assert_eq!((status, &book["meta"]), (200, &serde_json::json!("{not json")), "{book}");
    let (status, book) = json(&server, "PATCH", &path, r#"{"pages": 2}"#);
    assert_eq!((status, &book["meta"]), (200, &serde_json::json!("{not json")), "missing keeps");
    let (status, book) = json(&server, "PATCH", &path, r#"{"meta": null}"#);
    assert_eq!((status, &book["meta"]), (200, &serde_json::Value::Null), "{book}");
    let updated = graphql(&format!(
        r#"mutation {{ updateBook(id: {}, patch: {{meta: {{n: [1, "x"]}}}}) {{ meta }} }}"#,
        book["id"]
    ));
    assert_eq!(updated["data"]["updateBook"]["meta"], serde_json::json!({"n": [1, "x"]}), "JSON scalar: {updated}");
}

#[test]
#[ignore = "builds WebAssembly and runs wrangler dev; run with --ignored"]
fn generated_app_serves_full_crud_on_workerd() {
    let sandbox = Sandbox::new();
    sandbox.use_real_wrangler();
    let root = sandbox.new_app("e2e", &["--starter", "blog"]);
    let (report, ok) = sandbox.json(
        &["g", "scaffold", "Book", "title:string", "pages:integer", "rating:float", "big:integer", "extras:json?"],
        &root,
    );
    assert!(ok, "{report}");
    let server = start(&sandbox, &root);

    let home = get(&server, "/");
    assert_eq!(home.status, 200);
    assert!(home.body.contains("<h1>e2e</h1>"));
    assert_eq!(home.headers["x-content-type-options"], "nosniff", "security headers");
    assert_eq!(home.headers["x-frame-options"], "SAMEORIGIN");
    let up = get(&server, "/up");
    assert_eq!((up.status, up.body.as_str()), (200, "OK"));
    let robots = get(&server, "/robots.txt");
    assert!(robots.status == 200 && robots.body.contains("User-agent"), "static file from public/: {}", robots.body);

    // Create, with HTML that must come back escaped.
    let created = post(&server, "/posts", &[("title", "Hello <b>edge</b>"), ("body", "First"), ("published", "true")]);
    assert_eq!((created.status, created.location.as_str()), (303, "/posts/1"));
    assert!(created.cookie.starts_with("_ocre_session="), "flash travels in the session cookie");
    let session = [("cookie", created.cookie.as_str())];
    let shown = send(&server, "GET", "/posts/1", &session, &[]);
    assert!(shown.body.contains("<dd>Hello &#60;b&#62;edge&#60;/b&#62;</dd>"), "{}", shown.body);
    assert!(shown.body.contains("<dt>Published</dt><dd>true</dd>"));
    assert!(shown.body.contains(r#"<p class="notice">Post was successfully created.</p>"#), "{}", shown.body);
    let again = send(&server, "GET", "/posts/1", &[("cookie", shown.cookie.as_str())], &[]);
    assert!(!again.body.contains("successfully created"), "flash shows once");

    // Another site's form cannot post here (CSRF).
    let forged = send(&server, "POST", "/posts", &[("sec-fetch-site", "cross-site")], &[("title", "x"), ("body", "y")]);
    assert_eq!(forged.status, 403);
    let same_site = [("sec-fetch-site", "same-origin")];
    assert_eq!(send(&server, "POST", "/posts", &same_site, &[("title", "Own"), ("body", "form")]).status, 303);

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

    // Error pages use templates/error.html and the layout; JSON errors stay JSON.
    let missing = get(&server, "/no/such/page");
    assert_eq!(missing.status, 404);
    assert!(missing.body.contains("<title>Not found</title>") && missing.body.contains("<h1>Not found</h1>"));
    let bad_id = get(&server, "/posts/abc");
    assert_eq!(bad_id.status, 400);
    assert!(bad_id.body.contains("<h1>Bad Request</h1>"), "{}", bad_id.body);
    assert!(get(&server, "/posts/999").body.contains("<p>Error 404."), "handler errors too");

    // Pagination links: a full page links to the next one.
    let first = get(&server, "/posts?limit=2");
    assert!(first.body.contains(r#"<a href="?limit=2&#38;offset=2" rel="next">Next</a>"#), "{}", first.body);
    let second = get(&server, "/posts?limit=2&offset=2");
    assert!(second.body.contains(r#"<a href="?limit=2&#38;offset=0" rel="prev">Previous</a>"#), "{}", second.body);
    assert!(!second.body.contains("rel=\"next\""), "two posts: no third page");

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

    // JSON: typed in a textarea, shown compact and escaped, checked before it reaches D1.
    let book = [("title", "Emma"), ("pages", "1"), ("rating", "1"), ("big", "1")];
    let bad = post(&server, "/books", &[&book[..], &[("extras", "{oops")]].concat());
    assert_eq!(bad.status, 422);
    assert!(bad.body.contains("<li>Extras is not valid JSON</li>") && bad.body.contains(">{oops</textarea>"));
    let created = post(&server, "/books", &[&book[..], &[("extras", r#" { "tags": ["<b>", 2] } "#)]].concat());
    assert_eq!(created.status, 303, "{}", created.body);
    let compact = "{&#34;tags&#34;:[&#34;&#60;b&#62;&#34;,2]}";
    assert!(get(&server, &created.location).body.contains(&format!("<dd>{compact}</dd>")));
    assert!(get(&server, &format!("{}/edit", created.location)).body.contains(&format!(">{compact}</textarea>")));
    let cleared = post(&server, &created.location, &[&book[..], &[("extras", " ")]].concat());
    assert_eq!(cleared.status, 303);
    assert!(get(&server, &created.location).body.contains("<dt>Extras</dt><dd></dd>"), "blank clears");

    // Delete.
    let deleted = post(&server, "/posts/1/delete", &[]);
    assert_eq!((deleted.status, deleted.location.as_str()), (303, "/posts"));
    assert_eq!(get(&server, "/posts/1").status, 404);
}

/// Waits until the `ocre dev` log contains `needle` (workerd logs asynchronously).
fn wait_for_log(sandbox: &Sandbox, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let log = std::fs::read_to_string(sandbox.work.join("dev.log")).unwrap();
        if log.contains(needle) {
            return log;
        }
        assert!(Instant::now() < deadline, "the dev log never showed {needle:?}:\n{log}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Delivers a raw email through wrangler dev's local Email Routing endpoint.
fn deliver(server: &Server, from: &str, to: &str, raw: &str) -> (u16, String) {
    let url = format!("{}/cdn-cgi/local/email?from={from}&to={to}", server.base);
    let mut response = agent().post(&url).send(raw).unwrap();
    (response.status().as_u16(), response.body_mut().read_to_string().unwrap())
}

#[test]
#[ignore = "builds WebAssembly and runs wrangler dev; run with --ignored"]
fn generated_app_sends_and_receives_email_on_workerd() {
    let sandbox = Sandbox::new();
    sandbox.use_real_wrangler();
    let root = sandbox.new_app("e2e-mail", &[]);
    for args in [&["g", "mailer", "User", "welcome"][..], &["g", "mailbox"]] {
        let (report, ok) = sandbox.json(args, &root);
        assert!(ok, "{report}");
    }
    // A route sending the generated email, and a mailbox that bounces spam,
    // fails on request and forwards the rest.
    let lib = std::fs::read_to_string(root.join("src/lib.rs"))
        .unwrap()
        .replace("// ocre:routes", "// ocre:routes\n        .route(\"/welcome\", axum::routing::post(send_welcome))")
        + r#"
async fn send_welcome(
    axum::extract::State(ctx): axum::extract::State<Ctx>,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Result<&'static str> {
    let to = form.get("email").map_or("", String::as_str);
    ocre::mail::send(&ctx, mailers::user::welcome(to)?).await?;
    Ok("sent")
}
"#;
    std::fs::write(root.join("src/lib.rs"), lib).unwrap();
    let mailbox = r#"use ocre::{Ctx, Error, Result, mail::InboundEmail};

pub async fn receive(_ctx: Ctx, email: InboundEmail) -> Result<()> {
    match email.subject() {
        "spam" => email.reject("Spam is not welcome"),
        "fail" => return Err(Error::internal("boom")),
        _ => {
            worker::console_log!("[e2e] text: {} html: {}", email.text().unwrap_or("-"), email.html().unwrap_or("-"));
            email.forward("team@example.com").await?;
        }
    }
    Ok(())
}
"#;
    std::fs::write(root.join("src/mailbox.rs"), mailbox).unwrap();
    // Production would use Resend; MAIL_ADAPTER=log from .dev.vars wins in `ocre dev`.
    let wrangler = std::fs::read_to_string(root.join("wrangler.toml")).unwrap();
    std::fs::write(root.join("wrangler.toml"), wrangler.replace("# MAIL_ADAPTER = ", "MAIL_ADAPTER = ")).unwrap();
    let server = start(&sandbox, &root);

    // Sending, logged.
    let sent = post(&server, "/welcome", &[("email", "ada@example.com")]);
    assert_eq!((sent.status, sent.body.as_str()), (200, "sent"));
    let log = wait_for_log(&sandbox, "[ocre mail] end");
    assert!(log.contains("[ocre mail] not sent (MAIL_ADAPTER = \"log\")"), "{log}");
    let expected = "From: e2e-mail <noreply@example.com>\nTo: ada@example.com\nSubject: Welcome\n\n\
                    Hello ada@example.com,\n\nThis is the welcome email.";
    assert!(log.contains(expected), "{log}");
    assert!(log.contains("[ocre mail] HTML version:\n<!DOCTYPE html>"), "{log}");
    assert_eq!(post(&server, "/welcome", &[("email", "not an address")]).status, 400);

    // Receiving: parsed, forwarded, bounced.
    let raw = "From: Ada <ada@example.com>\r\nTo: support@example.com\r\nSubject: =?UTF-8?Q?Caf=C3=A9?=\r\n\
               Message-ID: <1@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n\
               --b\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
               Cr=C3=A8me br=C3=BBl=C3=A9e\r\n--b\r\nContent-Type: text/html; charset=utf-8\r\n\
               Content-Transfer-Encoding: base64\r\n\r\nPGI+aGk8L2I+\r\n--b--\r\n";
    let (status, body) = deliver(&server, "ada@example.com", "support@example.com", raw);
    assert_eq!((status, body.as_str()), (200, "Worker successfully processed email"));
    let log = wait_for_log(&sandbox, "rcptTo: team@example.com");
    assert!(log.contains("[ocre mail] received from ada@example.com to support@example.com: Café"), "{log}");
    assert!(log.contains("[e2e] text: Crème brûlée html: <b>hi</b>"), "{log}");
    let simple = |subject: &str, id: u8| {
        format!(
            "From: ada@example.com\r\nTo: support@example.com\r\nSubject: {subject}\r\nMessage-ID: <{id}@example.com>\r\n\r\nHi\r\n"
        )
    };
    let (status, body) = deliver(&server, "ada@example.com", "support@example.com", &simple("spam", 2));
    assert_eq!((status, body.as_str()), (400, "Worker rejected email with the following reason: Spam is not welcome"));
    let (status, body) = deliver(&server, "ada@example.com", "support@example.com", &simple("fail", 3));
    assert_eq!(
        (status, body.as_str()),
        (400, "Worker rejected email with the following reason: The message could not be processed")
    );
    wait_for_log(&sandbox, "[ocre mail] the mailbox failed: internal error: boom");
    drop(server);

    // Cloudflare Email Service: wrangler dev simulates the send_email binding.
    let wrangler = std::fs::read_to_string(root.join("wrangler.toml"))
        .unwrap()
        .replace("# [[send_email]]\n# name = \"EMAIL\"", "[[send_email]]\nname = \"EMAIL\"");
    std::fs::write(root.join("wrangler.toml"), wrangler).unwrap();
    let vars = std::fs::read_to_string(root.join(".dev.vars")).unwrap().replace("=log", "=cloudflare");
    std::fs::write(root.join(".dev.vars"), vars).unwrap();
    let server = start(&sandbox, &root);
    let sent = post(&server, "/welcome", &[("email", "ada@example.com")]);
    assert_eq!((sent.status, sent.body.as_str()), (200, "sent"));
    // One log entry: headers, then the files holding the text and HTML bodies.
    let log = wait_for_log(&sandbox, "send_email binding called with MessageBuilder:");
    assert!(log.contains("To: ada@example.com\nSubject: Welcome"), "{log}");
    let text_file = log.split("Text: ").nth(1).and_then(|rest| rest.lines().next()).unwrap().trim();
    let text = std::fs::read_to_string(text_file).unwrap();
    assert!(text.starts_with("Hello ada@example.com,"), "{text}");
}

/// Tracks the session cookie across requests, like a browser.
struct Browser<'a> {
    server: &'a Server,
    cookie: String,
}

impl<'a> Browser<'a> {
    fn new(server: &'a Server) -> Self {
        Self { server, cookie: String::new() }
    }

    fn get(&mut self, path: &str) -> Page {
        self.send("GET", path, &[])
    }

    fn post(&mut self, path: &str, form: &[(&str, &str)]) -> Page {
        self.send("POST", path, form)
    }

    fn send(&mut self, method: &str, path: &str, form: &[(&str, &str)]) -> Page {
        let page = send(self.server, method, path, &[("cookie", self.cookie.as_str())], form);
        if !page.cookie.is_empty() {
            self.cookie = page.cookie.clone();
        }
        page
    }
}

/// Size of the dev log, to look only at what a request logs after it.
fn log_len(log: &Path) -> usize {
    std::fs::read_to_string(log).unwrap().len()
}

/// The token of the first link `<prefix><token>` logged after byte `since`
/// of the dev log by an email that `ocre::mail` printed instead of sending.
fn mailed_token(log: &Path, since: usize, prefix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(log).unwrap();
        let new = &text[since.min(text.len())..];
        if let (Some(mail), Some(start)) = (new.find("[ocre mail]"), new.find(prefix)) {
            assert!(mail < start, "the link comes from a logged email:\n{new}");
            let token: String = new[start + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            assert_eq!(token.len(), 43, "{new}");
            return token;
        }
        assert!(Instant::now() < deadline, "no email with {prefix} in the dev log:\n{new}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// JSON request with `Authorization: Bearer <token>`.
fn bearer_json(server: &Server, method: &str, path: &str, token: &str, body: &str) -> (u16, serde_json::Value) {
    let url = format!("{}{path}", server.base);
    let authorization = format!("Bearer {token}");
    let agent = agent();
    let response = match method {
        "POST" => agent.post(&url).header("authorization", &authorization).content_type("application/json").send(body),
        "DELETE" => agent.delete(&url).header("authorization", &authorization).call(),
        _ => agent.get(&url).header("authorization", &authorization).call(),
    };
    let page = page(response.unwrap());
    (
        page.status,
        if page.body.is_empty() { serde_json::Value::Null } else { serde_json::from_str(&page.body).unwrap() },
    )
}

#[test]
#[ignore = "builds WebAssembly and runs wrangler dev; run with --ignored"]
fn generated_auth_signs_users_in_on_workerd() {
    let sandbox = Sandbox::new();
    sandbox.use_real_wrangler();
    let root = sandbox.new_app("e2e-auth", &[]);
    let (report, ok) = sandbox.json(&["g", "auth"], &root);
    assert!(ok, "{report}");
    // A digest made by another PBKDF2 implementation (Python's hashlib, and
    // Ocre's native build in its unit tests) verifies with WebCrypto.
    let native = "pbkdf2_sha256$100000$b2NyZS1lMmUtc2FsdC0xNg$+YK85drz1qhfp9cYahe/+p4WE+lEWkjCL7PbkzXNN+A";
    let server = start(&sandbox, &root);
    let insert = format!("INSERT INTO users (email, password_digest) VALUES ('native@example.com', '{native}')");
    let (report, ok) = sandbox.json(&["sql", &insert], &root);
    assert!(ok, "{report}");
    let log = sandbox.work.join("dev.log");
    let mut browser = Browser::new(&server);

    // A protected page sends visitors to /login.
    let visitor = browser.get("/account");
    assert_eq!((visitor.status, visitor.location.as_str()), (303, "/login"));
    let login = browser.get("/login");
    assert!(login.body.contains("Please log in to continue."));
    // The generated app's Content-Security-Policy and Permissions-Policy.
    let csp = login.headers["content-security-policy"].to_str().unwrap();
    assert!(csp.starts_with("default-src 'self'; script-src 'self' https://unpkg.com;"), "{csp}");
    assert!(login.headers["permissions-policy"].to_str().unwrap().starts_with("camera=()"));

    // Sign up (validation, then success), back to the page that asked.
    let short = browser.post("/signup", &[("email", "Ada@Example.com"), ("password", "short")]);
    assert_eq!(short.status, 422);
    assert!(short.body.contains("<li>Password is too short (minimum is 8 characters)</li>"), "{}", short.body);
    browser.get("/account");
    let since = log_len(&log);
    let signed_up = browser.post("/signup", &[("email", " Ada@Example.com"), ("password", "correct horse")]);
    assert_eq!((signed_up.status, signed_up.location.as_str()), (303, "/account"));
    let account = browser.get("/account");
    assert_eq!(account.status, 200);
    assert!(account.body.contains("<dd>ada@example.com</dd>"), "email normalized: {}", account.body);
    assert!(account.body.contains("Welcome! Your account is ready."));
    assert!(account.body.contains("Your email address is not confirmed yet."));
    let taken = browser.post("/signup", &[("email", "ADA@example.com"), ("password", "another one")]);
    assert!(taken.status == 422 && taken.body.contains("<li>Email has already been taken</li>"), "{}", taken.body);

    // Email confirmation: the sign-up email's link, shown as a button, single use.
    let link = format!("/confirmations/{}", mailed_token(&log, since, &format!("{}/confirmations/", server.base)));
    assert!(browser.get(&link).body.contains(&format!("<form action=\"{link}\" method=\"post\">")));
    let confirmed = browser.post(&link, &[]);
    assert_eq!((confirmed.status, confirmed.location.as_str()), (303, "/account"));
    let account = browser.get("/account");
    assert!(account.body.contains("Thanks, your email address is confirmed."), "{}", account.body);
    assert!(!account.body.contains("not confirmed yet"));
    browser.post(&link, &[]);
    assert!(browser.get("/account").body.contains("That confirmation link is invalid or has expired."));

    // Log out, then the wrong password (or an unknown email) is rejected.
    let logged_out = browser.post("/logout", &[]);
    assert_eq!((logged_out.status, logged_out.location.as_str()), (303, "/"));
    assert_eq!(browser.get("/account").status, 303);
    for (email, password) in [("ada@example.com", "wrong password"), ("nobody@example.com", "correct horse")] {
        let rejected = browser.post("/login", &[("email", email), ("password", password)]);
        assert_eq!(rejected.status, 422);
        assert!(rejected.body.contains("Invalid email or password."), "{}", rejected.body);
    }
    assert_eq!(browser.get("/account").status, 303, "still signed out");

    // Log in with a password (email case ignored), back to /account;
    // "Remember me" makes the cookie outlive the browser session (two weeks).
    let logged_in =
        browser.post("/login", &[("email", "ADA@example.com"), ("password", "correct horse"), ("remember_me", "1")]);
    assert_eq!((logged_in.status, logged_in.location.as_str()), (303, "/account"));
    let set_cookie = logged_in.headers["set-cookie"].to_str().unwrap();
    let max_age: i64 = set_cookie.split("Max-Age=").nth(1).unwrap().split(';').next().unwrap().parse().unwrap();
    assert!((1_209_590..=1_209_600).contains(&max_age), "{set_cookie}");
    assert!(browser.get("/account").body.contains("<dd>ada@example.com</dd>"));
    browser.post("/logout", &[]);

    // Magic link: emailed (logged in dev), shown as a button, single use.
    let since = log_len(&log);
    let requested = browser.post("/magic_link", &[("email", "ada@example.com")]);
    assert_eq!((requested.status, requested.location.as_str()), (303, "/login"));
    let link = format!("/magic_link/{}", mailed_token(&log, since, &format!("{}/magic_link/", server.base)));
    assert_eq!(browser.get("/account").status, 303, "opening the email does not sign in by itself");
    assert!(browser.get(&link).body.contains(&format!("<form action=\"{link}\" method=\"post\">")));
    let used = browser.post(&link, &[]);
    assert_eq!((used.status, used.location.as_str()), (303, "/account"), "back to the page that asked");
    assert!(browser.get("/account").body.contains("<dd>ada@example.com</dd>"));
    browser.post("/logout", &[]);
    let reused = browser.post(&link, &[]);
    assert_eq!((reused.status, reused.location.as_str()), (303, "/magic_link"));
    assert_eq!(browser.get("/account").status, 303);

    // Password reset by emailed link.
    let since = log_len(&log);
    browser.post("/passwords", &[("email", "ada@example.com")]);
    let link = format!("/passwords/{}", mailed_token(&log, since, &format!("{}/passwords/", server.base)));
    assert_eq!(browser.get(&link).status, 200);
    let mismatch = browser.post(&link, &[("password", "new password"), ("password_confirmation", "other")]);
    assert!(mismatch.status == 422 && mismatch.body.contains("doesn&#39;t match Password"), "{}", mismatch.body);
    let reset = browser.post(&link, &[("password", "new password"), ("password_confirmation", "new password")]);
    assert_eq!((reset.status, reset.location.as_str()), (303, "/login"));
    assert_eq!(browser.get(&link).location, "/passwords/new", "reset links work once");
    let old = browser.post("/login", &[("email", "ada@example.com"), ("password", "correct horse")]);
    assert_eq!(old.status, 422);
    assert_eq!(browser.post("/login", &[("email", "ada@example.com"), ("password", "new password")]).status, 303);
    browser.post("/logout", &[]);
    let native_login = browser.post("/login", &[("email", "native@example.com"), ("password", "native digest")]);
    assert_eq!(native_login.status, 303, "{}", native_login.body);

    // Account deletion needs the password; the account then no longer signs in.
    let refused = browser.post("/account/delete", &[("confirmation", "wrong")]);
    assert_eq!((refused.status, refused.location.as_str()), (303, "/account"));
    assert!(browser.get("/account").body.contains("That is not your password"));
    let deleted = browser.post("/account/delete", &[("confirmation", "native digest")]);
    assert_eq!((deleted.status, deleted.location.as_str()), (303, "/"));
    assert_eq!(browser.get("/account").status, 303, "signed out");
    assert_eq!(browser.post("/login", &[("email", "native@example.com"), ("password", "native digest")]).status, 422);

    // JWT for API clients; wrong passwords get a JSON 401.
    let credentials = r#"{"email": "ada@example.com", "password": "correct horse"}"#;
    assert_eq!(
        json(&server, "POST", "/api/auth/token", credentials),
        (401, serde_json::json!({"error": {"status": 401, "message": "Unauthorized"}}))
    );
    let (status, token) =
        json(&server, "POST", "/api/auth/token", r#"{"email": "ada@example.com", "password": "new password"}"#);
    assert_eq!((status, token["token_type"].as_str(), token["expires_in"].as_i64()), (200, Some("Bearer"), Some(3600)));
    let jwt = token["token"].as_str().unwrap();
    let (status, me) = bearer_json(&server, "GET", "/api/auth/me", jwt, "");
    assert_eq!((status, me["email"].as_str()), (200, Some("ada@example.com")));
    assert!(me.get("password_digest").is_none(), "{me}");
    let anonymous = get(&server, "/api/auth/me");
    assert_eq!((anonymous.status, anonymous.headers["www-authenticate"].to_str().unwrap()), (401, "Bearer"));
    let tampered = format!("{}x", &jwt[..jwt.len() - 1]);
    for bad in [tampered.as_str(), "not-a-key", "a.b.c"] {
        assert_eq!(bearer_json(&server, "GET", "/api/auth/me", bad, "").0, 401, "{bad}");
    }

    // API keys: created with the JWT, shown once, usable, revocable.
    let (status, created) = bearer_json(&server, "POST", "/api/auth/keys", jwt, r#"{"name": "CI"}"#);
    assert_eq!((status, created["api_key"]["name"].as_str()), (201, Some("CI")), "{created}");
    let key = created["key"].as_str().unwrap();
    assert_eq!(bearer_json(&server, "POST", "/api/auth/keys", jwt, r#"{"name": " "}"#).0, 422);
    let (status, me) = bearer_json(&server, "GET", "/api/auth/me", key, "");
    assert_eq!((status, me["email"].as_str()), (200, Some("ada@example.com")));
    let (status, keys) = bearer_json(&server, "GET", "/api/auth/keys", key, "");
    assert_eq!((status, keys.as_array().map(Vec::len)), (200, Some(1)));
    assert!(keys[0]["last_used_at"].is_string() && keys[0].get("digest").is_none(), "{keys}");
    let id = created["api_key"]["id"].as_i64().unwrap();
    assert_eq!(
        bearer_json(&server, "DELETE", &format!("/api/auth/keys/{id}"), jwt, ""),
        (204, serde_json::Value::Null)
    );
    assert_eq!(bearer_json(&server, "DELETE", &format!("/api/auth/keys/{id}"), jwt, "").0, 404);
    assert_eq!(bearer_json(&server, "GET", "/api/auth/me", key, "").0, 401, "revoked");

    // JSON account deletion: the password again, then the JWT stops working.
    let bob = r#"{"email": "bob@example.com", "password": "bob's password"}"#;
    assert_eq!(json(&server, "POST", "/api/auth/signup", bob).0, 201);
    let (_, token) = json(&server, "POST", "/api/auth/token", bob);
    let bob_jwt = token["token"].as_str().unwrap();
    let delete_me = |password: &str| {
        let response = agent()
            .delete(&format!("{}/api/auth/me", server.base))
            .header("authorization", &format!("Bearer {bob_jwt}"))
            .force_send_body()
            .content_type("application/json")
            .send(serde_json::json!({ "password": password }).to_string())
            .unwrap();
        response.status().as_u16()
    };
    assert_eq!(delete_me("wrong"), 403);
    assert_eq!(delete_me("bob's password"), 204);
    assert_eq!(bearer_json(&server, "GET", "/api/auth/me", bob_jwt, "").0, 401, "deleted");

    // Rate limit: 10 token requests a minute per IP address, then 429 JSON.
    let statuses: Vec<u16> = (0..12).map(|_| json(&server, "POST", "/api/auth/token", credentials).0).collect();
    assert_eq!(statuses.last(), Some(&429), "{statuses:?}");
    assert_eq!(
        json(&server, "POST", "/api/auth/token", credentials).1,
        serde_json::json!({"error": {"status": 429, "message": "Too many requests. Try again later."}})
    );
}

#[test]
#[ignore = "builds WebAssembly and runs wrangler dev; run with --ignored"]
fn generated_database_sessions_list_and_revoke_devices_on_workerd() {
    let sandbox = Sandbox::new();
    sandbox.use_real_wrangler();
    let root = sandbox.new_app("e2e-sessions", &[]);
    let (report, ok) = sandbox.json(&["g", "auth", "--db-sessions", "--oauth", "github"], &root);
    assert!(ok, "{report}");
    let server = start(&sandbox, &root);
    let (mut laptop, mut phone) = (Browser::new(&server), Browser::new(&server));
    let signed_up = laptop.post("/signup", &[("email", "ada@example.com"), ("password", "correct horse")]);
    assert_eq!(signed_up.status, 303);
    let logged_in = phone.post("/login", &[("email", "ada@example.com"), ("password", "correct horse")]);
    assert_eq!(logged_in.status, 303);
    assert!(phone.get("/login").body.contains("<form action=\"/auth/github\" method=\"post\">"), "OAuth button");

    let devices = laptop.get("/account/sessions");
    assert_eq!(devices.status, 200);
    assert_eq!(devices.body.matches("<form action=\"/account/sessions/").count(), 3, "{}", devices.body);
    assert_eq!(devices.body.matches("(this device)").count(), 1);

    // "Sign out everywhere else": the phone's next request asks it to log in.
    let revoked = laptop.post("/account/sessions/others/delete", &[]);
    assert_eq!((revoked.status, revoked.location.as_str()), (303, "/account/sessions"));
    assert!(laptop.get("/account/sessions").body.contains("Signed out of 1 other session(s)."));
    assert_eq!(phone.get("/account").location, "/login");
    assert_eq!(laptop.get("/account").status, 200, "this device stays signed in");

    // Logging out deletes this session's row too; /auth/github without secrets is a 500.
    laptop.post("/logout", &[]);
    assert_eq!(laptop.get("/account").status, 303);
    assert_eq!(laptop.post("/auth/github", &[]).status, 500);
    assert_eq!(laptop.post("/auth/myspace", &[]).status, 404);
}

/// Polls `path` until its body contains `needle` (jobs run in the background).
fn wait_for_body(server: &Server, path: &str, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let body = get(server, path).body;
        if body.contains(needle) {
            return body;
        }
        assert!(Instant::now() < deadline, "{path} never showed {needle:?}: {body}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Like `wait_for_log`, with room for a queue batch (`max_batch_timeout = 5`).
fn wait_for_job_log(sandbox: &Sandbox, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let log = std::fs::read_to_string(sandbox.work.join("dev.log")).unwrap();
        if log.contains(needle) {
            return log;
        }
        assert!(Instant::now() < deadline, "the dev log never showed {needle:?}:\n{log}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
#[ignore = "builds WebAssembly and runs wrangler dev; run with --ignored"]
fn generated_app_runs_jobs_and_crons_on_workerd() {
    let sandbox = Sandbox::new();
    sandbox.use_real_wrangler();
    let root = sandbox.new_app("e2e-jobs", &[]);
    for args in [
        &["g", "migration", "create_visits", "name:string"][..],
        &["g", "job", "RecordVisit", "name:string"],
        &["g", "schedule", "nightly", "0 3 * * *"],
    ] {
        let (report, ok) = sandbox.json(args, &root);
        assert!(ok, "{report}");
    }
    // The job and the task write a row; a route enqueues, another lists rows.
    let job = std::fs::read_to_string(root.join("src/jobs/record_visit.rs")).unwrap().replace(
        "    pub async fn perform(self, _ctx: &Ctx) -> Result<()> {\n        Ok(())",
        "    pub async fn perform(self, ctx: &Ctx) -> Result<()> {\n        if self.name == \"fail\" {\n            \
         return Err(ocre::Error::internal(\"boom\"));\n        }\n        ctx.db()?.execute(\"INSERT INTO visits (name) \
         VALUES (?1)\", ocre::params![self.name]).await?;\n        Ok(())",
    );
    std::fs::write(root.join("src/jobs/record_visit.rs"), job).unwrap();
    let task = std::fs::read_to_string(root.join("src/schedules/nightly.rs")).unwrap().replace(
        "pub async fn run(_ctx: &Ctx) -> Result<()> {\n    Ok(())",
        "pub async fn run(ctx: &Ctx) -> Result<()> {\n    ctx.db()?.execute(\"INSERT INTO visits (name) VALUES ('cron')\", \
         ocre::params![]).await?;\n    Ok(())",
    );
    std::fs::write(root.join("src/schedules/nightly.rs"), task).unwrap();
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap().replace(
        "// ocre:routes",
        "// ocre:routes\n        .route(\"/enqueue\", axum::routing::post(enqueue))\n        \
         .route(\"/garbage\", axum::routing::post(garbage))\n        \
         .route(\"/mail_later\", axum::routing::post(mail_later))\n        .route(\"/visits\", get(visits))",
    ) + r#"
type Form = axum::Form<std::collections::HashMap<String, String>>;

async fn enqueue(axum::extract::State(ctx): axum::extract::State<Ctx>, axum::Form(form): Form) -> Result<&'static str> {
    let job = jobs::Job::RecordVisit(jobs::RecordVisit { name: form["name"].clone() });
    match form.get("delay") {
        Some(delay) => {
            let delay = std::time::Duration::from_secs(delay.parse().unwrap());
            ocre::jobs::enqueue_in(&ctx, &job, delay).await?
        }
        None => ocre::jobs::enqueue(&ctx, &job).await?,
    }
    Ok("queued")
}

/// A message that is not an Ocre job: logged and dropped.
async fn garbage(axum::extract::State(ctx): axum::extract::State<Ctx>) -> Result<&'static str> {
    let queue = ctx.env().queue("JOBS")?;
    let message = worker::MessageBuilder::new("garbage".to_owned()).content_type(worker::QueueContentType::Text).build();
    worker::send::SendFuture::new(async move { queue.send(message).await }).await?;
    Ok("queued")
}

async fn mail_later(axum::extract::State(ctx): axum::extract::State<Ctx>, axum::Form(form): Form) -> Result<&'static str> {
    let email = ocre::mail::Email::new(form["to"].as_str(), "Later", "Sent from the jobs queue");
    ocre::mail::deliver_later(&ctx, email).await?;
    Ok("queued")
}

#[derive(serde::Deserialize)]
struct Visit {
    name: String,
}

async fn visits(axum::extract::State(ctx): axum::extract::State<Ctx>) -> Result<String> {
    let rows: Vec<Visit> = ctx.db()?.all("SELECT name FROM visits ORDER BY id", ocre::params![]).await?;
    Ok(rows.into_iter().map(|visit| visit.name).collect::<Vec<_>>().join(","))
}
"#;
    std::fs::write(root.join("src/lib.rs"), lib).unwrap();
    let server = start(&sandbox, &root);

    // A job enqueued by a request runs in the queue consumer and writes to D1.
    assert_eq!(post(&server, "/enqueue", &[("name", "ada")]).body, "queued");
    wait_for_body(&server, "/visits", "ada");
    wait_for_job_log(&sandbox, "[ocre jobs] record_visit done");

    // Delayed.
    let sent = Instant::now();
    assert_eq!(post(&server, "/enqueue", &[("name", "bob"), ("delay", "2")]).body, "queued");
    wait_for_body(&server, "/visits", "ada,bob");
    assert!(sent.elapsed() >= Duration::from_secs(2), "ran after {:?}", sent.elapsed());
    // Longer than Queues allow: the request fails, naming the fix.
    assert_eq!(post(&server, "/enqueue", &[("name", "x"), ("delay", "172800")]).status, 500);
    wait_for_log(&sandbox, "cannot delay a job by 172800 s");

    // A failing job is retried later; an undecodable message is dropped.
    assert_eq!(post(&server, "/enqueue", &[("name", "fail")]).body, "queued");
    wait_for_job_log(&sandbox, "[ocre jobs] record_visit failed, retrying in 30 s: internal error: boom");
    assert_eq!(post(&server, "/garbage", &[]).body, "queued");
    let log = wait_for_job_log(&sandbox, "[ocre jobs] dropped message");
    assert!(log.contains("not an Ocre job message (") && log.contains("): garbage"), "{log}");

    // deliver_later: checked now (a bad address is a 400), sent by the consumer.
    assert_eq!(post(&server, "/mail_later", &[("to", "not an address")]).status, 400);
    assert_eq!(post(&server, "/mail_later", &[("to", "ada@example.com")]).body, "queued");
    let log = wait_for_job_log(&sandbox, "[ocre jobs] mail done");
    assert!(log.contains("To: ada@example.com\nSubject: Later\n\nSent from the jobs queue"), "{log}");

    // Cron Triggers, fired through wrangler dev's local endpoint.
    let cron = get(&server, "/cdn-cgi/local/scheduled?cron=0+3+*+*+*");
    assert_eq!(cron.status, 200, "{}", cron.body);
    wait_for_body(&server, "/visits", "cron");
    wait_for_log(&sandbox, "[ocre cron] 0 3 * * * done");
    assert_eq!(get(&server, "/cdn-cgi/local/scheduled?cron=*/5+*+*+*+*").status, 200);
    wait_for_log(&sandbox, "[ocre cron] */5 * * * * failed: internal error: no scheduled task for cron `*/5 * * * *`");
}

type Socket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

/// Opens a WebSocket like a browser page would, with extra handshake headers;
/// `Err(status)` when the server refuses the handshake.
fn websocket(server: &Server, path: &str, headers: &[(&'static str, &str)]) -> Result<Socket, u16> {
    use tungstenite::client::IntoClientRequest;
    let mut request = format!("{}{path}", server.base.replace("http://", "ws://")).into_client_request().unwrap();
    for (name, value) in headers {
        request.headers_mut().insert(*name, value.parse().unwrap());
    }
    match tungstenite::connect(request) {
        Ok((socket, _)) => {
            if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_ref() {
                stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            }
            Ok(socket)
        }
        Err(tungstenite::Error::Http(response)) => Err(response.status().as_u16()),
        Err(err) => panic!("WebSocket handshake failed: {err}"),
    }
}

/// The next text message, skipping pings.
fn receive(socket: &mut Socket) -> String {
    loop {
        match socket.read().expect("a broadcast within 10 s") {
            tungstenite::Message::Text(text) => return text.to_string(),
            tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => {}
            other => panic!("unexpected message: {other:?}"),
        }
    }
}

#[test]
#[ignore = "builds WebAssembly and runs wrangler dev; run with --ignored"]
fn generated_app_broadcasts_changes_to_websockets_on_workerd() {
    let sandbox = Sandbox::new();
    sandbox.use_real_wrangler();
    let root = sandbox.new_app("e2e-live", &[]);
    let (report, ok) = sandbox.json(&["g", "scaffold", "Note", "title:string", "--realtime"], &root);
    assert!(ok, "{report}");
    let server = start(&sandbox, &root);

    // A broadcast to a channel nobody listens to still succeeds.
    assert_eq!(post(&server, "/notes", &[("title", "Before")]).status, 303);
    let index = get(&server, "/notes");
    assert!(index.body.contains("<div hx-ext=\"ws\" ws-connect=\"/realtime/notes\">"), "{}", index.body);
    assert!(index.body.contains("<tr id=\"note_1\"><td>Before</td>"), "{}", index.body);

    // Two browsers on the index page.
    let mut first = websocket(&server, "/realtime/notes", &[]).unwrap();
    let mut second = websocket(&server, "/realtime/notes", &[("sec-fetch-site", "same-origin")]).unwrap();

    // Create: the escaped row goes to the top of both tables.
    let created = post(&server, "/notes", &[("title", "Live <b>")]);
    assert_eq!((created.status, created.location.as_str()), (303, "/notes/2"));
    let row = "<tr id=\"note_2\"><td>Live &#60;b&#62;</td><td><a href=\"/notes/2\">Show</a> \
               <a href=\"/notes/2/edit\">Edit</a></td></tr>";
    let expected = format!("<tbody hx-swap-oob=\"afterbegin:#notes\">{row}</tbody>");
    assert_eq!(receive(&mut first), expected);
    assert_eq!(receive(&mut second), expected);

    // Update: the row replaces the one with the same id.
    assert_eq!(post(&server, "/notes/2", &[("title", "Renamed")]).status, 303);
    let renamed = receive(&mut first);
    assert!(renamed.starts_with("<tr id=\"note_2\"><td>Renamed</td>"), "{renamed}");
    assert_eq!(receive(&mut second), renamed);

    // One browser leaves; delete reaches the other.
    first.close(None).unwrap();
    while first.read().is_ok() {}
    assert_eq!(post(&server, "/notes/2/delete", &[]).status, 303);
    assert_eq!(receive(&mut second), "<div id=\"note_2\" hx-swap-oob=\"delete\"></div>");

    // Refused: unknown channel, a plain GET, a handshake from another site.
    assert_eq!(websocket(&server, "/realtime/secrets", &[]).err(), Some(404));
    assert_eq!(get(&server, "/realtime/notes").status, 400, "plain GET");
    let evil = [("origin", "https://evil.example"), ("sec-fetch-site", "cross-site")];
    assert_eq!(websocket(&server, "/realtime/notes", &evil).err(), Some(403), "cross-site WebSocket hijacking");
}

const E2E_EN: &str = r#"en:
  hello:
    title: "Hello"
    greeting: "Welcome, %{name}!"
    posts:
      one: "%{count} post"
      other: "%{count} posts"
    only_en: "English only"
"#;

const E2E_FR: &str = r#"fr:
  hello:
    title: "Bonjour"
    greeting: "Bienvenue, %{name} !"
    posts:
      one: "%{count} article"
      other: "%{count} articles"
"#;

const E2E_PAGES: &str = r#"use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use askama::Template;
use axum::{
    Router,
    extract::State,
    response::{Html, Response},
    routing::{get, post},
};
use ocre::{
    Ctx, Json, Result,
    cache::{CacheControl, Conditional, ETag},
    i18n::I18n,
    render,
};

/// Runs of the cached computation in this Worker instance.
static MISSES: AtomicUsize = AtomicUsize::new(0);

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/hello", get(hello))
        .route("/{locale}/hello", get(hello))
        .route("/cached", get(cached))
        .route("/cached/delete", post(forget))
        .route("/fresh", get(fresh))
}

#[derive(Template)]
#[template(source = "<html lang=\"{{ i18n.locale() }}\"><h1>{{ i18n.t(\"hello.title\") }}</h1><p>{{ i18n.t(\"hello.greeting\").arg(\"name\", name) }}</p><p>{{ i18n.t(\"hello.posts\").count(count) }}</p><p>{{ i18n.t(\"hello.only_en\") }}</p></html>", ext = "html")]
struct HelloView {
    i18n: I18n,
    name: String,
    count: usize,
}

async fn hello(i18n: I18n) -> Result<Html<String>> {
    render(&HelloView { i18n, name: "Ada <3".to_owned(), count: 3 })
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Token {
    value: String,
}

async fn cached(State(ctx): State<Ctx>) -> Result<Json<serde_json::Value>> {
    let token: Token = ocre::cache::fetch(&ctx, "e2e:token:v1", Duration::from_secs(3600), || async {
        MISSES.fetch_add(1, Ordering::Relaxed);
        Ok(Token { value: ocre::token::generate() })
    })
    .await?;
    let read: Option<Token> = ocre::cache::read(&ctx, "e2e:token:v1").await?;
    Ok(Json(serde_json::json!({
        "value": token.value,
        "read": read.map(|token| token.value),
        "misses": MISSES.load(Ordering::Relaxed),
    })))
}

async fn forget(State(ctx): State<Ctx>) -> Result<&'static str> {
    ocre::cache::delete(&ctx, "e2e:token:v1").await?;
    ocre::cache::write(&ctx, "e2e:other:v1", &1, Duration::from_secs(60)).await?;
    Ok("deleted")
}

async fn fresh(i18n: I18n, conditional: Conditional) -> Result<Response> {
    let etag = ETag::of(&("v1", i18n.locale()))?;
    conditional.fresh_when(etag, CacheControl::no_cache(), || Ok(Html(i18n.t("hello.title").to_string())))
}
"#;

#[test]
#[ignore = "builds WebAssembly and runs wrangler dev; run with --ignored"]
fn generated_app_translates_and_caches_on_workerd() {
    let sandbox = Sandbox::new();
    sandbox.use_real_wrangler();
    let root = sandbox.new_app("e2e-i18n", &[]);
    for args in [&["g", "locale", "en", "fr"][..], &["g", "cache"]] {
        let (report, ok) = sandbox.json(args, &root);
        assert!(ok, "{report}");
    }
    std::fs::write(root.join("locales/en.yml"), E2E_EN).unwrap();
    std::fs::write(root.join("locales/fr.yml"), E2E_FR).unwrap();
    std::fs::write(root.join("src/pages.rs"), E2E_PAGES).unwrap();
    let lib = std::fs::read_to_string(root.join("src/lib.rs"))
        .unwrap()
        .replace("// ocre:modules", "// ocre:modules\nmod pages;")
        .replace("// ocre:routes", "// ocre:routes\n        .merge(pages::routes())");
    std::fs::write(root.join("src/lib.rs"), lib).unwrap();
    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    std::fs::write(root.join("Cargo.toml"), cargo.replace("[dependencies]\n", "[dependencies]\nserde_json = \"1\"\n"))
        .unwrap();
    let server = start(&sandbox, &root);

    // Accept-Language picks French; interpolated values are escaped; plurals
    // follow French rules; a key missing in French is visible in dev builds.
    let french = send(&server, "GET", "/hello", &[("accept-language", "fr-CH, fr;q=0.9, en;q=0.8")], &[]);
    assert_eq!(french.status, 200, "{}", french.body);
    assert_eq!(
        french.body,
        "<html lang=\"fr\"><h1>Bonjour</h1><p>Bienvenue, Ada &#60;3 !</p><p>3 articles</p>\
         <p>translation missing: fr.hello.only_en</p></html>"
    );
    let english = get(&server, "/hello");
    assert!(english.body.starts_with("<html lang=\"en\"><h1>Hello</h1><p>Welcome, Ada &#60;3!</p><p>3 posts</p>"));
    assert!(english.body.contains("<p>English only</p>"), "{}", english.body);
    // The path segment wins over the header; the cookie over the header.
    let path = send(&server, "GET", "/en/hello", &[("accept-language", "fr")], &[]);
    assert!(path.body.contains("<h1>Hello</h1>"), "{}", path.body);
    assert_eq!(get(&server, "/xx/hello").status, 404);
    let cookie = send(&server, "GET", "/hello", &[("cookie", "locale=fr"), ("accept-language", "en")], &[]);
    assert!(cookie.body.contains("<h1>Bonjour</h1>"), "{}", cookie.body);

    // Read-through cache: the second request reads KV instead of computing.
    let (status, first) = json(&server, "GET", "/cached", "");
    assert_eq!(status, 200, "{first}");
    assert_eq!((first["misses"].as_u64(), &first["read"]), (Some(1), &first["value"]), "{first}");
    let (_, second) = json(&server, "GET", "/cached", "");
    assert_eq!((second["misses"].as_u64(), &second["value"]), (Some(1), &first["value"]), "{second}");
    let deleted = post(&server, "/cached/delete", &[]);
    assert_eq!((deleted.status, deleted.body.as_str()), (200, "deleted"));
    let (_, third) = json(&server, "GET", "/cached", "");
    assert_eq!(third["misses"].as_u64(), Some(2), "{third}");
    assert_ne!(third["value"], first["value"]);

    // Conditional GET: the same version answers 304 without a body.
    let page = send(&server, "GET", "/fresh", &[("accept-language", "fr")], &[]);
    let etag = page.headers["etag"].to_str().unwrap().to_owned();
    assert_eq!((page.status, page.body.as_str()), (200, "Bonjour"));
    assert_eq!(page.headers["cache-control"], "private, no-cache");
    let again = send(&server, "GET", "/fresh", &[("accept-language", "fr"), ("if-none-match", &etag)], &[]);
    assert_eq!((again.status, again.body.as_str()), (304, ""));
    let other = send(&server, "GET", "/fresh", &[("accept-language", "en"), ("if-none-match", &etag)], &[]);
    assert_eq!((other.status, other.body.as_str()), (200, "Hello"), "the locale is part of the version");
}

/// Routes exercising the rest of `ocre::storage` directly.
const E2E_FILES: &str = r#"use axum::{
    Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, header},
    routing::{get, post},
};
use ocre::{Ctx, Error, OptionExt, Result, storage};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/e2e/objects", get(object).delete(remove))
        .route("/e2e/raw", post(raw))
        .route("/e2e/bytes", post(bytes))
        .route("/e2e/photos/{id}/keys", get(keys))
}

#[derive(serde::Deserialize)]
struct Key {
    key: String,
}

async fn object(State(ctx): State<Ctx>, Query(Key { key }): Query<Key>) -> Result<Vec<u8>> {
    storage::read(&ctx, &key).await?.or_404()
}

async fn remove(State(ctx): State<Ctx>, Query(Key { key }): Query<Key>) -> Result<&'static str> {
    storage::delete(&ctx, &key).await?;
    Ok("deleted")
}

/// A request body streamed into R2 (no multipart).
async fn raw(State(ctx): State<Ctx>, headers: HeaderMap, body: Body) -> Result<String> {
    let size = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse().ok())
        .ok_or_else(|| Error::bad_request("Content-Length required"))?;
    Ok(storage::store_body(&ctx, "raw", "raw.bin", "application/octet-stream", size, body).await?.key)
}

async fn bytes(State(ctx): State<Ctx>) -> Result<String> {
    Ok(storage::store_bytes(&ctx, "generated", "hello.txt", "text/plain", b"hello".to_vec()).await?.key)
}

/// The R2 keys of a photo's files, `image` then `notes` (empty when none).
async fn keys(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<String> {
    let photo = crate::models::photo::find(&ctx, id).await?.or_404()?;
    Ok(format!("{} {}", photo.image_key, photo.notes_key.unwrap_or_default()))
}
"#;

/// A `multipart/form-data` request, as a browser form with file inputs sends it:
/// text fields, then `(name, filename, content type, bytes)` files.
fn multipart(
    server: &Server,
    method: &str,
    path: &str,
    fields: &[(&str, &str)],
    files: &[(&str, &str, &str, &[u8])],
) -> Page {
    let boundary = "----e2eBoundary7MA4YWxkTrZu0gW";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").bytes(),
        );
    }
    for (name, filename, content_type, bytes) in files {
        body.extend(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
            )
            .bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend(format!("--{boundary}--\r\n").bytes());
    let url = format!("{}{path}", server.base);
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let response = match method {
        "PUT" => agent().put(&url).content_type(&content_type).header("accept", "application/json").send(&body[..]),
        _ => agent().post(&url).content_type(&content_type).header("accept", "text/html").send(&body[..]),
    };
    page(response.unwrap())
}

/// GET with headers, returning the raw bytes of the body.
fn download(server: &Server, path: &str, headers: &[(&str, &str)]) -> (u16, ureq::http::HeaderMap, Vec<u8>) {
    let mut request = agent().get(&format!("{}{path}", server.base));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request.call().unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = if status == 304 { Vec::new() } else { response.body_mut().read_to_vec().unwrap() };
    (status, headers, body)
}

/// Bytes that break naive parsers: every byte value, CRLFs and boundary-like dashes.
fn binary_file(len: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..len).map(|i| (i * 7 % 256) as u8).collect();
    bytes.splice(0..0, b"\x89PNG\r\n\x1a\n\r\n------e2eBoundary\r\n".iter().copied());
    bytes
}

#[test]
#[ignore = "builds WebAssembly and runs wrangler dev; run with --ignored"]
fn generated_app_stores_uploads_in_r2_on_workerd() {
    let sandbox = Sandbox::new();
    sandbox.use_real_wrangler();
    let root = sandbox.new_app("e2e-files", &[]);
    for args in [
        &["g", "scaffold", "Photo", "title:string", "image:attachment", "notes:attachment?"][..],
        &["g", "api", "Document", "name:string", "file:attachment?"],
    ] {
        let (report, ok) = sandbox.json(args, &root);
        assert!(ok, "{report}");
    }
    let wrangler = std::fs::read_to_string(root.join("wrangler.toml")).unwrap();
    assert!(
        wrangler.contains("[[r2_buckets]]\nbinding = \"STORAGE\"\nbucket_name = \"e2e-files-storage\""),
        "{wrangler}"
    );
    // Small limits, to reach them with small requests.
    for (path, rules) in [("src/models/photo.rs", "IMAGE"), ("src/models/document.rs", "FILE")] {
        let model = std::fs::read_to_string(root.join(path)).unwrap().replacen(
            &format!("pub const {rules}: Rules = Rules {{\n    max_bytes: 10 * 1024 * 1024,"),
            &format!("pub const {rules}: Rules = Rules {{\n    max_bytes: 64 * 1024,"),
            1,
        );
        std::fs::write(root.join(path), model).unwrap();
    }
    std::fs::write(root.join("src/e2e_files.rs"), E2E_FILES).unwrap();
    let lib = std::fs::read_to_string(root.join("src/lib.rs"))
        .unwrap()
        .replace("// ocre:modules", "// ocre:modules\nmod e2e_files;")
        .replace("// ocre:routes", "// ocre:routes\n        .merge(e2e_files::routes())");
    std::fs::write(root.join("src/lib.rs"), lib).unwrap();
    let server = start(&sandbox, &root);
    let object = |key: &str| download(&server, &format!("/e2e/objects?key={key}"), &[]);

    // Upload through the scaffolded form, then download it back byte for byte.
    let form = get(&server, "/photos/new");
    assert!(
        form.body.contains(r#"enctype="multipart/form-data""#) && form.body.contains(r#"type="file" name="image""#)
    );
    let png = binary_file(40_000);
    let created =
        multipart(&server, "POST", "/photos", &[("title", "Sunset")], &[("image", "sunset é.png", "image/png", &png)]);
    assert_eq!((created.status, created.location.as_str()), (303, "/photos/1"), "{}", created.body);
    let shown = get(&server, "/photos/1");
    let link = r#"<a href="/photos/1/image" hx-boost="false">sunset é.png</a> (39.1 KB)"#;
    assert!(shown.body.contains(link), "{}", shown.body);
    let (status, headers, body) = download(&server, "/photos/1/image", &[]);
    assert_eq!(status, 200);
    assert!(body == png, "downloaded {} bytes, uploaded {}", body.len(), png.len());
    assert_eq!(headers["content-type"], "image/png", "{headers:?}");
    assert_eq!(headers["content-length"], png.len().to_string().as_str(), "{headers:?}");
    assert_eq!(
        headers["content-disposition"],
        "inline; filename=\"sunset _.png\"; filename*=UTF-8''sunset%20%C3%A9.png"
    );
    assert_eq!(headers["accept-ranges"], "bytes");
    assert_eq!(headers["cache-control"], "private, no-cache");
    let etag = headers["etag"].to_str().unwrap().to_owned();
    let (status, _, body) = download(&server, "/photos/1/image", &[("if-none-match", &etag)]);
    assert_eq!((status, body.len()), (304, 0), "conditional GET");
    let (status, headers, body) = download(&server, "/photos/1/image", &[("range", "bytes=4-9")]);
    assert_eq!((status, &body[..]), (206, &png[4..10]));
    assert_eq!(headers["content-range"], format!("bytes 4-9/{}", png.len()).as_str());
    let (status, headers, _) = download(&server, "/photos/1/image", &[("range", "bytes=999999-")]);
    assert_eq!(status, 416);
    assert_eq!(headers["content-range"], format!("bytes */{}", png.len()).as_str());
    assert_eq!(get(&server, "/photos/1/notes").status, 404, "no optional file yet");
    let keys = get(&server, "/e2e/photos/1/keys").body;
    let image_key = keys.split(' ').next().unwrap().to_owned();
    assert!(image_key.starts_with("photos/image/"), "{keys}");
    assert_eq!(object(&image_key).2, png, "stored in the local R2 bucket");

    // Validation: required, size and type; nothing is stored for invalid forms.
    let missing = multipart(&server, "POST", "/photos", &[("title", "No file")], &[]);
    assert_eq!(missing.status, 422);
    assert!(missing.body.contains("<li>Image can&#39;t be blank</li>"), "{}", missing.body);
    assert!(missing.body.contains(r#"value="No file""#), "typed values are kept");
    let big = binary_file(70_000);
    let svg: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>";
    let invalid = multipart(
        &server,
        "POST",
        "/photos",
        &[("title", "x")],
        &[("image", "a.svg", "image/svg+xml", svg), ("notes", "big.png", "image/png", &big)],
    );
    assert_eq!(invalid.status, 422);
    assert!(invalid.body.contains("<li>Image has an unsupported type (allowed: image/png, image/jpeg, image/gif, image/webp, application/pdf, text/plain)</li>"), "{}", invalid.body);
    let too_large =
        multipart(&server, "POST", "/photos", &[("title", "x")], &[("image", "big.png", "image/png", &big)]);
    assert!(too_large.body.contains("<li>Image is too large (maximum is 64 KB)</li>"), "{}", too_large.body);
    assert_eq!(get(&server, "/photos/2").status, 404);

    // Update: a new image and a notes file replace the old ones, which leave R2.
    let notes: &[u8] = b"Shot at 6 pm.\r\n";
    let updated = multipart(
        &server,
        "POST",
        "/photos/1",
        &[("title", "Sunset 2")],
        &[("image", "b.jpg", "image/jpeg", b"JPEG"), ("notes", "notes.txt", "text/plain", notes)],
    );
    assert_eq!((updated.status, updated.location.as_str()), (303, "/photos/1"), "{}", updated.body);
    assert_eq!(object(&image_key).0, 404, "the replaced image is deleted");
    let (status, headers, body) = download(&server, "/photos/1/image", &[]);
    assert_eq!((status, &body[..]), (200, &b"JPEG"[..]));
    assert_eq!(headers["content-type"], "image/jpeg");
    let (status, _, body) = download(&server, "/photos/1/notes", &[]);
    assert_eq!((status, &body[..]), (200, notes));
    let keys = get(&server, "/e2e/photos/1/keys").body;
    let (image_key, notes_key) = keys.split_once(' ').map(|(a, b)| (a.to_owned(), b.to_owned())).unwrap();
    assert!(get(&server, "/photos/1/edit").body.contains(r#"name="remove_notes""#));
    let kept = multipart(&server, "POST", "/photos/1", &[("title", "Kept"), ("remove_notes", "true")], &[]);
    assert_eq!(kept.status, 303, "{}", kept.body);
    assert_eq!(get(&server, "/photos/1/notes").status, 404, "removed");
    assert_eq!(object(&notes_key).0, 404, "and deleted from R2");
    assert_eq!(download(&server, "/photos/1/image", &[]).2, b"JPEG", "no new file keeps the image");

    // Delete removes the objects with the record.
    let deleted = post(&server, "/photos/1/delete", &[]);
    assert_eq!((deleted.status, deleted.location.as_str()), (303, "/photos"));
    assert_eq!(object(&image_key).0, 404);
    assert_eq!(get(&server, "/photos/1/image").status, 404);

    // JSON API: optional file uploaded with PUT, served, removed.
    let created = page(
        agent()
            .post(&format!("{}/api/documents", server.base))
            .content_type("application/json")
            .send(r#"{"name": "Spec"}"#)
            .unwrap(),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let (status, document): (u16, serde_json::Value) = (created.status, serde_json::from_str(&created.body).unwrap());
    assert_eq!((status, &document["file_key"]), (201, &serde_json::Value::Null), "{document}");
    let pdf = binary_file(1000);
    let put = multipart(&server, "PUT", "/api/documents/1/file", &[], &[("file", "spec.pdf", "application/pdf", &pdf)]);
    assert_eq!(put.status, 200, "{}", put.body);
    let document: serde_json::Value = serde_json::from_str(&put.body).unwrap();
    assert_eq!(
        (document["file_filename"].as_str(), document["file_size"].as_i64()),
        (Some("spec.pdf"), Some(pdf.len() as i64))
    );
    let (status, headers, body) = download(&server, "/api/documents/1/file", &[]);
    assert!(status == 200 && body == pdf);
    assert_eq!(headers["content-disposition"], "inline; filename=\"spec.pdf\"");
    let file_key = document["file_key"].as_str().unwrap().to_owned();
    let empty = multipart(&server, "PUT", "/api/documents/1/file", &[], &[]);
    assert_eq!(empty.status, 422);
    assert!(empty.body.contains(r#""fields":{"file":["can't be blank"]}"#), "{}", empty.body);
    let removed = page(agent().delete(&format!("{}/api/documents/1/file", server.base)).call().unwrap());
    assert_eq!(removed.status, 200, "{}", removed.body);
    let (status, document): (u16, serde_json::Value) = (removed.status, serde_json::from_str(&removed.body).unwrap());
    assert_eq!((status, &document["file_key"]), (200, &serde_json::Value::Null), "{document}");
    assert_eq!(get(&server, "/api/documents/1/file").status, 404);
    assert_eq!(object(&file_key).0, 404);

    // Framework API: bytes and a streamed body, read back, deleted.
    let key = post(&server, "/e2e/bytes", &[]).body;
    assert!(key.starts_with("generated/"), "{key}");
    assert_eq!(object(&key).2, b"hello");
    let raw = binary_file(300_000);
    let mut response = agent().post(&format!("{}/e2e/raw", server.base)).send(&raw[..]).unwrap();
    let raw_key = response.body_mut().read_to_string().unwrap();
    assert!(raw_key.starts_with("raw/"), "{raw_key}");
    assert!(object(&raw_key).2 == raw, "streamed body stored byte for byte");
    let removed = page(agent().delete(&format!("{}/e2e/objects?key={raw_key}", server.base)).call().unwrap());
    assert_eq!((removed.status, removed.body.as_str()), (200, "deleted"));
    assert_eq!(object(&raw_key).0, 404);

    // Over the request limit: refused from Content-Length, before the body is read.
    let over = multipart(
        &server,
        "PUT",
        "/api/documents/1/file",
        &[],
        &[("file", "big.pdf", "application/pdf", &binary_file(200_000))],
    );
    assert_eq!(over.status, 413, "{}", over.body);
    assert!(over.body.contains(r#""message":"The request is too large (maximum is 128 KB)""#), "{}", over.body);
}
