use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Handler error. Converts into an HTTP response; internal details are logged,
/// never sent to the client.
#[derive(Debug)]
pub enum Error {
    /// 404 page.
    NotFound,
    /// 400 page with a message shown to the user.
    BadRequest(String),
    /// 500 page; the message goes to the Worker logs only.
    Internal(String),
}

impl Error {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
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

impl From<askama::Error> for Error {
    fn from(err: askama::Error) -> Self {
        Self::Internal(format!("template rendering failed: {err}"))
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "Not found".to_owned()),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Internal(message) => {
                worker::console_error!("[ocre] {message}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_owned())
            }
        };
        (status, Html(format!("<h1>{}</h1><p>{}</p>", status.as_u16(), escape(&message)))).into_response()
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
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
