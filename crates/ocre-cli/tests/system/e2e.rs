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
    // 204 has no body by definition; wrangler dev still labels it gzip, which
    // makes the client's decompressor fail on the empty stream.
    let body = if status == 204 { String::new() } else { response.body_mut().read_to_string().unwrap() };
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
    let (report, ok) =
        sandbox.json(&["g", "api", "Book", "title:string", "pages:integer", "available:boolean", "--graphql"], &root);
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
    assert!(browser.get("/login").body.contains("Please log in to continue."));

    // Sign up (validation, then success), back to the page that asked.
    let short = browser.post("/signup", &[("email", "Ada@Example.com"), ("password", "short")]);
    assert_eq!(short.status, 422);
    assert!(short.body.contains("<li>Password is too short (minimum is 8 characters)</li>"), "{}", short.body);
    browser.get("/account");
    let signed_up = browser.post("/signup", &[("email", " Ada@Example.com"), ("password", "correct horse")]);
    assert_eq!((signed_up.status, signed_up.location.as_str()), (303, "/account"));
    let account = browser.get("/account");
    assert_eq!(account.status, 200);
    assert!(account.body.contains("<dd>ada@example.com</dd>"), "email normalized: {}", account.body);
    assert!(account.body.contains("Welcome! Your account is ready."));
    let taken = browser.post("/signup", &[("email", "ADA@example.com"), ("password", "another one")]);
    assert!(taken.status == 422 && taken.body.contains("<li>Email has already been taken</li>"), "{}", taken.body);

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

    // Log in with a password (email case ignored), back to /account.
    let logged_in = browser.post("/login", &[("email", "ADA@example.com"), ("password", "correct horse")]);
    assert_eq!((logged_in.status, logged_in.location.as_str()), (303, "/account"));
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
}
