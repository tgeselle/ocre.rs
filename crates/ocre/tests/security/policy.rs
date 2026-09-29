use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::get,
};

use super::*;
use crate::support::{block_on, body_text};

fn send(app: &mut Router, uri: &str) -> Response {
    block_on(app.call(Request::builder().uri(uri).body(Body::empty()).unwrap())).unwrap()
}

fn global() -> ContentSecurityPolicy {
    ContentSecurityPolicy::new().default_src(&[SELF]).script_src(&[SELF, NONCE])
}

#[test]
fn every_response_gets_the_policy_with_the_request_nonce() {
    let mut app = Router::new()
        .route("/", get(|nonce: CspNonce| async move { nonce.to_string() }))
        .route("/plain", get(|| async { "plain" }))
        .layer(global())
        .layer(PermissionsPolicy::new().deny(&["camera"]));
    let response = send(&mut app, "/");
    let csp = response.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().to_owned();
    assert_eq!(response.headers()["permissions-policy"], "camera=()");
    let nonce = body_text(response);
    assert_eq!(nonce.len(), 24, "16 bytes in base64");
    assert_eq!(csp, format!("default-src 'self'; script-src 'self' 'nonce-{nonce}'"));

    let again = send(&mut app, "/");
    assert_ne!(body_text(again), nonce, "a new nonce per request");
    assert!(send(&mut app, "/plain").headers().contains_key(header::CONTENT_SECURITY_POLICY));
}

#[test]
fn routes_and_handlers_override_the_global_policy() {
    let reports = Router::new()
        .route("/report", get(|nonce: CspNonce| async move { nonce.as_str().to_owned() }))
        .layer(ContentSecurityPolicy::new().script_src(&[NONCE]).report_only());
    let mut app = Router::new()
        .route(
            "/own",
            get(|| async {
                ([(header::CONTENT_SECURITY_POLICY, "default-src 'none'"), (permissions_policy(), "usb=()")], "own")
            }),
        )
        .merge(reports)
        .layer(global())
        .layer(PermissionsPolicy::new().deny(&["camera"]));

    let own = send(&mut app, "/own");
    assert_eq!(own.headers()[header::CONTENT_SECURITY_POLICY], "default-src 'none'");
    assert_eq!(own.headers()["permissions-policy"], "usb=()");

    let report = send(&mut app, "/report");
    let headers = report.headers().clone();
    assert!(!headers.contains_key(header::CONTENT_SECURITY_POLICY), "the route's report-only policy replaces it");
    let nonce = body_text(report);
    assert_eq!(
        headers[header::CONTENT_SECURITY_POLICY_REPORT_ONLY],
        format!("script-src 'nonce-{nonce}'").as_str(),
        "outer nonce reused"
    );
}

#[test]
fn policies_without_a_nonce_or_with_bad_values() {
    let mut app = Router::new()
        .route("/", get(|| async { "home" }))
        .layer(ContentSecurityPolicy::new().default_src(&["'self'\n"]))
        .layer(ContentSecurityPolicy::new().img_src(&[DATA]));
    let response = send(&mut app, "/");
    assert_eq!(response.headers()[header::CONTENT_SECURITY_POLICY], "img-src data:", "invalid inner value not sent");

    let mut app = Router::new().route("/", get(|| async { "home" })).layer(PermissionsPolicy::new());
    assert!(!send(&mut app, "/").headers().contains_key("permissions-policy"), "empty policy: no header");
}

#[test]
fn the_nonce_extractor_needs_the_layer() {
    let mut app = Router::new().route("/", get(|_: CspNonce| async { "never" }));
    let response = send(&mut app, "/");
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let _ = response.into_response();
}

#[test]
fn builders_render_every_directive() {
    let csp = ContentSecurityPolicy::new()
        .style_src(&[SELF, UNSAFE_INLINE])
        .font_src(&[SELF])
        .connect_src(&[SELF])
        .media_src(&[BLOB])
        .object_src(&[NONE])
        .frame_src(&[HTTPS])
        .frame_ancestors(&[SELF])
        .form_action(&[SELF])
        .base_uri(&[SELF])
        .upgrade_insecure_requests()
        .report_uri("/r")
        .report_to("csp")
        .script_src(&[STRICT_DYNAMIC, UNSAFE_EVAL]);
    assert_eq!(
        csp.header_value(None),
        "style-src 'self' 'unsafe-inline'; font-src 'self'; connect-src 'self'; media-src blob:; object-src 'none'; \
         frame-src https:; frame-ancestors 'self'; form-action 'self'; base-uri 'self'; upgrade-insecure-requests; \
         report-uri /r; report-to csp; script-src 'strict-dynamic' 'unsafe-eval'"
    );
    assert_eq!(csp.header_name(), header::CONTENT_SECURITY_POLICY);

    let csp = ContentSecurityPolicy::new()
        .directive("worker-src", &[SELF])
        .default_src(&[NONE])
        .directive("worker-src", &[BLOB]);
    assert_eq!(csp.header_value(None), "worker-src blob:; default-src 'none'", "replaced in place");

    let policy = PermissionsPolicy::new()
        .allow("fullscreen", &["self", "https://a\"b.example"])
        .allow("autoplay", &["*"])
        .allow("fullscreen", &[SELF]);
    assert_eq!(policy.header_value(), "fullscreen=(self), autoplay=*");
    let policy = PermissionsPolicy::new().allow("geolocation", &["https://a\"b.example"]);
    assert_eq!(policy.header_value(), r#"geolocation=("https://ab.example")"#);
}
