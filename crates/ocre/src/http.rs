//! Outgoing HTTP requests: calling another service's API from a handler, a job or a scheduled task.
//!
//! A [`Request`] is built step by step (headers, a JSON or form body, a
//! timeout) and [`Request::send`] returns a `Send` future, so axum handlers
//! await it directly: no `worker::send::SendFuture` and no JavaScript types
//! in the app. Every answer is a [`Response`], whatever its status; turn
//! the non-2xx ones into errors with [`Response::error_for_status`].
//!
//! - Requests time out after [`DEFAULT_TIMEOUT`] (30 s) unless
//!   [`timeout`](Request::timeout) says otherwise; the error then says so.
//! - Redirects are followed unless [`follow_redirects(false)`](Request::follow_redirects),
//!   for URLs a user typed (webhook endpoints), where a redirect could lead
//!   to an address the app refused.
//! - Errors are [`Error::Internal`] (500): the
//!   message names the method, the URL without its query string (which may
//!   hold credentials) and the cause.
//!
//! # Free plan
//!
//! Each `send` is one subrequest: 50 per invocation on the free plan (1,000
//! on Workers Paid), counted with the D1, R2 and KV calls. Time spent
//! waiting for the answer is not CPU time; parsing a JSON answer is (about
//! 1 ms per 100 KB).
//!
//! # Tests
//!
//! `ocre test` runs the app in workerd, which can reach a server the test
//! starts on `127.0.0.1` (`ocre::testing::FakeServer`, feature `testing`).
//! Point the app at it with a test header read by [`test_override`]
//! (debug builds only), so production code keeps the real URL.
//!
//! # Examples
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use axum::extract::State;
//! use ocre::{Ctx, Result, http};
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct Rates {
//!     eur: f64,
//! }
//!
//! async fn usd_to_eur(State(ctx): State<Ctx>) -> Result<String> {
//!     let key = ctx.secret("RATES_API_KEY").await?;
//!     let rates: Rates = http::Request::get("https://api.example.com/latest")
//!         .query(&[("base", "usd")])
//!         .bearer(&key)
//!         .timeout(Duration::from_secs(5))
//!         .send()
//!         .await?
//!         .error_for_status()?
//!         .json()?;
//!     Ok(rates.eur.to_string())
//! }
//! ```

use std::time::Duration;

use axum::http::{HeaderMap, Method};
use serde::{Serialize, de::DeserializeOwned};

use crate::{Error, Result, config::Environment};

/// How long [`Request::send`] waits for an answer unless [`Request::timeout`] says otherwise: 30 seconds.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::http::DEFAULT_TIMEOUT.as_secs(), 30);
/// ```
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The start of the cause of a request that got no answer within its timeout.
pub(crate) const TIMED_OUT: &str = "no answer within";

/// An outgoing HTTP request, built step by step and sent with [`send`](Request::send).
///
/// # Examples
///
/// ```
/// use ocre::http::Request;
/// use serde_json::json;
///
/// let request = Request::post("https://api.example.com/v1/jobs")
///     .bearer("rk_123")
///     .header("Idempotency-Key", "job-42")
///     .json(&json!({"input": {"video": "a.mp4"}}));
/// assert_eq!(request.method(), "POST");
/// assert_eq!(request.headers(), [
///     ("authorization".to_owned(), "Bearer rk_123".to_owned()),
///     ("idempotency-key".to_owned(), "job-42".to_owned()),
///     ("content-type".to_owned(), "application/json".to_owned()),
/// ]);
/// assert_eq!(request.body_bytes(), Some(br#"{"input":{"video":"a.mp4"}}"#.as_slice()));
/// ```
#[derive(Debug, Clone)]
pub struct Request {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    timeout: Duration,
    follow_redirects: bool,
    /// A body that failed to serialize: `send` fails with it.
    invalid: Option<String>,
}

impl Request {
    /// A request of `method` to `url` (absolute, `https://` or `http://`).
    ///
    /// # Examples
    ///
    /// ```
    /// use axum::http::Method;
    ///
    /// let request = ocre::http::Request::new(Method::OPTIONS, "https://example.com/");
    /// assert_eq!((request.method(), request.url()), ("OPTIONS", "https://example.com/"));
    /// ```
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout: DEFAULT_TIMEOUT,
            follow_redirects: true,
            invalid: None,
        }
    }

    /// A `GET` of `url`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::http::Request::get("https://example.com").method(), "GET");
    /// ```
    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::GET, url)
    }

    /// A `POST` to `url`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::http::Request::post("https://example.com").method(), "POST");
    /// ```
    pub fn post(url: impl Into<String>) -> Self {
        Self::new(Method::POST, url)
    }

    /// A `PUT` to `url`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::http::Request::put("https://example.com").method(), "PUT");
    /// ```
    pub fn put(url: impl Into<String>) -> Self {
        Self::new(Method::PUT, url)
    }

    /// A `PATCH` to `url`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::http::Request::patch("https://example.com").method(), "PATCH");
    /// ```
    pub fn patch(url: impl Into<String>) -> Self {
        Self::new(Method::PATCH, url)
    }

    /// A `DELETE` of `url`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::http::Request::delete("https://example.com").method(), "DELETE");
    /// ```
    pub fn delete(url: impl Into<String>) -> Self {
        Self::new(Method::DELETE, url)
    }

    /// A `HEAD` of `url`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::http::Request::head("https://example.com").method(), "HEAD");
    /// ```
    pub fn head(url: impl Into<String>) -> Self {
        Self::new(Method::HEAD, url)
    }

    /// Adds a header (the name is lowercased; a header set twice is sent twice).
    ///
    /// # Examples
    ///
    /// ```
    /// let request = ocre::http::Request::get("https://example.com").header("Accept", "text/csv");
    /// assert_eq!(request.headers(), [("accept".to_owned(), "text/csv".to_owned())]);
    /// ```
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        self
    }

    /// Adds `Authorization: Bearer <token>`.
    ///
    /// # Examples
    ///
    /// ```
    /// let request = ocre::http::Request::get("https://example.com").bearer("t0k");
    /// assert_eq!(request.headers(), [("authorization".to_owned(), "Bearer t0k".to_owned())]);
    /// ```
    pub fn bearer(self, token: &str) -> Self {
        let value = format!("Bearer {}", token.trim());
        self.header("authorization", &value)
    }

    /// Appends query parameters to the URL, percent-encoded ([`percent_encode`](crate::percent_encode)).
    ///
    /// # Examples
    ///
    /// ```
    /// let request = ocre::http::Request::get("https://example.com/search?page=2").query(&[("q", "a&b"), ("lang", "fr")]);
    /// assert_eq!(request.url(), "https://example.com/search?page=2&q=a%26b&lang=fr");
    /// ```
    pub fn query(mut self, pairs: &[(&str, &str)]) -> Self {
        for (name, value) in pairs {
            let separator = if self.url.contains('?') { '&' } else { '?' };
            self.url.push(separator);
            self.url.push_str(&crate::percent_encode(name, false));
            self.url.push('=');
            self.url.push_str(&crate::percent_encode(value, false));
        }
        self
    }

    /// Sends `value` as JSON (`Content-Type: application/json`).
    ///
    /// A value that cannot be serialized (a map with non-string keys) makes
    /// [`send`](Self::send) fail without sending anything.
    ///
    /// # Examples
    ///
    /// ```
    /// let request = ocre::http::Request::post("https://example.com").json(&[1, 2]);
    /// assert_eq!(request.body_bytes(), Some(b"[1,2]".as_slice()));
    /// ```
    pub fn json(self, value: &impl Serialize) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => self.body(body, "application/json"),
            Err(err) => self.invalid(format!("the JSON body cannot be serialized: {err}")),
        }
    }

    /// Sends `value` as a form (`Content-Type: application/x-www-form-urlencoded`).
    ///
    /// # Examples
    ///
    /// ```
    /// let request = ocre::http::Request::post("https://example.com/token").form(&[("grant_type", "client_credentials")]);
    /// assert_eq!(request.body_bytes(), Some(b"grant_type=client_credentials".as_slice()));
    /// ```
    pub fn form(self, value: &impl Serialize) -> Self {
        match serde_urlencoded::to_string(value) {
            Ok(body) => self.body(body.into_bytes(), "application/x-www-form-urlencoded"),
            Err(err) => self.invalid(format!("the form body cannot be serialized: {err}")),
        }
    }

    /// Sends `bytes` as the body, with `Content-Type: content_type`.
    ///
    /// # Examples
    ///
    /// ```
    /// let request = ocre::http::Request::put("https://example.com/a.txt").body(b"hello".to_vec(), "text/plain");
    /// assert_eq!(request.headers(), [("content-type".to_owned(), "text/plain".to_owned())]);
    /// ```
    pub fn body(mut self, bytes: impl Into<Vec<u8>>, content_type: &str) -> Self {
        self.headers.retain(|(name, _)| name != "content-type");
        self.body = Some(bytes.into());
        self.header("content-type", content_type)
    }

    /// Gives up waiting for the answer after `timeout` (default [`DEFAULT_TIMEOUT`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// let request = ocre::http::Request::get("https://example.com").timeout(Duration::from_secs(5));
    /// assert_eq!(request.timeout_duration(), Duration::from_secs(5));
    /// ```
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Whether to follow redirects (default `true`); with `false`, a 3xx is the answer.
    ///
    /// # Examples
    ///
    /// ```
    /// let request = ocre::http::Request::post("https://hooks.example.com").follow_redirects(false);
    /// assert!(!request.follows_redirects());
    /// ```
    pub fn follow_redirects(mut self, follow: bool) -> Self {
        self.follow_redirects = follow;
        self
    }

    /// The method, uppercase.
    pub fn method(&self) -> &str {
        self.method.as_str()
    }

    /// The URL, query parameters included.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The headers, lowercase names, in the order they were added.
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// The body, if any.
    pub fn body_bytes(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }

    /// The timeout.
    pub fn timeout_duration(&self) -> Duration {
        self.timeout
    }

    /// Whether redirects are followed.
    pub fn follows_redirects(&self) -> bool {
        self.follow_redirects
    }

    /// The URL without its query string or fragment, for error messages (a query may hold credentials).
    pub(crate) fn display_url(&self) -> &str {
        self.url.split(['?', '#']).next().unwrap_or_default()
    }

    /// Why the request cannot be sent, if it cannot.
    pub(crate) fn invalid_reason(&self) -> Option<&str> {
        self.invalid.as_deref()
    }

    fn invalid(mut self, reason: String) -> Self {
        self.invalid = Some(reason);
        self
    }

    /// An error about this request: `"POST https://api.example.com/v1/jobs failed: <cause>"`.
    pub(crate) fn error(&self, cause: impl std::fmt::Display) -> Error {
        Error::internal(format!("{} {} failed: {cause}", self.method, self.display_url()))
    }
}

/// The answer to a [`Request`]: status, headers and the whole body.
///
/// # Examples
///
/// ```
/// use ocre::http::Response;
///
/// let response = Response::new(200, Default::default(), br#"{"id": 7}"#.to_vec());
/// assert!(response.is_success());
/// let value: serde_json::Value = response.json().unwrap();
/// assert_eq!(value["id"], 7);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP status (`200`, `404`...).
    pub status: u16,
    /// Response headers.
    pub headers: HeaderMap,
    /// The body, read whole.
    pub body: Vec<u8>,
    /// `METHOD url` of the request (no query string), for error messages.
    request: String,
}

impl Response {
    /// A response with this status, headers and body (for tests of code that reads responses).
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::http::Response::new(404, Default::default(), Vec::new());
    /// assert!(!response.is_success());
    /// ```
    pub fn new(status: u16, headers: HeaderMap, body: Vec<u8>) -> Self {
        Self { status, headers, body, request: "the request".to_owned() }
    }

    /// The response to `request` (its method and URL name it in errors).
    pub(crate) fn of(request: &Request, status: u16, headers: HeaderMap, body: Vec<u8>) -> Self {
        Self { status, headers, body, request: format!("{} {}", request.method, request.display_url()) }
    }

    /// Whether the status is 2xx.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The value of header `name` (case-insensitive), if present and text.
    ///
    /// # Examples
    ///
    /// ```
    /// use axum::http::HeaderMap;
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("ETag", "\"abc\"".parse().unwrap());
    /// let response = ocre::http::Response::new(200, headers, Vec::new());
    /// assert_eq!(response.header("etag"), Some("\"abc\""));
    /// ```
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }

    /// The body as text (invalid UTF-8 replaced).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::http::Response::new(200, Default::default(), b"ok".to_vec()).text(), "ok");
    /// ```
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The body parsed as JSON.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when the body is not
    /// JSON of type `T`; the message quotes its first 200 characters.
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::http::Response::new(200, Default::default(), b"<html>".to_vec());
    /// assert!(response.json::<serde_json::Value>().unwrap_err().to_string().contains("<html>"));
    /// ```
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(|err| {
            Error::internal(format!(
                "{} answered a body that is not the JSON expected ({err}): {}",
                self.request,
                self.excerpt()
            ))
        })
    }

    /// `self` when the status is 2xx, else an error naming the request, the status and the start of the body.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] for a status outside 200-299.
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::http::Response::new(503, Default::default(), b"busy".to_vec());
    /// assert_eq!(response.error_for_status().unwrap_err().to_string(), "internal error: the request answered 503: busy");
    /// ```
    pub fn error_for_status(self) -> Result<Self> {
        if self.is_success() {
            return Ok(self);
        }
        Err(Error::internal(format!("{} answered {}: {}", self.request, self.status, self.excerpt())))
    }

    /// The first 200 characters of the body.
    fn excerpt(&self) -> String {
        self.text().chars().take(200).collect()
    }
}

/// The value of the request header `x-ocre-test-<name>` in debug builds (`ocre dev`, `ocre test`); always `None` in release builds.
///
/// How request tests point the app at a fake service without a setting:
/// the test sends `x-ocre-test-payments-api: http://127.0.0.1:41234` with its
/// requests (`ocre::testing::Client::header`),
/// and the code reads `http::test_override(&headers, "payments-api")
/// .unwrap_or(API_URL)`. Release builds (`ocre deploy`) never read the
/// header, so a visitor cannot redirect the app's calls.
///
/// # Examples
///
/// ```
/// use axum::http::HeaderMap;
///
/// let mut headers = HeaderMap::new();
/// headers.insert("x-ocre-test-payments-api", "http://127.0.0.1:41234".parse().unwrap());
/// let base = ocre::http::test_override(&headers, "payments-api").unwrap_or("https://api.payments.example");
/// assert_eq!(base, if cfg!(debug_assertions) { "http://127.0.0.1:41234" } else { "https://api.payments.example" });
/// ```
pub fn test_override<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    override_in(Environment::current(), headers, name)
}

/// [`test_override`] for an explicit environment.
pub(crate) fn override_in<'a>(environment: Environment, headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    if !environment.is_development() {
        return None;
    }
    headers.get(format!("x-ocre-test-{name}").as_str())?.to_str().ok().map(str::trim)
}

#[cfg(test)]
#[path = "../tests/http.rs"]
mod tests;
