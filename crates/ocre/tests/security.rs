use axum::http::HeaderMap;

use super::*;
use crate::support::block_on;

#[test]
fn url_from_accepts_only_this_app() {
    let uri: Uri = "https://app.example.com:8443/login".parse().unwrap();
    assert_eq!(url_from(&uri, "/a").as_deref(), Some("/a"));
    assert_eq!(url_from(&uri, "https://APP.example.com:8443/b?c=1#d").as_deref(), Some("/b?c=1"));
    assert_eq!(url_from(&uri, "http://app.example.com:8443").as_deref(), Some("/"));
    for refused in [
        "",
        "//evil.example",
        "/\\evil.example",
        "https://app.example.com/other-port",
        "https://evil.example/",
        "javascript:alert(1)",
        "ftp://app.example.com:8443/",
        "relative/path",
        "/a\tb",
    ] {
        assert_eq!(url_from(&uri, refused), None, "{refused}");
    }
    let relative: Uri = "/login".parse().unwrap();
    assert_eq!(url_from(&relative, "https://app.example.com/"), None, "no host to compare with");
}

#[test]
fn parameters_are_filtered() {
    assert_eq!(filter_parameters(""), "");
    assert_eq!(filter_parameters("page=2&flag&API_KEY=k&q=a%20b"), "page=2&flag&API_KEY=[FILTERED]&q=a%20b");
    assert_eq!(filter_parameters("reset_token=abc&otp_code=1"), "reset_token=[FILTERED]&otp_code=[FILTERED]");
    assert_eq!(filter_parameters("%zz=1"), "%zz=1");
    let json = serde_json::json!([{"password": {"nested": 1}}, "email", 3]);
    assert_eq!(filter_json(&json), serde_json::json!([{"password": "[FILTERED]"}, "email", 3]));
}

#[test]
fn basic_auth_extracts_and_challenges() {
    let extract = |value: Option<&str>| {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        }
        let (mut parts, ()) = axum::http::Request::new(()).into_parts();
        parts.headers = headers;
        block_on(BasicAuth::from_request_parts(&mut parts, &())).map_err(Box::new)
    };
    // "admin:s3cret:with:colons"
    let auth = extract(Some("basic YWRtaW46czNjcmV0OndpdGg6Y29sb25z")).unwrap();
    assert_eq!((auth.username.as_str(), auth.password.as_str()), ("admin", "s3cret:with:colons"));
    assert!(auth.matches("admin", "s3cret:with:colons"));
    assert!(!auth.matches("root", "s3cret:with:colons"));
    for bad in
        [None, Some("Bearer abc"), Some("Basic !!!"), Some("Basic bm9jb2xvbg=="), Some("Basic"), Some("Basic /w==")]
    {
        let response = extract(bad).unwrap_err();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{bad:?}");
        assert!(response.headers()[header::WWW_AUTHENTICATE].to_str().unwrap().starts_with("Basic realm="));
    }
    let mut parts = axum::http::Request::new(()).into_parts().0;
    parts.headers.insert(header::AUTHORIZATION, HeaderValue::from_bytes(b"Basic \xff").unwrap());
    assert!(block_on(BasicAuth::from_request_parts(&mut parts, &())).is_err(), "not UTF-8");
}
