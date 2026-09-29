//! Middleware every Ocre app runs, in this order (outermost first):
//!
//! 1. Security headers on every response.
//! 2. CORS for the origins listed in `ALLOWED_ORIGINS`.
//! 3. Cross-origin request protection (CSRF).
//! 4. The session cookie.
//!
//! CSRF protection checks where a request comes from instead of embedding
//! tokens in forms: browsers send `Sec-Fetch-Site` (or at least `Origin`) with
//! every unsafe request, and a request another site triggers is refused with
//! 403. Requests without either header do not come from a browser page, so
//! they cannot carry a victim's cookies by accident. This is the check Go 1.25
//! ships as `http.CrossOriginProtection`; session cookies are also
//! `SameSite=Lax`.

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use cookie::Key;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::session::Session;

/// Worker variable listing extra origins, comma-separated, that may call the
/// app from a browser (CORS) and submit to it (CSRF), e.g.
/// `ALLOWED_ORIGINS = "https://app.example.com"` under `[vars]` in wrangler.toml.
pub const ALLOWED_ORIGINS: &str = "ALLOWED_ORIGINS";

/// Per-request settings read from the Worker environment.
pub(crate) struct Config {
    pub key: Result<Key, String>,
    pub allowed_origins: Vec<HeaderValue>,
}

/// `"https://a.example, https://b.example"` -> the two origins. Invalid
/// entries are skipped.
pub(crate) fn parse_origins(value: Option<String>) -> Vec<HeaderValue> {
    value
        .unwrap_or_default()
        .split(',')
        .map(|origin| origin.trim().trim_end_matches('/'))
        .filter(|origin| !origin.is_empty())
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect()
}

/// Wraps the application router with Ocre's middleware.
pub(crate) fn wrap(router: Router, config: Config) -> Router {
    let Config { key, allowed_origins } = config;
    let trusted = allowed_origins.clone();
    let mut router = router
        .layer(middleware::from_fn(move |req: Request, next: Next| {
            let key = key.clone();
            async move { session(key, req, next).await }
        }))
        .layer(middleware::from_fn(move |req: Request, next: Next| {
            let refused = cross_origin(req.method(), req.headers(), &trusted);
            async move {
                match refused {
                    Some(reason) => (StatusCode::FORBIDDEN, reason).into_response(),
                    None => next.run(req).await,
                }
            }
        }));
    if !allowed_origins.is_empty() {
        router = router.layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(allowed_origins))
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::ACCEPT])
                .allow_credentials(true),
        );
    }
    router.layer(middleware::from_fn(security_headers))
}

async fn session(key: Result<Key, String>, mut req: Request, next: Next) -> Response {
    let secure = req.uri().scheme_str() == Some("https");
    let session = Session::from_headers(req.headers(), key, secure);
    req.extensions_mut().insert(session.clone());
    let mut response = next.run(req).await;
    match session.set_cookie() {
        Ok(Some(cookie)) => {
            response.headers_mut().append(header::SET_COOKIE, cookie);
            response
        }
        Ok(None) => response,
        #[cfg(feature = "html")]
        Err(err) => err.into_response(),
        #[cfg(not(feature = "html"))]
        Err(err) => crate::ApiError::from(err).into_response(),
    }
}

/// Why a request is refused, or `None` when it may proceed.
pub(crate) fn cross_origin(method: &Method, headers: &HeaderMap, trusted: &[HeaderValue]) -> Option<&'static str> {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return None;
    }
    let origin = headers.get(header::ORIGIN);
    if origin.is_some_and(|origin| trusted.contains(origin)) {
        return None;
    }
    match headers.get("sec-fetch-site").map(HeaderValue::as_bytes) {
        Some(b"same-origin" | b"none") => return None,
        Some(_) => return Some("Forbidden: cross-site request. Add the origin to ALLOWED_ORIGINS to allow it."),
        None => {}
    }
    // Older browsers: compare Origin with Host.
    let (Some(origin), Some(host)) = (origin, headers.get(header::HOST)) else {
        return None;
    };
    let origin_host = origin.to_str().ok().and_then(|origin| origin.split_once("://")).map(|(_, host)| host);
    if origin_host.is_some_and(|origin_host| origin_host.as_bytes() == host.as_bytes()) {
        None
    } else {
        Some("Forbidden: cross-origin request. Add the origin to ALLOWED_ORIGINS to allow it.")
    }
}

/// Rails' default headers, plus HSTS on HTTPS. A handler that sets one of
/// these headers keeps its own value.
async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let https = req.uri().scheme_str() == Some("https");
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    let defaults: [(HeaderName, &str); 5] = [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "SAMEORIGIN"),
        (header::REFERRER_POLICY, "strict-origin-when-cross-origin"),
        (header::X_XSS_PROTECTION, "0"),
        (HeaderName::from_static("x-permitted-cross-domain-policies"), "none"),
    ];
    for (name, value) in defaults {
        headers.entry(name).or_insert(HeaderValue::from_static(value));
    }
    if https {
        headers.entry(header::STRICT_TRANSPORT_SECURITY).or_insert(HeaderValue::from_static("max-age=63072000"));
    }
    response
}

#[cfg(test)]
mod tests;
