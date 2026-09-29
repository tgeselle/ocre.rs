use axum::http::StatusCode;

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
    /// 500; the message goes to the Worker logs only.
    Internal(String),
}

impl Error {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    /// Status and client-safe message. Internal details are logged here and
    /// replaced by a generic message.
    pub(crate) fn into_public(self) -> (StatusCode, String) {
        match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "Not found".to_owned()),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Internal(message) => {
                log_internal(&message);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_owned())
            }
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("not found"),
            Self::BadRequest(message) => write!(f, "bad request: {message}"),
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
