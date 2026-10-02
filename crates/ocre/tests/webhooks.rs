use axum::http::HeaderMap;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};

use super::*;

/// RFC 4231 test case 2: HMAC-SHA256("Jefe", "what do ya want for nothing?").
const JEFE: &str = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";

fn bytes(hex: &str) -> Vec<u8> {
    decode_hex(hex).unwrap()
}

#[test]
fn signatures_are_hmac_sha256_in_hex_or_base64() {
    let message = b"what do ya want for nothing?";
    assert_eq!(sign(b"Jefe", message), JEFE);
    for signature in [
        JEFE.to_owned(),
        JEFE.to_uppercase(),
        format!("sha256={JEFE}"),
        format!(" {JEFE}\n"),
        STANDARD.encode(bytes(JEFE)),
        URL_SAFE_NO_PAD.encode(bytes(JEFE)),
    ] {
        assert!(verify(b"Jefe", message, &signature).is_ok(), "{signature}");
    }
    for signature in ["", "abc", "zz", &JEFE[..62], &sign(b"Jefe", b"other body"), "not base64!"] {
        assert!(matches!(verify(b"Jefe", message, signature), Err(Error::Unauthorized)), "{signature:?}");
    }
}

fn standard(id: &str, timestamp: &str, signatures: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("webhook-id", id.parse().unwrap());
    headers.insert("webhook-timestamp", timestamp.parse().unwrap());
    headers.insert("webhook-signature", signatures.parse().unwrap());
    headers
}

#[test]
fn standard_webhooks_check_the_signature_and_the_timestamp() {
    let key = b"key";
    let secret = format!("whsec_{}", STANDARD.encode(key));
    let good = STANDARD.encode(bytes(&sign(key, b"msg_1.1000.{\"a\":1}")));
    assert_eq!(sign_standard(&secret, "msg_1", 1000, b"{\"a\":1}").unwrap(), format!("v1,{good}"));
    let body = b"{\"a\":1}";
    // Rotated secrets: any listed v1 signature may match; other versions are ignored.
    let headers = standard("msg_1", "1000", &format!("v1,bad v2,{good} v1,{good}"));
    assert_eq!(verify_standard(&secret, &headers, body, 300, 1200).unwrap(), "msg_1");
    assert_eq!(verify_standard(&secret, &headers, body, 300, 800).unwrap(), "msg_1", "clock skew both ways");
    let unauthorized = |headers: &HeaderMap, now| {
        matches!(verify_standard(&secret, headers, body, 300, now), Err(Error::Unauthorized))
    };
    assert!(unauthorized(&headers, 1301), "replayed too late");
    assert!(unauthorized(&standard("msg_1", "1000", &format!("v2,{good}")), 1000), "no v1 signature");
    assert!(unauthorized(&standard("msg_2", "1000", &format!("v1,{good}")), 1000), "the id is signed");
    assert!(unauthorized(&standard("msg_1", "soon", &format!("v1,{good}")), 1000));
    let mut missing = headers.clone();
    missing.remove("webhook-signature");
    assert!(unauthorized(&missing, 1000));
    assert!(sign_standard("whsec_!", "msg_1", 1000, body).is_err());
    let err = verify_standard("plain", &headers, body, 300, 1000).unwrap_err();
    assert!(err.to_string().contains("`whsec_` followed by base64"), "{err}");
}
