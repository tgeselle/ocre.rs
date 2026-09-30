use super::*;

fn dsn(name: &str) -> Option<String> {
    match name {
        "SENTRY_DSN" => Some("https://abc:legacy@errors.example/sentry/42/".to_owned()),
        "SENTRY_RELEASE" => Some("v1".to_owned()),
        _ => None,
    }
}

#[test]
fn reports_merge_the_request_context_under_their_own_and_log_with_the_request_fields() {
    let reporter = Reporter::new(Logger::new().with("request_id", "r1"));
    reporter.set_context("user_id", 7);
    reporter.set_context("tenant", "acme");
    let options = Options::new().context("tenant", "other").source("billing").severity(Severity::Info).handled(false);
    reporter.clone().report(&crate::Error::NotFound, options);
    let reports = reporter.take();
    assert_eq!(reports.len(), 1);
    let report = &reports[0];
    assert_eq!(report.class, "ocre::error::Error");
    assert_eq!(report.message, "not found");
    assert_eq!((report.handled, report.severity, report.source.as_str()), (false, Severity::Info, "billing"));
    assert_eq!(Value::Object(report.context.clone()), json!({"user_id": 7, "tenant": "other"}));
    assert_eq!(report.environment, Environment::Development);
    assert!(report.timestamp > 1_700_000_000);
    assert!(reporter.take().is_empty(), "take empties the queue");
}

#[test]
fn handle_record_and_unexpected_use_rails_defaults() {
    let reporter = Reporter::default();
    assert_eq!(reporter.handle::<u8, &str>(Ok(1)), Some(1));
    assert_eq!(reporter.record::<u8, &str>(Ok(2)), Ok(2));
    assert!(reporter.take().is_empty());
    assert_eq!(reporter.record::<u8, &str>(Err("bad")), Err("bad"));
    reporter.unexpected_in(&"never", false);
    reporter.report_unhandled("D1 down", "ocre.request", Map::from_iter([("path".to_owned(), json!("/"))]), true);
    reporter.report_unhandled("job failed", "ocre.job", Map::new(), false);
    let reports = reporter.take();
    let summary: Vec<(&str, bool, Severity, &str)> =
        reports.iter().map(|r| (r.message.as_str(), r.handled, r.severity, r.source.as_str())).collect();
    assert_eq!(
        summary,
        [
            ("bad", false, Severity::Error, "application"),
            ("never", true, Severity::Error, "unexpected"),
            ("D1 down", false, Severity::Error, "ocre.request"),
            ("job failed", false, Severity::Error, "ocre.job"),
        ]
    );
    assert!(reports[1].context["location"].as_str().unwrap().contains("tests/errors.rs:"));
    assert_eq!(reports[2].context["path"], "/");
    assert_eq!(reports[2].class, "ocre::Error");
}

#[test]
#[should_panic(expected = "unexpected: no invoice")]
fn unexpected_panics_in_debug_builds() {
    Reporter::default().unexpected("no invoice");
}

#[test]
fn severities_and_options_serialize_like_rails() {
    let names: Vec<&str> = [Severity::Error, Severity::Warning, Severity::Info].map(Severity::as_str).to_vec();
    assert_eq!(names, ["error", "warning", "info"]);
    struct Broken;
    impl Serialize for Broken {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("no"))
        }
    }
    let reporter = Reporter::default();
    reporter.set_context("a", Broken);
    reporter.report(&"x", Options::new().context("b", Broken));
    assert_eq!(Value::Object(reporter.take()[0].context.clone()), json!({"a": null, "b": null}));
}

#[test]
fn sentry_envelopes_carry_the_event_and_the_auth_header() {
    let reporter = Reporter::default();
    reporter.set_context("request_id", "r9");
    reporter.set_context("password", "hunter2");
    reporter.report(&"card declined", Options::new());
    let report = &reporter.take()[0];
    let delivery = Sentry.deliver(report, &dsn).unwrap();
    assert_eq!(Sentry.name(), "sentry");
    assert_eq!(delivery.url, "https://errors.example/sentry/api/42/envelope/");
    assert_eq!(delivery.headers[0], ("content-type".to_owned(), "application/x-sentry-envelope".to_owned()));
    assert!(delivery.headers[1].1.starts_with("Sentry sentry_version=7, sentry_key=abc, sentry_client=ocre/"));
    let lines: Vec<Value> = delivery.body.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["dsn"], "https://abc:legacy@errors.example/sentry/42/");
    assert_eq!(lines[0]["event_id"], lines[2]["event_id"]);
    assert_eq!(lines[0]["event_id"].as_str().unwrap().len(), 32);
    assert_eq!(lines[1], json!({"type": "event"}));
    let event = &lines[2];
    assert_eq!(event["level"], "warning");
    assert_eq!(event["environment"], "development");
    assert_eq!(event["release"], "v1");
    assert_eq!(event["tags"], json!({"source": "application", "request_id": "r9"}));
    assert_eq!(event["extra"]["password"], "[FILTERED]");
    assert_eq!(
        event["exception"]["values"][0],
        json!({"type": "&str", "value": "card declined", "mechanism": {"type": "application", "handled": true}})
    );
    let no_release = |name: &str| (name == "SENTRY_DSN").then(|| "http://k@localhost:9000/1".to_owned());
    let reporter = Reporter::default();
    reporter.report(&"x", Options::new());
    let delivery = Sentry.deliver(&reporter.take()[0], &no_release).unwrap();
    assert_eq!(delivery.url, "http://localhost:9000/api/1/envelope/");
    assert!(!delivery.body.contains("\"release\"") && !delivery.body.contains("request_id"));
}

#[test]
fn a_malformed_dsn_sends_nothing() {
    let reporter = Reporter::default();
    reporter.report(&"x", Options::new());
    let report = &reporter.take()[0];
    for bad in ["nope", "https://host/1", "https://@host/1", "https://k@/1", "https://k@host/abc", "https://k@host"] {
        assert!(Sentry.deliver(report, &|_| Some(bad.to_owned())).is_none(), "{bad}");
    }
}

#[test]
fn deliveries_go_to_every_subscriber_but_the_excepted_ones() {
    struct Hook(&'static str);
    impl Subscriber for Hook {
        fn name(&self) -> &'static str {
            self.0
        }
        fn deliver(&self, report: &Report, vars: &dyn Fn(&str) -> Option<String>) -> Option<Delivery> {
            let url = vars("HOOK_URL")?;
            Some(Delivery { url, headers: vec![], body: format!("{}:{}", self.0, report.message) })
        }
    }
    subscribe(Hook("test-hook-a"));
    subscribe(Hook("test-hook-a"));
    subscribe(Hook("test-hook-b"));
    let reporter = Reporter::default();
    reporter.report(&"one", Options::new().except("test-hook-b"));
    reporter.report(&"two", Options::new());
    let reports = reporter.take();
    let vars = |name: &str| (name == "HOOK_URL").then(|| "https://hook.example".to_owned());
    let sent: Vec<(&str, String)> = deliveries(&reports, &vars)
        .into_iter()
        .filter(|(name, _)| name.starts_with("test-hook"))
        .map(|(name, delivery)| (name, delivery.body))
        .collect();
    assert_eq!(
        sent,
        [
            ("test-hook-a", "test-hook-a:one".to_owned()),
            ("test-hook-a", "test-hook-a:two".to_owned()),
            ("test-hook-b", "test-hook-b:two".to_owned()),
        ]
    );
    assert!(deliveries(&reports, &|_| None).is_empty());
}

#[test]
fn the_dev_page_escapes_everything_and_lists_the_statements() {
    let request = RequestDetails {
        request_id: "r1".into(),
        method: "POST".into(),
        path: "/posts/<1>".into(),
        query: "password=[FILTERED]".into(),
        headers: vec![("accept".into(), "text/html".into())],
    };
    let statements = [crate::instrument::Timing { sql: "SELECT * FROM t WHERE a < ?1".into(), ms: 3.0 }];
    let page = dev_page(500, "D1 query failed: \"x\" & <y>\nSQL: ...", &request, &statements);
    assert!(page.contains("<title>500 D1 query failed: &quot;x&quot; &amp; &lt;y&gt;</title>"), "{page}");
    assert!(page.contains("<pre>D1 query failed: &quot;x&quot; &amp; &lt;y&gt;\nSQL: ...</pre>"));
    assert!(page.contains("<td>/posts/&lt;1&gt;</td>"));
    assert!(page.contains("<tr><th>accept</th><td>text/html</td></tr>"));
    assert!(page.contains("<h2>D1 statements (1)</h2>"));
    assert!(page.contains("<tr><td>3 ms</td><td><code>SELECT * FROM t WHERE a &lt; ?1</code></td></tr>"));
    assert!(dev_page(500, "", &RequestDetails::default(), &[]).contains("<title>500 </title>"));
}

#[test]
fn secret_headers_are_hidden_on_the_dev_page() {
    assert_eq!(shown_header("cookie", "id=1"), "[FILTERED]");
    assert_eq!(shown_header("authorization", "Bearer x"), "[FILTERED]");
    assert_eq!(shown_header("x-api-token", "t"), "[FILTERED]");
    assert_eq!(shown_header("accept", "*/*"), "*/*");
}

fn details(dev: bool) -> RequestDetails {
    let uri: axum::http::Uri = "/posts?password=x&page=2".parse().unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("cookie", "id=1".parse().unwrap());
    headers.insert("accept", "text/html".parse().unwrap());
    RequestDetails::new("r1", "POST", &uri, &headers, dev)
}

fn internal(content_type: &str, body: &str) -> axum::response::Response {
    let mut response =
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, [("content-type", content_type)], body.to_owned())
            .into_response();
    crate::error::mark(Some("D1 down".into()), &mut response);
    response
}

use axum::response::IntoResponse;

#[test]
fn request_details_show_query_and_headers_only_in_development() {
    let dev = details(true);
    assert_eq!((dev.path.as_str(), dev.query.as_str()), ("/posts", "password=[FILTERED]&page=2"));
    assert_eq!(
        dev.headers,
        [("cookie".to_owned(), "[FILTERED]".to_owned()), ("accept".to_owned(), "text/html".to_owned())]
    );
    let prod = details(false);
    assert!(prod.query.is_empty() && prod.headers.is_empty());
    assert_eq!(prod.logger().field("request_id"), Some(&json!("r1")));
}

#[test]
fn finish_reports_internal_errors_and_shows_details_only_in_development() {
    use crate::support::{block_on, body_text};
    let timings = crate::instrument::Timings::default();
    timings.record("SELECT 1", 2.0);
    let reporter = Reporter::default();

    let page = block_on(finish(internal("text/html", "<h1>500</h1>"), &details(true), &reporter, &timings, 9.0, true));
    assert_eq!(page.headers()["x-request-id"], "r1");
    assert_eq!(page.headers()["server-timing"], "db;dur=2;desc=\"1 query\", total;dur=9");
    assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
    let html = body_text(page);
    assert!(html.contains("<pre>D1 down</pre>") && html.contains("<code>SELECT 1</code>"), "{html}");
    let report = &reporter.take()[0];
    assert_eq!((report.source.as_str(), report.handled), ("ocre.request", false));
    assert_eq!(report.context["path"], "/posts");

    let body = r#"{"error":{"status":500,"message":"Internal server error"}}"#;
    let api = block_on(finish(internal("application/json", body), &details(true), &reporter, &timings, 9.0, true));
    let value: Value = serde_json::from_str(&body_text(api)).unwrap();
    assert_eq!(value["error"], json!({"status": 500, "message": "Internal server error", "detail": "D1 down"}));
    let broken = block_on(finish(internal("application/json", "nope"), &details(true), &reporter, &timings, 1.0, true));
    assert_eq!(body_text(broken), r#"{"error":{"detail":"D1 down"}}"#);
    let plain = block_on(finish(internal("text/plain", "oops"), &details(true), &reporter, &timings, 1.0, true));
    assert_eq!(body_text(plain), "oops");
    let mut bare = axum::response::Response::default();
    crate::error::mark(Some("x".into()), &mut bare);
    assert_eq!(body_text(block_on(finish(bare, &details(true), &reporter, &timings, 1.0, true))), "");

    let prod =
        block_on(finish(internal("text/html", "<h1>500</h1>"), &details(false), &reporter, &timings, 1.0, false));
    assert!(prod.headers().get("server-timing").is_none());
    assert_eq!(body_text(prod), "<h1>500</h1>");
    assert_eq!(reporter.take().len(), 5);

    let mut own = axum::response::Response::default();
    own.headers_mut().insert("x-request-id", "mine".parse().unwrap());
    let own = block_on(finish(own, &details(false), &reporter, &timings, 1.0, false));
    assert_eq!(own.headers()["x-request-id"], "mine");
    assert!(reporter.take().is_empty(), "no report without an internal error");
}

#[test]
fn handle_returns_the_value_or_reports_the_error() {
    let reporter = Reporter::default();
    assert_eq!(reporter.handle::<u8, &str>(Ok(4)), Some(4));
    assert_eq!(reporter.handle::<u8, &str>(Err("rates API down")), None);
    let reports = reporter.take();
    assert_eq!(reports.len(), 1);
    assert!(reports[0].handled);
}
