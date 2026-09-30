use axum::{http::HeaderMap, response::IntoResponse};

use super::*;

const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn key() -> std::result::Result<Keys, String> {
    key_from_secret(Some(SECRET.to_owned()))
}

#[test]
fn previous_secrets_decrypt_and_are_rotated_out() {
    let old = SECRET.replace('0', "1");
    let old_keys = key_from_secret(Some(old.clone()));
    let session = Session::from_headers(&HeaderMap::new(), old_keys, true);
    session.insert("user_id", 9).unwrap();
    let old_cookie = cookie_pair(&session.set_cookie().unwrap().unwrap());

    let rotated = keys_from_secrets(Some(SECRET.to_owned()), Some(format!(" {}, ,{old}", SECRET.replace('0', "2"))));
    let session = Session::from_headers(&headers(&old_cookie), rotated.clone(), true);
    assert_eq!(session.get::<i64>("user_id").unwrap(), Some(9), "read with a previous key");
    let new_cookie = cookie_pair(&session.set_cookie().unwrap().expect("re-encrypted with the current key"));
    let session = Session::from_headers(&headers(&new_cookie), key(), true);
    assert_eq!(session.get::<i64>("user_id").unwrap(), Some(9), "readable with the current key alone");

    let session = Session::from_headers(&headers(&new_cookie), rotated, true);
    assert_eq!(session.get::<i64>("user_id").unwrap(), Some(9));
    assert_eq!(session.set_cookie().unwrap(), None, "current key: nothing to rewrite");

    let Err(message) = keys_from_secrets(Some(SECRET.to_owned()), Some("short".into())) else { panic!("short") };
    assert!(message.starts_with("SECRET_KEY_BASE_PREVIOUS has a value shorter than 64"), "{message}");
    assert!(keys_from_secrets(None, None).is_err());
}

#[test]
fn expired_sessions_start_empty() {
    let set = round_trip("", |s| {
        s.insert("user_id", 1).unwrap();
        s.expire_in(3600).unwrap();
    })
    .unwrap();
    assert!(!set.to_str().unwrap().contains("Max-Age"), "expire_in keeps a browser-session cookie");
    let session = Session::from_headers(&headers(&cookie_pair(&set)), key(), true);
    assert_eq!(session.get::<i64>("user_id").unwrap(), Some(1));
    let expires_at = session.expires_at().unwrap().unwrap();
    assert!((expires_at - crate::now() - 3600).abs() <= 1);

    let set = round_trip("", |s| {
        s.insert("user_id", 1).unwrap();
        s.flash("notice", "hi").unwrap();
        s.expire_in(-1).unwrap();
    })
    .unwrap();
    let session = Session::from_headers(&headers(&cookie_pair(&set)), key(), true);
    assert_eq!(session.get::<i64>("user_id").unwrap(), None, "expired");
    assert!(session.flashes().unwrap().is_empty(), "flash expired with it");
    assert_eq!(session.expires_at().unwrap(), None);
    let removal = session.set_cookie().unwrap().unwrap();
    assert!(removal.to_str().unwrap().contains("Max-Age=0"), "the expired cookie is deleted");
}

#[test]
fn remember_for_sets_a_persistent_cookie() {
    let set = round_trip("", |s| {
        s.insert("user_id", 1).unwrap();
        s.remember_for(30 * 86_400).unwrap();
    })
    .unwrap();
    let text = set.to_str().unwrap().to_owned();
    let max_age: i64 = text.split("Max-Age=").nth(1).unwrap().split(';').next().unwrap().parse().unwrap();
    assert!((max_age - 30 * 86_400).abs() <= 1, "{text}");

    // A later change keeps the same end time; `expire_in` makes it a browser-session cookie again.
    let next = round_trip(&cookie_pair(&set), |s| s.insert("theme", "dark").unwrap()).unwrap();
    assert!(next.to_str().unwrap().contains("Max-Age="));
    let next = round_trip(&cookie_pair(&next), |s| s.expire_in(60).unwrap()).unwrap();
    assert!(!next.to_str().unwrap().contains("Max-Age"));
    let cleared = round_trip(&cookie_pair(&set), |s| s.clear().unwrap()).unwrap();
    assert!(cleared.to_str().unwrap().contains("Max-Age=0"), "sign-out deletes it");
}

fn headers(cookie: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
    headers
}

/// The `name=value` part of a Set-Cookie header, as the browser sends it back.
fn cookie_pair(set_cookie: &HeaderValue) -> String {
    set_cookie.to_str().unwrap().split(';').next().unwrap().to_owned()
}

/// Runs one "request": the closure uses the session, the result is the next cookie.
fn round_trip(cookie: &str, f: impl FnOnce(&Session)) -> Option<HeaderValue> {
    let session = Session::from_headers(&headers(cookie), key(), true);
    f(&session);
    session.set_cookie().unwrap()
}

#[test]
fn secrets_must_be_long_enough() {
    assert!(
        key_from_secret(None).unwrap_err().starts_with("the SECRET_KEY_BASE secret is not set. Fix: run `ocre secret`")
    );
    assert!(key_from_secret(Some("short".into())).unwrap_err().starts_with("SECRET_KEY_BASE is shorter than 64"));
    assert!(key().is_ok());
}

#[test]
fn values_survive_a_round_trip_encrypted() {
    let set = round_trip("", |s| s.insert("user_id", 42).unwrap()).unwrap();
    let text = set.to_str().unwrap();
    assert!(text.starts_with("_ocre_session="), "{text}");
    assert!(!text.contains("user_id"), "encrypted: {text}");
    for attribute in ["HttpOnly", "SameSite=Lax", "Secure", "Path=/"] {
        assert!(text.contains(attribute), "{attribute} in {text}");
    }

    let session = Session::from_headers(&headers(&format!("theme=dark; {}", cookie_pair(&set))), key(), false);
    assert_eq!(session.get::<i64>("user_id").unwrap(), Some(42));
    assert_eq!(session.get::<String>("user_id").unwrap(), None, "wrong type");
    assert_eq!(session.get::<i64>("missing").unwrap(), None);
    assert_eq!(session.set_cookie().unwrap(), None, "reading does not rewrite the cookie");
    assert!(session.remove("user_id").unwrap());
    assert!(!session.remove("user_id").unwrap());
    let removal = session.set_cookie().unwrap().unwrap();
    let removal = removal.to_str().unwrap();
    assert!(removal.starts_with("_ocre_session=;") && removal.contains("Max-Age=0"), "{removal}");
    assert!(!removal.contains("Secure"), "plain HTTP");
}

#[test]
fn tampered_or_foreign_cookies_start_empty() {
    let set = round_trip("", |s| s.insert("admin", true).unwrap()).unwrap();
    let mut pair = cookie_pair(&set);
    // Always a different last character (the value is random, so it may already end in `A`).
    let last = pair.pop().unwrap();
    pair.push(if last == 'A' { 'B' } else { 'A' });
    let session = Session::from_headers(&headers(&pair), key(), true);
    assert_eq!(session.get::<bool>("admin").unwrap(), None);

    let other = key_from_secret(Some(SECRET.replace('0', "1")));
    let session = Session::from_headers(&headers(&cookie_pair(&set)), other, true);
    assert_eq!(session.get::<bool>("admin").unwrap(), None, "another key cannot read it");
}

#[test]
fn flash_shows_once_on_the_next_request() {
    let set = round_trip("", |s| {
        s.insert("user_id", 1).unwrap();
        s.flash("notice", "Saved.").unwrap();
        s.flash("alert", "Careful.").unwrap();
        assert!(s.flashes().unwrap().is_empty(), "not on the request that sets it");
    })
    .unwrap();

    let session = Session::from_headers(&headers(&cookie_pair(&set)), key(), true);
    let flash = session.flashes().unwrap();
    assert_eq!((flash.notice(), flash.alert(), flash.get("info")), (Some("Saved."), Some("Careful."), None));
    assert_eq!(flash.iter().count(), 2);
    let next = session.set_cookie().unwrap().expect("flash consumed, cookie rewritten");

    let session = Session::from_headers(&headers(&cookie_pair(&next)), key(), true);
    assert!(session.flashes().unwrap().is_empty());
    assert_eq!(session.get::<i64>("user_id").unwrap(), Some(1), "the rest stays");
}

#[test]
fn clear_keeps_pending_flash() {
    let set = round_trip("", |s| {
        s.insert("user_id", 1).unwrap();
        s.flash("notice", "Signed out.").unwrap();
        s.clear().unwrap();
    })
    .unwrap();
    let session = Session::from_headers(&headers(&cookie_pair(&set)), key(), true);
    assert_eq!(session.get::<i64>("user_id").unwrap(), None);
    assert_eq!(session.flashes().unwrap().notice(), Some("Signed out."));
}

#[test]
fn a_missing_secret_only_fails_when_the_session_is_used() {
    let session = Session::from_headers(&HeaderMap::new(), key_from_secret(None), true);
    assert_eq!(session.get::<i64>("x").unwrap(), None, "no cookie: nothing to decrypt");
    assert!(matches!(session.insert("x", 1), Err(Error::Internal(message)) if message.contains("SECRET_KEY_BASE")));
    assert_eq!(session.set_cookie().unwrap(), None);

    let session = Session::from_headers(&headers("_ocre_session=abc"), key_from_secret(None), true);
    assert!(session.get::<i64>("x").is_err());
}

#[test]
fn oversized_sessions_and_unserializable_values_are_errors() {
    let session = Session::from_headers(&HeaderMap::new(), key(), true);
    session.insert("big", "x".repeat(5000)).unwrap();
    assert!(matches!(session.set_cookie(), Err(Error::Internal(message)) if message.contains("store ids")));

    let map: std::collections::HashMap<(i32, i32), i32> = [((1, 2), 3)].into();
    assert!(session.insert("map", map).is_err(), "JSON keys must be strings");
}

#[test]
fn extractors_need_the_middleware() {
    use axum::extract::FromRequestParts;
    let (mut parts, ()) = axum::http::Request::new(()).into_parts();
    let err = crate::support::block_on(Session::from_request_parts(&mut parts, &())).err().unwrap();
    assert_eq!(err.into_response().status(), 500);

    parts.extensions.insert(Session::from_headers(&HeaderMap::new(), key(), true));
    assert!(crate::support::block_on(Flash::from_request_parts(&mut parts, &())).unwrap().is_empty());
}

#[test]
fn percent_encoded_cookies_decode() {
    // Random nonces make the base64 value contain `/`, `+` or `=` in most of
    // these runs, which Set-Cookie percent-encodes.
    for n in 0..50 {
        let set = round_trip("", |s| s.insert("n", n).unwrap()).unwrap();
        let session = Session::from_headers(&headers(&cookie_pair(&set)), key(), true);
        assert_eq!(session.get::<i32>("n").unwrap(), Some(n), "{set:?}");
    }
}
