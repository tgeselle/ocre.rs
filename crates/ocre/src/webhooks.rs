//! Webhooks: signatures for calls in both directions, and processing each
//! event once.
//!
//! A payment provider, a GPU service or a mail service calls the app back
//! with a signed POST. The handler checks the signature over the raw body,
//! then runs the effect inside [`once`], which records the event in the
//! `webhook_events` table (`ocre g webhook` creates it) and skips a
//! delivery it has already processed: providers retry until they get a 2xx,
//! so the same event often arrives twice.
//!
//! ```no_run
//! use axum::{body::Bytes, extract::State, http::HeaderMap};
//! use ocre::{Ctx, Result, webhooks};
//!
//! # #[allow(dead_code)]
//!
//! async fn receive(State(ctx): State<Ctx>, headers: HeaderMap, body: Bytes) -> Result<&'static str> {
//!     let secret = ctx.secret("PAYMENTS_WEBHOOK_SECRET").await?;
//!     let signature = headers.get("x-signature").and_then(|v| v.to_str().ok()).unwrap_or_default();
//!     webhooks::verify(secret.as_bytes(), &body, signature)?;
//!     let event: serde_json::Value =
//!         serde_json::from_slice(&body).map_err(|_| ocre::Error::bad_request("invalid JSON"))?;
//!     let id = event["id"].as_str().unwrap_or_default().to_owned();
//!     let db = ctx.db()?;
//!     webhooks::once(&db, "payments", &id, &body, || async {
//!         // ... mark the order paid: runs once per event id
//!         Ok(())
//!     })
//!     .await?;
//!     Ok("ok")
//! }
//! ```
//!
//! [`sign`] is the other direction: the app signs what it sends (a job
//! submitted to an external service), and the service checks it the same way.
//!
//! # Sending webhooks to your users
//!
//! When users register their own endpoint URL (an API's "notify me" setting),
//! [`deliver`] POSTs each event as a [Standard Webhooks](https://www.standardwebhooks.com)
//! delivery: JSON body, `webhook-id`, `webhook-timestamp` and
//! `webhook-signature` headers signed with the endpoint's secret
//! ([`new_secret`] makes one), a 10-second timeout and no redirects. In
//! release builds it first refuses URLs that are not `http(s)` or whose host
//! is or resolves to a private, loopback or link-local address
//! ([`check_public_url`], also for validating the URL when it is saved), so
//! a user cannot make the app call its own network (SSRF). Retries belong
//! to a job: run `deliver` from one and let the queue retry it.
//!
//! ```no_run
//! use ocre::{Ctx, Error, Result, webhooks};
//! use serde_json::json;
//!
//! // src/jobs/deliver_webhook.rs: the queue retries it on an error.
//! async fn perform(ctx: &Ctx, endpoint_url: &str, secret: &str, event_id: &str) -> Result<()> {
//!     let event = json!({"type": "video.completed", "data": {"video_id": 7}});
//!     let response = webhooks::deliver(endpoint_url, secret, event_id, &event).await?;
//!     if !response.is_success() {
//!         return Err(Error::internal(format!("{endpoint_url} answered {}", response.status)));
//!     }
//!     Ok(())
//! }
//! ```

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{Error, Result, http};

pub use crate::runtime::webhooks::{Answer, Delivery, STALE_AFTER, check_public_url, deliver, once, post_signed};

/// How long [`deliver`] waits for the endpoint's answer: 10 seconds.
pub const DELIVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A new Standard Webhooks secret for an endpoint: `whsec_` and 32 random bytes in base64.
///
/// Show it to the user once, store it (encrypted, see
/// [`encryption`](crate::encryption)), and pass it to [`deliver`].
///
/// # Examples
///
/// ```
/// let secret = ocre::webhooks::new_secret();
/// assert!(secret.starts_with("whsec_") && secret.len() == 6 + 44);
/// assert!(ocre::webhooks::sign_standard(&secret, "msg_1", 0, b"{}").is_ok());
/// ```
pub fn new_secret() -> String {
    format!("whsec_{}", base64::engine::general_purpose::STANDARD.encode(crate::token::random_bytes::<32>()))
}

/// Whether `ip` is an address a webhook must never reach: private, loopback, link-local, shared (CGNAT), unspecified, multicast, broadcast or reserved.
///
/// IPv4-mapped IPv6 addresses (`::ffff:10.0.0.1`) are checked as IPv4.
///
/// # Examples
///
/// ```
/// use ocre::webhooks::private_ip;
///
/// assert!(private_ip("169.254.169.254".parse().unwrap()), "cloud metadata");
/// assert!(private_ip("::ffff:127.0.0.1".parse().unwrap()));
/// assert!(private_ip("fd00::1".parse().unwrap()));
/// assert!(!private_ip("93.184.216.34".parse().unwrap()));
/// assert!(!private_ip("2606:4700::1111".parse().unwrap()));
/// ```
pub fn private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => private_ipv4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(ip) => private_ipv4(ip),
            None => private_ipv6(ip),
        },
    }
}

fn private_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || a >= 240
        || (a == 100 && (64..=127).contains(&b))
        || (a == 198 && (18..=19).contains(&b))
        || (a == 192 && b == 0 && ip.octets()[2] == 0)
}

fn private_ipv6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00 // unique local fc00::/7
        || (first & 0xffc0) == 0xfe80 // link-local fe80::/10
        || (first == 0x2001 && ip.segments()[1] == 0x0db8) // documentation
        || (first == 0x0064 && ip.segments()[1] == 0xff9b) // NAT64 to IPv4
}

/// The host of a webhook URL: an IP address to check as is, or a name to resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UrlHost {
    Ip(IpAddr),
    Name(String),
}

/// The host of `url`, or a 400 naming the problem: not `http(s)`, credentials in the URL.
pub(crate) fn url_host(url: &str) -> Result<UrlHost> {
    let refuse = |reason: &str| Error::bad_request(format!("The webhook URL {reason}"));
    let parsed = worker::Url::parse(url.trim()).map_err(|_| refuse("is not a valid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(refuse("must start with https:// or http://"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(refuse("must not contain a user name or password"));
    }
    match parsed.host().expect("the URL parser refuses http(s) URLs without a host") {
        worker::url::Host::Ipv4(ip) => Ok(UrlHost::Ip(IpAddr::V4(ip))),
        worker::url::Host::Ipv6(ip) => Ok(UrlHost::Ip(IpAddr::V6(ip))),
        worker::url::Host::Domain(name) if name.eq_ignore_ascii_case("localhost") || name.ends_with(".localhost") => {
            Ok(UrlHost::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)))
        }
        worker::url::Host::Domain(name) => Ok(UrlHost::Name(name.trim_end_matches('.').to_ascii_lowercase())),
    }
}

/// The `A` (type 1) and `AAAA` (type 28) addresses of a DNS-over-HTTPS JSON answer (RFC 8484's JSON form, as Cloudflare's resolver answers).
pub(crate) fn dns_addresses(answer: &serde_json::Value) -> Vec<IpAddr> {
    answer["Answer"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|record| matches!(record["type"].as_i64(), Some(1 | 28)))
        .filter_map(|record| record["data"].as_str()?.parse().ok())
        .collect()
}

/// Why the resolved `addresses` of `host` make it unfit for a webhook, if they do.
pub(crate) fn refused_addresses(host: &str, addresses: &[IpAddr]) -> Option<Error> {
    if addresses.is_empty() {
        return Some(Error::bad_request(format!("The webhook URL's host {host} was not found")));
    }
    addresses
        .iter()
        .any(|ip| private_ip(*ip))
        .then(|| Error::bad_request(format!("The webhook URL's host {host} is a private address")))
}

/// The signed POST of a Standard Webhooks delivery of `body` to `url`.
pub(crate) fn delivery_request(url: &str, secret: &str, id: &str, body: Vec<u8>, now: i64) -> Result<http::Request> {
    let signature = sign_standard(secret, id, now, &body)?;
    Ok(http::Request::post(url.trim())
        .header("user-agent", "Ocre-Webhooks/1")
        .header("webhook-id", id)
        .header("webhook-timestamp", &now.to_string())
        .header("webhook-signature", &signature)
        .body(body, "application/json")
        .timeout(DELIVERY_TIMEOUT)
        .follow_redirects(false))
}

/// The `webhook_events` table [`once`] records deliveries in, created by
/// the migration of `ocre g webhook`.
pub const TABLE_SQL: &str = "CREATE TABLE webhook_events (
  id INTEGER PRIMARY KEY,
  source TEXT NOT NULL,
  event_id TEXT NOT NULL,
  payload TEXT NOT NULL,
  status TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 1,
  error TEXT,
  received_at INTEGER NOT NULL,
  processed_at INTEGER,
  UNIQUE (source, event_id)
);
";

/// HMAC-SHA256 of `message` with `secret`, as lowercase hex: the signature
/// to send in a header (`X-Signature`) of an outgoing call.
///
/// # Examples
///
/// ```
/// let signature = ocre::webhooks::sign(b"secret", b"{\"id\":1}");
/// assert_eq!(signature.len(), 64);
/// assert!(ocre::webhooks::verify(b"secret", b"{\"id\":1}", &signature).is_ok());
/// ```
pub fn sign(secret: &[u8], message: &[u8]) -> String {
    let mut mac = mac(secret);
    mac.update(message);
    let bytes = mac.finalize().into_bytes();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Checks that `signature` is the HMAC-SHA256 of `message` with `secret`,
/// in constant time. The signature may be hex (any case, optionally
/// prefixed `sha256=` as GitHub sends it) or base64 (standard or URL-safe,
/// padded or not), the common encodings of webhook providers. Sign the raw
/// request body: parsed and re-serialized JSON may differ by a space.
///
/// # Errors
///
/// [`Error::Unauthorized`] when the signature is missing, malformed or
/// does not match.
///
/// # Examples
///
/// ```
/// use base64::Engine as _;
/// use ocre::webhooks::{sign, verify};
///
/// let hex = sign(b"secret", b"body");
/// assert!(verify(b"secret", b"body", &format!("sha256={hex}")).is_ok());
/// assert!(verify(b"other", b"body", &hex).is_err());
/// ```
pub fn verify(secret: &[u8], message: &[u8], signature: &str) -> Result<()> {
    let signature = signature.trim();
    let signature = signature.strip_prefix("sha256=").unwrap_or(signature);
    let expected = decode_hex(signature).or_else(|| decode_base64(signature)).ok_or(Error::Unauthorized)?;
    let mut mac = mac(secret);
    mac.update(message);
    mac.verify_slice(&expected).map_err(|_| Error::Unauthorized)
}

/// Checks a [Standard Webhooks](https://www.standardwebhooks.com) delivery
/// (Svix, Resend, and other providers): `webhook-signature` holds one or
/// more space-separated `v1,<base64>` signatures of `{id}.{timestamp}.{body}`
/// keyed with the base64 part of the `whsec_...` secret, and the
/// `webhook-timestamp` must be within `tolerance` seconds of `now` (replays
/// of an old delivery are refused). Returns the `webhook-id`, the event id
/// to give [`once`].
///
/// # Errors
///
/// [`Error::Unauthorized`] when a header is missing, the timestamp is out
/// of tolerance, or no signature matches; [`Error::Internal`] when the
/// secret is not `whsec_` followed by base64.
///
/// # Examples
///
/// ```
/// use axum::http::HeaderMap;
/// use base64::Engine as _;
/// use ocre::webhooks::{sign, verify_standard};
///
/// let key = b"0123456789abcdef";
/// let secret = format!("whsec_{}", base64::engine::general_purpose::STANDARD.encode(key));
/// let signature = sign(key, b"msg_1.1700000000.{}");
/// let bytes: Vec<u8> = (0..32).map(|i| u8::from_str_radix(&signature[i * 2..i * 2 + 2], 16).unwrap()).collect();
/// let mut headers = HeaderMap::new();
/// headers.insert("webhook-id", "msg_1".parse().unwrap());
/// headers.insert("webhook-timestamp", "1700000000".parse().unwrap());
/// let value = format!("v1,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
/// headers.insert("webhook-signature", value.parse().unwrap());
/// assert_eq!(verify_standard(&secret, &headers, b"{}", 300, 1_700_000_100).unwrap(), "msg_1");
/// assert!(verify_standard(&secret, &headers, b"{}", 300, 1_700_001_000).is_err(), "too old");
/// ```
pub fn verify_standard(
    secret: &str,
    headers: &axum::http::HeaderMap,
    body: &[u8],
    tolerance: i64,
    now: i64,
) -> Result<String> {
    let key = standard_key(secret)?;
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).ok_or(Error::Unauthorized);
    let id = header("webhook-id")?;
    let timestamp = header("webhook-timestamp")?;
    let sent_at: i64 = timestamp.parse().map_err(|_| Error::Unauthorized)?;
    if (now - sent_at).abs() > tolerance {
        return Err(Error::Unauthorized);
    }
    let message = [id.as_bytes(), b".", timestamp.as_bytes(), b".", body].concat();
    let signed = header("webhook-signature")?
        .split_whitespace()
        .filter_map(|entry| entry.strip_prefix("v1,"))
        .any(|signature| verify(&key, &message, signature).is_ok());
    if signed { Ok(id.to_owned()) } else { Err(Error::Unauthorized) }
}

fn mac(secret: &[u8]) -> Hmac<Sha256> {
    Hmac::<Sha256>::new_from_slice(secret).expect("HMAC takes keys of any length")
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

/// The `webhook-signature` header value (`v1,<base64>`) of a [Standard
/// Webhooks](https://www.standardwebhooks.com) delivery of `body` with this
/// `id` and `timestamp` (Unix seconds): what [`verify_standard`] checks.
/// For an app that sends Standard Webhooks, and for tests of one that
/// receives them.
///
/// # Errors
///
/// [`Error::Internal`] when the secret is not `whsec_` followed by base64.
///
/// # Examples
///
/// ```
/// use axum::http::HeaderMap;
/// use ocre::webhooks::{sign_standard, verify_standard};
///
/// let secret = "whsec_c2VjcmV0";
/// let mut headers = HeaderMap::new();
/// headers.insert("webhook-id", "msg_1".parse().unwrap());
/// headers.insert("webhook-timestamp", "1700000000".parse().unwrap());
/// let signature = sign_standard(secret, "msg_1", 1_700_000_000, b"{}").unwrap();
/// headers.insert("webhook-signature", signature.parse().unwrap());
/// assert!(verify_standard(secret, &headers, b"{}", 300, 1_700_000_000).is_ok());
/// ```
pub fn sign_standard(secret: &str, id: &str, timestamp: i64, body: &[u8]) -> Result<String> {
    let key = standard_key(secret)?;
    let mut mac = mac(&key);
    mac.update(&[id.as_bytes(), b".", timestamp.to_string().as_bytes(), b".", body].concat());
    Ok(format!("v1,{}", base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())))
}

/// The HMAC key of a `whsec_<base64>` secret.
fn standard_key(secret: &str) -> Result<Vec<u8>> {
    secret
        .strip_prefix("whsec_")
        .and_then(decode_base64)
        .ok_or_else(|| Error::internal("webhooks: a Standard Webhooks secret is `whsec_` followed by base64"))
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD].iter().find_map(|engine| engine.decode(text).ok())
}

#[cfg(test)]
#[path = "../tests/webhooks.rs"]
mod tests;
