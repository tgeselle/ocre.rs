//! Random tokens for links and API keys, stored hashed.
//!
//! A token is 32 random bytes (256 bits) from the platform's secure random
//! generator (WebCrypto on Workers), encoded as URL-safe base64 without
//! padding: 43 characters, safe in URLs, headers and emails.
//!
//! Store only [`digest`](crate::token::digest)s: a leaked database then holds no usable token. A
//! fast hash is enough here because tokens are random and long; passwords,
//! which people choose, need [`password`](crate::password) instead.
//!
//! ```
//! let token = ocre::token::generate();
//! let stored = ocre::token::digest(&token);
//! // Later, from a request: look the row up by digest.
//! assert_eq!(ocre::token::digest(&token), stored);
//! ```

use std::fmt::Write as _;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest as _, Sha256};

/// Random bytes in a token.
pub const TOKEN_BYTES: usize = 32;

/// A new random token: 32 bytes as URL-safe base64 (43 characters).
///
/// ```
/// let token = ocre::token::generate();
/// assert_eq!(token.len(), 43);
/// ```
pub fn generate() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<TOKEN_BYTES>())
}

/// SHA-256 of `token` as 64 lowercase hex characters: the value to store and
/// look up (`WHERE digest = ?1`), so the database never holds the token.
///
/// ```
/// assert_eq!(ocre::token::digest("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
/// ```
pub fn digest(token: &str) -> String {
    let hash = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(hash.len() * 2);
    for byte in hash {
        write!(hex, "{byte:02x}").expect("writing to a String");
    }
    hex
}

/// Compares secrets in constant time for equal lengths, so response timing
/// does not reveal how many leading bytes matched. Lengths are not secret.
///
/// ```
/// assert!(ocre::token::constant_time_eq(b"secret", b"secret"));
/// assert!(!ocre::token::constant_time_eq(b"secret", b"secreT"));
/// ```
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let difference = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    // Keeps the optimizer from turning the loop into an early exit.
    std::hint::black_box(difference) == 0
}

/// `N` bytes from the secure random generator.
pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).expect("the secure random generator is available");
    bytes
}

#[cfg(test)]
#[path = "../tests/token.rs"]
mod tests;
