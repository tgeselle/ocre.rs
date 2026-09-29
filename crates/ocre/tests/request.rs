use axum::{http::Request, response::IntoResponse};

use super::*;
use crate::support::block_on;

fn parts(headers: &[(&str, &str)]) -> Parts {
    let mut builder = Request::builder();
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(()).unwrap().into_parts().0
}

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    parts(pairs).headers
}

#[test]
fn remote_ip_trusts_only_cloudflares_header() {
    let mut p = parts(&[("cf-connecting-ip", " 2001:db8::1 "), ("x-forwarded-for", "10.0.0.1")]);
    let Ok(RemoteIp(ip)) = block_on(RemoteIp::from_request_parts(&mut p, &()));
    assert_eq!(ip.unwrap().to_string(), "2001:db8::1");
    assert_eq!(remote_ip(&headers(&[("x-forwarded-for", "10.0.0.1")])), None);
    assert_eq!(remote_ip(&headers(&[("cf-connecting-ip", "not an ip")])), None);
}

#[test]
fn request_id_prefers_cf_ray_then_a_valid_client_id_then_random() {
    let mut p = parts(&[("cf-ray", "8c2f1a0b9d3e4f5a-CDG"), ("x-request-id", "abc")]);
    let Ok(RequestId(id)) = block_on(RequestId::from_request_parts(&mut p, &()));
    assert_eq!(id, "8c2f1a0b9d3e4f5a-CDG");
    assert_eq!(request_id(&headers(&[("x-request-id", "job_42")])), "job_42");
    for bad in ["", "has space", &"x".repeat(65)] {
        let generated = request_id(&headers(&[("x-request-id", bad)]));
        assert_eq!(generated.len(), 16, "{bad:?}");
        assert!(generated.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    assert_ne!(request_id(&HeaderMap::new()), request_id(&HeaderMap::new()));
}

#[test]
fn format_follows_the_highest_quality_accept_entry() {
    let format = |accept: &str| Format::from_headers(&headers(&[("accept", accept)]));
    assert_eq!(format("application/json"), Format::Json);
    assert_eq!(format("application/vnd.api+json"), Format::Json);
    assert_eq!(format("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"), Format::Html);
    assert_eq!(format("application/json;q=0.5, text/xml"), Format::Xml);
    assert_eq!(format("application/atom+xml"), Format::Xml);
    assert_eq!(format("text/plain"), Format::Text);
    assert_eq!(format("*/*"), Format::Html);
    assert_eq!(format("application/pdf"), Format::Other);
    // q=0 means "not acceptable"; ties keep the first entry.
    assert_eq!(format("application/json;q=0, text/plain"), Format::Text);
    assert_eq!(format("application/json, text/html"), Format::Json);
    assert_eq!(format(" , ;q=1"), Format::Html);
    assert_eq!(format("application/json; charset=utf-8; q=bad"), Format::Json);
    let mut p = parts(&[]);
    let Ok(format) = block_on(Format::from_request_parts(&mut p, &()));
    assert_eq!(format, Format::Html);
}

#[test]
fn redirect_back_only_follows_same_host_referers() {
    let location = |pairs: &[(&str, &str)]| {
        let response = redirect_back(&headers(pairs), "/fallback").into_response();
        assert_eq!(response.status(), 303);
        response.headers()["location"].to_str().unwrap().to_owned()
    };
    let host = ("host", "Blog.example");
    assert_eq!(location(&[host, ("referer", "http://blog.example/posts/1?x=2#top")]), "/posts/1?x=2");
    assert_eq!(location(&[host, ("referer", "https://blog.example")]), "/");
    assert_eq!(location(&[host, ("referer", "https://blog.example//evil.example/")]), "/fallback");
    assert_eq!(location(&[host, ("referer", "https://other.example/posts")]), "/fallback");
    assert_eq!(location(&[host, ("referer", "ftp://blog.example/")]), "/fallback");
    assert_eq!(location(&[host]), "/fallback");
    assert_eq!(location(&[("referer", "https://blog.example/")]), "/fallback");
}
