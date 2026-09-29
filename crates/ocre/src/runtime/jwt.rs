use crate::{
    Ctx, Result,
    jwt::{Claims, Key, decode_with, encode_with},
    session::SECRET_KEY_BASE,
};

fn key(ctx: &Ctx) -> Result<Key> {
    Key::from_secret(ctx.env().secret(SECRET_KEY_BASE).ok().map(|secret| secret.to_string()))
}

/// Signs `claims` with the key derived from `SECRET_KEY_BASE`.
///
/// ```ignore
/// let token = ocre::jwt::encode(&ctx, &ocre::jwt::Claims::new(user.id.to_string(), 3600))?;
/// ```
pub fn encode(ctx: &Ctx, claims: &Claims) -> Result<String> {
    Ok(encode_with(&key(ctx)?, claims))
}

/// Checks the signature, the algorithm (HS256 only) and the expiry.
/// Any failure is [`Error::Unauthorized`](crate::Error::Unauthorized).
///
/// ```ignore
/// let claims = ocre::jwt::decode(&ctx, bearer_token)?;
/// ```
pub fn decode(ctx: &Ctx, token: &str) -> Result<Claims> {
    decode_with(&key(ctx)?, token, crate::now())
}
