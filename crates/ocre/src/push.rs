//! Web push notifications: messages a browser shows even when the app's
//! page is closed (the Push API, with the service worker of `ocre g pwa`).
//!
//! The browser subscribes with the app's public VAPID key and hands the
//! app a [`Subscription`]: an endpoint at its push service (Google's for
//! Chrome, Apple's for Safari, Mozilla's for Firefox) and the keys to
//! encrypt for it. [`send`] encrypts the message (`aes128gcm`, RFC 8291),
//! signs the request with the app's private VAPID key (RFC 8292) and posts
//! it to the endpoint: one subrequest per subscription. A subscription the
//! push service answers 404 or 410 to is gone ([`Sent::Gone`]): delete it.
//!
//! Settings: `VAPID_PUBLIC_KEY` and `VAPID_SUBJECT` (`mailto:` or `https:`
//! contact, required by push services) are variables, `VAPID_PRIVATE_KEY`
//! a secret; [`VapidKeys::generate`] (or `ocre g push`) makes a pair.
//!
//! CPU: each message takes three P-256 operations in WebAssembly (an
//! ephemeral key, the key agreement, the VAPID signature), the costliest
//! part of sending (not yet measured on Workers): send to many subscribers
//! from a job, a batch per run, rather than in a request.

use aes_gcm::{Aes128Gcm, KeyInit as _, aead::Aead as _};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use p256::{
    PublicKey, SecretKey,
    ecdsa::{Signature, SigningKey, signature::Signer as _},
    elliptic_curve::sec1::ToEncodedPoint as _,
};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub use crate::runtime::push::{Sent, send};
use crate::{Error, Result, token::random_bytes};

/// Worker variable holding the public VAPID key (URL-safe base64), which browsers subscribe with.
pub const VAPID_PUBLIC_KEY: &str = "VAPID_PUBLIC_KEY";
/// Worker secret holding the private VAPID key (URL-safe base64, 32 bytes).
pub const VAPID_PRIVATE_KEY: &str = "VAPID_PRIVATE_KEY";
/// Worker variable holding the contact push services may write to: `mailto:you@example.com` or an `https:` URL.
pub const VAPID_SUBJECT: &str = "VAPID_SUBJECT";

/// Largest message, in bytes: one 4,096-byte record less the encryption's overhead.
pub const MAX_PAYLOAD: usize = 4096 - 16 - 1;

/// What a browser's `PushSubscription.toJSON()` gives: where and how to push to it.
///
/// # Examples
///
/// ```
/// let json = r#"{"endpoint":"https://fcm.googleapis.com/fcm/send/abc","expirationTime":null,"keys":{"p256dh":"BCV...","auth":"BTB..."}}"#;
/// let subscription: ocre::push::Subscription = serde_json::from_str(json).unwrap();
/// assert_eq!(subscription.keys.auth, "BTB...");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    /// The push service URL for this browser.
    pub endpoint: String,
    /// The browser's keys.
    pub keys: SubscriptionKeys,
}

/// The browser's public key and authentication secret, URL-safe base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionKeys {
    /// P-256 public key (65 bytes, uncompressed).
    pub p256dh: String,
    /// Authentication secret (16 bytes).
    pub auth: String,
}

/// A VAPID key pair, URL-safe base64 without padding.
#[derive(Clone, PartialEq, Eq)]
pub struct VapidKeys {
    /// The public key (65 bytes, uncompressed point): `VAPID_PUBLIC_KEY`, given to browsers.
    pub public_key: String,
    /// The private key (32 bytes): `VAPID_PRIVATE_KEY`, a secret.
    pub private_key: String,
}

impl std::fmt::Debug for VapidKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VapidKeys").field("public_key", &self.public_key).field("private_key", &"[redacted]").finish()
    }
}

impl VapidKeys {
    /// A new random key pair.
    ///
    /// # Examples
    ///
    /// ```
    /// let keys = ocre::push::VapidKeys::generate();
    /// assert_eq!(keys.public_key.len(), 87); // 65 bytes
    /// assert_eq!(keys.private_key.len(), 43); // 32 bytes
    /// assert!(!format!("{keys:?}").contains(&keys.private_key));
    /// ```
    pub fn generate() -> Self {
        let secret = random_secret();
        Self {
            public_key: URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes()),
            private_key: URL_SAFE_NO_PAD.encode(secret.to_bytes()),
        }
    }
}

/// A random P-256 secret key (retried in the astronomically rare case the bytes are not a valid scalar).
fn random_secret() -> SecretKey {
    std::iter::repeat_with(|| SecretKey::from_slice(&random_bytes::<32>())).find_map(Result::ok).expect("random bytes")
}

/// The message the service worker of `ocre g pwa` shows: `{"title", "options": {"body", "data": {"path"}}}`;
/// clicking the notification opens `path`.
///
/// # Examples
///
/// ```
/// let message = ocre::push::message("Ready", "Your video is ready.", "/videos/42");
/// assert_eq!(message["options"]["data"]["path"], "/videos/42");
/// ```
pub fn message(title: &str, body: &str, path: &str) -> serde_json::Value {
    serde_json::json!({ "title": title, "options": { "body": body, "data": { "path": path } } })
}

/// Encrypts `payload` for a subscription (`aes128gcm` content coding, RFC 8291), with a new key and salt.
///
/// # Errors
///
/// [`Error::BadRequest`] when the subscription's keys are not valid
/// base64 P-256 keys, or the payload is over [`MAX_PAYLOAD`].
pub fn encrypt(keys: &SubscriptionKeys, payload: &[u8]) -> Result<Vec<u8>> {
    encrypt_with(keys, payload, &random_secret(), random_bytes::<16>())
}

/// [`encrypt`] with the sender's key and salt given (RFC 8291's test vector uses fixed ones).
pub(crate) fn encrypt_with(
    keys: &SubscriptionKeys,
    payload: &[u8],
    sender: &SecretKey,
    salt: [u8; 16],
) -> Result<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD {
        return Err(Error::bad_request(format!("a push message is at most {MAX_PAYLOAD} bytes")));
    }
    let invalid = || Error::bad_request("the push subscription's keys are invalid");
    let browser_bytes = URL_SAFE_NO_PAD.decode(keys.p256dh.trim_end_matches('=')).map_err(|_| invalid())?;
    let auth = URL_SAFE_NO_PAD.decode(keys.auth.trim_end_matches('=')).map_err(|_| invalid())?;
    let browser = PublicKey::from_sec1_bytes(&browser_bytes).map_err(|_| invalid())?;
    let shared = p256::ecdh::diffie_hellman(sender.to_nonzero_scalar(), browser.as_affine());
    let sender_public = sender.public_key().to_encoded_point(false);
    let sender_bytes = sender_public.as_bytes();
    // IKM = HKDF(auth, ecdh_secret, "WebPush: info" || 0 || ua_public || as_public, 32)
    let key_info = [b"WebPush: info\0".as_slice(), &browser_bytes, sender_bytes].concat();
    let mut ikm = [0; 32];
    Hkdf::<Sha256>::new(Some(&auth), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .expect("32 bytes is a valid HKDF-SHA256 length");
    let content = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let (mut key, mut nonce) = ([0; 16], [0; 12]);
    content.expand(b"Content-Encoding: aes128gcm\0", &mut key).expect("valid length");
    content.expand(b"Content-Encoding: nonce\0", &mut nonce).expect("valid length");
    // One record: the payload, then the last-record delimiter 0x02, no padding.
    let plaintext = [payload, &[2]].concat();
    let ciphertext = Aes128Gcm::new(&key.into())
        .encrypt(&nonce.into(), plaintext.as_slice())
        .expect("AES-GCM encrypts any message this short");
    let record_size: u32 = 4096;
    let mut body = Vec::with_capacity(16 + 4 + 1 + sender_bytes.len() + ciphertext.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&record_size.to_be_bytes());
    body.push(u8::try_from(sender_bytes.len()).expect("65 bytes"));
    body.extend_from_slice(sender_bytes);
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

/// The `Authorization` header of a push to `endpoint` (VAPID, RFC 8292):
/// `vapid t=<JWT signed with ES256>, k=<public key>`, valid 12 hours.
///
/// # Errors
///
/// [`Error::Internal`] when the private key is not 32 bytes of URL-safe
/// base64, or `endpoint` is not an `https://` (or, for local fakes, `http://`) URL.
///
/// # Examples
///
/// ```
/// let keys = ocre::push::VapidKeys::generate();
/// let header = ocre::push::vapid_authorization("https://push.example/send/1", "mailto:ops@example.com", &keys, 1_790_000_000).unwrap();
/// assert!(header.starts_with("vapid t=eyJ"));
/// assert!(header.ends_with(&format!(", k={}", keys.public_key)));
/// ```
pub fn vapid_authorization(endpoint: &str, subject: &str, keys: &VapidKeys, now: i64) -> Result<String> {
    let invalid = || Error::internal("VAPID_PRIVATE_KEY is not a P-256 private key in URL-safe base64");
    let private = URL_SAFE_NO_PAD.decode(keys.private_key.trim().trim_end_matches('=')).map_err(|_| invalid())?;
    let signing = SigningKey::from_slice(&private).map_err(|_| invalid())?;
    // The push service's origin. Real ones are https; http serves local fakes in tests.
    let audience = ["https://", "http://"]
        .iter()
        .find_map(|scheme| {
            endpoint.strip_prefix(scheme).map(|rest| format!("{scheme}{}", rest.split('/').next().unwrap_or_default()))
        })
        .ok_or_else(|| Error::internal(format!("a push endpoint is an https:// URL, not `{endpoint}`")))?;
    let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = serde_json::json!({ "aud": audience, "exp": now + 12 * 3600, "sub": subject });
    let unsigned = format!("{header}.{}", URL_SAFE_NO_PAD.encode(claims.to_string()));
    let signature: Signature = signing.sign(unsigned.as_bytes());
    Ok(format!("vapid t={unsigned}.{}, k={}", URL_SAFE_NO_PAD.encode(signature.to_bytes()), keys.public_key.trim()))
}

#[cfg(test)]
#[path = "../tests/push.rs"]
mod tests;
