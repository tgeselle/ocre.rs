use super::*;

const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn key() -> Key {
    Key::from_secret_key_base(SECRET)
}

fn claims() -> Claims {
    Claims { sub: "42".into(), iat: 1_000, exp: 2_000 }
}

fn b64(text: &str) -> String {
    URL_SAFE_NO_PAD.encode(text)
}

/// A token with any header and payload, signed with `key()`.
fn signed(header: &str, payload: &str) -> String {
    let input = format!("{}.{}", b64(header), b64(payload));
    let mut mac = key().mac();
    mac.update(input.as_bytes());
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

#[test]
fn round_trips_until_expiry() {
    let token = encode_with(&key(), &claims());
    assert_eq!(token.split('.').count(), 3);
    assert_eq!(decode_with(&key(), &token, 1_999).unwrap(), claims());
    assert!(matches!(decode_with(&key(), &token, 2_000), Err(Error::Unauthorized)), "expired at exp");
}

#[test]
fn matches_the_standard_hs256_encoding() {
    // Same bytes as any JWT library: header, payload and HMAC-SHA256 over them.
    let token = encode_with(&key(), &claims());
    assert_eq!(token, signed(HEADER, r#"{"sub":"42","iat":1000,"exp":2000}"#));
    assert!(token.starts_with("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9."), "{token}");
}

#[test]
fn rejects_other_keys_and_tampering() {
    let token = encode_with(&key(), &claims());
    let other = Key::from_secret_key_base(&SECRET.replace('0', "1"));
    assert!(decode_with(&other, &token, 1_500).is_err(), "other key");
    let (input, _) = token.rsplit_once('.').unwrap();
    let forged_payload =
        format!("{}.{}", input.split('.').next().unwrap(), b64(r#"{"sub":"1","iat":1000,"exp":2000}"#));
    let signature = token.rsplit('.').next().unwrap();
    assert!(decode_with(&key(), &format!("{forged_payload}.{signature}"), 1_500).is_err(), "changed payload");
    assert!(decode_with(&key(), &format!("{input}.AAAA"), 1_500).is_err(), "wrong signature");
    assert!(decode_with(&key(), &format!("{input}.!!"), 1_500).is_err(), "signature is not base64");
}

#[test]
fn rejects_other_algorithms_including_none() {
    let payload = r#"{"sub":"42","iat":1000,"exp":2000}"#;
    for header in [r#"{"alg":"none"}"#, r#"{"alg":"HS512"}"#, r#"{"alg":"RS256"}"#, r#"{"typ":"JWT"}"#] {
        assert!(decode_with(&key(), &signed(header, payload), 1_500).is_err(), "{header}");
    }
    let unsigned = format!("{}.{}.", b64(r#"{"alg":"none"}"#), b64(payload));
    assert!(decode_with(&key(), &unsigned, 1_500).is_err());
    assert_eq!(decode_with(&key(), &signed(r#"{"alg":"HS256"}"#, payload), 1_500).unwrap().sub, "42");
}

#[test]
fn rejects_malformed_tokens() {
    for token in ["", "abc", "a.b", "!!.e30.sig", &format!("{}.!!.sig", b64(HEADER))] {
        assert!(matches!(decode_with(&key(), token, 0), Err(Error::Unauthorized)), "{token}");
    }
    let missing_claims = signed(HEADER, r#"{"sub":"42"}"#);
    assert!(decode_with(&key(), &missing_claims, 0).is_err());
    let not_json = signed("not json", r#"{"sub":"42","iat":1,"exp":2}"#);
    assert!(decode_with(&key(), &not_json, 0).is_err());
}

#[test]
fn claims_expire_after_the_ttl() {
    let claims = Claims::new("7", 60);
    assert_eq!((claims.sub.as_str(), claims.exp - claims.iat), ("7", 60));
    assert!((claims.iat - crate::now()).abs() <= 1);
}

#[test]
fn the_key_comes_from_secret_key_base() {
    let token = encode_with(&Key::from_secret(Some(SECRET.to_owned())).unwrap(), &claims());
    assert!(decode_with(&key(), &token, 1_500).is_ok());
    let Err(Error::Internal(message)) = Key::from_secret(None) else { panic!("missing secret") };
    assert!(message.starts_with("the SECRET_KEY_BASE secret is not set"), "{message}");
    assert!(matches!(Key::from_secret(Some("short".into())), Err(Error::Internal(_))));
    // Not the session key: a JWT key never decrypts cookies and vice versa.
    assert_ne!(key().0.as_slice(), crate::session::key_from_secret(Some(SECRET.into())).unwrap().current.master());
}

#[test]
fn tokens_are_found_in_order_of_locations() {
    use axum::http::{HeaderMap, HeaderValue, Uri};
    let mut headers = HeaderMap::new();
    headers.insert("cookie", HeaderValue::from_static("a=1; token=from-cookie"));
    headers.insert("authorization", HeaderValue::from_static("Basic abc"));
    let uri: Uri = "/x?page=2&token=from%20query&token=second".parse().unwrap();
    let all = [Location::Bearer, Location::Query("token"), Location::Cookie("token")];
    assert_eq!(token_from(&headers, &uri, &all).as_deref(), Some("from query"), "not a Bearer header");
    assert_eq!(
        token_from(&headers, &uri, &[Location::Cookie("token"), Location::Query("token")]).as_deref(),
        Some("from-cookie")
    );
    headers.insert("authorization", HeaderValue::from_static("Bearer  spaced "));
    assert_eq!(token_from(&headers, &uri, &all).as_deref(), Some("spaced"));
    headers.insert("authorization", HeaderValue::from_static("Bearer "));
    let empty: Uri = "/x?token=".parse().unwrap();
    assert_eq!(
        token_from(&headers, &empty, &[Location::Bearer, Location::Query("token")]),
        None,
        "empty tokens skipped"
    );
    assert_eq!(token_from(&HeaderMap::new(), &"/x".parse().unwrap(), &all), None);
    assert_eq!(token_from(&headers, &"/x?%zz".parse().unwrap(), &[Location::Query("token")]), None);
}
