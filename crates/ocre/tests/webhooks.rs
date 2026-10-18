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

#[test]
fn private_addresses_cover_every_internal_range() {
    let private = [
        "10.1.2.3",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.1",
        "100.64.0.1",
        "100.127.0.1",
        "0.0.0.0",
        "0.1.2.3",
        "255.255.255.255",
        "224.0.0.1",
        "240.0.0.1",
        "198.18.0.1",
        "192.0.0.8",
        "192.0.2.1",
        "::1",
        "::",
        "fc00::1",
        "fdff::1",
        "fe80::1",
        "ff02::1",
        "::ffff:10.0.0.1",
        "2001:db8::1",
        "64:ff9b::a00:1",
    ];
    for ip in private {
        assert!(private_ip(ip.parse().unwrap()), "{ip}");
    }
    let public = [
        "8.8.8.8",
        "172.32.0.1",
        "172.15.0.1",
        "100.128.0.1",
        "1.1.1.1",
        "192.169.0.1",
        "198.20.0.1",
        "2606:4700::1111",
        "::ffff:1.1.1.1",
    ];
    for ip in public {
        assert!(!private_ip(ip.parse().unwrap()), "{ip}");
    }
}

#[test]
fn webhook_urls_need_an_http_scheme_and_a_host_without_credentials() {
    assert_eq!(url_host("https://Hooks.Example.com./x").unwrap(), UrlHost::Name("hooks.example.com".into()));
    assert_eq!(url_host(" http://10.0.0.5:8080/ ").unwrap(), UrlHost::Ip("10.0.0.5".parse().unwrap()));
    assert_eq!(url_host("http://[::1]/").unwrap(), UrlHost::Ip("::1".parse().unwrap()));
    assert_eq!(url_host("http://api.localhost/").unwrap(), UrlHost::Ip("127.0.0.1".parse().unwrap()));
    for (url, reason) in [
        ("ftp://example.com", "must start with https:// or http://"),
        ("https://user:pw@example.com", "must not contain a user name or password"),
        ("not a url", "is not a valid URL"),
        ("file:///etc/passwd", "must start with https:// or http://"),
    ] {
        assert_eq!(url_host(url).unwrap_err().to_string(), format!("bad request: The webhook URL {reason}"), "{url}");
    }
}

#[test]
fn dns_answers_give_a_and_aaaa_records() {
    let answer = serde_json::json!({"Status": 0, "Answer": [
        {"name": "x.example.", "type": 5, "data": "y.example."},
        {"name": "y.example.", "type": 1, "data": "93.184.216.34"},
        {"name": "y.example.", "type": 28, "data": "2606:4700::1111"},
        {"name": "y.example.", "type": 1, "data": "not an ip"}
    ]});
    let addresses = dns_addresses(&answer);
    assert_eq!(addresses, ["93.184.216.34".parse::<IpAddr>().unwrap(), "2606:4700::1111".parse().unwrap()]);
    assert!(dns_addresses(&serde_json::json!({"Status": 3})).is_empty());
    assert!(refused_addresses("y.example", &addresses).is_none());
    let unknown = refused_addresses("nx.example", &[]).unwrap().to_string();
    assert_eq!(unknown, "bad request: The webhook URL's host nx.example was not found");
    let private = refused_addresses("in.example", &[addresses[0], "10.0.0.1".parse().unwrap()]).unwrap().to_string();
    assert_eq!(private, "bad request: The webhook URL's host in.example is a private address");
}

#[test]
fn deliveries_are_signed_standard_webhooks_without_redirects() {
    let secret = new_secret();
    let request =
        delivery_request(" https://hooks.example.com/in ", &secret, "evt_1", b"{\"a\":1}".to_vec(), 1_700_000_000)
            .unwrap();
    assert_eq!(request.url(), "https://hooks.example.com/in");
    assert!(!request.follows_redirects());
    assert_eq!(request.timeout_duration(), DELIVERY_TIMEOUT);
    let mut headers = HeaderMap::new();
    for (name, value) in request.headers() {
        headers.insert(axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(), value.parse().unwrap());
    }
    assert_eq!(headers["content-type"], "application/json");
    let id = verify_standard(&secret, &headers, request.body_bytes().unwrap(), 300, 1_700_000_010).unwrap();
    assert_eq!(id, "evt_1");
    assert!(delivery_request("https://x", "not-whsec", "evt_1", Vec::new(), 0).is_err());
    assert_ne!(new_secret(), secret, "secrets are random");
}
