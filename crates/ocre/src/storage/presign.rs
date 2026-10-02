//! Presigned URLs (AWS Signature Version 4, query-string form) for R2's S3
//! API and other S3-compatible stores, and the signed keys of direct uploads.
//!
//! Signing is local: two SHA-256 and five HMAC-SHA256 over a few hundred
//! bytes (a few microseconds of CPU), no binding call and no R2 operation.
//! The browser's `GET` (class B) or `PUT` (class A) on the URL is what R2 counts.

use std::{collections::BTreeMap, fmt, fmt::Write as _};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Attachment, BINARY_TYPES, Disposition, Rules, StoredObject, content_disposition, essence, new_key};
use crate::{Error, Result, Validator, helpers::civil_from_days, token::constant_time_eq};

/// Worker variable holding the Cloudflare account ID, for presigned R2 URLs.
///
/// Set it in cloudflare.config.ts (`R2_ACCOUNT_ID: bindings.text("<id>"),` in
/// `worker.env`); the ID is on the dashboard's R2 overview page. Not a secret.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::storage::R2_ACCOUNT_ID, "R2_ACCOUNT_ID");
/// ```
pub const R2_ACCOUNT_ID: &str = "R2_ACCOUNT_ID";

/// Worker variable holding the R2 bucket name (`<app>-storage`), for presigned R2 URLs.
///
/// The `STORAGE` binding knows its bucket, but R2's S3 API needs the name in
/// the URL: set `R2_BUCKET: bindings.text("<app>-storage"),` in `worker.env`.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::storage::R2_BUCKET, "R2_BUCKET");
/// ```
pub const R2_BUCKET: &str = "R2_BUCKET";

/// Worker secret holding the access key ID of an R2 API token, for presigned R2 URLs.
///
/// Create the token in the dashboard (R2 > Manage API tokens, "Object Read &
/// Write" on the bucket), put it in `.dev.vars` and upload it with `ocre
/// secrets push R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY --file .prod.vars`.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::storage::R2_ACCESS_KEY_ID, "R2_ACCESS_KEY_ID");
/// ```
pub const R2_ACCESS_KEY_ID: &str = "R2_ACCESS_KEY_ID";

/// Worker secret holding the secret access key of an R2 API token, for presigned R2 URLs.
///
/// Shown once when the token is created; set it like [`R2_ACCESS_KEY_ID`].
/// It also signs the keys handed out by [`direct_upload`](crate::storage::direct_upload),
/// so replacing it invalidates uploads in progress.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::storage::R2_SECRET_ACCESS_KEY, "R2_SECRET_ACCESS_KEY");
/// ```
pub const R2_SECRET_ACCESS_KEY: &str = "R2_SECRET_ACCESS_KEY";

/// Longest lifetime of a presigned URL, in seconds: 7 days, the SigV4 maximum.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::storage::MAX_EXPIRES_IN, 7 * 24 * 3600);
/// ```
pub const MAX_EXPIRES_IN: u64 = 604_800;

/// An S3-compatible bucket and the credentials to presign URLs for it.
///
/// [`S3Endpoint::r2`] builds the one of an R2 bucket; the runtime helpers
/// ([`presign_get`](crate::storage::presign_get),
/// [`direct_upload`](crate::storage::direct_upload)...) read it from the
/// `R2_*` variables and secrets. Fill the fields yourself for another
/// S3-compatible store (AWS S3, MinIO...). `Debug` hides the secret.
///
/// # Examples
///
/// ```
/// use ocre::storage::S3Endpoint;
///
/// let r2 = S3Endpoint::r2("0123abcd", "blog-storage", "AKID", "s3cr3t");
/// assert_eq!(r2.host, "0123abcd.r2.cloudflarestorage.com");
/// assert_eq!(r2.region, "auto");
/// assert!(!format!("{r2:?}").contains("s3cr3t"));
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct S3Endpoint {
    /// Host name of the S3 API, without scheme (`<account_id>.r2.cloudflarestorage.com`).
    pub host: String,
    /// Signing region: `auto` for R2.
    pub region: String,
    /// Bucket name, put first in the path (`/<bucket>/<key>`, path style); empty when the host already names the bucket.
    pub bucket: String,
    /// Access key ID of the credentials.
    pub access_key_id: String,
    /// Secret access key of the credentials.
    pub secret_access_key: String,
}

impl fmt::Debug for S3Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Endpoint")
            .field("host", &self.host)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"[redacted]")
            .finish()
    }
}

impl S3Endpoint {
    /// The S3 endpoint of an R2 bucket: host `<account_id>.r2.cloudflarestorage.com`, region `auto`, path-style URLs.
    ///
    /// # Examples
    ///
    /// ```
    /// let r2 = ocre::storage::S3Endpoint::r2("acc", "blog-storage", "AKID", "secret");
    /// assert_eq!((r2.host.as_str(), r2.bucket.as_str()), ("acc.r2.cloudflarestorage.com", "blog-storage"));
    /// ```
    pub fn r2(account_id: &str, bucket: &str, access_key_id: &str, secret_access_key: &str) -> Self {
        Self {
            host: format!("{account_id}.r2.cloudflarestorage.com"),
            region: "auto".to_owned(),
            bucket: bucket.to_owned(),
            access_key_id: access_key_id.to_owned(),
            secret_access_key: secret_access_key.to_owned(),
        }
    }

    /// Presigns a request on `key` (AWS SigV4, query-string form) and returns its `https://` URL.
    ///
    /// `headers` are signed too (`host` always is): the client must send
    /// them with exactly these values, or the store refuses the request.
    /// `query` adds parameters such as `response-content-disposition`. The
    /// payload is not signed (`UNSIGNED-PAYLOAD`). `now` is the Unix time
    /// of signing ([`ocre::now()`](crate::now)); the URL works for
    /// `expires_in` seconds. Pure: no binding, a few microseconds of CPU.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when `expires_in` is 0 or over [`MAX_EXPIRES_IN`] (7 days).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::storage::S3Endpoint;
    ///
    /// let r2 = S3Endpoint::r2("acc", "blog-storage", "AKID", "secret");
    /// let url = r2.presign("GET", "exports/report 1.csv", &[], &[], 1_790_000_000, 300).unwrap();
    /// assert!(url.starts_with("https://acc.r2.cloudflarestorage.com/blog-storage/exports/report%201.csv?X-Amz-Algorithm="));
    /// assert!(url.contains("&X-Amz-Expires=300&"));
    /// assert!(r2.presign("GET", "k", &[], &[], 1_790_000_000, 8 * 24 * 3600).is_err());
    /// ```
    pub fn presign(
        &self,
        method: &str,
        key: &str,
        headers: &[(&str, &str)],
        query: &[(&str, &str)],
        now: i64,
        expires_in: u64,
    ) -> Result<String> {
        if expires_in == 0 || expires_in > MAX_EXPIRES_IN {
            return Err(Error::internal(format!(
                "a presigned URL lasts 1 to {MAX_EXPIRES_IN} seconds (7 days), not {expires_in}"
            )));
        }
        let (date, timestamp) = amz_date(now);
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let path = match self.bucket.as_str() {
            "" => format!("/{}", encode(key, false)),
            bucket => format!("/{}/{}", encode(bucket, false), encode(key, false)),
        };
        let mut signed: BTreeMap<String, String> =
            headers.iter().map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned())).collect();
        signed.insert("host".to_owned(), self.host.clone());
        let signed_names = signed.keys().map(String::as_str).collect::<Vec<_>>().join(";");
        let credential = format!("{}/{scope}", self.access_key_id);
        let expires = expires_in.to_string();
        let mut params: Vec<(String, String)> = [
            ("X-Amz-Algorithm", "AWS4-HMAC-SHA256"),
            ("X-Amz-Credential", credential.as_str()),
            ("X-Amz-Date", timestamp.as_str()),
            ("X-Amz-Expires", expires.as_str()),
            ("X-Amz-SignedHeaders", signed_names.as_str()),
        ]
        .iter()
        .chain(query)
        .map(|(name, value)| (encode(name, true), encode(value, true)))
        .collect();
        params.sort();
        let canonical_query =
            params.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join("&");
        let canonical_headers: String = signed.iter().map(|(name, value)| format!("{name}:{value}\n")).collect();
        let method = method.to_ascii_uppercase();
        let canonical_request =
            format!("{method}\n{path}\n{canonical_query}\n{canonical_headers}\n{signed_names}\nUNSIGNED-PAYLOAD");
        let string_to_sign =
            format!("AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}", hex(&Sha256::digest(canonical_request.as_bytes())));
        let mut key = hmac(format!("AWS4{}", self.secret_access_key).as_bytes(), date.as_bytes());
        for part in [self.region.as_str(), "s3", "aws4_request"] {
            key = hmac(&key, part.as_bytes());
        }
        let signature = hex(&hmac(&key, string_to_sign.as_bytes()));
        Ok(format!("https://{}{path}?{canonical_query}&X-Amz-Signature={signature}", self.host))
    }
}

/// What the browser declares before a direct upload: the file's name, type and size.
///
/// The JSON body of the "start an upload" request (`{"filename": "a.png",
/// "content_type": "image/png", "size": 1234}`), read from `File.name`,
/// `File.type` and `File.size`. [`direct_upload`](crate::storage::direct_upload)
/// checks it against [`Rules`] before signing anything.
///
/// # Examples
///
/// ```
/// let request: ocre::storage::DirectUploadRequest =
///     serde_json::from_str(r#"{"filename":"a.png","content_type":"image/png","size":1234}"#).unwrap();
/// assert_eq!(request.size, 1234);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectUploadRequest {
    /// File name, as the browser gives it (cleaned up when attached).
    pub filename: String,
    /// Declared content type; signed into the upload URL, so R2 stores this one.
    pub content_type: String,
    /// Declared size in bytes; signed into the upload URL as `Content-Length`.
    pub size: u64,
}

/// A direct upload the browser may perform: `PUT` the file to `url` with `headers`, then submit `signed_key`.
///
/// Returned by [`direct_upload`](crate::storage::direct_upload) and sent to
/// the browser as JSON (Rails' `direct_upload: { url, headers }` plus
/// `signed_id`). `signed_key` is the new object key followed by an HMAC:
/// [`attach_direct_upload`](crate::storage::attach_direct_upload) accepts
/// only keys this app issued, so a client cannot claim another record's file.
///
/// # Examples
///
/// ```
/// use ocre::storage::{DirectUpload, DirectUploadRequest, Rules, S3Endpoint};
///
/// const PHOTO: Rules = Rules { max_bytes: 1024, content_types: &["image/png"] };
/// let r2 = S3Endpoint::r2("acc", "blog-storage", "AKID", "secret");
/// let request = DirectUploadRequest { filename: "a.png".into(), content_type: "image/png".into(), size: 10 };
/// let upload = DirectUpload::sign(&r2, "uploads/photos", "image", &request, &PHOTO, 1_790_000_000, 600).unwrap();
/// assert!(upload.signed_key.starts_with("uploads/photos/"));
/// assert_eq!(upload.headers["Content-Type"], "image/png");
/// let json = serde_json::to_value(&upload).unwrap();
/// assert!(json["url"].as_str().unwrap().contains("X-Amz-SignedHeaders=content-length%3Bcontent-type%3Bhost"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectUpload {
    /// The object key and its signature (`<prefix>/<22 characters>.<43 characters>`); the form submits it.
    pub signed_key: String,
    /// Presigned `PUT` URL on R2's S3 API.
    pub url: String,
    /// Headers the `PUT` must carry, exactly (`Content-Type`); the browser adds `Content-Length` itself.
    pub headers: BTreeMap<String, String>,
}

impl DirectUpload {
    /// Checks `request` against `rules` and presigns a `PUT` of a new key under `prefix`, valid `expires_in` seconds.
    ///
    /// The URL signs `Content-Type` (the declared type, normalized) and
    /// `Content-Length` (the declared size), so R2 refuses a `PUT` of
    /// another type or size. [`direct_upload`](crate::storage::direct_upload)
    /// calls it with the `R2_*` settings and [`ocre::now()`](crate::now).
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] (422) on `field` when the declared size or type
    /// breaks `rules` (the messages of [`Validator::file`]);
    /// [`Error::Internal`] when `expires_in` is out of range.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::storage::{DirectUpload, DirectUploadRequest, Rules, S3Endpoint};
    ///
    /// const PHOTO: Rules = Rules { max_bytes: 1024, content_types: &["image/png"] };
    /// let r2 = S3Endpoint::r2("acc", "blog-storage", "AKID", "secret");
    /// let svg = DirectUploadRequest { filename: "a.svg".into(), content_type: "image/svg+xml".into(), size: 10 };
    /// let err = DirectUpload::sign(&r2, "uploads", "image", &svg, &PHOTO, 1_790_000_000, 600).unwrap_err();
    /// assert_eq!(err.to_string(), "invalid: Image has an unsupported type (allowed: image/png)");
    /// ```
    pub fn sign(
        endpoint: &S3Endpoint,
        prefix: &str,
        field: &str,
        request: &DirectUploadRequest,
        rules: &Rules,
        now: i64,
        expires_in: u64,
    ) -> Result<Self> {
        Validator::new().file_size_and_type(field, request.size, &request.content_type, rules).finish()?;
        let key = new_key(prefix);
        let content_type = essence(&request.content_type);
        let size = request.size.to_string();
        let headers = [("content-type", content_type.as_str()), ("content-length", size.as_str())];
        let url = endpoint.presign("PUT", &key, &headers, &[], now, expires_in)?;
        let signed_key = sign_key(&endpoint.secret_access_key, &key);
        Ok(Self { signed_key, url, headers: BTreeMap::from([("Content-Type".to_owned(), content_type)]) })
    }
}

/// `<key>.<HMAC-SHA256 of the key, URL-safe base64>`, with a key derived from the R2 secret.
pub(crate) fn sign_key(secret: &str, key: &str) -> String {
    let mac = hmac(&hmac(secret.as_bytes(), b"ocre.storage.direct_upload"), key.as_bytes());
    format!("{key}.{}", URL_SAFE_NO_PAD.encode(mac))
}

/// The key of a `signed_key` made by [`sign_key`] with `secret`; a 422 on `field` when it was not.
pub(crate) fn verify_key(field: &str, secret: &str, signed_key: &str) -> Result<String> {
    let key = signed_key.rsplit_once('.').map(|(key, _)| key).unwrap_or_default();
    let valid = constant_time_eq(sign_key(secret, key).as_bytes(), signed_key.as_bytes());
    Validator::new().check(field, !valid, "is not a valid upload").finish()?;
    Ok(key.to_owned())
}

/// The secret that signs upload keys: `R2_SECRET_ACCESS_KEY` (which direct
/// uploads need anyway), else `SECRET_KEY_BASE` for uploads that go through
/// the Worker.
pub(crate) fn upload_secret(var: &dyn Fn(&str) -> Option<String>) -> Result<String> {
    [R2_SECRET_ACCESS_KEY, crate::session::SECRET_KEY_BASE]
        .iter()
        .find_map(|name| var(name).map(|value| value.trim().to_owned()).filter(|value| !value.is_empty()))
        .ok_or_else(|| {
            Error::internal(
                "uploads need R2_SECRET_ACCESS_KEY or SECRET_KEY_BASE to sign their keys (neither is set). Fix: \
                 `ocre secret` makes a SECRET_KEY_BASE for .dev.vars; `ocre deploy` sets it in production",
            )
        })
}

/// The endpoint of the `R2_*` variables and secrets read with `var`; the error names every missing one.
pub(crate) fn r2_endpoint(var: &dyn Fn(&str) -> Option<String>) -> Result<S3Endpoint> {
    let names = [R2_ACCOUNT_ID, R2_BUCKET, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY];
    let values = names.map(|name| var(name).map(|value| value.trim().to_owned()).filter(|v| !v.is_empty()));
    if let [Some(account), Some(bucket), Some(id), Some(secret)] = &values {
        return Ok(S3Endpoint::r2(account, bucket, id, secret));
    }
    let missing: Vec<&str> = names.iter().zip(&values).filter(|(_, value)| value.is_none()).map(|(n, _)| *n).collect();
    Err(Error::internal(format!(
        "presigned R2 URLs need {} (not set). Fix: add `R2_ACCOUNT_ID: bindings.text(\"<account id>\"),` and \
         `R2_BUCKET: bindings.text(\"<app>-storage\"),` to worker.env in cloudflare.config.ts; create an R2 API \
         token (dashboard: R2 > Manage API tokens, Object Read & Write), put R2_ACCESS_KEY_ID and \
         R2_SECRET_ACCESS_KEY in .dev.vars and run `ocre secrets push R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY \
         --file .prod.vars`",
        missing.join(", ")
    )))
}

/// Presigned `GET` of an attachment: R2 answers with a safe `Content-Type` and `Content-Disposition`.
pub(crate) fn presign_get_url(
    endpoint: &S3Endpoint,
    attachment: &Attachment,
    disposition: Disposition,
    now: i64,
    expires_in: u64,
) -> Result<String> {
    let binary = BINARY_TYPES.contains(&attachment.content_type.as_str());
    let content_type = if binary { "application/octet-stream" } else { &attachment.content_type };
    let disposition = content_disposition(disposition, &attachment.filename, &attachment.content_type);
    let query = [("response-content-disposition", disposition.as_str()), ("response-content-type", content_type)];
    endpoint.presign("GET", &attachment.key, &[], &query, now, expires_in)
}

/// The attachment of a direct upload found by `head`, after checking it against `rules`.
pub(crate) fn attachment_from_head(
    field: &str,
    head: Option<&StoredObject>,
    filename: &str,
    rules: &Rules,
) -> Result<Attachment> {
    let mut v = Validator::new();
    match head {
        None => {
            v.check(field, true, "was not uploaded");
        }
        Some(object) => {
            v.file_size_and_type(field, object.size, &object.content_type, rules);
        }
    }
    v.finish()?;
    let object = head.expect("the validator refused a missing upload");
    Ok(object.attachment(filename))
}

/// `(yyyymmdd, yyyymmddThhmmssZ)` of a Unix time, in UTC.
fn amz_date(now: i64) -> (String, String) {
    let (year, month, day) = civil_from_days(now.div_euclid(86_400));
    let seconds = now.rem_euclid(86_400);
    let date = format!("{year:04}{month:02}{day:02}");
    let time = format!("{date}T{:02}{:02}{:02}Z", seconds / 3600, seconds % 3600 / 60, seconds % 60);
    (date, time)
}

/// RFC 3986 percent-encoding of everything but unreserved characters (and `/` in paths).
fn encode(text: &str, slash: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || (byte == b'/' && !slash) {
            out.push(byte as char);
        } else {
            write!(out, "%{byte:02X}").expect("writing to a String");
        }
    }
    out
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a String");
        out
    })
}

#[cfg(test)]
#[path = "../../tests/storage/presign.rs"]
mod tests;
