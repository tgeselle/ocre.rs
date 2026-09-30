//! Middleware every Ocre app runs, in this order (outermost first):
//!
//! 1. Security headers on every response.
//! 2. Host authorization for the hosts listed in `ALLOWED_HOSTS`.
//! 3. CORS for the origins listed in `ALLOWED_ORIGINS`.
//! 4. Cross-origin request protection (CSRF).
//! 5. The session cookie.
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
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::session::{Keys, Session};

/// Name of the Worker variable listing extra origins that may call the app from a browser.
///
/// Comma-separated; spaces and a trailing `/` are ignored and invalid entries
/// skipped. Listed origins get CORS headers (methods GET, POST, PUT, PATCH,
/// DELETE; headers `Content-Type`, `Authorization`, `Accept`; credentials
/// allowed) and pass the CSRF check. When the variable is unset or empty,
/// [`serve`](crate::serve) adds no CORS layer and only same-origin browser
/// requests may change data. Read once per request by `serve`; no binding
/// call.
///
/// ```ts
/// // worker.env in cloudflare.config.ts
/// ALLOWED_ORIGINS: bindings.text("https://app.example.com, https://admin.example.com"),
/// ```
///
/// # Examples
///
/// ```no_run
/// use axum::extract::State;
/// use ocre::{ALLOWED_ORIGINS, Ctx, Result};
///
/// async fn origins(State(ctx): State<Ctx>) -> Result<String> {
///     Ok(ctx.env().var(ALLOWED_ORIGINS).map(|v| v.to_string()).unwrap_or_default())
/// }
/// # let _ = origins;
pub const ALLOWED_ORIGINS: &str = "ALLOWED_ORIGINS";

/// Name of the Worker variable listing the host names the app answers to (Rails' `config.hosts`).
///
/// Comma-separated; an entry starting with `.` also allows every subdomain
/// (`.example.com` allows `example.com` and `www.example.com`). When set,
/// requests for any other `Host` get a plain-text `403 Forbidden` before any
/// handler or session code runs; `localhost`, `127.0.0.1` and `[::1]` are
/// always allowed so `ocre dev` keeps working (Cloudflare only routes your
/// own host names to the Worker, so these never reach it in production).
/// Unset or empty: every host is allowed.
///
/// On Workers, DNS rebinding cannot reach the app, but the same Worker also
/// answers on `<name>.<account>.workers.dev` and preview URLs: list your
/// custom domain to keep search engines and users on it. Read once per
/// request by [`serve`](crate::serve); no binding call.
///
/// ```ts
/// // worker.env in cloudflare.config.ts
/// ALLOWED_HOSTS: bindings.text("example.com, .example.com"),
/// ```
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::ALLOWED_HOSTS, "ALLOWED_HOSTS");
/// ```
pub const ALLOWED_HOSTS: &str = "ALLOWED_HOSTS";

/// Per-request settings read from the Worker environment.
pub(crate) struct Config {
    pub keys: Result<Keys, String>,
    pub allowed_origins: Vec<HeaderValue>,
    pub allowed_hosts: Vec<String>,
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

/// `"Example.com, .example.com"` -> `["example.com", ".example.com"]`.
pub(crate) fn parse_hosts(value: Option<String>) -> Vec<String> {
    let value = value.unwrap_or_default();
    value.split(',').map(|host| host.trim().to_ascii_lowercase()).filter(|host| !host.is_empty()).collect()
}

/// Whether a request for `host` (with or without a port) may proceed.
pub(crate) fn host_allowed(host: Option<&str>, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let Some(host) = host else { return false };
    let host = host.to_ascii_lowercase();
    // `[::1]:8787` -> `[::1]`; `example.com:443` -> `example.com`.
    let name = match host.rsplit_once(':') {
        Some((name, port)) if !name.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host.as_str(),
    };
    if matches!(name, "localhost" | "127.0.0.1" | "[::1]") {
        return true;
    }
    allowed.iter().any(|entry| match entry.strip_prefix('.') {
        Some(domain) => name == domain || name.strip_suffix(domain).is_some_and(|sub| sub.ends_with('.')),
        None => name == entry,
    })
}

/// Wraps the application router with Ocre's middleware.
pub(crate) fn wrap(router: Router, config: Config) -> Router {
    let Config { keys, allowed_origins, allowed_hosts } = config;
    let trusted = allowed_origins.clone();
    let mut router = router
        .layer(middleware::from_fn(move |req: Request, next: Next| {
            let keys = keys.clone();
            async move { session(keys, req, next).await }
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
    if !allowed_hosts.is_empty() {
        router = router.layer(middleware::from_fn(move |req: Request, next: Next| {
            let header = req.headers().get(header::HOST).and_then(|host| host.to_str().ok());
            let allowed = host_allowed(req.uri().host().or(header), &allowed_hosts);
            async move {
                if allowed {
                    next.run(req).await
                } else {
                    (StatusCode::FORBIDDEN, "Forbidden: blocked host. Add it to ALLOWED_HOSTS to allow it.")
                        .into_response()
                }
            }
        }));
    }
    router.layer(middleware::from_fn(security_headers))
}

async fn session(keys: Result<Keys, String>, mut req: Request, next: Next) -> Response {
    let secure = req.uri().scheme_str() == Some("https");
    let cookies = crate::cookies::Cookies::from_headers(req.headers(), keys.clone(), secure);
    let session = Session::from_headers(req.headers(), keys, secure);
    req.extensions_mut().insert(session.clone());
    req.extensions_mut().insert(cookies.clone());
    let mut response = next.run(req).await;
    for cookie in cookies.set_cookies() {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
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

/// Whether the request is a WebSocket handshake (`Upgrade: websocket`).
pub(crate) fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers.get(header::UPGRADE).is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"websocket"))
}

/// Why a request is refused, or `None` when it may proceed. WebSocket
/// handshakes are GETs, but browsers send cookies with them and let any site
/// open them (cross-site WebSocket hijacking), so they are checked like forms.
pub(crate) fn cross_origin(method: &Method, headers: &HeaderMap, trusted: &[HeaderValue]) -> Option<&'static str> {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) && !is_websocket_upgrade(headers) {
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
#[path = "../tests/protect.rs"]
mod tests;
