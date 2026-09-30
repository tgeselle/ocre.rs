//! Security helpers beyond what [`serve`](crate::serve) always does: policies, rate limits, safe redirects, HTML cleaning.
//!
//! `serve` already applies security headers, host authorization
//! ([`ALLOWED_HOSTS`](crate::ALLOWED_HOSTS)), CORS, cross-site request
//! (CSRF) protection and encrypted session cookies. This module adds what an
//! app opts into:
//!
//! | Item | Rails equivalent |
//! |---|---|
//! | [`ContentSecurityPolicy`], [`CspNonce`] | `content_security_policy`, `content_security_policy_nonce` |
//! | [`PermissionsPolicy`] | `permissions_policy` |
//! | [`rate_limit`] | `rate_limit to:, within:, by:` |
//! | [`url_from`] | `url_from`, `redirect_to ... allow_other_host: false` |
//! | [`sanitize`], [`sanitize_with`], [`strip_tags`] | `sanitize`, `strip_tags` |
//! | [`json_escape`], [`escape_javascript`] | `json_escape`, `escape_javascript` |
//! | [`filter_parameters`], [`filter_json`], [`FILTERED_PARAMETERS`] | `filter_parameters` |
//! | [`BasicAuth`] | `http_basic_authenticate_with` |
//! | [`AllowBrowser`] | `allow_browser versions: :modern` |
//!
//! ```
//! use axum::{Router, routing::get};
//! use ocre::security::{ContentSecurityPolicy, PermissionsPolicy, SELF};
//!
//! let app: Router = Router::new()
//!     .route("/", get(|| async { "home" }))
//!     .layer(ContentSecurityPolicy::new().default_src(&[SELF]))
//!     .layer(PermissionsPolicy::new().deny(&["camera", "microphone"]));
//! # let _ = app;
//! ```

mod browser;
mod html;
mod policy;

use axum::{
    extract::FromRequestParts,
    http::{HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;

pub use self::{
    browser::{AllowBrowser, AllowBrowserService, Browser},
    html::{SANITIZE_ATTRIBUTES, SANITIZE_TAGS, escape_javascript, json_escape, sanitize, sanitize_with, strip_tags},
    policy::{
        BLOB, ContentSecurityPolicy, CspNonce, DATA, HTTPS, NONCE, NONE, PermissionsPolicy, PolicyService, SELF,
        STRICT_DYNAMIC, UNSAFE_EVAL, UNSAFE_INLINE,
    },
};
pub use crate::runtime::security::rate_limit;
use crate::token::constant_time_eq;

/// The URL to redirect to when `candidate` points inside this app, else `None` (Rails' `url_from`).
///
/// Use it for every redirect target that comes from the request (a
/// `return_to` parameter, a `Referer`), so the app cannot be used to send
/// users to another site (open redirect). Accepted:
///
/// - a path: `/account?tab=keys` (but not `//evil.example`, which browsers
///   read as another host, nor `/\evil.example`);
/// - an absolute `http(s)` URL whose host (and port) is the request's own
///   host: it is returned as its path and query.
///
/// Anything with control characters (CR, LF, tab: header injection) or
/// backslashes is refused. `uri` is the request's URI (the `Uri`
/// extractor); on Workers it includes the host. No binding call.
///
/// # Examples
///
/// ```
/// use axum::http::Uri;
/// use ocre::security::url_from;
///
/// let request: Uri = "https://app.example.com/login".parse().unwrap();
/// assert_eq!(url_from(&request, "/account?tab=keys").as_deref(), Some("/account?tab=keys"));
/// assert_eq!(url_from(&request, "https://app.example.com/posts/1").as_deref(), Some("/posts/1"));
/// assert_eq!(url_from(&request, "https://evil.example/"), None);
/// assert_eq!(url_from(&request, "//evil.example"), None);
/// assert_eq!(url_from(&request, "/\r\nSet-Cookie: x=1"), None);
///
/// // In a handler: `Redirect::to(&url_from(&uri, &form.return_to).unwrap_or_else(|| "/".to_owned()))`.
/// ```
pub fn url_from(uri: &Uri, candidate: &str) -> Option<String> {
    if candidate.is_empty() || candidate.chars().any(|c| c.is_control() || c == '\\') {
        return None;
    }
    if candidate.starts_with('/') {
        return (!candidate.starts_with("//")).then(|| candidate.to_owned());
    }
    let target: Uri = candidate.parse().ok()?;
    let same_scheme = matches!(target.scheme_str(), Some("http" | "https"));
    let (Some(target_host), Some(own)) = (target.authority(), uri.authority()) else { return None };
    let same_host = target_host.as_str().eq_ignore_ascii_case(own.as_str());
    (same_scheme && same_host).then(|| target.path_and_query().map_or("/", |path| path.as_str()).to_owned())
}

/// Parameter name fragments whose values [`filter_parameters`] and [`filter_json`] hide.
///
/// Rails' default `filter_parameters`: a parameter is hidden when its name,
/// lowercased, contains one of these (`password`, `password_confirmation`,
/// `api_key`, `reset_token`...).
///
/// # Examples
///
/// ```
/// assert!(ocre::security::FILTERED_PARAMETERS.contains(&"passw"));
/// ```
pub const FILTERED_PARAMETERS: &[&str] =
    &["passw", "email", "secret", "token", "_key", "crypt", "salt", "certificate", "otp", "ssn", "cvv", "cvc"];

/// The replacement for hidden values: `[FILTERED]`.
const FILTERED: &str = "[FILTERED]";

fn is_filtered(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    FILTERED_PARAMETERS.iter().any(|fragment| name.contains(fragment))
}

/// A query string or form body with sensitive values replaced by `[FILTERED]` (Rails' `filter_parameters`).
///
/// Log requests through it: `password=hunter2&next=/` becomes
/// `password=[FILTERED]&next=/`. Names are matched against
/// [`FILTERED_PARAMETERS`] after percent-decoding. No binding call.
///
/// # Examples
///
/// ```
/// use ocre::security::filter_parameters;
///
/// assert_eq!(
///     filter_parameters("email=ada%40example.com&password=hunter2&page=2"),
///     "email=[FILTERED]&password=[FILTERED]&page=2"
/// );
/// assert_eq!(filter_parameters("user%5Bpassword%5D=x"), "user%5Bpassword%5D=[FILTERED]");
/// ```
pub fn filter_parameters(query: &str) -> String {
    let pairs: Vec<String> = query
        .split('&')
        .map(|pair| {
            let (name, _) = pair.split_once('=').unwrap_or((pair, ""));
            // One pair decodes to one name; collecting avoids a fallback for an impossible empty result.
            let decoded: String = serde_urlencoded::from_str::<Vec<(String, String)>>(&format!("{name}="))
                .unwrap_or_default()
                .into_iter()
                .map(|(name, _)| name)
                .collect();
            if pair.contains('=') && is_filtered(&decoded) { format!("{name}={FILTERED}") } else { pair.to_owned() }
        })
        .collect();
    pairs.join("&")
}

/// A JSON value with sensitive values replaced by `"[FILTERED]"`, at any depth.
///
/// For logging JSON request bodies; keys are matched like
/// [`filter_parameters`]. No binding call.
///
/// # Examples
///
/// ```
/// use ocre::security::filter_json;
/// use serde_json::json;
///
/// let body = json!({"user": {"email": "ada@example.com", "name": "Ada"}, "api_key": "k"});
/// assert_eq!(filter_json(&body), json!({"user": {"email": "[FILTERED]", "name": "Ada"}, "api_key": "[FILTERED]"}));
/// ```
pub fn filter_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let value = if is_filtered(key) { Value::from(FILTERED) } else { filter_json(value) };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(filter_json).collect()),
        other => other.clone(),
    }
}

/// HTTP Basic credentials from `Authorization: Basic ...`, as an extractor (Rails' `http_basic_authenticate_with`).
///
/// Rejects requests without valid Basic credentials with
/// [`BasicAuth::challenge`]: `401` and `WWW-Authenticate: Basic`, so the
/// browser asks for a user name and password. Check them with
/// [`matches`](Self::matches) (constant time) against Worker secrets, and
/// answer [`challenge`](Self::challenge) when they are wrong. Browsers resend
/// the credentials on every request over HTTPS; use it for a staging site or
/// an internal page, not for user accounts (`ocre g auth`).
///
/// # Examples
///
/// ```no_run
/// use axum::{extract::State, response::{IntoResponse, Response}};
/// use ocre::{Ctx, Result, security::BasicAuth};
///
/// async fn admin(State(ctx): State<Ctx>, auth: BasicAuth) -> Result<Response> {
///     let password = ctx.env().secret("ADMIN_PASSWORD")?.to_string();
///     if !auth.matches("admin", &password) {
///         return Ok(BasicAuth::challenge());
///     }
///     Ok("Welcome, admin".into_response())
/// }
/// # let _ = admin;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicAuth {
    /// The user name the client sent.
    pub username: String,
    /// The password the client sent.
    pub password: String,
}

impl BasicAuth {
    /// Whether the credentials are `username` and `password`, compared in constant time.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::BasicAuth;
    ///
    /// let auth = BasicAuth { username: "admin".into(), password: "s3cret".into() };
    /// assert!(auth.matches("admin", "s3cret"));
    /// assert!(!auth.matches("admin", "guess"));
    /// ```
    pub fn matches(&self, username: &str, password: &str) -> bool {
        // Both comparisons run, so timing does not reveal which one failed.
        let user = constant_time_eq(self.username.as_bytes(), username.as_bytes());
        let pass = constant_time_eq(self.password.as_bytes(), password.as_bytes());
        user & pass
    }

    /// `401 Unauthorized` with `WWW-Authenticate: Basic realm="Application"`: the browser asks again.
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::security::BasicAuth::challenge();
    /// assert_eq!(response.status(), 401);
    /// assert_eq!(response.headers()["www-authenticate"], r#"Basic realm="Application", charset="UTF-8""#);
    /// ```
    pub fn challenge() -> Response {
        let challenge = HeaderValue::from_static(r#"Basic realm="Application", charset="UTF-8""#);
        (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, challenge)], "HTTP Basic: Access denied.\n")
            .into_response()
    }

    fn from_header(value: Option<&HeaderValue>) -> Option<Self> {
        let value = value?.to_str().ok()?;
        let (scheme, encoded) = value.split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("basic") {
            return None;
        }
        let decoded = String::from_utf8(STANDARD.decode(encoded.trim()).ok()?).ok()?;
        let (username, password) = decoded.split_once(':')?;
        Some(Self { username: username.to_owned(), password: password.to_owned() })
    }
}

impl<S: Sync> FromRequestParts<S> for BasicAuth {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Response> {
        Self::from_header(parts.headers.get(header::AUTHORIZATION)).ok_or_else(Self::challenge)
    }
}

#[cfg(test)]
#[path = "../tests/security.rs"]
mod tests;
