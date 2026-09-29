//! Password hashing with PBKDF2-HMAC-SHA256.
//!
//! Workers have no CPU budget for bcrypt or argon2 in WebAssembly (10 ms per
//! request on the free plan), but WebCrypto runs PBKDF2 natively, outside the
//! WebAssembly module. Workers cap it at 100,000 iterations; Ocre uses the cap.
//! Native builds (tests, tools) compute the same function in pure Rust.
//!
//! Digests are self-describing, like Django's:
//! `pbkdf2_sha256$100000$<salt, base64>$<hash, base64>` (16-byte random salt,
//! 32-byte hash, standard base64 without padding). The iteration count is
//! stored, so it can grow later without invalidating existing passwords.
//!
//! Passwords are never logged: errors name the operation only.
//!
//! ```ignore
//! // Sign up: store the digest, never the password.
//! let password_digest = ocre::password::hash(&form.password).await?;
//! // Log in: compare in constant time.
//! if ocre::password::verify(&form.password, &user.password_digest).await? { /* signed in */ }
//! ```

use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};

use crate::{Error, Result, token::constant_time_eq};

/// Iterations for new digests: the most Workers' WebCrypto accepts.
pub const ITERATIONS: u32 = 100_000;
/// Identifies the algorithm at the start of a digest.
const SCHEME: &str = "pbkdf2_sha256";
const SALT_BYTES: usize = 16;
const HASH_BYTES: usize = 32;

/// Hashes `password` with a new random salt. Store the result (e.g. in a
/// `password_digest` column) and check passwords with [`verify`].
///
/// Costs one PBKDF2 run (see the README for the measured CPU time).
pub async fn hash(password: &str) -> Result<String> {
    hash_with(password, ITERATIONS).await
}

async fn hash_with(password: &str, iterations: u32) -> Result<String> {
    let salt = crate::token::random_bytes::<SALT_BYTES>();
    let hash = derive(password, &salt, iterations).await?;
    Ok(format!("{SCHEME}${iterations}${}${}", STANDARD_NO_PAD.encode(salt), STANDARD_NO_PAD.encode(hash)))
}

/// Whether `password` matches `digest` (made by [`hash`]). The comparison
/// runs in constant time. A digest that is not in Ocre's format is an
/// internal error (corrupted data), not a failed login.
pub async fn verify(password: &str, digest: &str) -> Result<bool> {
    let Some(parsed) = Parsed::from_digest(digest) else {
        return Err(Error::internal(
            "the password digest is not in the pbkdf2_sha256$<iterations>$<salt>$<hash> format",
        ));
    };
    let hash = derive(password, &parsed.salt, parsed.iterations).await?;
    Ok(constant_time_eq(&hash, &parsed.hash))
}

/// The iteration count stored in `digest`, e.g. to re-hash old passwords
/// after a login when it is lower than [`ITERATIONS`].
///
/// ```
/// let digest = "pbkdf2_sha256$100000$c2FsdA$aGFzaA";
/// assert_eq!(ocre::password::iterations(digest), Some(100_000));
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
