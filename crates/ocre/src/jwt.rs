//! JSON Web Tokens (HS256) for API clients.
//!
//! For API clients that cannot keep a session cookie (mobile apps, scripts):
//! they send `Authorization: Bearer <token>` instead. [`token_from`] also
//! reads it from a query parameter or a cookie, trying several
//! [`Location`]s in order (Loco's `auth.jwt.location`).
//!
//! The signing key is derived from the `SECRET_KEY_BASE` Worker secret
//! (HMAC-SHA256 of a fixed label), so apps have one secret to create, upload
//! and rotate: rotating it signs everyone out of sessions and tokens at once,
//! unless the old value is kept in `SECRET_KEY_BASE_PREVIOUS` for a while.
//! Tokens carry only the subject (usually the user id), `iat` and `exp`;
//! [`decode`] rejects expired tokens, bad signatures and any `alg` other than
//! [`ALGORITHM`] (including `none`) with [`Error::Unauthorized`] (401). Keep
//! TTLs short (`ocre g auth` issues one-hour tokens) and use
//! [`crate::token`] API keys for long-lived access.
//!
//! Cost: one HMAC-SHA256 over a few hundred bytes; no D1, KV or other
//! binding is used.
//!
//! [`encode`] and [`decode`] read the key from the Worker's environment;
//! [`encode_with`] and [`decode_with`] take a [`Key`] and run anywhere.
//!
//! ```rust,no_run
//! use axum::{extract::State, http::{HeaderMap, Uri}};
//! use ocre::{Ctx, Error, Result, jwt::{self, Claims, Location}};
//!
//! // POST /api/auth/token, after checking the password:
//! async fn token(State(ctx): State<Ctx>) -> Result<String> {
//!     let user_id = 42;
//!     jwt::encode(&ctx, &Claims::new(user_id.to_string(), 3600))
//! }
//!
//! // Later, from `Authorization: Bearer <token>` or, for browsers, a cookie:
//! async fn me(State(ctx): State<Ctx>, headers: HeaderMap, uri: Uri) -> Result<String> {
//!     let locations = [Location::Bearer, Location::Cookie("token")];
//!     let token = jwt::token_from(&headers, &uri, &locations).ok_or(Error::Unauthorized)?;
//!     let claims = jwt::decode(&ctx, &token)?; // 401 when invalid or expired
//!     let user_id: i64 = claims.sub.parse().map_err(|_| Error::Unauthorized)?;
//!     Ok(user_id.to_string())
//! }
//! ```

use axum::http::{HeaderMap, Uri, header};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use cookie::Cookie;
use hmac::{Hmac, Mac as _};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub use crate::runtime::jwt::{decode, encode};
use crate::{Error, Result, session::checked_secret};

/// The only JWT algorithm Ocre signs with and accepts: `HS256` (HMAC-SHA256).
///
/// Tokens whose header names any other `alg`, including `none`, fail
/// [`decode`] and [`decode_with`] with [`Error::Unauthorized`].
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::jwt::ALGORITHM, "HS256");
/// ```
pub const ALGORITHM: &str = "HS256";
/// Label mixed into `SECRET_KEY_BASE` so the JWT key differs from the session key.
const KEY_LABEL: &[u8] = b"ocre/jwt/hs256";
/// `{"alg":"HS256","typ":"JWT"}`, the header of every token Ocre issues.
const HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

/// Where [`token_from`] looks for a token (Loco's `auth.jwt.location`).
///
/// # Examples
///
/// ```
/// use ocre::jwt::Location;
///
/// // Browsers keep the token in a cookie; API clients send a Bearer header.
/// let locations = [Location::Bearer, Location::Cookie("token")];
/// assert_eq!(locations[1], Location::Cookie("token"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// `Authorization: Bearer <token>`, the default for API clients.
    Bearer,
    /// A query parameter, e.g. `Query("token")` for `?token=...`. URLs end up
    /// in logs and `Referer` headers: use it only where a header is
    /// impossible (an `EventSource`, a WebSocket from a browser, a download
    /// link), with short-lived tokens.
    Query(&'static str),
    /// A cookie, e.g. `Cookie("token")`. Set it `HttpOnly` and `SameSite`:
    /// cookies are sent automatically, so the CSRF check applies.
    Cookie(&'static str),
}

/// The token of the first `locations` entry that has a non-empty one.
///
/// Locations are tried in order, so `[Location::Bearer, Location::Cookie("token")]`
/// accepts API clients and browsers (Loco's multiple JWT locations with
/// fallback). It only finds the token: verify it with [`decode`]. Works for
/// API keys ([`crate::token`]) too. No binding call.
///
/// # Examples
///
/// ```
/// use axum::http::{HeaderMap, HeaderValue, Uri};
/// use ocre::jwt::{Location, token_from};
///
/// let mut headers = HeaderMap::new();
/// headers.insert("cookie", HeaderValue::from_static("theme=dark; token=abc"));
/// let uri: Uri = "/events?token=xyz".parse().unwrap();
///
/// assert_eq!(token_from(&headers, &uri, &[Location::Bearer]), None);
/// assert_eq!(token_from(&headers, &uri, &[Location::Bearer, Location::Cookie("token")]).as_deref(), Some("abc"));
/// assert_eq!(token_from(&headers, &uri, &[Location::Query("token")]).as_deref(), Some("xyz"));
/// headers.insert("authorization", HeaderValue::from_static("Bearer 123"));
/// assert_eq!(token_from(&headers, &uri, &[Location::Bearer, Location::Cookie("token")]).as_deref(), Some("123"));
/// ```
pub fn token_from(headers: &HeaderMap, uri: &Uri, locations: &[Location]) -> Option<String> {
    locations.iter().find_map(|location| {
        match *location {
            Location::Bearer => headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .map(|token| token.trim().to_owned()),
            Location::Query(name) => serde_urlencoded::from_str::<Vec<(String, String)>>(uri.query().unwrap_or(""))
                .ok()?
                .into_iter()
                .find_map(|(key, value)| (key == name).then_some(value)),
            Location::Cookie(name) => headers
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .flat_map(Cookie::split_parse_encoded)
                .filter_map(std::result::Result::ok)
                .find(|cookie| cookie.name() == name)
                .map(|cookie| cookie.value().to_owned()),
        }
        .filter(|token| !token.is_empty())
    })
}

/// The payload of a token: who it is for and when it expires.
///
/// Serialized as the JWT's JSON payload `{"sub":...,"iat":...,"exp":...}`.
/// Build it with [`Claims::new`]; [`decode`] returns it once the token is
/// verified.
///
/// # Examples
///
/// ```
/// let claims = ocre::jwt::Claims { sub: "42".to_owned(), iat: 1_767_225_600, exp: 1_767_229_200 };
/// assert_eq!(serde_json::to_string(&claims)?, r#"{"sub":"42","iat":1767225600,"exp":1767229200}"#);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// Subject: the user id, as a string (the JWT standard's type).
    pub sub: String,
    /// Issued at, in Unix seconds.
    pub iat: i64,
    /// Expires at, in Unix seconds; the token is rejected from this second on.
    pub exp: i64,
}

impl Claims {
    /// Claims for `sub`, issued now ([`crate::now`]) and valid for `ttl_seconds`.
    ///
    /// # Examples
    ///
    /// ```
    /// let claims = ocre::jwt::Claims::new("42", 3600);
    /// assert_eq!(claims.sub, "42");
    /// assert_eq!(claims.exp - claims.iat, 3600);
    /// ```
    pub fn new(sub: impl Into<String>, ttl_seconds: i64) -> Self {
        let iat = crate::now();
        Self { sub: sub.into(), iat, exp: iat + ttl_seconds }
    }
}

/// An HS256 signing key, derived from `SECRET_KEY_BASE`.
///
/// Handlers do not build one: [`encode`] and [`decode`] derive it from the
/// Worker secret. Use it with [`encode_with`] and [`decode_with`] in tests,
/// tools and scripts.
///
/// # Examples
///
/// ```
/// let key = ocre::jwt::Key::from_secret_key_base(&"x".repeat(64));
/// let token = ocre::jwt::encode_with(&key, &ocre::jwt::Claims::new("42", 60));
/// assert_eq!(ocre::jwt::decode_with(&key, &token, ocre::now())?.sub, "42");
/// # Ok::<(), ocre::Error>(())
/// ```
pub struct Key([u8; 32]);

impl Key {
    /// Derives the key from a `SECRET_KEY_BASE` value: HMAC-SHA256 of a fixed label.
    ///
    /// The label makes the JWT key differ from the session cookie key derived
    /// from the same secret. Any length is accepted here; the Worker secret
    /// read by [`encode`] and [`decode`] must be 64 characters or more.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::jwt::{Claims, Key, decode_with, encode_with};
    ///
    /// let key = Key::from_secret_key_base(&"x".repeat(64));
    /// let token = encode_with(&key, &Claims::new("42", 60));
    /// assert_eq!(decode_with(&key, &token, ocre::now())?.sub, "42");
    /// // Another secret, another key: the token no longer verifies.
    /// assert!(decode_with(&Key::from_secret_key_base(&"y".repeat(64)), &token, ocre::now()).is_err());
    /// # Ok::<(), ocre::Error>(())
    /// ```
    pub fn from_secret_key_base(secret: &str) -> Self {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes keys of any length");
        mac.update(KEY_LABEL);
        Self(mac.finalize().into_bytes().into())
    }

    /// The key for the Worker's `SECRET_KEY_BASE`, or an internal error
    /// naming the fix when it is missing or too short.
    pub(crate) fn from_secret(secret: Option<String>) -> Result<Self> {
        checked_secret(secret).map(|secret| Self::from_secret_key_base(&secret)).map_err(Error::Internal)
    }

    fn mac(&self) -> Hmac<Sha256> {
        Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC takes keys of any length")
    }
}

/// Signs `claims` with `key` and returns the compact token `header.payload.signature`.
///
/// The header is always `{"alg":"HS256","typ":"JWT"}`; the three parts are
/// URL-safe base64 without padding. Handlers use [`encode`], which takes the
/// key from `SECRET_KEY_BASE`.
///
/// # Examples
///
/// ```
/// use ocre::jwt::{Claims, Key, encode_with};
///
/// let key = Key::from_secret_key_base(&"x".repeat(64));
/// let token = encode_with(&key, &Claims { sub: "42".to_owned(), iat: 0, exp: 60 });
/// assert!(token.starts_with("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9."));
/// assert_eq!(token.split('.').count(), 3);
/// ```
pub fn encode_with(key: &Key, claims: &Claims) -> String {
    let payload = serde_json::to_vec(claims).expect("claims serialize");
    let signing_input = format!("{}.{}", URL_SAFE_NO_PAD.encode(HEADER), URL_SAFE_NO_PAD.encode(payload));
    let mut mac = key.mac();
    mac.update(signing_input.as_bytes());
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

/// Verifies `token` with `key` at Unix time `now` (seconds) and returns its claims.
///
/// Checks, in order: the shape, the header's `alg` ([`ALGORITHM`] only), the
/// signature (constant-time comparison), then the expiry (`exp > now`).
/// Handlers use [`decode`], which takes the key from `SECRET_KEY_BASE` and
/// the current time.
///
/// # Errors
///
/// [`Error::Unauthorized`] (401) for any failure: malformed token, other
/// algorithm, bad signature, expired. The error does not say which check
/// failed.
///
/// # Examples
///
/// ```
/// use ocre::jwt::{Claims, Key, decode_with, encode_with};
///
/// let key = Key::from_secret_key_base(&"x".repeat(64));
/// let token = encode_with(&key, &Claims { sub: "42".to_owned(), iat: 0, exp: 60 });
/// assert_eq!(decode_with(&key, &token, 59)?.sub, "42");
/// assert!(matches!(decode_with(&key, &token, 60), Err(ocre::Error::Unauthorized))); // expired
/// assert!(decode_with(&key, "not.a.token", 0).is_err());
/// # Ok::<(), ocre::Error>(())
/// ```
pub fn decode_with(key: &Key, token: &str, now: i64) -> Result<Claims> {
    verify(key, token, now).ok_or(Error::Unauthorized)
}

fn verify(key: &Key, token: &str, now: i64) -> Option<Claims> {
    #[derive(Deserialize)]
    struct Header {
        alg: String,
    }
    let (signing_input, signature) = token.rsplit_once('.')?;
    let (header, payload) = signing_input.split_once('.')?;
    // The algorithm is fixed: a token cannot pick `none` or another scheme.
    let header: Header = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
    if header.alg != ALGORITHM {
        return None;
    }
    let mut mac = key.mac();
    mac.update(signing_input.as_bytes());
    // `verify_slice` compares in constant time.
    mac.verify_slice(&URL_SAFE_NO_PAD.decode(signature).ok()?).ok()?;
    let claims: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    (claims.exp > now).then_some(claims)
}

#[cfg(test)]
#[path = "../tests/jwt.rs"]
mod tests;
