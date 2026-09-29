//! `multipart/form-data` bodies (RFC 7578): the whole body is read (up to a
//! limit), then split in place, so file parts are slices of one buffer.

use std::pin::Pin;

use axum::{
    body::{Bytes, HttpBody},
    extract::{FromRequest, Request},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use memchr::memmem;
use serde::de::DeserializeOwned;

use super::{Upload, essence, human_size, sanitize_filename};
use crate::{ApiError, Error, Result};

/// Extractor for a `multipart/form-data` body of at most `LIMIT` bytes: HTML
/// forms with `enctype="multipart/form-data"` (file inputs) and API clients
/// (`curl -F avatar=@me.png`). Take the text fields with
/// [`MultipartForm::form`] and the files with [`MultipartForm::file`].
///
/// ```ignore
/// const FORM_LIMIT: usize = 6 * 1024 * 1024;
///
/// async fn create(State(ctx): State<Ctx>, Multipart(mut form): Multipart<FORM_LIMIT>) -> ocre::Result<Redirect> {
///     let fields: NewPostForm = form.form()?;
///     let avatar = form.file("avatar"); // None when no file was chosen
///     // ...
/// }
/// ```
///
/// A larger body is refused with 413 before it is read (from
/// `Content-Length`) or as soon as it passes the limit. Keep `LIMIT` well
/// under the 128 MB a Worker may use: the body is held in memory, and R2
/// gets a copy. Cloudflare refuses request bodies over 100 MB on the Free and
/// Pro plans before they reach the Worker. Errors are HTML pages for
/// browsers (`Accept: text/html`, feature `html`) and JSON otherwise.
///
/// CPU: splitting a 10 MB body takes about 1.2 ms in WebAssembly (memchr's
/// substring search, measured in V8), far below the free plan's 10 ms.
#[derive(Debug)]
pub struct Multipart<const LIMIT: usize>(pub MultipartForm);

impl<const LIMIT: usize, S: Send + Sync> FromRequest<S> for Multipart<LIMIT> {
    type Rejection = Response;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let html = wants_html(req.headers());
        read(req, LIMIT).await.map(Self).map_err(|err| rejection(err, html))
    }
}

/// Browsers ask for HTML; API clients (`curl`, `fetch`) do not.
fn wants_html(headers: &HeaderMap) -> bool {
    headers.get(header::ACCEPT).and_then(|value| value.to_str().ok()).is_some_and(|accept| accept.contains("text/html"))
}

fn rejection(err: Error, html: bool) -> Response {
    #[cfg(feature = "html")]
    if html {
        return err.into_response();
    }
    let _ = html;
    ApiError(err).into_response()
}

fn too_large(limit: usize) -> Error {
    Error::PayloadTooLarge(format!("The request is too large (maximum is {})", human_size(limit as u64)))
}

/// Reads and parses the body of `req`, refusing more than `limit` bytes.
async fn read(req: Request, limit: usize) -> Result<MultipartForm> {
    let content_type = req.headers().get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or("");
    let boundary = boundary(content_type).ok_or_else(|| {
        Error::bad_request(
            "Expected a multipart/form-data body: give the <form> enctype=\"multipart/form-data\", or send it with `curl -F name=value`",
        )
    })?;
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > limit) {
        return Err(too_large(limit));
    }
    let mut body = req.into_body();
    let mut buffer = Vec::with_capacity(declared.unwrap_or(0));
    while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        let frame = frame.map_err(|err| Error::bad_request(format!("Could not read the request body: {err}")))?;
        // Trailer frames carry no data.
        let data = frame.into_data().unwrap_or_default();
        if buffer.len() + data.len() > limit {
            return Err(too_large(limit));
        }
        buffer.extend_from_slice(&data);
    }
    MultipartForm::parse(Bytes::from(buffer), &boundary)
}

/// The `boundary` parameter of a `multipart/form-data` content type.
pub(crate) fn boundary(content_type: &str) -> Option<String> {
    let mut parts = split_params(content_type).into_iter();
    if !parts.next()?.trim().eq_ignore_ascii_case("multipart/form-data") {
        return None;
    }
    parts
        .filter_map(|part| part.split_once('='))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("boundary"))
        .map(|(_, value)| unquote(value.trim()))
        .filter(|boundary| !boundary.is_empty() && boundary.len() <= 70)
}

/// Splits on `;` outside double quotes.
fn split_params(value: &str) -> Vec<&str> {
    let (mut parts, mut start, mut quoted, mut escaped) = (Vec::new(), 0, false, false);
    for (i, c) in value.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
}

/// `"a \"b\""` -> `a "b"`; unquoted values are returned as they are.
fn unquote(value: &str) -> String {
    let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
        return value.to_owned();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    out
}

/// The parts of a multipart body: text fields and files, by field name.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MultipartForm {
    fields: Vec<(String, String)>,
    files: Vec<(String, Upload)>,
}

impl MultipartForm {
    /// The text fields deserialized into `T`, exactly like axum's `Form`:
    /// numbers and booleans are parsed from their text, `#[serde(default)]`
    /// fills in missing ones. A field `T` cannot read is a 400.
    ///
    /// ```ignore
    /// #[derive(Deserialize)]
    /// struct Profile { name: String, #[serde(default)] public: bool }
    /// let profile: Profile = form.form()?;
    /// ```
    pub fn form<T: DeserializeOwned>(&self) -> Result<T> {
        serde_urlencoded::from_str(&self.encoded()).map_err(|err| Error::bad_request(format!("Invalid form: {err}")))
    }

    fn encoded(&self) -> String {
        serde_urlencoded::to_string(&self.fields).expect("string pairs always encode")
    }

    /// The first text field named `name`.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.fields.iter().find(|(field, _)| field == name).map(|(_, value)| value.as_str())
    }

    /// Takes the first file sent as `name`. `None` when the field is missing
    /// or no file was chosen (browsers then send an empty, nameless file).
    pub fn file(&mut self, name: &str) -> Option<Upload> {
        let index = self.files.iter().position(|(field, _)| field == name)?;
        Some(self.files.remove(index).1)
    }

    /// Splits `body` at `boundary`. Parts without a `name` are skipped.
    pub(crate) fn parse(body: Bytes, boundary: &str) -> Result<Self> {
        let malformed = |detail: &str| Error::bad_request(format!("Malformed multipart/form-data body: {detail}"));
        let delimiter = format!("\r\n--{boundary}");
        let finder = memmem::Finder::new(delimiter.as_bytes());
        // The first delimiter has no line break before it when there is no preamble.
        let mut pos = if body.starts_with(&delimiter.as_bytes()[2..]) {
            delimiter.len() - 2
        } else {
            finder.find(&body).ok_or_else(|| malformed("the boundary never appears"))? + delimiter.len()
        };
        let mut form = Self::default();
        loop {
            let rest = &body[pos..];
            if rest.starts_with(b"--") {
                return Ok(form);
            }
            // Transport padding (spaces, tabs) may follow a delimiter.
            let padding = rest.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
            if !rest[padding..].starts_with(b"\r\n") {
                return Err(malformed("expected a line break after the boundary"));
            }
            let start = pos + padding + 2;
            let (head, content_start) = if body[start..].starts_with(b"\r\n") {
                (&body[start..start], start + 2)
            } else {
                let end = memmem::find(&body[start..], b"\r\n\r\n")
                    .ok_or_else(|| malformed("a part has no end of headers"))?;
                (&body[start..start + end], start + end + 4)
            };
            let content_end = content_start
                + finder.find(&body[content_start..]).ok_or_else(|| malformed("the closing boundary is missing"))?;
            form.push(head, body.slice(content_start..content_end))?;
            pos = content_end + delimiter.len();
        }
    }

    fn push(&mut self, head: &[u8], content: Bytes) -> Result<()> {
        let head = String::from_utf8_lossy(head);
        let mut disposition = None;
        let mut content_type = None;
        for line in head.split("\r\n") {
            let Some((name, value)) = line.split_once(':') else { continue };
            if name.trim().eq_ignore_ascii_case("content-disposition") {
                disposition = Some(value.trim().to_owned());
            } else if name.trim().eq_ignore_ascii_case("content-type") {
                content_type = Some(value.trim().to_owned());
            }
        }
        let params = disposition.as_deref().map(split_params).unwrap_or_default();
        let param = |wanted: &str| {
            params.iter().skip(1).find_map(|part| {
                let (name, value) = part.split_once('=')?;
                name.trim().eq_ignore_ascii_case(wanted).then(|| unquote(value.trim()))
            })
        };
        let Some(name) = param("name") else { return Ok(()) };
        match param("filename") {
            Some(filename) if filename.is_empty() && content.is_empty() => {}
            Some(filename) => {
                let content_type = essence(content_type.as_deref().unwrap_or(""));
                self.files
                    .push((name, Upload { filename: sanitize_filename(&filename), content_type, bytes: content }));
            }
            None => {
                let value = String::from_utf8(content.to_vec())
                    .map_err(|_| Error::bad_request(format!("Form field `{name}` is not valid UTF-8")))?;
                self.fields.push((name, value));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/storage/multipart.rs"]
mod tests;
