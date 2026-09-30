use axum::http::{HeaderMap, Method, header};

use super::*;

fn cookies(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, value.parse().unwrap());
    headers
}

#[test]
fn only_on_turns_replicas_on() {
    assert!(enabled(Some("on")) && enabled(Some(" ON ")));
    assert!(!enabled(Some("off")) && !enabled(Some("true")) && !enabled(None));
}

#[test]
fn sessions_resume_from_the_bookmark_else_start_by_method() {
    let empty = HeaderMap::new();
    assert_eq!(session_start(&empty, &Method::GET, "DB"), "first-unconstrained");
    assert_eq!(session_start(&empty, &Method::HEAD, "DB"), "first-unconstrained");
    assert_eq!(session_start(&empty, &Method::POST, "DB"), "first-primary", "writes start on the primary");
    let headers = cookies("theme=dark; ocre_d1_db=0000001-abc; ocre_d1_analytics=0000009-x");
    assert_eq!(session_start(&headers, &Method::POST, "DB"), "0000001-abc");
    assert_eq!(session_start(&headers, &Method::GET, "ANALYTICS"), "0000009-x");
    assert_eq!(session_start(&cookies("ocre_d1_db="), &Method::GET, "DB"), "first-unconstrained");
}

#[test]
fn the_bookmark_cookie_is_short_lived_and_hidden_from_scripts() {
    let cookie = bookmark_cookie("DB", "0000001-abc").unwrap();
    assert_eq!(cookie.to_str().unwrap(), "ocre_d1_db=0000001-abc; HttpOnly; SameSite=Lax; Secure; Path=/; Max-Age=300");
    assert!(bookmark_cookie("DB", "a\nb").unwrap().to_str().unwrap().starts_with("ocre_d1_db=a%0Ab;"));
}
