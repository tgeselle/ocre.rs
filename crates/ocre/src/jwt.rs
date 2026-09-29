//! JSON Web Tokens signed with HS256, for API clients that cannot keep a
//! session cookie.
//!
//! The signing key is derived from the `SECRET_KEY_BASE` Worker secret
//! (HMAC-SHA256 of a fixed label), so apps have one secret to create, upload
//! and rotate: rotating it signs everyone out of sessions and tokens at once.
//! Tokens carry only the subject (usually the user id), `iat` and `exp`;
//! [`decode`](crate::jwt::decode) rejects expired tokens, bad signatures and any `alg` other than
//! `HS256` (including `none`) with [`Error::Unauthorized`](crate::Error::Unauthorized).
//!
//! ```ignore
//! use ocre::jwt::{self, Claims};
//!
//! // POST /api/auth/token, after checking the password:
//! let token = jwt::encode(&ctx, &Claims::new(user.id.to_string(), 3600))?;
//! // Later, from `Authorization: Bearer <token>`:
//! let claims = jwt::decode(&ctx, &token)?; // 401 when invalid or expired
//! let user_id: i64 = claims.sub.parse().map_err(|_| ocre::Error::Unauthorized)?;
//! ```

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac as _};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub use crate::runtime::jwt::{decode, encode};
use crate::{Error, Result, session::checked_secret};

/// The only algorithm Ocre signs with and accepts.
pub const ALGORITHM: &str = "HS256";
/// Label mixed into `SECRET_KEY_BASE` so the JWT key differs from the session key.
const KEY_LABEL: &[u8] = b"ocre/jwt/hs256";
/// `{"alg":"HS256","typ":"JWT"}`, the header of every token Ocre issues.
const HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

/// What a token says: who it is for and when it expires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// Subject: the user id, as a string (the JWT standard's type).
    pub sub: String,
    /// Issued at, Unix seconds.
    pub iat: i64,
    /// Expires at, Unix seconds.
    pub exp: i64,
}

impl Claims {
    /// Claims for `sub`, issued now and valid for `ttl_seconds`.
    ///
    /// ```
    /// let claims = ocre::jwt::Claims::new("42", 3600);
    /// assert_eq!(claims.exp - claims.iat, 3600);
    /// ```
    pub fn new(sub: impl Into<String>, ttl_seconds: i64) -> Self {
        let iat = crate::now();
        Self { sub: sub.into(), iat, exp: iat + ttl_seconds }
    }
}

/// HS256 signing key.
pub struct Key([u8; 32]);

impl Key {
    /// Derives the key from a `SECRET_KEY_BASE` value.
    ///
    /// ```
    /// let key = ocre::jwt::Key::from_secret_key_base(&"x".repeat(64));
    /// let token = ocre::jwt::encode_with(&key, &ocre::jwt::Claims::new("42", 60));
    /// assert_eq!(ocre::jwt::decode_with(&key, &token, ocre::now()).unwrap().sub, "42");
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

/// Signs `claims` with `key`. Handlers use [`encode`], which takes the key
/// from `SECRET_KEY_BASE`.
pub fn encode_with(key: &Key, claims: &Claims) -> String {
    let payload = serde_json::to_vec(claims).expect("claims serialize");
    let signing_input = format!("{}.{}", URL_SAFE_NO_PAD.encode(HEADER), URL_SAFE_NO_PAD.encode(payload));
    let mut mac = key.mac();
    mac.update(signing_input.as_bytes());
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

/// Checks `token` with `key` at Unix time `now`. Handlers use [`decode`].
/// Every failure is [`Error::Unauthorized`], without saying which check failed.
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
