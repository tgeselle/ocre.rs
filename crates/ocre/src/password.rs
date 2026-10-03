//! Password hashing (PBKDF2-HMAC-SHA256).
//!
//! Like Rails' `has_secure_password`, without bcrypt: Workers have no CPU
//! budget for bcrypt or argon2 in WebAssembly (10 ms per request on the free
//! plan), but WebCrypto (`crypto.subtle.deriveBits`) runs PBKDF2 natively,
//! outside the WebAssembly module. Workers cap it at 100,000 iterations;
//! Ocre uses the cap ([`ITERATIONS`]). Native builds (tests, tools) compute
//! the same function in pure Rust.
//!
//! Digests are self-describing, like Django's:
//! `pbkdf2_sha256$100000$<salt, base64>$<hash, base64>` (16-byte random salt,
//! 32-byte hash, standard base64 without padding). The iteration count is
//! stored, so it can grow later without invalidating existing passwords
//! ([`iterations`] reads it back).
//!
//! Cost: one hash measured at 5.5 ms of CPU in `wrangler dev` (see the
//! [cost model](https://ocre.rs/explanations/cost-model)), about half of the free plan's 10 ms per request, so
//! only sign-up, login and password changes should hash. [`hash`] and
//! [`verify`] each run PBKDF2 once. Passwords are never logged: errors name
//! the operation only.
//!
//! ```rust,no_run
//! use axum::{Form, extract::State};
//! use ocre::{Ctx, Error, Result};
//! # #[derive(serde::Deserialize)]
//! # struct Login { email: String, password: String }
//! # struct User { password_digest: String }
//! # async fn find_by_email(_ctx: &Ctx, _email: &str) -> Result<Option<User>> { Ok(None) }
//!
//! // Sign up: store the digest, never the password.
//! async fn sign_up(Form(form): Form<Login>) -> Result<String> {
//!     let password_digest = ocre::password::hash(&form.password).await?;
//!     Ok(password_digest) // INSERT INTO users (email, password_digest) ...
//! }
//!
//! // Log in: compare in constant time.
//! async fn log_in(State(ctx): State<Ctx>, Form(form): Form<Login>) -> Result<&'static str> {
//!     let user = find_by_email(&ctx, &form.email).await?.ok_or(Error::Unauthorized)?;
//!     if ocre::password::verify(&form.password, &user.password_digest).await? {
//!         Ok("signed in")
//!     } else {
//!         Err(Error::Unauthorized)
//!     }
//! }
//! ```

use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};

use crate::{Error, Result, token::constant_time_eq};

/// Iterations for new digests: 100,000, the most Workers' WebCrypto accepts.
///
/// OWASP recommends 600,000 for PBKDF2-HMAC-SHA256; the cap is the
/// platform's. Digests store their own count, so raising it later keeps old
/// digests verifiable.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::password::ITERATIONS, 100_000);
/// ```
pub const ITERATIONS: u32 = 100_000;
/// Identifies the algorithm at the start of a digest.
const SCHEME: &str = "pbkdf2_sha256";
const SALT_BYTES: usize = 16;
const HASH_BYTES: usize = 32;

/// Hashes `password` with a new random 16-byte salt into a self-describing digest.
///
/// The result looks like `pbkdf2_sha256$100000$<salt>$<hash>`; store it
/// (e.g. in a `password_digest` column) and check passwords with [`verify`].
/// Two calls with the same password give different digests (new salt).
///
/// Cost: one PBKDF2 run with [`ITERATIONS`], measured at 5.5 ms of CPU on
/// Workers (see the [cost model](https://ocre.rs/explanations/cost-model)): about half of the free plan's 10 ms
/// per request.
///
/// # Errors
///
/// On Workers, [`Error::Internal`] (500) when WebCrypto fails (the message
/// names the step, never the password). Native builds never fail.
///
/// # Panics
///
/// If the platform's secure random generator is unavailable, which does not
/// happen on supported targets.
///
/// # Examples
///
/// ```
/// # pollster::block_on(async {
/// let digest = ocre::password::hash("correct horse").await?;
/// assert!(digest.starts_with("pbkdf2_sha256$100000$"));
/// assert!(ocre::password::verify("correct horse", &digest).await?);
/// # Ok::<(), ocre::Error>(())
/// # }).unwrap();
/// ```
pub async fn hash(password: &str) -> Result<String> {
    hash_with(password, ITERATIONS).await
}

async fn hash_with(password: &str, iterations: u32) -> Result<String> {
    let salt = crate::token::random_bytes::<SALT_BYTES>();
    let hash = derive(password, &salt, iterations).await?;
    Ok(format!("{SCHEME}${iterations}${}${}", STANDARD_NO_PAD.encode(salt), STANDARD_NO_PAD.encode(hash)))
}

/// Whether `password` matches `digest` (made by [`hash`]), compared in constant time.
///
/// The password is hashed with the digest's own salt and iteration count, so
/// digests made with an older [`ITERATIONS`] still verify. A wrong password
/// is `Ok(false)`, not an error.
///
/// Cost: one PBKDF2 run with the stored iteration count (5.5 ms of CPU on
/// Workers at 100,000). To keep timing from revealing which emails have
/// accounts, verify against some digest even when the user is unknown.
///
/// # Errors
///
/// - [`Error::Internal`] (500) when `digest` is not in the
///   `pbkdf2_sha256$<iterations>$<salt>$<hash>` format (corrupted data, not a
///   failed login).
/// - On Workers, [`Error::Internal`] when WebCrypto fails.
///
/// # Examples
///
/// ```
/// # pollster::block_on(async {
/// let digest = ocre::password::hash("correct horse").await?;
/// assert!(ocre::password::verify("correct horse", &digest).await?);
/// assert!(!ocre::password::verify("wrong horse", &digest).await?);
/// assert!(ocre::password::verify("correct horse", "plaintext").await.is_err());
/// # Ok::<(), ocre::Error>(())
/// # }).unwrap();
/// ```
pub async fn verify(password: &str, digest: &str) -> Result<bool> {
    let Some(parsed) = Parsed::from_digest(digest) else {
        return Err(Error::internal(
            "the password digest is not in the pbkdf2_sha256$<iterations>$<salt>$<hash> format",
        ));
    };
    let hash = derive(password, &parsed.salt, parsed.iterations).await?;
    Ok(constant_time_eq(&hash, &parsed.hash))
}

/// The iteration count stored in `digest`, or `None` when it is not in Ocre's format.
///
/// Use it to re-hash old passwords after a successful login when the count
/// is lower than [`ITERATIONS`]. Parses only; no hashing.
///
/// # Examples
///
/// ```
/// let digest = "pbkdf2_sha256$100000$c2FsdA$aGFzaA";
/// assert_eq!(ocre::password::iterations(digest), Some(100_000));
/// assert_eq!(ocre::password::iterations("$2b$12$bcrypt"), None);
/// ```
pub fn iterations(digest: &str) -> Option<u32> {
    Parsed::from_digest(digest).map(|parsed| parsed.iterations)
}

struct Parsed {
    iterations: u32,
    salt: Vec<u8>,
    hash: Vec<u8>,
}

impl Parsed {
    fn from_digest(digest: &str) -> Option<Self> {
        let mut parts = digest.split('$');
        if parts.next()? != SCHEME {
            return None;
        }
        let iterations = parts.next()?.parse().ok().filter(|&n| n > 0)?;
        let salt = STANDARD_NO_PAD.decode(parts.next()?).ok()?;
        let hash = STANDARD_NO_PAD.decode(parts.next()?).ok()?;
        if parts.next().is_some() || salt.is_empty() || hash.is_empty() {
            return None;
        }
        Some(Self { iterations, salt, hash })
    }
}

/// PBKDF2-HMAC-SHA256, 32 bytes: WebCrypto on Workers.
#[cfg(target_arch = "wasm32")]
async fn derive(password: &str, salt: &[u8], iterations: u32) -> Result<Vec<u8>> {
    crate::runtime::pbkdf2_sha256(password.as_bytes(), salt, iterations, HASH_BYTES).await
}

/// PBKDF2-HMAC-SHA256, 32 bytes: pure Rust on native targets.
#[cfg(not(target_arch = "wasm32"))]
async fn derive(password: &str, salt: &[u8], iterations: u32) -> Result<Vec<u8>> {
    let mut hash = vec![0u8; HASH_BYTES];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password.as_bytes(), salt, iterations, &mut hash);
    Ok(hash)
}

#[cfg(test)]
#[path = "../tests/password.rs"]
mod tests;
