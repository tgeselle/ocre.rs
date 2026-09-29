//! Attribute encryption for model columns: values are encrypted in Rust
//! before they reach D1 and decrypted when rows are read, like Rails'
//! `encrypts`.
//!
//! Keys are derived from [`SECRET_KEY_BASE`](crate::SECRET_KEY_BASE) with
//! HKDF-SHA256 (separate from the session cookie key), and the cipher is
//! AES-256-GCM, so a changed ciphertext fails to decrypt instead of giving a
//! wrong value. A value is stored as text: `v1:` then the nonce and the
//! ciphertext in URL-safe base64 (about 4/3 of the value plus 42
//! characters).
//!
//! - [`Encrypted`]: a random nonce per write. The same value never gives the
//!   same text twice, so the column cannot be searched.
//! - [`Deterministic`]: the nonce comes from the value (HMAC-SHA256), so
//!   equal values give equal texts: `WHERE email = ?1` and unique indexes
//!   work, at the price of revealing which rows share a value. Normalize
//!   before encrypting (e.g. lowercase an email) for case-insensitive lookups.
//!
//! Both are field types for a model's row struct: they deserialize by
//! decrypting, bind as parameters by encrypting, and serialize to JSON as
//! the plain value (see the Models guide, "Encrypted columns").
//!
//! # Keys and rotation
//!
//! [`Ctx`](crate::Ctx) installs the keys the first time a Worker instance
//! handles a request: one HKDF derivation, then pure Rust (AES-GCM of a short
//! value costs microseconds of CPU). Values encrypted with a secret listed in
//! [`SECRET_KEY_BASE_PREVIOUS`](crate::SECRET_KEY_BASE_PREVIOUS) still
//! decrypt; writes use the current secret. Deterministic lookups must try
//! every key during a rotation ([`Encryptor::deterministic_candidates`] with
//! `Query::is_in`), until a job has rewritten the rows with the current key.
//! Losing `SECRET_KEY_BASE` loses the data: back it up.

use std::{cell::RefCell, fmt};

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::Sha256;

use crate::{Error, IntoParam, Param, Result, session};

/// Prefix of every encrypted value: the format version.
const PREFIX: &str = "v1:";
const NONCE_BYTES: usize = 12;

/// One secret's keys: the cipher and the HMAC key of deterministic nonces.
#[derive(Clone)]
struct Key {
    cipher: Aes256Gcm,
    nonce_key: [u8; 32],
}

impl Key {
    fn derive(secret: &str) -> Self {
        let hkdf = hkdf::Hkdf::<Sha256>::new(None, secret.as_bytes());
        let mut cipher_key = [0u8; 32];
        let mut nonce_key = [0u8; 32];
        hkdf.expand(b"ocre attribute encryption: AES-256-GCM", &mut cipher_key).expect("32 bytes is a valid length");
        hkdf.expand(b"ocre attribute encryption: deterministic nonce", &mut nonce_key)
            .expect("32 bytes is a valid length");
        Self { cipher: Aes256Gcm::new(&cipher_key.into()), nonce_key }
    }

    fn seal(&self, nonce: [u8; NONCE_BYTES], plaintext: &str) -> String {
        let payload = Payload { msg: plaintext.as_bytes(), aad: PREFIX.as_bytes() };
        let ciphertext = self.cipher.encrypt(Nonce::from_slice(&nonce), payload).expect("AES-GCM encrypts any length");
        let mut bytes = nonce.to_vec();
        bytes.extend(ciphertext);
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
    }

    fn deterministic_nonce(&self, plaintext: &str) -> [u8; NONCE_BYTES] {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.nonce_key).expect("HMAC takes any key length");
        mac.update(plaintext.as_bytes());
        let digest = mac.finalize().into_bytes();
        digest[..NONCE_BYTES].try_into().expect("SHA-256 is longer than a nonce")
    }

    fn open(&self, nonce: &[u8], ciphertext: &[u8]) -> Option<String> {
        let payload = Payload { msg: ciphertext, aad: PREFIX.as_bytes() };
        let plaintext = self.cipher.decrypt(Nonce::from_slice(nonce), payload).ok()?;
        String::from_utf8(plaintext).ok()
    }
}

/// Encrypts and decrypts column values with keys derived from `SECRET_KEY_BASE`.
///
/// Models use it through [`Encrypted`] and [`Deterministic`]; use it
/// directly for values outside a model (a job argument, an API token to call
/// another service).
///
/// # Examples
///
/// ```
/// use ocre::encryption::Encryptor;
///
/// let secret = "a".repeat(64);
/// let encryptor = Encryptor::new(&secret, &[]).unwrap();
/// let stored = encryptor.encrypt("123-45-6789");
/// assert!(stored.starts_with("v1:"));
/// assert_ne!(stored, encryptor.encrypt("123-45-6789")); // a random nonce each time
/// assert_eq!(encryptor.decrypt(&stored).unwrap(), "123-45-6789");
/// // Deterministic: the same text for the same value, so it can be looked up.
/// assert_eq!(encryptor.encrypt_deterministic("ada@example.com"), encryptor.encrypt_deterministic("ada@example.com"));
/// ```
#[derive(Clone)]
pub struct Encryptor {
    current: Key,
    previous: Vec<Key>,
}

impl fmt::Debug for Encryptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encryptor").field("previous_keys", &self.previous.len()).finish_non_exhaustive()
    }
}

impl Encryptor {
    /// Derives the keys from the current secret and the previous ones (newest first).
    ///
    /// Each secret must be 64 characters or more, like `SECRET_KEY_BASE`.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] naming the fix when a secret is too short.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::encryption::Encryptor;
    ///
    /// let (old, new) = ("o".repeat(64), "n".repeat(64));
    /// let stored = Encryptor::new(&old, &[]).unwrap().encrypt("secret");
    /// let rotated = Encryptor::new(&new, &[old.as_str()]).unwrap();
    /// assert_eq!(rotated.decrypt(&stored).unwrap(), "secret");
    /// assert!(Encryptor::new("short", &[]).is_err());
    /// ```
    pub fn new(secret: &str, previous: &[&str]) -> Result<Self> {
        let current = session::checked_secret(Some(secret.to_owned())).map_err(Error::internal)?;
        let previous = previous
            .iter()
            .map(|secret| session::checked_secret(Some((*secret).to_owned())).map(|secret| Key::derive(&secret)))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::internal)?;
        Ok(Self { current: Key::derive(&current), previous })
    }

    /// Encrypts `plaintext` with a random nonce: `v1:` plus URL-safe base64.
    ///
    /// # Examples
    ///
    /// ```
    /// let encryptor = ocre::encryption::Encryptor::new(&"k".repeat(64), &[]).unwrap();
    /// assert_eq!(encryptor.decrypt(&encryptor.encrypt("")).unwrap(), "");
    /// ```
    pub fn encrypt(&self, plaintext: &str) -> String {
        self.current.seal(crate::token::random_bytes(), plaintext)
    }

    /// Encrypts `plaintext` so that equal values give equal texts (see [`Deterministic`]).
    ///
    /// # Examples
    ///
    /// ```
    /// let encryptor = ocre::encryption::Encryptor::new(&"k".repeat(64), &[]).unwrap();
    /// let a = encryptor.encrypt_deterministic("ada@example.com");
    /// assert_ne!(a, encryptor.encrypt_deterministic("bob@example.com"));
    /// assert_eq!(encryptor.decrypt(&a).unwrap(), "ada@example.com");
    /// ```
    pub fn encrypt_deterministic(&self, plaintext: &str) -> String {
        self.current.seal(self.current.deterministic_nonce(plaintext), plaintext)
    }

    /// The deterministic texts of `plaintext` under every key, current first:
    /// look rows up with `Query::is_in` during a key rotation.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::encryption::Encryptor;
    ///
    /// let (old, new) = ("o".repeat(64), "n".repeat(64));
    /// let stored = Encryptor::new(&old, &[]).unwrap().encrypt_deterministic("ada@example.com");
    /// let rotated = Encryptor::new(&new, &[old.as_str()]).unwrap();
    /// let candidates = rotated.deterministic_candidates("ada@example.com");
    /// assert_eq!(candidates.len(), 2);
    /// assert!(candidates.contains(&stored));
    /// ```
    pub fn deterministic_candidates(&self, plaintext: &str) -> Vec<String> {
        std::iter::once(&self.current)
            .chain(&self.previous)
            .map(|key| key.seal(key.deterministic_nonce(plaintext), plaintext))
            .collect()
    }

    /// Decrypts a value from [`encrypt`](Self::encrypt) or
    /// [`encrypt_deterministic`](Self::encrypt_deterministic), with the
    /// current key or a previous one.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when `ciphertext` is not an encrypted value, was
    /// changed, or was encrypted with a key that is neither current nor
    /// previous. The message never contains the value.
    ///
    /// # Examples
    ///
    /// ```
    /// let encryptor = ocre::encryption::Encryptor::new(&"k".repeat(64), &[]).unwrap();
    /// assert!(encryptor.decrypt("plain text").is_err());
    /// ```
    pub fn decrypt(&self, ciphertext: &str) -> Result<String> {
        let failed = |why: &str| Error::internal(format!("cannot decrypt an encrypted column: {why}"));
        let encoded = ciphertext.strip_prefix(PREFIX).ok_or_else(|| failed("the value is not encrypted"))?;
        let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| failed("the value is not valid base64"))?;
        if bytes.len() < NONCE_BYTES {
            return Err(failed("the value is too short"));
        }
        let (nonce, sealed) = bytes.split_at(NONCE_BYTES);
        std::iter::once(&self.current).chain(&self.previous).find_map(|key| key.open(nonce, sealed)).ok_or_else(|| {
            failed("wrong key or changed value (SECRET_KEY_BASE changed without SECRET_KEY_BASE_PREVIOUS?)")
        })
    }

    /// Like [`decrypt`](Self::decrypt), but a value without the `v1:` prefix
    /// is returned as is: read a column while a data migration encrypts its
    /// existing rows (Rails' `support_unencrypted_data`).
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when an encrypted value does not decrypt.
    ///
    /// # Examples
    ///
    /// ```
    /// let encryptor = ocre::encryption::Encryptor::new(&"k".repeat(64), &[]).unwrap();
    /// assert_eq!(encryptor.decrypt_or_plaintext("not yet encrypted").unwrap(), "not yet encrypted");
    /// ```
    pub fn decrypt_or_plaintext(&self, text: &str) -> Result<String> {
        if is_encrypted(text) { self.decrypt(text) } else { Ok(text.to_owned()) }
    }
}

/// Whether `text` looks like an encrypted value (starts with `v1:`).
///
/// # Examples
///
/// ```
/// assert!(!ocre::encryption::is_encrypted("hello"));
/// ```
pub fn is_encrypted(text: &str) -> bool {
    text.starts_with(PREFIX)
}

thread_local! {
    /// The Worker instance's keys. WebAssembly Workers have one thread;
    /// native tests get one per test thread.
    static INSTALLED: RefCell<Option<Encryptor>> = const { RefCell::new(None) };
}

/// Makes `encryptor` the one [`Encrypted`] and [`Deterministic`] use.
///
/// [`Ctx`](crate::Ctx) calls it with the keys of `SECRET_KEY_BASE`; call it
/// yourself in tests of code that reads encrypted columns.
///
/// # Examples
///
/// ```
/// use ocre::encryption::{self, Encrypted, Encryptor};
///
/// encryption::install(Encryptor::new(&"t".repeat(64), &[]).unwrap());
/// let stored = encryption::installed().unwrap().encrypt("42");
/// let value: Encrypted = serde_json::from_value(serde_json::json!(stored)).unwrap();
/// assert_eq!(value.as_str(), "42");
/// ```
pub fn install(encryptor: Encryptor) {
    INSTALLED.with(|installed| *installed.borrow_mut() = Some(encryptor));
}

/// The installed encryptor.
///
/// # Errors
///
/// [`Error::Internal`] when none is installed: the Worker has no valid
/// `SECRET_KEY_BASE` (the message names the fix), or native code did not
/// call [`install`].
///
/// # Examples
///
/// ```
/// use ocre::encryption::{self, Encryptor};
///
/// encryption::install(Encryptor::new(&"t".repeat(64), &[]).unwrap());
/// assert!(encryption::installed().is_ok());
/// ```
pub fn installed() -> Result<Encryptor> {
    INSTALLED.with(|installed| installed.borrow().clone()).ok_or_else(|| {
        Error::internal(
            "no encryption key: SECRET_KEY_BASE is missing or shorter than 64 characters. \
             Fix: `ocre secret` and put it in .dev.vars (local) or `wrangler secret put SECRET_KEY_BASE`",
        )
    })
}

/// Installs the keys from the Worker's secrets unless already done.
pub(crate) fn ensure_installed(secret: &dyn Fn(&str) -> Option<String>) {
    if INSTALLED.with(|installed| installed.borrow().is_some()) {
        return;
    }
    let Ok(current) = session::checked_secret(secret(session::SECRET_KEY_BASE)) else { return };
    let Ok(previous) = session::previous_secrets(secret(session::SECRET_KEY_BASE_PREVIOUS)) else { return };
    let previous: Vec<&str> = previous.iter().map(String::as_str).collect();
    if let Ok(encryptor) = Encryptor::new(&current, &previous) {
        install(encryptor);
    }
}

fn decrypt_for_serde<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<String, D::Error> {
    let text = String::deserialize(deserializer)?;
    let encryptor = installed().map_err(serde::de::Error::custom)?;
    encryptor.decrypt(&text).map_err(serde::de::Error::custom)
}

/// A column encrypted with a random nonce: reads decrypt, writes encrypt.
///
/// Use it as the type of an encrypted column in the row struct: reading a
/// row decrypts the stored text with the [`installed`] keys (a failure is a
/// deserialization error, so the query fails with a 500). Input structs
/// (`New`/`Changes`, forms, JSON bodies) keep a plain `String` and the
/// model binds `Encrypted::from(value)`, which stores a new ciphertext.
/// Serializing (JSON responses, templates) writes the plain value; `Debug`
/// hides it.
///
/// # Examples
///
/// ```
/// use ocre::{IntoParam, encryption::{self, Encrypted, Encryptor}};
///
/// encryption::install(Encryptor::new(&"t".repeat(64), &[]).unwrap());
/// let ssn = Encrypted::from("123-45-6789");
/// assert_eq!(serde_json::to_string(&ssn).unwrap(), r#""123-45-6789""#);
/// assert_ne!(ssn.clone().into_param(), "123-45-6789".into_param());
/// assert_eq!(format!("{ssn:?}"), "Encrypted(..)");
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Encrypted(pub String);

/// A column encrypted deterministically: equal values give equal stored
/// texts, so `query().eq("email", Deterministic::from(email))` finds the row
/// and a `UNIQUE` index works.
///
/// Same reading and serializing rules as [`Encrypted`]. Equal values being
/// visible as equal texts is the trade-off: use it only for columns you
/// must look up.
///
/// # Examples
///
/// ```
/// use ocre::{IntoParam, encryption::{self, Deterministic, Encryptor}};
///
/// encryption::install(Encryptor::new(&"t".repeat(64), &[]).unwrap());
/// let a = Deterministic::from("ada@example.com").into_param();
/// assert_eq!(a, Deterministic::from("ada@example.com").into_param());
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Deterministic(pub String);

macro_rules! encrypted_type {
    ($name:ident, $encrypt:ident) => {
        impl $name {
            /// The plain value.
            ///
            /// # Examples
            ///
            /// ```
            #[doc = concat!("assert_eq!(ocre::encryption::", stringify!($name), "::from(\"x\").as_str(), \"x\");")]
            /// ```
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "(..)"))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                decrypt_for_serde(deserializer).map(Self)
            }
        }

        /// Binds the encrypted text. Without installed keys (no valid
        /// `SECRET_KEY_BASE`) it binds `NULL`, which a `NOT NULL` column
        /// refuses; the error is logged.
        impl IntoParam for $name {
            fn into_param(self) -> Param {
                match installed() {
                    Ok(encryptor) => encryptor.$encrypt(&self.0).into_param(),
                    Err(err) => {
                        crate::error::log_internal(&err.to_string());
                        None::<String>.into_param()
                    }
                }
            }
        }
    };
}

encrypted_type!(Encrypted, encrypt);
encrypted_type!(Deterministic, encrypt_deterministic);

#[cfg(test)]
#[path = "../tests/encryption.rs"]
mod tests;
