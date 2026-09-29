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

/// Extractor for a `multipart/form-data` body of at most `LIMIT` bytes.
///
/// It reads HTML forms with `enctype="multipart/form-data"` (file inputs)
/// and API clients (`curl -F avatar=@me.png`). Take the text fields with
/// [`MultipartForm::form`] or [`MultipartForm::text`] and the files with
/// [`MultipartForm::file`]; check files with
/// [`Validator::file`](crate::Validator::file) before storing them.
///
/// The whole body is read into memory, then split in place: file parts are
/// slices of that one buffer. The rejection is a response, not an
/// [`Error`] in the handler:
///
/// - 400 when the content type is not `multipart/form-data` with a
///   `boundary`, the body cannot be read, or it is malformed;
/// - 413 "The request is too large (maximum is 6 MB)" when `Content-Length`
///   announces more than `LIMIT` (before anything is read) or as soon as the
///   body passes it.
///
/// Errors are HTML pages for browsers (`Accept: text/html`, feature `html`)
/// and JSON (like [`ApiError`](crate::ApiError)) otherwise.
///
/// Free plan: keep `LIMIT` in the tens of MB. A Worker has 128 MB, the body
/// is held in memory while R2 gets a copy, and Cloudflare refuses request
/// bodies over 100 MB on the Free plan before they reach the Worker.
/// Splitting takes about 1.2 ms of CPU per 10 MB in WebAssembly (memchr's
/// substring search, measured in V8). For a single large file, stream the
/// raw body with [`store_body`](crate::storage::store_body) instead.
///
/// # Examples
///
/// ```no_run
/// use axum::{extract::State, response::Redirect};
/// use ocre::storage::{self, Multipart, Rules};
/// use ocre::{Ctx, Result, Validator};
/// use serde::Deserialize;
///
/// const IMAGE: Rules = Rules { max_bytes: 10 * 1024 * 1024, content_types: &["image/png", "image/jpeg"] };
/// const FORM_LIMIT: usize = IMAGE.max_bytes + 1024 * 1024;
///
/// #[derive(Deserialize)]
/// struct NewPhoto {
///     title: String,
/// }
///
/// async fn create(State(ctx): State<Ctx>, Multipart(mut form): Multipart<FORM_LIMIT>) -> Result<Redirect> {
///     let fields: NewPhoto = form.form()?;
///     let image = form.file("image"); // None when no file was chosen
///     let mut v = Validator::new();
///     v.required("title", &fields.title);
///     if let Some(image) = &image {
///         v.file("image", image, &IMAGE);
///     }
///     v.finish()?;
///     if let Some(image) = image {
///         let stored = storage::store(&ctx, "photos/image", image).await?;
///         # let _ = stored;
///     }
///     Ok(Redirect::to("/photos"))
/// }
/// ```
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

/// The parsed parts of a multipart body: text fields and files, by field name.
///
/// Obtained from the [`Multipart`] extractor. Text fields are read with
/// [`MultipartForm::form`] (all at once, into a struct) or
/// [`MultipartForm::text`] (one by one); files are taken with
/// [`MultipartForm::file`]. Parts without a `name` are skipped; a text
/// field that is not UTF-8 is a 400.
///
/// # Examples
///
/// ```
/// use ocre::storage::MultipartForm;
///
/// // An empty form, as `Default` builds it.
/// let mut form = MultipartForm::default();
/// assert_eq!(form.text("title"), None);
/// assert!(form.file("image").is_none());
/// ```
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MultipartForm {
    fields: Vec<(String, String)>,
    files: Vec<(String, Upload)>,
}

impl MultipartForm {
    /// Deserializes the text fields into `T`, exactly like axum's `Form`.
    ///
    /// Numbers and booleans are parsed from their text, `#[serde(default)]`
    /// fills in missing fields, and file fields are ignored. Unchecked HTML
    /// checkboxes send nothing, so give `bool` fields `#[serde(default)]`.
    ///
    /// # Errors
    ///
    /// [`Error::BadRequest`] (400) `Invalid form: ...` when a field is missing
    /// or `T` cannot parse it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use axum::{body::Body, extract::FromRequest, http::Request};
    /// # use ocre::storage::Multipart;
    /// # use std::task::{Context, Poll, Waker};
    /// # fn block_on<F: Future>(future: F) -> F::Output {
    /// #     match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
    /// #         Poll::Ready(output) => output,
    /// #         Poll::Pending => unreachable!("an in-memory body is always ready"),
    /// #     }
    /// # }
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct Profile {
    ///     name: String,
    ///     age: u8,
    ///     #[serde(default)]
    ///     public: bool,
    /// }
    ///
    /// let body = "--x\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nAda\r\n\
    ///             --x\r\nContent-Disposition: form-data; name=\"age\"\r\n\r\n36\r\n--x--\r\n";
    /// let request = Request::post("/profile")
    ///     .header("content-type", "multipart/form-data; boundary=x")
    ///     .body(Body::from(body))
    ///     .unwrap();
    /// let Multipart(form) = block_on(Multipart::<1024>::from_request(request, &())).unwrap();
    ///
    /// let profile: Profile = form.form()?;
    /// assert_eq!((profile.name.as_str(), profile.age, profile.public), ("Ada", 36, false));
    ///
    /// #[derive(Debug, Deserialize)]
    /// struct Strict {
    ///     #[allow(dead_code)]
    ///     email: String,
    /// }
    /// let err = form.form::<Strict>().unwrap_err();
    /// assert!(matches!(err, ocre::Error::BadRequest(message) if message == "Invalid form: missing field `email`"));
    /// # Ok::<(), ocre::Error>(())
    /// ```
    pub fn form<T: DeserializeOwned>(&self) -> Result<T> {
        serde_urlencoded::from_str(&self.encoded()).map_err(|err| Error::bad_request(format!("Invalid form: {err}")))
    }

    fn encoded(&self) -> String {
        serde_urlencoded::to_string(&self.fields).expect("string pairs always encode")
    }

    /// Returns the first text field named `name`, or `None` when it was not sent.
    ///
    /// File fields are not text fields: use [`MultipartForm::file`] for them.
    ///
    /// # Examples
    ///
    /// ```
    /// # use axum::{body::Body, extract::FromRequest, http::Request};
    /// # use ocre::storage::Multipart;
    /// # use std::task::{Context, Poll, Waker};
    /// # fn block_on<F: Future>(future: F) -> F::Output {
    /// #     match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
    /// #         Poll::Ready(output) => output,
    /// #         Poll::Pending => unreachable!("an in-memory body is always ready"),
    /// #     }
    /// # }
    /// let body = "--x\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nHoliday\r\n--x--\r\n";
    /// let request = Request::post("/photos")
    ///     .header("content-type", "multipart/form-data; boundary=x")
    ///     .body(Body::from(body))
    ///     .unwrap();
    /// let Multipart(form) = block_on(Multipart::<1024>::from_request(request, &())).unwrap();
    /// assert_eq!(form.text("title"), Some("Holiday"));
    /// assert_eq!(form.text("missing"), None);
    /// ```
    pub fn text(&self, name: &str) -> Option<&str> {
        self.fields.iter().find(|(field, _)| field == name).map(|(_, value)| value.as_str())
    }

    /// Takes the first file sent as `name`, leaving the others.
    ///
    /// `None` when the field is missing or no file was chosen (browsers then
    /// send an empty, nameless file, which is dropped while parsing). Taking
    /// moves the [`Upload`] out without copying its bytes; a second call for
    /// the same name returns the next file with that name, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// # use axum::{body::Body, extract::FromRequest, http::Request};
    /// # use ocre::storage::Multipart;
    /// # use std::task::{Context, Poll, Waker};
    /// # fn block_on<F: Future>(future: F) -> F::Output {
    /// #     match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
    /// #         Poll::Ready(output) => output,
    /// #         Poll::Pending => unreachable!("an in-memory body is always ready"),
    /// #     }
    /// # }
    /// let body = "--x\r\n\
    ///     Content-Disposition: form-data; name=\"image\"; filename=\"C:\\\\photos\\\\beach.png\"\r\n\
    ///     Content-Type: Image/PNG\r\n\r\n\
    ///     PNG...\r\n\
    ///     --x\r\n\
    ///     Content-Disposition: form-data; name=\"notes\"; filename=\"\"\r\n\
    ///     Content-Type: application/octet-stream\r\n\r\n\
    ///     \r\n\
    ///     --x--\r\n";
    /// let request = Request::post("/photos")
    ///     .header("content-type", "multipart/form-data; boundary=x")
    ///     .body(Body::from(body))
    ///     .unwrap();
    /// let Multipart(mut form) = block_on(Multipart::<1024>::from_request(request, &())).unwrap();
    ///
    /// let image = form.file("image").unwrap();
    /// assert_eq!(image.filename, "beach.png");
    /// assert_eq!(image.content_type, "image/png");
    /// assert_eq!(image.size(), 6);
    /// assert!(form.file("image").is_none()); // taken
    /// assert!(form.file("notes").is_none()); // no file chosen
    /// ```
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
