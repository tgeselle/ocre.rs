use axum::http::StatusCode;

use crate::FieldError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Handler error. Internal details are logged, never sent to the client.
/// Renders as an HTML page (feature `html`); JSON endpoints return
/// [`ApiError`](crate::ApiError), which converts from it with `?`.
#[derive(Debug)]
pub enum Error {
    /// 404.
    NotFound,
    /// 400 with a message shown to the user.
    BadRequest(String),
    /// 401: missing or invalid credentials (password, session, token).
    /// JSON responses add `WWW-Authenticate: Bearer`.
    Unauthorized,
    /// 403: signed in, but not allowed to do this.
    Forbidden,
    /// 422: failed validations, one entry per field error (see [`Validator`](crate::Validator)).
    Invalid(Vec<FieldError>),
    /// 500; the message goes to the Worker logs only.
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
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

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
fn log_internal(message: &str) {
    #[cfg(target_arch = "wasm32")]
    worker::console_error!("[ocre] {message}");
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("[ocre] {message}");
}

/// `option.or_404()?` turns a missing record into a 404 response.
pub trait OptionExt<T> {
    fn or_404(self) -> Result<T>;
}

impl<T> OptionExt<T> for Option<T> {
    fn or_404(self) -> Result<T> {
        self.ok_or(Error::NotFound)
    }
}

#[cfg(test)]
mod tests;
