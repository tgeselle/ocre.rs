use crate::{
    Ctx, Result,
    jwt::{Claims, Key, decode_with, encode_with},
    session::SECRET_KEY_BASE,
};

fn key(ctx: &Ctx) -> Result<Key> {
    Key::from_secret(ctx.env().secret(SECRET_KEY_BASE).ok().map(|secret| secret.to_string()))
}

/// Signs `claims` with the HS256 key derived from the `SECRET_KEY_BASE` Worker secret.
///
/// Same token as [`encode_with`](crate::jwt::encode_with) with
/// [`Key::from_secret_key_base`](crate::jwt::Key::from_secret_key_base).
/// The secret is read on every call; no D1 or KV operation.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when `SECRET_KEY_BASE`
/// is not set or is shorter than 64 characters; the message names the fix
/// (`ocre secret`, `.dev.vars` for `ocre dev`, `ocre deploy` uploads it).
///
/// # Examples
///
/// ```rust,no_run
/// use axum::{Json, extract::State};
/// use ocre::{Ctx, Result, jwt::{self, Claims}};
///
/// // POST /api/auth/token, after checking the password:
/// async fn token(State(ctx): State<Ctx>) -> Result<Json<serde_json::Value>> {
///     let user_id = 42;
///     let token = jwt::encode(&ctx, &Claims::new(user_id.to_string(), 3600))?;
///     Ok(Json(serde_json::json!({ "token": token, "expires_in": 3600 })))
/// }
/// ```
pub fn encode(ctx: &Ctx, claims: &Claims) -> Result<String> {
    Ok(encode_with(&key(ctx)?, claims))
}

/// Verifies a token from a client with the key derived from `SECRET_KEY_BASE` and returns its claims.
///
/// Checks the signature, the algorithm ([`ALGORITHM`](crate::jwt::ALGORITHM)
/// only) and the expiry against the current time ([`crate::now`]), like
/// [`decode_with`](crate::jwt::decode_with). No D1 or KV operation.
///
/// # Errors
///
/// - [`Error::Unauthorized`](crate::Error::Unauthorized) (401) for any
///   invalid token: malformed, other algorithm, bad signature, expired.
/// - [`Error::Internal`](crate::Error::Internal) (500) when `SECRET_KEY_BASE`
///   is not set or is shorter than 64 characters (message names the fix).
///
/// # Examples
///
/// ```rust,no_run
/// use axum::{extract::State, http::HeaderMap};
/// use ocre::{Ctx, Error, Result};
///
/// async fn me(State(ctx): State<Ctx>, headers: HeaderMap) -> Result<String> {
///     let bearer = headers.get("authorization").and_then(|value| value.to_str().ok());
///     let token = bearer.and_then(|value| value.strip_prefix("Bearer ")).ok_or(Error::Unauthorized)?;
///     let claims = ocre::jwt::decode(&ctx, token)?;
///     Ok(claims.sub)
/// }
/// ```
pub fn decode(ctx: &Ctx, token: &str) -> Result<Claims> {
    decode_with(&key(ctx)?, token, crate::now())
}
