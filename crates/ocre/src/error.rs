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
/// Internal details are logged to the Worker logs (Workers Logs), never
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
/// | [`Conflict`](Self::Conflict) | 409 | its message |
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
    /// 409 Conflict: the record changed since it was read (optimistic
    /// locking with a `lock_version` column), or a unique key already
    /// exists; the message is shown to the user.
    ///
    /// Generated `update` functions of models with a `lock_version` field
    /// return it for a stale version (Rails' `ActiveRecord::StaleObjectError`).
    Conflict(String),
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

/// What a client may see of an [`Error`], plus the internal message of a 500 (never sent).
pub(crate) struct Public {
    pub status: StatusCode,
    pub message: String,
    pub fields: Vec<FieldError>,
    pub internal: Option<String>,
}

/// Response extension of a 500 built from [`Error::Internal`]: `serve`
/// logs and reports the message with the request's details, and in debug
/// builds shows it on the development error page.
#[derive(Debug, Clone)]
pub(crate) struct InternalError(pub String);

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

    /// Whether the error says a unique value is already used: a "has
    /// already been taken" validation error (generated models check unique
    /// fields before writing), D1's `UNIQUE constraint failed` (two requests
    /// raced past the check), or a [`Conflict`](Self::Conflict).
    ///
    /// [`Query::create_or_first`](crate::Query::create_or_first) uses it to
    /// fall back to the existing row.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Error, FieldError};
    ///
    /// assert!(Error::Invalid(vec![FieldError::new("email", "has already been taken")]).is_taken());
    /// assert!(Error::internal("D1_ERROR: UNIQUE constraint failed: users.email").is_taken());
    /// assert!(!Error::Invalid(vec![FieldError::new("email", "can't be blank")]).is_taken());
    /// assert!(!Error::NotFound.is_taken());
    /// ```
    pub fn is_taken(&self) -> bool {
        match self {
            Self::Invalid(fields) => fields.iter().any(|field| field.message == "has already been taken"),
            Self::Internal(message) => message.contains("UNIQUE constraint failed"),
            Self::Conflict(_) => true,
            _ => false,
        }
    }

    /// Client-safe form: internal details move to [`Public::internal`] and
    /// are replaced by a generic message.
    pub(crate) fn into_public(self) -> Public {
        let mut internal = None;
        let (status, message, fields) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "Not found".to_owned(), vec![]),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message, vec![]),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized".to_owned(), vec![]),
            Self::Forbidden => (StatusCode::FORBIDDEN, "Forbidden".to_owned(), vec![]),
            Self::Invalid(fields) => (StatusCode::UNPROCESSABLE_ENTITY, "Validation failed".to_owned(), fields),
            Self::PayloadTooLarge(message) => (StatusCode::PAYLOAD_TOO_LARGE, message, vec![]),
            Self::Conflict(message) => (StatusCode::CONFLICT, message, vec![]),
            Self::TooManyRequests => {
                (StatusCode::TOO_MANY_REQUESTS, "Too many requests. Try again later.".to_owned(), vec![])
            }
            Self::Internal(message) => {
                internal = Some(message);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_owned(), vec![])
            }
        };
        Public { status, message, fields, internal }
    }
}

/// Marks `response` with [`InternalError`] when it answers an
/// [`Error::Internal`] (`internal` from [`Public::internal`]), for `serve` to report.
pub(crate) fn mark(internal: Option<String>, response: &mut axum::response::Response) {
    if let Some(message) = internal {
        response.extensions_mut().insert(InternalError(message));
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
            Self::Conflict(message) => write!(f, "conflict: {message}"),
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

/// Logs `[ocre] <message>` as an `error` line (Workers Logs in the
/// dashboard; stderr in native unit tests), for failures outside a request's
/// error reporting.
pub(crate) fn log_internal(message: &str) {
    crate::log::Logger::new().error(format_args!("[ocre] {message}"));
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
