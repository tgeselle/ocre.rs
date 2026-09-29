use axum::http::StatusCode;

use crate::FieldError;

/// `Result` with [`Error`] as the default error type, returned by Ocre APIs and handlers.
///
/// # Examples
///
/// ```
/// fn parse_id(text: &str) -> ocre::Result<i64> {
///     text.parse().map_err(|_| ocre::Error::bad_request("id must be a number"))
/// }
///
/// assert_eq!(parse_id("7").unwrap(), 7);
/// assert!(parse_id("x").is_err());
/// ```
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Handler error: an HTTP status plus, for client errors, a message the user may see.
///
/// Internal details are logged to the Worker logs (`wrangler tail`), never
/// sent to the client. With the `html` feature it renders as an HTML error
/// page (validation errors listed as full messages); JSON endpoints return
/// [`ApiError`](crate::ApiError), which converts from it with `?`:
/// `{"error": {"status": 404, "message": "Not found"}}`. `?` also converts a
/// [`worker::Error`] into [`Error::Internal`].
///
/// | Variant | Status | Message sent |
/// |---|---|---|
/// | [`NotFound`](Self::NotFound) | 404 | `Not found` |
/// | [`BadRequest`](Self::BadRequest) | 400 | its message |
/// | [`Unauthorized`](Self::Unauthorized) | 401 | `Unauthorized` |
/// | [`Forbidden`](Self::Forbidden) | 403 | `Forbidden` |
/// | [`Invalid`](Self::Invalid) | 422 | `Validation failed` plus the field errors |
/// | [`PayloadTooLarge`](Self::PayloadTooLarge) | 413 | its message |
/// | [`TooManyRequests`](Self::TooManyRequests) | 429 | `Too many requests. Try again later.` |
/// | [`Internal`](Self::Internal) | 500 | `Internal server error` (message logged) |
///
/// # Examples
///
/// ```
/// use ocre::{Error, OptionExt, Result};
///
/// fn find(id: i64) -> Result<&'static str> {
///     match id {
///         0 => Err(Error::bad_request("id must be positive")),
///         1 => Ok("First post"),
///         _ => None.or_404(),
///     }
/// }
///
/// assert_eq!(find(1).unwrap(), "First post");
/// assert!(matches!(find(2), Err(Error::NotFound)));
/// assert_eq!(find(0).unwrap_err().to_string(), "bad request: id must be positive");
/// ```
#[derive(Debug)]
pub enum Error {
    /// 404 Not Found: the record or route does not exist.
    NotFound,
    /// 400 Bad Request, with a message shown to the user.
    BadRequest(String),
    /// 401 Unauthorized: missing or invalid credentials (password, session, token).
    ///
    /// JSON responses add `WWW-Authenticate: Bearer`.
    Unauthorized,
    /// 403 Forbidden: signed in, but not allowed to do this.
    Forbidden,
    /// 422 Unprocessable Entity: failed validations, one entry per field error.
    ///
    /// Built by [`Validator::finish`](crate::Validator::finish). JSON answers
    /// group messages by field: `"fields": {"title": ["can't be blank"]}`.
    Invalid(Vec<FieldError>),
    /// 413 Payload Too Large: the request body is over a limit, with a message shown to the user.
    ///
    /// See [`storage::Multipart`](crate::storage::Multipart).
    PayloadTooLarge(String),
    /// 429 Too Many Requests: a rate limit was hit.
    ///
    /// Returned by [`security::rate_limit`](crate::security::rate_limit).
    TooManyRequests,
    /// 500 Internal Server Error; the message goes to the Worker logs only.
    ///
    /// Logged as `[ocre] <message>` when the response is built; the client
    /// sees `Internal server error`.
    Internal(String),
}

/// What a client may see of an [`Error`].
pub(crate) struct Public {
    pub status: StatusCode,
    pub message: String,
    pub fields: Vec<FieldError>,
}

impl Public {
    /// `{"title": ["can't be blank", ...], ...}`, the shape Rails APIs use.
    pub fn fields_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for field in &self.fields {
            let messages = map.entry(field.field.clone()).or_insert_with(|| serde_json::Value::Array(vec![]));
            messages.as_array_mut().expect("inserted as an array").push(field.message.clone().into());
        }
        serde_json::Value::Object(map)
    }
}

impl Error {
    /// Builds a 400 [`Error::BadRequest`] whose message is shown to the user.
    ///
    /// # Examples
    ///
    /// ```
    /// let err = ocre::Error::bad_request("limit must be a number");
    /// assert_eq!(err.to_string(), "bad request: limit must be a number");
    /// ```
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    /// Builds a 500 [`Error::Internal`] whose message is logged, never shown to the user.
    ///
    /// Name the fix in the message, as Ocre's own errors do.
    ///
    /// # Examples
    ///
    /// ```
    /// let err = ocre::Error::internal("KV binding `CACHE` is missing");
    /// assert!(matches!(&err, ocre::Error::Internal(message) if message.contains("CACHE")));
    /// ```
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    /// Client-safe form. Internal details are logged here and replaced by a
    /// generic message.
    pub(crate) fn into_public(self) -> Public {
        let (status, message, fields) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "Not found".to_owned(), vec![]),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message, vec![]),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized".to_owned(), vec![]),
            Self::Forbidden => (StatusCode::FORBIDDEN, "Forbidden".to_owned(), vec![]),
            Self::Invalid(fields) => (StatusCode::UNPROCESSABLE_ENTITY, "Validation failed".to_owned(), fields),
            Self::PayloadTooLarge(message) => (StatusCode::PAYLOAD_TOO_LARGE, message, vec![]),
            Self::TooManyRequests => {
                (StatusCode::TOO_MANY_REQUESTS, "Too many requests. Try again later.".to_owned(), vec![])
            }
            Self::Internal(message) => {
                log_internal(&message);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_owned(), vec![])
            }
        };
        Public { status, message, fields }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("not found"),
            Self::BadRequest(message) => write!(f, "bad request: {message}"),
            Self::Unauthorized => f.write_str("unauthorized"),
            Self::Forbidden => f.write_str("forbidden"),
            Self::Invalid(fields) => {
                let messages: Vec<String> = fields.iter().map(FieldError::full_message).collect();
                write!(f, "invalid: {}", messages.join(", "))
            }
            Self::PayloadTooLarge(message) => write!(f, "payload too large: {message}"),
            Self::TooManyRequests => f.write_str("too many requests"),
            Self::Internal(message) => write!(f, "internal error: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<worker::Error> for Error {
    fn from(err: worker::Error) -> Self {
        Self::Internal(err.to_string())
    }
}

/// Worker logs (visible in `wrangler tail`); stderr in native unit tests.
pub(crate) fn log_internal(message: &str) {
    #[cfg(target_arch = "wasm32")]
    worker::console_error!("[ocre] {message}");
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("[ocre] {message}");
}

/// Extension for `Option`: `option.or_404()?` turns a missing record into a 404 response.
///
/// # Examples
///
/// ```
/// use ocre::{Error, OptionExt};
///
/// assert_eq!(Some(3).or_404().unwrap(), 3);
/// assert!(matches!(None::<i32>.or_404(), Err(Error::NotFound)));
/// ```
pub trait OptionExt<T> {
    /// The value, or [`Error::NotFound`] (404) when `None`.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the option is `None`.
    fn or_404(self) -> Result<T>;
}

impl<T> OptionExt<T> for Option<T> {
    fn or_404(self) -> Result<T> {
        self.ok_or(Error::NotFound)
    }
}

#[cfg(test)]
#[path = "../tests/error.rs"]
mod tests;
