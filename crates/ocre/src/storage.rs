//! Files in Cloudflare R2: uploads from HTML forms and API clients, stored
//! under generated keys and served back through the Worker.
//!
//! ```ignore
//! use ocre::storage::{self, Disposition, Multipart, Rules};
//!
//! const AVATAR: Rules = Rules { max_bytes: 5 * 1024 * 1024, content_types: &["image/png", "image/jpeg"] };
//! const FORM_LIMIT: usize = AVATAR.max_bytes + 1024 * 1024;
//!
//! async fn upload(State(ctx): State<Ctx>, Multipart(mut form): Multipart<FORM_LIMIT>) -> ocre::Result<String> {
//!     let upload = form.file("avatar").ok_or_else(|| Error::bad_request("choose a file"))?;
//!     let mut v = Validator::new();
//!     v.file("avatar", &upload, &AVATAR).finish()?;
//!     let attachment = storage::store(&ctx, "avatars", upload).await?;
//!     Ok(attachment.key) // save it with the record: key, filename, content_type, size
//! }
//!
//! async fn download(State(ctx): State<Ctx>, headers: HeaderMap) -> ocre::Result<Response> {
//!     let attachment = /* loaded from the record */;
//!     storage::serve(&ctx, &attachment, &headers, Disposition::Inline).await
//! }
//! ```
//!
//! Every file lives in the R2 bucket bound as `STORAGE` in wrangler.toml
//! (`ocre g scaffold Post avatar:attachment` adds the binding, `ocre dev`
//! keeps a local copy under `.wrangler/state`, `ocre deploy` creates the
//! bucket). Keys are random (`<prefix>/<22 characters>`) and never reused, so
//! a stored object never changes: replacing a file means storing a new key
//! and deleting the old one.

mod multipart;

use std::fmt::Write as _;

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

pub use crate::runtime::storage::{delete, delete_attachments, read, serve, store, store_body, store_bytes};
use crate::{IntoParam, Param, Validator, token::random_bytes};
pub use multipart::{Multipart, MultipartForm};

/// Name of the R2 binding holding every file: `[[r2_buckets]] binding = "STORAGE"`.
pub const STORAGE_BINDING: &str = "STORAGE";

/// `Cache-Control` of served files: browsers keep them but ask again each
/// time, and get a body-less `304 Not Modified` while the `ETag` matches.
/// Override it on the returned response for files that may be cached longer.
pub const CACHE_CONTROL: &str = "private, no-cache";

/// A stored file, as saved with its record in four columns
/// (`avatar_key`, `avatar_filename`, `avatar_content_type`, `avatar_size`).
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
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Object key in the R2 bucket.
    pub key: String,
    /// The uploader's file name, without directories or control characters.
    pub filename: String,
    /// `image/png`: lowercase, without parameters.
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

    /// The size for people: `512 bytes`, `2 KB`, `1.5 MB`.
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
/// Get it with [`MultipartForm::file`], check it with
/// [`Validator::file`](crate::Validator::file), store it with [`store`].
///
/// ```
/// use ocre::storage::Upload;
///
/// let upload = Upload { filename: "notes.txt".into(), content_type: "text/plain".into(), bytes: "hi".into() };
/// assert_eq!(upload.size(), 2);
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Upload {
    /// File name sent by the browser, cleaned up (no directories or control characters).
    pub filename: String,
    /// Content type sent by the browser, lowercase without parameters
    /// (`application/octet-stream` when none was sent). The client chooses
    /// it: check it against an allowlist with [`Validator::file`](crate::Validator::file).
    pub content_type: String,
    /// The file's bytes.
    pub bytes: axum::body::Bytes,
}

impl Upload {
    /// Size in bytes.
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// What a file field accepts, checked by [`Validator::file`](crate::Validator::file).
/// A `const`, so the request limit can be computed from it:
///
/// ```
/// use ocre::storage::Rules;
///
/// const AVATAR: Rules = Rules { max_bytes: 5 * 1024 * 1024, content_types: &["image/png", "image/jpeg"] };
/// const FORM_LIMIT: usize = AVATAR.max_bytes + 1024 * 1024;
/// assert!(AVATAR.allows("image/PNG"));
/// assert!(!AVATAR.allows("image/svg+xml"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// Largest accepted file, in bytes.
    pub max_bytes: usize,
    /// Accepted content types, exact and lowercase (`image/png`). There is no
    /// wildcard: `image/*` would admit SVG, which can carry scripts.
    pub content_types: &'static [&'static str],
}

impl Rules {
    /// Whether `content_type` (parameters and case ignored) is in the list.
    pub fn allows(&self, content_type: &str) -> bool {
        let essence = essence(content_type);
        self.content_types.contains(&essence.as_str())
    }
}

impl Validator {
    /// An uploaded file within `rules`: "is too large (maximum is 5 MB)" and
    /// "has an unsupported type (allowed: image/png, image/jpeg)".
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
    /// ```
    pub fn file(&mut self, field: &str, upload: &Upload, rules: &Rules) -> &mut Self {
        let too_large = upload.size() > rules.max_bytes as u64;
        self.check(field, too_large, format!("is too large (maximum is {})", human_size(rules.max_bytes as u64)));
        let allowed = rules.content_types.join(", ");
        self.check(field, !rules.allows(&upload.content_type), format!("has an unsupported type (allowed: {allowed})"))
    }
}

/// How [`serve`] asks the browser to handle a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Show it in the page or tab when its type is safe to display (images,
    /// PDF, plain text, audio, video); download anything else, so an uploaded
    /// HTML or SVG file never runs as part of the app.
    Inline,
    /// Always download, with the original file name.
    Download,
}

/// Query parameters for the columns of one attachment, in column order
/// (`<name>_key, <name>_filename, <name>_content_type, <name>_size`); `None`
/// binds four NULLs. Generated `create` functions extend their `params!` with it.
///
/// ```
/// use ocre::{params, storage::{self, Attachment}};
///
/// let avatar = Attachment { key: "k".into(), filename: "a.png".into(), content_type: "image/png".into(), size: 3 };
/// let mut values = params!["Ada"];
/// values.extend(storage::columns(Some(&avatar)));
/// assert_eq!(values, params!["Ada", "k", "a.png", "image/png", 3_i64]);
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

/// Parameters for an `UPDATE` of one attachment's columns: a flag (whether
/// they change), then the four [`columns`]. `None` keeps the file,
/// `Some(None)` clears the columns, `Some(Some(file))` points them to `file`.
/// The generated SQL reads `avatar_key = CASE WHEN ?1 THEN ?2 ELSE avatar_key END, ...`.
///
/// ```
/// use ocre::{params, storage};
///
/// assert_eq!(storage::column_changes(None)[0], params![false][0]);
/// assert_eq!(storage::column_changes(Some(None))[0], params![true][0]);
/// ```
pub fn column_changes(change: Option<Option<&Attachment>>) -> [Param; 5] {
    let [key, filename, content_type, size] = columns(change.flatten());
    [change.is_some().into_param(), key, filename, content_type, size]
}

/// Bytes for people, in powers of 1024 like Rails' `number_to_human_size`:
/// `512 bytes`, `2 KB`, `1.5 MB`, `10 GB`.
///
/// ```
/// assert_eq!(ocre::storage::human_size(10 * 1024 * 1024), "10 MB");
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

/// Error for a missing `STORAGE` binding, naming the wrangler.toml entry.
pub(crate) fn missing_binding(detail: &str) -> crate::Error {
    crate::Error::internal(format!(
        "R2 binding `{STORAGE_BINDING}` is missing ({detail}). Fix: add to wrangler.toml\n[[r2_buckets]]\nbinding = \"{STORAGE_BINDING}\"\nbucket_name = \"<app>-storage\"\n(`ocre g scaffold <Model> <name>:attachment` adds it; `ocre deploy` creates the bucket)"
    ))
}

#[cfg(test)]
#[path = "../tests/storage.rs"]
mod tests;
