use std::{
    cell::Cell,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::mpsc,
};

use cookie::{Cookie, CookieJar, Key};
use serde_json::json;

use super::*;

const SECRET: &str = "0123456789012345678901234567890123456789012345678901234567890123";

/// A server answering each connection with the next canned response; sends back the raw requests.
fn serve(responses: Vec<String>) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (sent, requests) = mpsc::channel();
    std::thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            request.push_str(&String::from_utf8(body).unwrap());
            sent.send(request).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (base, requests)
}

fn response(status: &str, headers: &[&str], body: &str) -> String {
    let mut text = format!("HTTP/1.1 {status}\r\nconnection: close\r\ncontent-length: {}\r\n", body.len());
    for header in headers {
        text.push_str(header);
        text.push_str("\r\n");
    }
    format!("{text}\r\n{body}")
}

/// The session cookie's value for `data`, percent-encoded as the server sends it.
fn encrypted_session(data: &serde_json::Value) -> String {
    let mut jar = CookieJar::new();
    jar.private_mut(&Key::derive_from(SECRET.as_bytes())).add(Cookie::new(SESSION_COOKIE, data.to_string()));
    let encoded = jar.get(SESSION_COOKIE).unwrap().encoded().to_string();
    encoded.strip_prefix(&format!("{SESSION_COOKIE}=")).unwrap().to_owned()
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ocre-testing-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn panics<R>(f: impl FnOnce() -> R) -> String {
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(f()))).unwrap_err();
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default()
}

#[test]
fn new_reads_the_test_url() {
    // SAFETY: the only test reading or writing OCRE_TEST_URL.
    unsafe { std::env::remove_var(TEST_URL) };
    assert!(panics(|| drop(Client::new())).contains("ocre test --e2e"));
    let (base, requests) = serve(vec![response("200 OK", &[], "OK")]);
    // SAFETY: as above.
    unsafe { std::env::set_var(TEST_URL, format!("{base}/")) };
    Client::new().get("/up").assert_status(200).assert_contains("OK");
    assert!(requests.recv().unwrap().starts_with("GET /up HTTP/1.1"));
}

#[test]
fn requests_send_browser_headers_forms_and_json() {
    let ok = || response("200 OK", &["content-type: application/json"], r#"{"ok":true}"#);
    let (base, requests) = serve(vec![ok(), ok(), ok(), ok(), ok(), ok(), response("204 No Content", &[], "")]);
    let mut client = Client::with_base_url(&base).header("X-Custom", "1").header("x-custom", "2").htmx();
    let page = client.get("/posts");
    let request = requests.recv().unwrap();
    assert!(request.contains("sec-fetch-site: same-origin"), "{request}");
    assert!(request.contains("x-custom: 2") && !request.contains("x-custom: 1"), "{request}");
    assert!(request.contains("hx-request: true"), "{request}");
    assert_eq!(page.json::<serde_json::Value>(), json!({"ok": true}));
    page.assert_header("Content-Type", "application/json");

    client.post("/posts", &[("title", "Hello world"), ("body", "a&b")]);
    let request = requests.recv().unwrap();
    assert!(request.starts_with("POST /posts "), "{request}");
    assert!(request.contains("content-type: application/x-www-form-urlencoded"), "{request}");
    assert!(request.ends_with("title=Hello+world&body=a%26b"), "{request}");

    client.post_json("/a", &json!({"t": 1}));
    assert!(requests.recv().unwrap().ends_with(r#"{"t":1}"#));
    client.patch_json("/a", &json!({}));
    assert!(requests.recv().unwrap().starts_with("PATCH /a "));
    client.put_json("/a", &json!({}));
    assert!(requests.recv().unwrap().starts_with("PUT /a "));
    client.delete("/a");
    assert!(requests.recv().unwrap().starts_with("DELETE /a "));
    let empty = client.request("HEAD", "/up", None);
    assert_eq!(empty.body, "");
    empty.assert_success();
}

#[test]
fn cross_site_requests_are_marked() {
    let (base, requests) = serve(vec![response("403 Forbidden", &[], "Forbidden")]);
    Client::with_base_url(&base).cross_site().post("/posts", &()).assert_status(403);
    let request = requests.recv().unwrap();
    assert!(request.contains("sec-fetch-site: cross-site") && !request.contains("same-origin"), "{request}");
}

#[test]
fn the_cookie_jar_keeps_the_session_and_follows_redirects() {
    // SAFETY: the only test reading SECRET_KEY_BASE from the environment.
    unsafe { std::env::set_var("SECRET_KEY_BASE", SECRET) };
    let session = encrypted_session(&json!({"user_id": 7, "_flash": {"notice": "Saved."}}));
    let (base, requests) = serve(vec![
        response(
            "303 See Other",
            &[
                "location: /posts/1",
                &format!("set-cookie: {SESSION_COOKIE}={session}; Path=/; HttpOnly"),
                "set-cookie: =broken",
                "set-cookie: theme=dark",
            ],
            "",
        ),
        response("200 OK", &["set-cookie: theme=; Max-Age=0", "set-cookie: lang=fr; Max-Age=0"], "Post 1"),
    ]);
    let mut client = Client::with_base_url(&base);
    assert!(client.session().is_empty());
    assert_eq!(client.flash("notice"), None);
    let created = client.post("/posts", &[("title", "x")]);
    created.assert_redirect_to("/posts/1");
    assert_eq!(client.cookie("theme"), Some("dark"));
    assert_eq!(client.session()["user_id"], 7);
    assert_eq!(client.flash("notice").as_deref(), Some("Saved."));
    assert_eq!(client.flash("alert"), None);
    requests.recv().unwrap();

    client.follow_redirect(&created).assert_contains("Post 1");
    let request = requests.recv().unwrap();
    assert!(request.starts_with("GET /posts/1 "), "{request}");
    assert!(request.contains(&format!("{SESSION_COOKIE}={session}")) && request.contains("theme=dark"), "{request}");
    assert_eq!(client.cookie("theme"), None);

    let (other_base, other_requests) = serve(vec![response("200 OK", &[], "")]);
    let mut other = Client::with_base_url(&other_base);
    other.set_cookie("locale", "fr");
    let absolute =
        Response { status: 302, headers: vec![("location".into(), format!("{other_base}/p"))], body: String::new() };
    other.follow_redirect(&absolute);
    let request = other_requests.recv().unwrap();
    assert!(request.starts_with("GET /p ") && request.contains("locale=fr"), "{request}");

    let not_redirect = Response { status: 200, headers: vec![], body: "page".into() };
    assert!(panics(|| drop(client.follow_redirect(&not_redirect))).contains("expected a redirect, got 200"));

    client.set_cookie(SESSION_COOKIE, "garbage");
    assert!(panics(|| drop(client.session())).contains("does not decrypt"));
}

#[test]
fn deliveries_and_broadcasts_read_the_captures() {
    let sent = json!([{"id": 1, "from": "app@example.com", "email": {"to": ["ada@example.com"], "subject": "Hi", "text": "Hello", "html": null, "reply_to": null}}]);
    let broadcasts = json!([{"id": 1, "channel": "posts", "message": "<li>Hi</li>"}]);
    let (base, requests) =
        serve(vec![response("200 OK", &[], &sent.to_string()), response("200 OK", &[], &broadcasts.to_string())]);
    let mut client = Client::with_base_url(&base);
    let emails = client.deliveries();
    assert!(requests.recv().unwrap().starts_with("GET /ocre/dev/mailers/sent.json "));
    assert_eq!(emails.len(), 1);
    assert_eq!(emails[0].to, ["ada@example.com"]);
    assert_eq!(emails[0].subject, "Hi");
    let broadcast = Broadcast { id: 1, channel: "posts".into(), message: "<li>Hi</li>".into() };
    assert_eq!(client.broadcasts(), [broadcast]);
    assert!(requests.recv().unwrap().starts_with("GET /ocre/dev/realtime/sent.json "));
    assert_eq!(block_on(async { 1 }), 1);
}

#[test]
fn jobs_read_the_capture_and_emails_reach_the_mailbox() {
    let jobs = json!({"enqueued": [{"id": 1, "queue": "default", "job": {"send_welcome": {"user_id": 7}}},
                                   {"id": 2, "queue": "default", "job": 3}, {"id": 4, "queue": "urgent", "job": "cleanup"}],
                      "performed": [{"id": 3, "job": "send_welcome", "outcome": "done"}]});
    let (base, requests) = serve(vec![response("200 OK", &[], &jobs.to_string()), response("200 OK", &[], "accepted")]);
    let mut client = Client::with_base_url(&base);
    let jobs = client.jobs();
    assert!(requests.recv().unwrap().starts_with("GET /ocre/dev/jobs.json "));
    assert_eq!(
        jobs.enqueued.iter().map(EnqueuedJob::name).collect::<Vec<_>>(),
        [Some("send_welcome"), None, Some("cleanup")]
    );
    assert_eq!(jobs.performed[0].outcome, "done");

    client.receive_email("ada@example.com", "support@example.com", "Help", "Line 1\nLine 2").assert_success();
    let request = requests.recv().unwrap();
    assert!(
        request.starts_with("POST /cdn-cgi/local/email?from=ada%40example.com&to=support%40example.com "),
        "{request}"
    );
    assert!(request.contains("content-type: message/rfc822"), "{request}");
    assert!(
        request.contains("\r\n\r\nFrom: ada@example.com\r\nTo: support@example.com\r\nSubject: Help\r\n"),
        "{request}"
    );
    assert!(request.ends_with("Content-Transfer-Encoding: 8bit\r\n\r\nLine 1\r\nLine 2"), "{request}");
}

#[test]
fn raw_emails_encode_non_ascii_subjects() {
    let raw = raw_email("a@x.test", "b@x.test", "Café", "Hi\r\nthere", 7);
    assert!(raw.contains("Subject: =?UTF-8?B?Q2Fmw6k=?=\r\nMessage-ID: <7."), "{raw}");
    assert!(raw.ends_with("\r\n\r\nHi\r\nthere"));
}

#[test]
fn an_unreachable_server_panics_with_the_url() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let message = panics(|| drop(Client::with_base_url(&format!("http://127.0.0.1:{port}")).get("/up")));
    assert!(message.contains(&format!("GET http://127.0.0.1:{port}/up failed")), "{message}");
}

#[test]
fn vars_prefer_the_environment_then_dev_vars() {
    let dir = temp_dir("dotenv");
    let dotenv = dir.join(".dev.vars");
    std::fs::write(&dotenv, "OTHER=1\nSECRET_KEY_BASEX=1\nSECRET_KEY_BASE = \"abc\"\n").unwrap();
    assert_eq!(dev_var("SECRET_KEY_BASE", None, &dotenv).as_deref(), Some("abc"));
    assert_eq!(dev_var("SECRET_KEY_BASE", Some("env".into()), &dotenv).as_deref(), Some("env"));
    std::fs::write(&dotenv, "OTHER=1\n").unwrap();
    assert_eq!(dev_var("SECRET_KEY_BASE", None, &dotenv), None);
    assert_eq!(dev_var("SECRET_KEY_BASE", None, &dir.join("missing")), None);
    assert!(decrypt_session("%%%;", SECRET).is_none());
}

#[test]
fn response_assertions_fail_with_the_body() {
    let page =
        Response { status: 500, headers: vec![("Content-Type".into(), "text/html".into())], body: "x".repeat(3000) };
    assert_eq!(page.header("content-type"), Some("text/html"));
    assert!(page.location().is_none());
    page.assert_not_contains("y").assert_contains("x");
    assert!(panics(|| page.assert_status(200).status).contains("expected status 200, got 500"));
    assert!(panics(|| page.assert_success().status).contains("expected a 2xx status"));
    assert!(panics(|| page.assert_redirect_to("/").status).contains("expected a redirect to /"));
    assert!(panics(|| page.assert_contains("y").status).contains("to contain \"y\""));
    assert!(panics(|| page.assert_not_contains("x").status).contains("not to contain"));
    assert!(panics(|| page.assert_header("content-type", "json").status).contains("expected header"));
    let message = panics(|| drop(page.json::<serde_json::Value>()));
    assert!(message.contains("not the expected JSON") && message.len() < 2200, "{}", message.len());
    let redirect = Response { status: 200, headers: vec![("location".into(), "/".into())], body: String::new() };
    assert!(panics(|| redirect.assert_redirect_to("/").status).contains("got 200"));
}

fn fake_wrangler(dir: &Path, script: &str) {
    let bin = dir.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = bin.join("wrangler");
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
}

#[test]
fn sql_runs_wrangler_on_the_test_state() {
    let dir = temp_dir("sql");
    // Echoes its arguments as the result rows, like `d1 execute --json`.
    fake_wrangler(
        &dir,
        r#"case "$*" in
  *COUNT*) echo '[{"results":[{"count":3}]}]' ;;
  *RETURNING*) echo '[{"results":[{"id":42}]}]' ;;
  *fail*) echo 'no such table' ; echo 'boom' >&2 ; exit 1 ;;
  *garbage*) echo 'not json' ;;
  *) printf '[{"results":[]},{"results":[{"args":"%s"},1]}]' "$*" ;;
esac"#,
    );
    APP_ROOT.with(|root| *root.borrow_mut() = dir.clone());
    let rows = sql("SELECT 1");
    assert_eq!(rows.len(), 1);
    let args = rows[0]["args"].as_str().unwrap();
    assert!(
        args.contains("d1 execute DB --local --json --command SELECT 1 -c .wrangler/ocre-d1.json --persist-to"),
        "{args}"
    );
    assert!(args.ends_with(&state_dir()), "{args}");
    assert_eq!(count("posts"), 3);
    assert_eq!(insert("posts", &[("title", json!("it's")), ("published", json!(true))]), 42);
    assert_eq!(fixture("posts", "first")["args"].as_str().map(|a| a.contains("WHERE id = ")), Some(true));
    assert!(panics(|| drop(sql("fail"))).contains("SQL failed: fail\nno such table\nboom"));
    assert!(panics(|| drop(sql("garbage"))).contains("unexpected wrangler output"));

    fake_wrangler(&dir, "echo '[]'");
    assert!(panics(|| drop(fixture("posts", "missing"))).contains("no fixture missing in posts"));
    assert!(sql("SELECT 1").is_empty());

    APP_ROOT.with(|root| *root.borrow_mut() = dir.join("nowhere"));
    assert!(panics(|| drop(sql("SELECT 1"))).contains("npm install"));
}

#[test]
fn state_dir_defaults_to_the_test_state() {
    if std::env::var(TEST_STATE).is_err() {
        assert_eq!(state_dir(), ".wrangler/test-state");
    }
}

#[test]
fn insert_sql_quotes_every_value() {
    assert_eq!(
        insert_sql(
            "posts",
            &[
                ("title", json!("it's")),
                ("n", json!(1.5)),
                ("ok", json!(false)),
                ("tags", json!(["a"])),
                ("x", json!(null))
            ]
        ),
        r#"INSERT INTO posts (title, n, ok, tags, x) VALUES ('it''s', 1.5, 0, '["a"]', NULL) RETURNING id"#
    );
    assert_eq!(quote(json!({"q": "it's"})), r#"'{"q":"it''s"}'"#);
}

#[test]
fn fixture_ids_match_rails() {
    // Ruby: Zlib.crc32("first") % (2**30 - 1)
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(fixture_id("david"), 127_326_141);
    assert!(sequence() < sequence());
}

#[test]
fn the_log_is_read_from_the_mark() {
    let dir = temp_dir("log");
    let path = dir.join("dev.log");
    let missing = Log::mark_at(path.clone());
    assert_eq!(missing.text(), "");
    std::fs::write(&path, "before\n").unwrap();
    let log = Log::mark_at(path.clone());
    std::fs::write(&path, "before\n[ocre jobs] welcome done\n").unwrap();
    assert_eq!(log.text(), "[ocre jobs] welcome done\n");
    assert_eq!(log.wait_for("welcome done"), "[ocre jobs] welcome done");
    let message = panics(|| drop(log.wait_for_within("never", Duration::from_millis(150))));
    assert!(message.contains("no log line containing \"never\"") && message.contains("welcome done"), "{message}");
    if std::env::var(TEST_LOG).is_err() {
        assert_eq!(Log::mark().path, PathBuf::from(".wrangler/test-state/dev.log"));
    }
}

#[test]
fn eventually_retries_until_some() {
    let tries = Cell::new(0);
    let value = eventually(|| {
        tries.set(tries.get() + 1);
        (tries.get() == 2).then_some(5)
    });
    assert_eq!((value, tries.get()), (5, 2));
}

#[test]
fn difference_and_change_assertions() {
    let count = Cell::new(1);
    // One instantiation for both calls: boxed closures of the same types.
    let counter = &count;
    let expression = || Box::new(move || counter.get()) as Box<dyn FnMut() -> i64 + '_>;
    let set = |value| Box::new(move || counter.set(value)) as Box<dyn FnOnce() + '_>;
    assert_eq!(assert_difference(expression(), 2, set(3)), ());
    assert_no_difference(|| count.get(), || ());
    let message = panics(|| assert_difference(expression(), 1, set(3)));
    assert!(message.contains("expected a difference of 1, got 0 (3 -> 3)"), "{message}");
    let (before, after) = assert_changes(|| count.get(), || count.set(4));
    assert_eq!((before, after), (3, 4));
    assert!(panics(|| assert_changes(|| count.get(), || ())).contains("expected a change, still 4"));
    assert_no_changes(|| count.get(), || ());
    assert!(panics(|| assert_no_changes(|| count.get(), || count.set(5))).contains("4 became 5"));
}

#[test]
fn time_travel_freezes_now_on_this_thread() {
    travel_to(1_000);
    assert_eq!(crate::now(), 1_000);
    travel(-10);
    assert_eq!(crate::now(), 990);
    std::thread::spawn(|| assert!(crate::now() > 1_000_000)).join().unwrap();
    travel_back();
    let frozen = freeze_time();
    assert_eq!(crate::now(), frozen);
    travel_back();
}

#[test]
fn redact_hides_ids_dates_and_tokens() {
    let text = "id 123e4567-e89b-42d3-a456-426614174000, x2026-09-29, \
                123e4567-e89b-42d3-a456-426614174000a on 2026-09-29, at 2026-09-29T10:00:00.123Z, \
                2026-09-29 10:00+02:00, 2026-09-29T10:00:00-05:00 token abcdefghij0123456789abcdefghij0123 \
                and abcdefghijabcdefghijabcdefghijabcdefghij, é";
    assert_eq!(
        redact(text),
        "id <UUID>, x2026-09-29, <TOKEN> on <DATE>, at <DATE>, <DATE>, <DATE> \
         token <TOKEN> and abcdefghijabcdefghijabcdefghijabcdefghij, é"
    );
    assert_eq!(redact("2026-09-29T10"), "<DATE>T10");
}

#[test]
fn eventually_retries_then_names_the_timeout() {
    let mut calls = 0;
    let mut soon = || {
        calls += 1;
        (calls > 1).then_some(5_i64)
    };
    let within = Duration::from_millis(20);
    assert_eq!(eventually_within(within, &mut soon as &mut dyn FnMut() -> Option<i64>), 5);
    let mut never = || None;
    let message = panics(|| eventually_within(within, &mut never as &mut dyn FnMut() -> Option<i64>));
    assert_eq!(message, "the condition did not hold within 20ms");
}

#[test]
fn unreadable_bodies_and_wrong_json_panic_with_details() {
    assert_eq!(body_bytes(Ok(b"ok".to_vec()), "GET", "http://x/"), b"ok");
    let message = panics(|| body_bytes(Err(ureq::Error::BodyExceedsLimit(10)), "GET", "http://x/posts"));
    assert!(message.starts_with("GET http://x/posts: body: "), "{message}");
    let response = Response { status: 200, headers: vec![], body: "not json".into() };
    assert!(panics(|| response.json::<Value>()).starts_with("the body is not the expected JSON"));
    let response = Response { status: 200, headers: vec![], body: "[1]".into() };
    assert_eq!(response.json::<Value>(), json!([1]));
}
