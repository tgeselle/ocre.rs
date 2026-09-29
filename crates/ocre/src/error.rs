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
                log_internal(&message);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_owned())
            }
        };
        (status, Html(format!("<h1>{}</h1><p>{}</p>", status.as_u16(), escape(&message)))).into_response()
    }
}

/// Worker logs (visible in `wrangler tail`); stderr in native unit tests.
fn log_internal(message: &str) {
    #[cfg(target_arch = "wasm32")]
    worker::console_error!("[ocre] {message}");
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("[ocre] {message}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::body_text;

    fn respond(err: Error) -> (u16, String) {
        let response = err.into_response();
        let status = response.status().as_u16();
        (status, body_text(response))
    }

    #[test]
    fn not_found_is_a_404_page() {
        assert_eq!(respond(Error::NotFound), (404, "<h1>404</h1><p>Not found</p>".to_owned()));
    }

    #[test]
    fn bad_request_shows_the_message_escaped() {
        let (status, body) = respond(Error::bad_request(r#"<b>"Tom" & 'Jerry'</b>"#));
        assert_eq!(status, 400);
        assert_eq!(body, "<h1>400</h1><p>&lt;b&gt;&quot;Tom&quot; &amp; &#39;Jerry&#39;&lt;/b&gt;</p>");
    }

    #[test]
    fn internal_error_hides_the_message() {
        let (status, body) = respond(Error::internal("password=hunter2"));
        assert_eq!(status, 500);
        assert_eq!(body, "<h1>500</h1><p>Internal server error</p>");
    }

    #[test]
    fn display_describes_each_variant() {
        assert_eq!(Error::NotFound.to_string(), "not found");
        assert_eq!(Error::bad_request("x").to_string(), "bad request: x");
        assert_eq!(Error::internal("y").to_string(), "internal error: y");
    }

    #[test]
    fn runtime_and_template_errors_become_internal() {
        let worker_err: Error = worker::Error::RustError("boom".into()).into();
        assert_eq!(worker_err.to_string(), "internal error: boom");
        let template_err: Error = askama::Error::Fmt.into();
        assert!(template_err.to_string().starts_with("internal error: template rendering failed"));
    }

    #[test]
    fn or_404_maps_none_to_not_found() {
        assert_eq!(Some(3).or_404().unwrap(), 3);
        assert!(matches!(None::<i32>.or_404(), Err(Error::NotFound)));
    }
}
