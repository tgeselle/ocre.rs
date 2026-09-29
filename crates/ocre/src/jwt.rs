//! JSON Web Tokens (HS256) for API clients.
//!
//! For API clients that cannot keep a session cookie (mobile apps, scripts):
//! they send `Authorization: Bearer <token>` instead.
//!
//! The signing key is derived from the `SECRET_KEY_BASE` Worker secret
//! (HMAC-SHA256 of a fixed label), so apps have one secret to create, upload
//! and rotate: rotating it signs everyone out of sessions and tokens at once.
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
//! use axum::{extract::State, http::HeaderMap};
//! use ocre::{Ctx, Error, Result, jwt::{self, Claims}};
//!
//! // POST /api/auth/token, after checking the password:
//! async fn token(State(ctx): State<Ctx>) -> Result<String> {
//!     let user_id = 42;
//!     jwt::encode(&ctx, &Claims::new(user_id.to_string(), 3600))
//! }
//!
//! // Later, from `Authorization: Bearer <token>`:
//! async fn me(State(ctx): State<Ctx>, headers: HeaderMap) -> Result<String> {
//!     let bearer = headers.get("authorization").and_then(|value| value.to_str().ok());
//!     let token = bearer.and_then(|value| value.strip_prefix("Bearer ")).ok_or(Error::Unauthorized)?;
//!     let claims = jwt::decode(&ctx, token)?; // 401 when invalid or expired
//!     let user_id: i64 = claims.sub.parse().map_err(|_| Error::Unauthorized)?;
//!     Ok(user_id.to_string())
//! }
//! ```

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
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
