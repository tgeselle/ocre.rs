use axum::{
    http::{Request, header},
    routing::{get, post},
};
use tower_service::Service;

use super::*;
use crate::{
    Session,
    support::{block_on, body_text},
};

const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn app(key: std::result::Result<Key, String>, origins: &str) -> Router {
    let router = Router::new()
        .route("/", get(|| async { "home" }))
        .route(
            "/login",
            post(|session: Session| async move {
                session.insert("user_id", 7)?;
                Ok::<_, crate::Error>("in")
            }),
        )
        .route("/framed", get(|| async { ([(header::X_FRAME_OPTIONS, "DENY")], "custom") }));
    wrap(router, Config { key, allowed_origins: parse_origins(Some(origins.to_owned())) })
}

fn send(app: &mut Router, request: Request<Body>) -> Response {
    block_on(app.call(request)).unwrap()
}

fn request(method: &str, uri: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::empty()).unwrap()
}

fn key() -> std::result::Result<Key, String> {
    crate::session::key_from_secret(Some(SECRET.to_owned()))
}

#[test]
fn parses_allowed_origins() {
    assert_eq!(parse_origins(None), Vec::<HeaderValue>::new());
    assert_eq!(
        parse_origins(Some(" https://a.example/, ,https://b.example ,bad\u{7f}".into())),
        ["https://a.example", "https://b.example"]
    );
}

#[test]
fn refuses_cross_site_unsafe_requests() {
    let trusted = parse_origins(Some("https://app.example".into()));
    let check = |method: Method, headers: &[(&str, &str)]| {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.insert(HeaderName::from_bytes(name.as_bytes()).unwrap(), HeaderValue::from_str(value).unwrap());
        }
        cross_origin(&method, &map, &trusted).is_none()
    };
    assert!(check(Method::GET, &[("sec-fetch-site", "cross-site")]), "safe methods pass");
    assert!(check(Method::POST, &[("sec-fetch-site", "same-origin")]));
    assert!(check(Method::POST, &[("sec-fetch-site", "none")]), "typed URL, bookmarks");
    assert!(!check(Method::POST, &[("sec-fetch-site", "cross-site")]));
    assert!(!check(Method::DELETE, &[("sec-fetch-site", "same-site")]), "sibling subdomains too");
    assert!(check(Method::POST, &[("sec-fetch-site", "cross-site"), ("origin", "https://app.example")]), "trusted");
    assert!(check(Method::POST, &[]), "curl, server-to-server");
    assert!(check(Method::POST, &[("origin", "https://x.dev"), ("host", "x.dev")]), "old browser, same host");
    assert!(!check(Method::POST, &[("origin", "https://evil.dev"), ("host", "x.dev")]));
    assert!(!check(Method::POST, &[("origin", "null"), ("host", "x.dev")]), "sandboxed or opaque origin");
    assert!(check(Method::POST, &[("origin", "https://x.dev")]), "no Host to compare");
}

#[test]
fn middleware_stack_end_to_end() {
    let mut app = app(key(), "");
    let response = send(&mut app, request("GET", "https://x.dev/", &[]));
    let headers = response.headers();
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["x-frame-options"], "SAMEORIGIN");
    assert_eq!(headers["referrer-policy"], "strict-origin-when-cross-origin");
    assert_eq!(headers["x-xss-protection"], "0");
    assert_eq!(headers["x-permitted-cross-domain-policies"], "none");
    assert_eq!(headers["strict-transport-security"], "max-age=63072000");
    assert!(headers.get(header::SET_COOKIE).is_none(), "untouched session sets no cookie");

    let response = send(&mut app, request("GET", "http://localhost/framed", &[]));
    assert_eq!(response.headers()["x-frame-options"], "DENY", "handlers win");
    assert!(response.headers().get("strict-transport-security").is_none(), "HSTS only on HTTPS");

    let response = send(&mut app, request("POST", "https://x.dev/login", &[("sec-fetch-site", "same-origin")]));
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()[header::SET_COOKIE].to_str().unwrap().contains("Secure"));

    let response = send(&mut app, request("POST", "https://x.dev/login", &[("sec-fetch-site", "cross-site")]));
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.headers()["x-content-type-options"], "nosniff", "refusals get headers too");
    assert!(body_text(response).starts_with("Forbidden: cross-site request."));
}

#[test]
fn session_errors_become_responses() {
    let mut app = app(Err("no secret".into()), "");
    let response = send(&mut app, request("POST", "http://localhost/login", &[]));
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    // The handler succeeds but the cookie cannot be written.
    let router = Router::new().route(
        "/big",
        get(|session: Session| async move {
            session.insert("big", "x".repeat(5000))?;
            Ok::<_, crate::Error>("ok")
        }),
    );
    let mut app = wrap(router, Config { key: key(), allowed_origins: vec![] });
    assert_eq!(send(&mut app, request("GET", "http://localhost/big", &[])).status(), 500);
}

#[test]
fn cors_only_for_allowed_origins() {
    let mut app = app(key(), "https://app.example");
    let preflight = request(
        "OPTIONS",
        "https://x.dev/login",
        &[("origin", "https://app.example"), ("access-control-request-method", "POST")],
    );
    let response = send(&mut app, preflight);
    assert_eq!(response.headers()["access-control-allow-origin"], "https://app.example");
    assert_eq!(response.headers()["access-control-allow-credentials"], "true");

    let response = send(&mut app, request("GET", "https://x.dev/", &[("origin", "https://evil.dev")]));
    assert!(response.headers().get("access-control-allow-origin").is_none());

    let response = send(
        &mut app,
        request("POST", "https://x.dev/login", &[("origin", "https://app.example"), ("sec-fetch-site", "cross-site")]),
    );
    assert_eq!(response.status(), StatusCode::OK, "allowed origins pass the CSRF check");
}
