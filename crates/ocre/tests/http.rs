use std::collections::BTreeMap;

use axum::http::HeaderMap;

use super::*;

#[test]
fn a_new_body_replaces_the_previous_content_type() {
    let request = Request::post("https://example.com").json(&[1]).form(&[("a", "b")]);
    assert_eq!(request.headers(), [("content-type".to_owned(), "application/x-www-form-urlencoded".to_owned())]);
    assert_eq!(request.body_bytes(), Some(b"a=b".as_slice()));
}

#[test]
fn bodies_that_cannot_be_serialized_are_kept_as_the_reason_to_fail() {
    let map = BTreeMap::from([((1, 2), "pair keys are not JSON keys")]);
    let request = Request::post("https://example.com").json(&map);
    assert!(request.invalid_reason().unwrap().starts_with("the JSON body cannot be serialized"));
    assert!(request.body_bytes().is_none());
    let nested = Request::post("https://example.com").form(&[("a", [1, 2])]);
    assert!(nested.invalid_reason().unwrap().starts_with("the form body cannot be serialized"));
}

#[test]
fn errors_name_the_request_without_its_query() {
    let request = Request::get("https://api.example.com/v1/x?api_key=secret#frag");
    assert_eq!(
        request.error("timeout").to_string(),
        "internal error: GET https://api.example.com/v1/x failed: timeout"
    );
    let response = Response::of(&request, 502, HeaderMap::new(), "x".repeat(300).into_bytes());
    let message = response.clone().error_for_status().unwrap_err().to_string();
    assert_eq!(message, format!("internal error: GET https://api.example.com/v1/x answered 502: {}", "x".repeat(200)));
    assert!(!message.contains("secret"));
    assert!(
        response.json::<i32>().unwrap_err().to_string().contains("GET https://api.example.com/v1/x answered a body")
    );
}

#[test]
fn successes_pass_through_error_for_status() {
    let response = Response::new(204, HeaderMap::new(), Vec::new());
    assert_eq!(response.clone().error_for_status().unwrap(), response);
    assert!(Response::new(299, HeaderMap::new(), Vec::new()).is_success());
    assert!(!Response::new(300, HeaderMap::new(), Vec::new()).is_success());
    assert!(!Response::new(199, HeaderMap::new(), Vec::new()).is_success());
}

#[test]
fn defaults_follow_redirects_with_the_default_timeout() {
    let request = Request::get("https://example.com");
    assert!(request.follows_redirects());
    assert_eq!(request.timeout_duration(), DEFAULT_TIMEOUT);
    assert!(request.invalid_reason().is_none());
}

#[test]
fn test_overrides_are_read_only_in_development() {
    let mut headers = HeaderMap::new();
    headers.insert("x-ocre-test-payments-api", " http://127.0.0.1:1 ".parse().unwrap());
    assert_eq!(override_in(Environment::Development, &headers, "payments-api"), Some("http://127.0.0.1:1"));
    assert_eq!(override_in(Environment::Production, &headers, "payments-api"), None);
    assert_eq!(override_in(Environment::Development, &headers, "other"), None);
}

#[test]
fn constructors_set_the_method_and_builders_the_parts() {
    let url = "https://example.com/x";
    let built = [
        (Request::get(url), "GET"),
        (Request::post(url), "POST"),
        (Request::put(url), "PUT"),
        (Request::patch(url), "PATCH"),
        (Request::delete(url), "DELETE"),
        (Request::head(url), "HEAD"),
    ];
    for (request, method) in built {
        assert_eq!((request.method(), request.url()), (method, url));
    }
    let request = Request::get("https://example.com/s")
        .header(" Accept ", " text/csv ")
        .bearer(" t0k ")
        .query(&[("q", "a b"), ("page", "2")])
        .body(b"x".to_vec(), "text/plain")
        .timeout(std::time::Duration::from_secs(3))
        .follow_redirects(false);
    assert_eq!(request.url(), "https://example.com/s?q=a%20b&page=2");
    assert_eq!(
        request.headers(),
        [
            ("accept".to_owned(), "text/csv".to_owned()),
            ("authorization".to_owned(), "Bearer t0k".to_owned()),
            ("content-type".to_owned(), "text/plain".to_owned())
        ]
    );
    assert_eq!(request.timeout_duration(), std::time::Duration::from_secs(3));
    assert!(!request.follows_redirects());
}

#[test]
fn responses_read_headers_and_text() {
    let mut headers = HeaderMap::new();
    headers.insert("etag", "\"v1\"".parse().unwrap());
    let response = Response::new(200, headers, vec![0xff, b'a']);
    assert_eq!(response.header("ETag"), Some("\"v1\""));
    assert_eq!(response.header("missing"), None);
    assert_eq!(response.text(), "\u{fffd}a");
}

#[test]
fn the_test_override_follows_the_build() {
    let mut headers = HeaderMap::new();
    headers.insert("x-ocre-test-api", "http://127.0.0.1:9".parse().unwrap());
    assert_eq!(test_override(&headers, "api").is_some(), cfg!(debug_assertions));
}
