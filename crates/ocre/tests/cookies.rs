use axum::http::{HeaderMap, Request};

use super::*;
use crate::session::key_from_secret;

const SECRET: &str = "0123456789012345678901234567890123456789012345678901234567890123";

fn keys() -> std::result::Result<Keys, String> {
    key_from_secret(Some(SECRET.to_owned()))
}

/// The `name=value` pairs of `set`, as a browser sends them back.
fn cookie_header(set: &[HeaderValue]) -> HeaderMap {
    let pairs: Vec<String> =
        set.iter().map(|value| value.to_str().unwrap().split(';').next().unwrap().to_owned()).collect();
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, pairs.join("; ").parse().unwrap());
    headers
}

#[test]
fn plain_signed_and_encrypted_cookies_round_trip() {
    let cookies = Cookies::from_headers(&HeaderMap::new(), keys(), true);
    cookies.set("theme", "dark", None).unwrap();
    cookies.set_signed("seen", "1", Some(Duration::from_secs(60))).unwrap();
    cookies.set_encrypted("token", "s3cret", None).unwrap();
    let set = cookies.set_cookies();
    assert_eq!(set.len(), 3);
    let texts: Vec<&str> = set.iter().map(|value| value.to_str().unwrap()).collect();
    assert!(texts.contains(&"theme=dark; HttpOnly; SameSite=Lax; Secure; Path=/"), "{texts:?}");
    assert!(texts.iter().any(|text| text.starts_with("seen=") && text.ends_with("Max-Age=60")), "{texts:?}");
    assert!(!texts.iter().any(|text| text.contains("s3cret")), "encrypted values are hidden");

    let next = Cookies::from_headers(&cookie_header(&set), keys(), true);
    assert_eq!(next.get("theme").as_deref(), Some("dark"));
    assert_eq!(next.signed("seen").unwrap().as_deref(), Some("1"));
    assert_eq!(next.encrypted("token").unwrap().as_deref(), Some("s3cret"));
    assert_eq!(next.signed("theme").unwrap(), None, "a plain cookie is not signed");
    assert_eq!(next.encrypted("seen").unwrap(), None);
    assert!(next.set_cookies().is_empty(), "nothing changed");
}

#[test]
fn tampered_cookies_read_as_none_and_removal_expires_them() {
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, "seen=AAAA1; token=BBBB".parse().unwrap());
    let cookies = Cookies::from_headers(&headers, keys(), false);
    assert_eq!((cookies.signed("seen").unwrap(), cookies.encrypted("token").unwrap()), (None, None));
    cookies.remove("seen");
    let removal = cookies.set_cookies()[0].to_str().unwrap().to_owned();
    assert!(removal.starts_with("seen=; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970"), "{removal}");
}

#[test]
fn the_session_cookie_and_missing_keys_are_errors() {
    let cookies = Cookies::from_headers(&HeaderMap::new(), key_from_secret(None), false);
    let message = |err: Error| err.to_string();
    assert!(message(cookies.set(SESSION_COOKIE, "x", None).unwrap_err()).contains("is the session cookie"));
    assert!(cookies.set_signed("a", "b", None).is_err() && cookies.set_encrypted("a", "b", None).is_err());
    assert!(cookies.signed("a").is_err() && cookies.encrypted("a").is_err());
}

#[test]
fn handlers_extract_them_from_serve() {
    let mut parts = Request::new(()).into_parts().0;
    assert!(crate::support::block_on(Cookies::from_request_parts(&mut parts, &())).is_err(), "outside serve");
    parts.extensions.insert(Cookies::from_headers(&HeaderMap::new(), keys(), false));
    assert!(crate::support::block_on(Cookies::from_request_parts(&mut parts, &())).is_ok());
}
