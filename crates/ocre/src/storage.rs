//! File storage in Cloudflare R2: multipart uploads, attachments, streamed downloads.
//!
//! Like Active Storage without its extra tables: a file lives in the R2 bucket
//! bound as `STORAGE` in cloudflare.config.ts, and the record that owns it keeps four
//! columns (`<name>_key`, `<name>_filename`, `<name>_content_type`,
//! `<name>_size`), read back as an [`Attachment`].
//!
//! The flow of an upload: the [`Multipart`] extractor reads the request,
//! [`MultipartForm::file`] takes an [`Upload`], [`Validator::file`](crate::Validator::file)
//! checks it against [`Rules`], [`store`] writes it to R2 and returns the
//! [`Attachment`] to save with [`columns`] / [`column_changes`]. Downloads go
//! through [`serve`] (ETag/304, `Range`, safe `Content-Disposition`); [`read`],
//! [`delete`] and [`delete_attachments`] cover the rest. [`store_bytes`] stores
//! app-made files and [`store_body`] streams a raw request body.
//!
//! Beyond the Worker: [`head`], [`exists`] and [`list`] inspect the bucket;
//! [`presign_get`] / [`serve_redirect`] let browsers download straight from
//! R2's S3 API; [`direct_upload`] and [`attach_direct_upload`] let them
//! upload straight to it (no 100 MB request limit, no Worker memory), and
//! [`purge_unattached`] deletes direct uploads no row adopted. [`analyze`]
//! reads a file's real type and image size, [`Variant`] builds Cloudflare
//! Image Transformations URLs, [`public_url`] links to a public bucket.
//!
//! Keys are random (`<prefix>/<22 characters>`, 128 bits, never derived from
//! file names) and never reused, so a stored object never changes: replacing
//! a file means storing a new key and deleting the old one. `ocre g scaffold
//! Post avatar:attachment` adds the binding, `ocre dev` keeps a local copy
//! under `.wrangler/state`, `ocre deploy` creates the bucket.
//!
//! # Free plan
//!
//! R2 (free every month): 10 GB-month stored, 1M class A operations (each
//! upload is one), 10M class B operations (each download or 304 is one),
//! deletes free, no egress fees. R2 has to be enabled once in the dashboard,
//! which asks for a payment method even for the free tier.
//! Listing is a class A operation per call (up to 1,000 keys), `head` a
//! class B one; presigning costs no operation (the browser's `PUT` or
//! `GET` on the URL does).
//!
//! CPU: downloads never pass through WebAssembly ([`serve`] hands R2's stream
//! to [`crate::serve`]). Uploads are read into memory and split: about 1.2 ms
//! per 10 MB in WebAssembly, plus 0.15 ms per 10 MB to copy them to R2. A
//! Worker has 128 MB and Cloudflare refuses request bodies over 100 MB on the
//! Free plan, so keep limits in the tens of MB.
//!
//! # Examples
//!
//! ```rust,no_run
//! use axum::{
//!     extract::{Path, State},
//!     http::HeaderMap,
//!     response::Response,
//! };
//! use ocre::storage::{self, Attachment, Disposition, Multipart, Rules};
//! use ocre::{Ctx, Error, IntoParam, OptionExt, Result, Validator, params};
//!
//! const AVATAR: Rules = Rules { max_bytes: 5 * 1024 * 1024, content_types: &["image/png", "image/jpeg"] };
//! const FORM_LIMIT: usize = AVATAR.max_bytes + 1024 * 1024;
//!
//! async fn upload(
//!     State(ctx): State<Ctx>,
//!     Path(id): Path<i64>,
//!     Multipart(mut form): Multipart<FORM_LIMIT>,
//! ) -> Result<String> {
//!     let upload = form.file("avatar").ok_or_else(|| Error::bad_request("Choose a file"))?;
//!     Validator::new().file("avatar", &upload, &AVATAR).finish()?;
//!     let avatar = storage::store(&ctx, "users/avatar", upload).await?;
//!     let mut values = Vec::from(storage::columns(Some(&avatar)));
//!     values.push(id.into_param());
//!     let sql = "UPDATE users SET avatar_key = ?1, avatar_filename = ?2, avatar_content_type = ?3, \
//!                avatar_size = ?4 WHERE id = ?5";
//!     if let Err(err) = ctx.db()?.execute(sql, values).await {
//!         storage::delete(&ctx, &avatar.key).await?; // no row points to it
//!         return Err(err);
//!     }
//!     Ok(avatar.key)
//! }
//!
//! async fn download(State(ctx): State<Ctx>, Path(id): Path<i64>, headers: HeaderMap) -> Result<Response> {
//!     let sql = "SELECT avatar_key AS key, avatar_filename AS filename, avatar_content_type AS content_type, \
//!                avatar_size AS size FROM users WHERE id = ?1 AND avatar_key IS NOT NULL";
//!     let avatar: Attachment = ctx.db()?.first(sql, params![id]).await?.or_404()?;
//!     storage::serve(&ctx, &avatar, &headers, Disposition::Inline).await
//! }
//! ```

mod analyze;
mod multipart;
mod presign;
mod variant;

use std::fmt::Write as _;

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

pub use crate::runtime::storage::{
    attach_direct_upload, delete, delete_attachments, direct_upload, exists, head, list, presign_get, presign_put,
    public_url, purge_unattached, read, read_first, serve, serve_redirect, store, store_body, store_bytes,
};
use crate::{Error, IntoParam, Param, Result, Validator, token::random_bytes};
pub use analyze::{Analysis, analyze};
pub use multipart::{Multipart, MultipartForm};
pub use presign::{
    DirectUpload, DirectUploadRequest, MAX_EXPIRES_IN, R2_ACCESS_KEY_ID, R2_ACCOUNT_ID, R2_BUCKET,
    R2_SECRET_ACCESS_KEY, S3Endpoint,
};
pub(crate) use presign::{attachment_from_head, presign_get_url, r2_endpoint, verify_key};
pub use variant::{Fit, Variant};

/// Name of the R2 binding holding every file: `STORAGE: bindings.r2({ name: "<app>-storage" })` in cloudflare.config.ts.
///
/// The bucket itself is `<app>-storage`; the first generator that needs it
/// adds the entry, and `ocre deploy` creates the bucket. Every function of
/// this module fails with [`Error::Internal`] naming
/// this entry when the binding is missing.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::storage::STORAGE_BINDING, "STORAGE");
/// ```
pub const STORAGE_BINDING: &str = "STORAGE";

/// `Cache-Control` of files sent by [`serve`]: browsers keep them but revalidate each time.
///
/// While the `ETag` matches, the browser gets a body-less `304 Not Modified`
/// (still one R2 class B operation). `private` keeps shared caches from
/// storing files that may belong to one user. Override the header on the
/// returned response for files that may be cached longer.
///
/// # Examples
///
/// ```no_run
/// use axum::{extract::State, http::{HeaderMap, HeaderValue, header}, response::Response};
/// use ocre::{Ctx, Result, storage::{self, Attachment, Disposition}};
///
/// async fn logo(State(ctx): State<Ctx>, headers: HeaderMap) -> Result<Response> {
///     let logo = Attachment {
///         key: "public/logo".into(),
///         filename: "logo.png".into(),
///         content_type: "image/png".into(),
///         size: 4096,
///     };
///     let mut response = storage::serve(&ctx, &logo, &headers, Disposition::Inline).await?;
///     // Keys never change, so a public file can be cached for a year.
///     let forever = HeaderValue::from_static("public, max-age=31536000, immutable");
///     response.headers_mut().insert(header::CACHE_CONTROL, forever);
///     Ok(response)
/// }
/// # assert_eq!(storage::CACHE_CONTROL, "private, no-cache");
/// ```
pub const CACHE_CONTROL: &str = "private, no-cache";

/// A stored file, as saved with its record in four columns.
///
/// The columns of an attachment named `avatar` are `avatar_key`,
/// `avatar_filename`, `avatar_content_type` and `avatar_size` (NULL-able for
/// an optional attachment). [`store`] returns it, [`columns`] /
/// [`column_changes`] bind it, and it deserializes from a row whose columns
/// are aliased to `key`, `filename`, `content_type` and `size` (generated
/// models expose it as `photo.image()`). It also serializes to JSON as is.
///
/// # Examples
///
/// ```
/// use ocre::storage::Attachment;
///
/// let avatar = Attachment {
///     key: "posts/avatar/2u1Vd0zJ8sQqS6rJq0rVmA".into(),
///     filename: "me.png".into(),
///     content_type: "image/png".into(),
///     size: 2048,
/// };
/// assert_eq!(avatar.human_size(), "2 KB");
///
/// let row = serde_json::json!({ "key": "k", "filename": "a.pdf", "content_type": "application/pdf", "size": 10 });
/// let from_row: Attachment = serde_json::from_value(row).unwrap();
/// assert_eq!(from_row.filename, "a.pdf");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Object key in the R2 bucket: `<prefix>/<22 random URL-safe characters>`.
    pub key: String,
    /// The uploader's file name, without directories or control characters.
    pub filename: String,
    /// Content type, lowercase and without parameters (`image/png`).
    pub content_type: String,
    /// Size in bytes.
    pub size: i64,
}

impl Attachment {
    /// A new attachment with a fresh random key under `prefix`, a cleaned-up
    /// filename and a normalized content type.
    pub(crate) fn prepare(prefix: &str, filename: &str, content_type: &str, size: u64) -> Self {
        Self {
            key: new_key(prefix),
            filename: sanitize_filename(filename),
            content_type: essence(content_type),
            size: i64::try_from(size).unwrap_or(i64::MAX),
        }
    }

    /// Formats the size for people with [`human_size`]: `512 bytes`, `2 KB`, `1.5 MB`.
    ///
    /// A negative size (only possible from a hand-edited row) reads `0 bytes`.
    ///
    /// # Examples
    ///
    /// ```
    /// let file = ocre::storage::Attachment { size: 1536 * 1024, ..Default::default() };
    /// assert_eq!(file.human_size(), "1.5 MB");
    /// ```
    pub fn human_size(&self) -> String {
        human_size(u64::try_from(self.size).unwrap_or(0))
    }
}

/// A file received in a `multipart/form-data` request, before it is stored.
///
/// Get it with [`MultipartForm::file`], check it with
/// [`Validator::file`](crate::Validator::file), store it with [`store`]. The
/// bytes are a slice of the request body held in memory (no copy until R2
/// gets them).
///
/// # Examples
///
/// ```
/// use ocre::storage::Upload;
///
/// let upload = Upload { filename: "notes.txt".into(), content_type: "text/plain".into(), bytes: "hi".into() };
/// assert_eq!(upload.size(), 2);
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Upload {
    /// File name sent by the browser, cleaned up.
    ///
    /// Directories (`C:\Users\me\` from old Windows browsers) and control
    /// characters are removed, the name is trimmed to 200 characters with its
    /// extension kept, and `file` stands in when nothing is left.
    pub filename: String,
    /// Content type sent by the browser, lowercase without parameters.
    ///
    /// `application/octet-stream` when none was sent. The client chooses it:
    /// check it against an allowlist with [`Validator::file`](crate::Validator::file).
    pub content_type: String,
    /// The file's bytes.
    pub bytes: axum::body::Bytes,
}

impl Upload {
    /// An upload made by the app (Active Storage's `attach(io:, filename:, content_type:)`), cleaned up like a browser's.
    ///
    /// For bytes that did not come from a form: a generated PDF, a fetched
    /// image, a test fixture. The file name loses directories and control
    /// characters, the content type is lowercased without parameters, as
    /// [`MultipartForm::file`] does. Hand it to a model (`NewPhoto { image:
    /// Some(upload), .. }`) or to [`store`]; [`store_bytes`] stores bytes directly.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::storage::Upload;
    ///
    /// let upload = Upload::new("reports/2026.pdf", "Application/PDF; x=y", b"%PDF-1.7".to_vec());
    /// assert_eq!((upload.filename.as_str(), upload.content_type.as_str()), ("2026.pdf", "application/pdf"));
    /// assert_eq!(upload.size(), 8);
    /// ```
    pub fn new(filename: &str, content_type: &str, bytes: impl Into<Bytes>) -> Self {
        Self { filename: sanitize_filename(filename), content_type: essence(content_type), bytes: bytes.into() }
    }

    /// Returns the size of the file in bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::storage::{Upload, human_size};
    ///
    /// let upload = Upload { bytes: vec![0; 1536].into(), ..Default::default() };
    /// assert_eq!(upload.size(), 1536);
    /// assert_eq!(human_size(upload.size()), "1.5 KB");
    /// ```
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// An object in the bucket, as [`head`] and [`list`] describe it (without its bytes).
///
/// # Examples
///
/// ```
/// use ocre::storage::StoredObject;
///
/// let object = StoredObject {
///     key: "uploads/2u1Vd0zJ8sQqS6rJq0rVmA".into(),
///     size: 2048,
///     content_type: "image/png".into(),
///     etag: "b6ab5f279cbcf9a1b96b3ab5b207cf94".into(),
///     uploaded_at: 1_790_000_000,
///     filename: None,
/// };
/// let attachment = object.attachment("C:\\me.png");
/// assert_eq!((attachment.filename.as_str(), attachment.size), ("me.png", 2048));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredObject {
    /// Object key.
    pub key: String,
    /// Size in bytes.
    pub size: u64,
    /// Content type recorded with the object (`application/octet-stream` when none was).
    pub content_type: String,
    /// R2's entity tag, unquoted (the MD5 of the content for single-part uploads).
    pub etag: String,
    /// Upload time, in Unix seconds (compare with [`crate::now`]).
    pub uploaded_at: i64,
    /// File name recorded by [`store`] and friends; `None` for objects uploaded directly.
    pub filename: Option<String>,
}

impl StoredObject {
    /// The [`Attachment`] of this object under `filename` (cleaned up), with its recorded size and type.
    ///
    /// Check the object against [`Rules`] first ([`attach_direct_upload`] does both).
    ///
    /// # Examples
    ///
    /// ```
    /// let object = ocre::storage::StoredObject { key: "k".into(), size: 3, content_type: "text/plain".into(), ..Default::default() };
    /// assert_eq!(object.attachment("a.txt").content_type, "text/plain");
    /// ```
    pub fn attachment(&self, filename: &str) -> Attachment {
        Attachment {
            key: self.key.clone(),
            filename: sanitize_filename(filename),
            content_type: essence(&self.content_type),
            size: i64::try_from(self.size).unwrap_or(i64::MAX),
        }
    }
}

/// One page of [`list`]: the objects, and the cursor of the next page.
///
/// # Examples
///
/// ```
/// use ocre::storage::Listing;
///
/// let last_page = Listing { objects: vec![], cursor: None };
/// assert!(last_page.cursor.is_none());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// Objects in key order.
    pub objects: Vec<StoredObject>,
    /// Pass it to the next [`list`] call; `None` on the last page.
    pub cursor: Option<String>,
}

/// What one [`purge_unattached`] call did: the keys it deleted, and where the next call resumes.
///
/// # Examples
///
/// ```
/// let purged = ocre::storage::Purged { deleted: vec!["uploads/a".into()], cursor: None };
/// assert_eq!(purged.deleted.len(), 1);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Purged {
    /// Keys deleted by this call.
    pub deleted: Vec<String>,
    /// Cursor of the next page of the listing; `None` once the prefix was listed to the end.
    pub cursor: Option<String>,
}

/// Name of the Worker variable holding the base URL of a public bucket, for [`public_url`].
///
/// An `r2.dev` URL or a custom domain connected to the bucket (dashboard:
/// R2 > bucket > Settings > Public access), as
/// `STORAGE_PUBLIC_URL: bindings.text("https://files.example.com"),` in `worker.env`.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::storage::STORAGE_PUBLIC_URL, "STORAGE_PUBLIC_URL");
/// ```
pub const STORAGE_PUBLIC_URL: &str = "STORAGE_PUBLIC_URL";

/// `<base>/<key>`, with each key segment percent-encoded.
pub(crate) fn join_public_url(base: Option<String>, key: &str) -> Result<String> {
    let base = base.filter(|base| !base.trim().is_empty()).ok_or_else(|| {
        Error::internal(format!(
            "public file URLs need the {STORAGE_PUBLIC_URL} variable. Fix: allow public access to the bucket (dashboard: \
             R2 > bucket > Settings > Public access: an r2.dev URL or a custom domain), then add \
             `{STORAGE_PUBLIC_URL}: bindings.text(\"https://files.example.com\"),` to worker.env in cloudflare.config.ts"
        ))
    })?;
    let mut url = base.trim().trim_end_matches('/').to_owned();
    for segment in key.split('/') {
        url.push('/');
        for byte in segment.bytes() {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                url.push(byte as char);
            } else {
                write!(url, "%{byte:02X}").expect("writing to a String");
            }
        }
    }
    Ok(url)
}

/// `302 Found` to a presigned URL; browsers and shared caches may reuse it for half its lifetime.
pub(crate) fn redirect_response(url: &str, expires_in: u64) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    response.headers_mut().insert(header::LOCATION, header_value(url));
    let cache = format!("private, max-age={}", expires_in / 2);
    response.headers_mut().insert(header::CACHE_CONTROL, header_value(&cache));
    response
}

/// Largest page [`list`] asks R2 for.
pub(crate) const MAX_LIST: u32 = 1000;

/// Largest number of keys looked up per D1 query (D1 binds at most 100 parameters).
pub(crate) const KEYS_PER_QUERY: usize = 100;

/// Keys of the objects uploaded before `cutoff` (Unix seconds).
pub(crate) fn stale_keys(objects: &[StoredObject], cutoff: i64) -> Vec<String> {
    objects.iter().filter(|object| object.uploaded_at < cutoff).map(|object| object.key.clone()).collect()
}

/// `SELECT <column> AS key FROM <table> WHERE <column> IN (?1, ...)` for `count` keys.
///
/// `table` and `column` come from app code, never from requests; anything
/// but ASCII letters, digits and `_` is refused.
pub(crate) fn referenced_keys_sql(table: &str, column: &str, count: usize) -> Result<String> {
    let identifier = |name: &str| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    if !identifier(table) || !identifier(column) {
        return Err(Error::internal(format!(
            "purge_unattached takes a table and a column name (letters, digits, `_`), not `{table}` / `{column}`"
        )));
    }
    let placeholders: Vec<String> = (1..=count).map(|n| format!("?{n}")).collect();
    Ok(format!("SELECT {column} AS key FROM {table} WHERE {column} IN ({})", placeholders.join(", ")))
}

/// Describes what a file field accepts: a size limit and a content-type allowlist.
///
/// Checked by [`Validator::file`](crate::Validator::file) before anything is
/// stored. A `const`, so the [`Multipart`] request limit can be computed from
/// it (generated forms use the sum of their files' limits plus 1 MB). The
/// content type comes from the browser: the allowlist limits it, and
/// [`Validator::file_content`](crate::Validator::file_content) checks the bytes match it.
///
/// # Examples
///
/// ```
/// use ocre::storage::Rules;
///
/// const AVATAR: Rules = Rules { max_bytes: 5 * 1024 * 1024, content_types: &["image/png", "image/jpeg"] };
/// const FORM_LIMIT: usize = AVATAR.max_bytes + 1024 * 1024;
/// assert_eq!(FORM_LIMIT, 6 * 1024 * 1024);
/// assert!(AVATAR.allows("image/PNG"));
/// assert!(!AVATAR.allows("image/svg+xml"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// Largest accepted file, in bytes.
    pub max_bytes: usize,
    /// Accepted content types, exact and lowercase (`image/png`).
    ///
    /// There is no wildcard: `image/*` would admit SVG, which can carry scripts.
    pub content_types: &'static [&'static str],
}

impl Rules {
    /// Returns whether `content_type` is in the allowlist, ignoring case and parameters.
    ///
    /// `Image/PNG; charset=binary` is compared as `image/png`; an empty type
    /// counts as `application/octet-stream`. There is no wildcard matching.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::storage::Rules;
    ///
    /// const DOC: Rules = Rules { max_bytes: 1024, content_types: &["application/pdf", "text/plain"] };
    /// assert!(DOC.allows("text/plain; charset=utf-8"));
    /// assert!(DOC.allows("Application/PDF"));
    /// assert!(!DOC.allows("text/html"));
    /// assert!(!DOC.allows(""));
    /// ```
    pub fn allows(&self, content_type: &str) -> bool {
        let essence = essence(content_type);
        self.content_types.contains(&essence.as_str())
    }
}

impl Validator {
    /// Checks an uploaded file against `rules`: its size and its content type.
    ///
    /// Adds "is too large (maximum is 5 MB)" when the file is over
    /// [`Rules::max_bytes`] and "has an unsupported type (allowed: image/png,
    /// image/jpeg)" when [`Rules::allows`] refuses its type; both can be
    /// reported at once. Run it before [`store`], so a refused file costs no
    /// R2 operation. Returns `self` for chaining; [`finish`](crate::Validator::finish)
    /// turns the collected messages into a 422.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Validator, storage::{Rules, Upload}};
    ///
    /// const DOC: Rules = Rules { max_bytes: 4, content_types: &["text/plain"] };
    /// let upload = Upload { filename: "a.html".into(), content_type: "text/html".into(), bytes: "<p>hello</p>".into() };
    /// let err = Validator::new().file("doc", &upload, &DOC).finish().unwrap_err();
    /// assert_eq!(
    ///     err.to_string(),
    ///     "invalid: Doc is too large (maximum is 4 bytes), Doc has an unsupported type (allowed: text/plain)"
    /// );
    ///
    /// let note = Upload { filename: "a.txt".into(), content_type: "text/plain".into(), bytes: "ok".into() };
    /// assert!(Validator::new().file("doc", &note, &DOC).finish().is_ok());
    /// ```
    pub fn file(&mut self, field: &str, upload: &Upload, rules: &Rules) -> &mut Self {
        self.file_size_and_type(field, upload.size(), &upload.content_type, rules)
    }

    /// [`Validator::file`]'s checks on a size and a content type (declared for a direct upload, or read by `head`).
    pub(crate) fn file_size_and_type(
        &mut self,
        field: &str,
        size: u64,
        content_type: &str,
        rules: &Rules,
    ) -> &mut Self {
        let too_large = size > rules.max_bytes as u64;
        self.check(field, too_large, format!("is too large (maximum is {})", human_size(rules.max_bytes as u64)));
        let allowed = rules.content_types.join(", ");
        self.check(field, !rules.allows(content_type), format!("has an unsupported type (allowed: {allowed})"))
    }
}

/// Tells [`serve`] whether the browser shows a file or downloads it.
///
/// Either way `Content-Disposition` carries the original file name (ASCII in
/// `filename=`, UTF-8 in `filename*=` when needed, RFC 6266).
///
/// # Examples
///
/// ```
/// use ocre::storage::Disposition;
///
/// // A download link: `?download=1` forces the "Save as" dialog.
/// let download = true;
/// let disposition = if download { Disposition::Download } else { Disposition::Inline };
/// assert_eq!(disposition, Disposition::Download);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Shows the file in the page or tab when its type is safe to display, and downloads anything else.
    ///
    /// Safe types: raster images (PNG, JPEG, GIF, WebP, AVIF, BMP, TIFF,
    /// icons), PDF, plain text, audio and video (MPEG, Ogg, WAV, WebM, MP4).
    /// HTML, SVG, XML and JavaScript are sent as `application/octet-stream`
    /// downloads, so an uploaded file never runs as part of the app (Rails'
    /// `content_types_allowed_inline` / `content_types_to_serve_as_binary`).
    Inline,
    /// Always downloads the file, with its original name.
    Download,
}

/// Builds the query parameters for the four columns of one attachment, in column order.
///
/// The order is `<name>_key, <name>_filename, <name>_content_type,
/// <name>_size`; `None` binds four NULLs (an optional attachment left empty).
/// Generated `create` functions extend their `params!` with it. Reading the
/// columns back needs no helper: `Attachment` deserializes from a row with
/// `key`, `filename`, `content_type` and `size` columns.
///
/// # Examples
///
/// ```
/// use ocre::{params, storage::{self, Attachment}};
///
/// let avatar = Attachment { key: "k".into(), filename: "a.png".into(), content_type: "image/png".into(), size: 3 };
/// let mut values = params!["Ada"];
/// values.extend(storage::columns(Some(&avatar)));
/// assert_eq!(values, params!["Ada", "k", "a.png", "image/png", 3_i64]);
///
/// let empty = storage::columns(None);
/// assert_eq!(empty[..], params![None::<i64>, None::<i64>, None::<i64>, None::<i64>]);
/// ```
pub fn columns(attachment: Option<&Attachment>) -> [Param; 4] {
    match attachment {
        Some(file) => [
            file.key.as_str().into_param(),
            file.filename.as_str().into_param(),
            file.content_type.as_str().into_param(),
            file.size.into_param(),
        ],
        None => {
            [None::<i64>.into_param(), None::<i64>.into_param(), None::<i64>.into_param(), None::<i64>.into_param()]
        }
    }
}

/// Builds the parameters of an `UPDATE` of one attachment's columns: a change flag, then the four [`columns`].
///
/// `None` keeps the stored file (flag `false`, four NULLs that the SQL
/// ignores), `Some(None)` clears the columns (flag `true`, four NULLs),
/// `Some(Some(file))` points them to `file`. The generated SQL reads
/// `avatar_key = CASE WHEN ?1 THEN ?2 ELSE avatar_key END, ...`, so one
/// statement handles "unchanged", "removed" and "replaced" without building
/// SQL at runtime. Deleting the replaced object is up to the caller
/// ([`delete_attachments`], after the write succeeds).
///
/// # Examples
///
/// ```
/// use ocre::{params, storage::{self, Attachment}};
///
/// let photo = Attachment { key: "k".into(), filename: "a.png".into(), content_type: "image/png".into(), size: 3 };
/// assert_eq!(storage::column_changes(Some(Some(&photo))), params![true, "k", "a.png", "image/png", 3_i64][..]);
/// let null = None::<i64>;
/// assert_eq!(storage::column_changes(Some(None))[..], params![true, null, null, null, null]);
/// assert_eq!(storage::column_changes(None)[0], params![false][0]);
/// ```
pub fn column_changes(change: Option<Option<&Attachment>>) -> [Param; 5] {
    let [key, filename, content_type, size] = columns(change.flatten());
    [change.is_some().into_param(), key, filename, content_type, size]
}

/// Formats a byte count for people, in powers of 1024 like Rails' `number_to_human_size`.
///
/// Below 1024 the count is exact (`1 byte`, `512 bytes`); above, it is
/// rounded to one decimal, dropped when it is zero (`2 KB`, `1.5 MB`,
/// `10 GB`). Units stop at TB. Used in the "is too large (maximum is 5 MB)"
/// validation message and the 413 of [`Multipart`].
///
/// # Examples
///
/// ```
/// use ocre::storage::human_size;
///
/// assert_eq!(human_size(1), "1 byte");
/// assert_eq!(human_size(512), "512 bytes");
/// assert_eq!(human_size(2048), "2 KB");
/// assert_eq!(human_size(1536 * 1024), "1.5 MB");
/// assert_eq!(human_size(10 * 1024 * 1024), "10 MB");
/// assert_eq!(human_size(3 * 1024_u64.pow(5)), "3072 TB");
/// ```
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} {}", if bytes == 1 { "byte" } else { "bytes" });
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let rounded = (value * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{rounded:.0} {}", UNITS[unit])
    } else {
        format!("{rounded:.1} {}", UNITS[unit])
    }
}

/// Sends bytes built by the handler as a file (Rails' `send_data`): a CSV export, a generated image, an `.ics` file.
///
/// Sets `Content-Type`, `Content-Length` and `Content-Disposition` with
/// `filename` (sanitized; non-ASCII names kept in `filename*`). As with
/// [`serve`], [`Disposition::Inline`] shows only types that are safe to
/// display (images, PDF, plain text, audio, video) and downloads the rest,
/// and types a browser could run (HTML, SVG, XML, JavaScript) are sent as
/// `application/octet-stream`. Files already in R2 go through [`serve`]
/// instead: it streams them without copying through WebAssembly, and
/// answers `Range` and `If-None-Match` (Rails' `send_file`). The whole body
/// is in memory, and the Worker's 128 MB memory limit applies; building it
/// counts toward the 10 ms CPU budget.
///
/// # Examples
///
/// ```
/// use ocre::storage::{Disposition, send_data};
///
/// let csv = "id,title\n1,Hello\n";
/// let response = send_data(csv, "posts.csv", "text/csv", Disposition::Download);
/// assert_eq!(response.headers()["content-type"], "text/csv");
/// assert_eq!(response.headers()["content-disposition"], "attachment; filename=\"posts.csv\"");
/// ```
pub fn send_data(data: impl Into<Bytes>, filename: &str, content_type: &str, disposition: Disposition) -> Response {
    data_response(data.into(), filename, content_type, disposition)
}

/// The browser side of direct uploads (Active Storage's `activestorage.js`):
/// a script that sends the files of `<input type="file"
/// data-direct-upload-url="...">` to R2 before its form is submitted.
///
/// Each file is signed by a POST to the input's URL (a handler calling
/// [`direct_upload`]), then `PUT` to R2 with progress events
/// (`direct-upload:start|progress|error|end`); the form then submits
/// `<name>_key` and `<name>_filename` for [`attach_direct_upload`] instead
/// of the file. It also uploads the files dropped into a Trix editor with
/// `data-embeds-url` (Action Text attachments). Serve it with [`direct_upload_script`].
pub const DIRECT_UPLOAD_JS: &str = include_str!("storage/direct_upload.js");

/// Serves [`DIRECT_UPLOAD_JS`] at `GET /ocre/direct-upload.js`: merge it into
/// the routes, then load it in the layout with
/// `<script src="/ocre/direct-upload.js" defer></script>`.
///
/// # Examples
///
/// ```
/// use axum::Router;
/// use ocre::Ctx;
///
/// fn routes() -> Router<Ctx> {
///     Router::new().merge(ocre::storage::direct_upload_script())
/// }
/// # let _ = routes;
/// ```
pub fn direct_upload_script<S: Clone + Send + Sync + 'static>() -> axum::Router<S> {
    let script = || async {
        let headers =
            [(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=3600")];
        (headers, DIRECT_UPLOAD_JS)
    };
    axum::Router::new().route("/ocre/direct-upload.js", axum::routing::get(script))
}

fn data_response(data: Bytes, filename: &str, content_type: &str, disposition: Disposition) -> Response {
    let filename = sanitize_filename(filename);
    let content_type = essence(content_type);
    let binary = BINARY_TYPES.contains(&content_type.as_str());
    let sent_type = if binary { "application/octet-stream" } else { &content_type };
    let disposition = content_disposition(disposition, &filename, &content_type);
    let length = HeaderValue::from(data.len());
    let mut response = Response::new(Body::from(data));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, header_value(sent_type));
    headers.insert(header::CONTENT_DISPOSITION, header_value(&disposition));
    headers.insert(header::CONTENT_LENGTH, length);
    response
}

/// `<prefix>/<22 random URL-safe characters>` (128 random bits).
pub(crate) fn new_key(prefix: &str) -> String {
    let id = URL_SAFE_NO_PAD.encode(random_bytes::<16>());
    match prefix.trim_matches('/') {
        "" => id,
        prefix => format!("{prefix}/{id}"),
    }
}

/// Longest kept file name, in characters.
const MAX_FILENAME: usize = 200;

/// The last path segment (browsers on Windows used to send `C:\...`),
/// without control characters, trimmed, at most 200 characters with the
/// extension kept; `file` when nothing is left.
pub(crate) fn sanitize_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = base.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    if clean.is_empty() || clean == "." || clean == ".." {
        return "file".to_owned();
    }
    if clean.chars().count() <= MAX_FILENAME {
        return clean.to_owned();
    }
    let extension = clean.rsplit_once('.').map(|(_, ext)| ext).filter(|ext| ext.chars().count() <= 16);
    match extension {
        Some(ext) => {
            let stem: String = clean.chars().take(MAX_FILENAME - ext.chars().count() - 1).collect();
            format!("{stem}.{ext}")
        }
        None => clean.chars().take(MAX_FILENAME).collect(),
    }
}

/// `Image/PNG; charset=x` -> `image/png`; empty -> `application/octet-stream`.
pub(crate) fn essence(content_type: &str) -> String {
    let essence = content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    if essence.is_empty() { "application/octet-stream".to_owned() } else { essence }
}

/// Types a browser displays without running scripts in the app's origin
/// (Rails' `content_types_allowed_inline`, plus common media).
const INLINE_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/bmp",
    "image/tiff",
    "image/vnd.microsoft.icon",
    "image/x-icon",
    "application/pdf",
    "text/plain",
    "audio/mpeg",
    "audio/ogg",
    "audio/wav",
    "audio/webm",
    "video/mp4",
    "video/ogg",
    "video/webm",
];

/// Types a browser could run as a page or script: served as
/// `application/octet-stream`, like Rails' `content_types_to_serve_as_binary`.
const BINARY_TYPES: &[&str] = &[
    "text/html",
    "text/javascript",
    "text/xml",
    "application/xml",
    "application/xhtml+xml",
    "application/javascript",
    "application/mathml+xml",
    "image/svg+xml",
    "text/cache-manifest",
];

/// `inline` or `attachment`, with the file name as ASCII (`filename=`) and,
/// when that loses characters, UTF-8 (`filename*=`, RFC 6266).
pub(crate) fn content_disposition(disposition: Disposition, filename: &str, content_type: &str) -> String {
    let kind = match disposition {
        Disposition::Inline if INLINE_TYPES.contains(&content_type) => "inline",
        _ => "attachment",
    };
    let ascii: String = filename
        .chars()
        .map(|c| if c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\' { c } else { '_' })
        .collect();
    let mut value = format!("{kind}; filename=\"{ascii}\"");
    if ascii != filename {
        value.push_str("; filename*=UTF-8''");
        for byte in filename.bytes() {
            if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
                value.push(byte as char);
            } else {
                write!(value, "%{byte:02X}").expect("writing to a String");
            }
        }
    }
    value
}

/// Which bytes a `Range` header asks for, checked against the file size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ByteRange {
    /// No (usable) range: the whole file, status 200.
    Full,
    /// `length` bytes from `offset`, status 206.
    Partial { offset: u64, length: u64 },
    /// Starts past the end: status 416.
    Unsatisfiable,
}

/// One `bytes=a-b`, `bytes=a-` or `bytes=-n` range (RFC 9110). Several
/// ranges, other units and malformed values are ignored: the whole file is
/// sent, which the RFC allows.
pub(crate) fn byte_range(header: Option<&str>, size: u64) -> ByteRange {
    let Some(spec) = header.and_then(|value| value.trim().strip_prefix("bytes=")) else {
        return ByteRange::Full;
    };
    let Some((start, end)) = spec.split_once('-').filter(|_| !spec.contains(',')) else {
        return ByteRange::Full;
    };
    let (start, end) = (start.trim(), end.trim());
    let parse = |text: &str| text.parse::<u64>().ok();
    if start.is_empty() {
        return match parse(end) {
            Some(0) => ByteRange::Unsatisfiable,
            Some(_) if size == 0 => ByteRange::Unsatisfiable,
            Some(suffix) => ByteRange::Partial { offset: size - suffix.min(size), length: suffix.min(size) },
            None => ByteRange::Full,
        };
    }
    let Some(offset) = parse(start) else { return ByteRange::Full };
    let last = if end.is_empty() {
        size.saturating_sub(1)
    } else {
        match parse(end) {
            Some(last) if last >= offset => last.min(size.saturating_sub(1)),
            _ => return ByteRange::Full,
        }
    };
    if offset >= size { ByteRange::Unsatisfiable } else { ByteRange::Partial { offset, length: last - offset + 1 } }
}

/// The entity tag of `If-None-Match`, unquoted, for R2's `etagDoesNotMatch`.
/// Only the first tag of a list is used; `*` is ignored.
pub(crate) fn if_none_match(header: Option<&str>) -> Option<String> {
    let first = header?.split(',').next()?.trim();
    let tag = first.strip_prefix("W/").unwrap_or(first).trim_matches('"');
    (!tag.is_empty() && tag != "*").then(|| tag.to_owned())
}

/// What [`serve`] asks R2 for, read from the request headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fetch {
    pub range: ByteRange,
    pub if_none_match: Option<String>,
}

impl Fetch {
    /// `If-Range` is not compared: when present, the whole file is sent.
    pub(crate) fn from_headers(headers: &HeaderMap, size: i64) -> Self {
        let text = |name| headers.get(name).and_then(|value: &HeaderValue| value.to_str().ok());
        let size = u64::try_from(size).unwrap_or(0);
        let range = if headers.contains_key(header::IF_RANGE) {
            ByteRange::Full
        } else {
            byte_range(text(header::RANGE), size)
        };
        Self { range, if_none_match: if_none_match(text(header::IF_NONE_MATCH)) }
    }
}

fn header_value(text: &str) -> HeaderValue {
    HeaderValue::from_str(text).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))
}

/// A file response: 200 with the whole body, or 206 with `range`.
pub(crate) fn file_response(
    attachment: &Attachment,
    disposition: Disposition,
    etag: &str,
    range: ByteRange,
    body: Body,
) -> Response {
    let size = u64::try_from(attachment.size).unwrap_or(0);
    let content_type = if BINARY_TYPES.contains(&attachment.content_type.as_str()) {
        "application/octet-stream"
    } else {
        &attachment.content_type
    };
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, header_value(content_type));
    headers.insert(
        header::CONTENT_DISPOSITION,
        header_value(&content_disposition(disposition, &attachment.filename, &attachment.content_type)),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(CACHE_CONTROL));
    headers.insert(header::ETAG, header_value(etag));
    let length = match range {
        ByteRange::Partial { offset, length } => {
            *response.status_mut() = StatusCode::PARTIAL_CONTENT;
            let last = offset + length - 1;
            response
                .headers_mut()
                .insert(header::CONTENT_RANGE, header_value(&format!("bytes {offset}-{last}/{size}")));
            length
        }
        _ => size,
    };
    response.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    response
}

/// `304 Not Modified`: the browser's copy is current.
pub(crate) fn not_modified(etag: &str) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NOT_MODIFIED;
    response.headers_mut().insert(header::ETAG, header_value(etag));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(CACHE_CONTROL));
    response
}

/// `416 Range Not Satisfiable`, with the file size in `Content-Range`.
pub(crate) fn unsatisfiable(size: i64) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
    response.headers_mut().insert(header::CONTENT_RANGE, header_value(&format!("bytes */{size}")));
    response
}

/// Error for a missing `STORAGE` binding, naming the cloudflare.config.ts entry.
pub(crate) fn missing_binding(detail: &str) -> crate::Error {
    crate::Error::internal(format!(
        "R2 binding `{STORAGE_BINDING}` is missing ({detail}). Fix: add `{STORAGE_BINDING}: bindings.r2({{ name: \"<app>-storage\" }}),` to worker.env in cloudflare.config.ts\n(`ocre g scaffold <Model> <name>:attachment` adds it; `ocre deploy` creates the bucket)"
    ))
}

#[cfg(test)]
#[path = "../tests/storage.rs"]
mod tests;
