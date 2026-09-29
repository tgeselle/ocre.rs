//! Request helpers: the client's IP address, a request id, the response
//! format a client asks for, and redirecting back to the previous page.

use std::{convert::Infallible, net::IpAddr};

use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, header, request::Parts},
    response::Redirect,
};

/// Extractor for the client's IP address, from Cloudflare's `CF-Connecting-IP` header.
///
/// Cloudflare sets `CF-Connecting-IP` on every request that reaches a Worker
/// and replaces any value the client sent, so it can be trusted, unlike
/// `X-Forwarded-For` (Rails' `request.remote_ip`, Loco's `RemoteIP`). It is
/// `RemoteIp(None)` only when the header is missing or malformed, e.g. in unit
/// tests that build requests by hand. Never rejects. Use it to key rate limits
/// or to record where a sign-in came from; it is personal data under the GDPR.
///
/// # Examples
///
/// ```no_run
/// use ocre::RemoteIp;
///
/// async fn whoami(RemoteIp(ip): RemoteIp) -> String {
///     ip.map_or_else(|| "unknown".to_owned(), |ip| ip.to_string())
/// }
/// # let _ = whoami;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteIp(pub Option<IpAddr>);

impl<S: Send + Sync> FromRequestParts<S> for RemoteIp {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(remote_ip(&parts.headers)))
    }
}

/// The client's IP address from the `CF-Connecting-IP` header, for code that has the headers but no extractor.
///
/// See [`RemoteIp`].
///
/// # Examples
///
/// ```
/// use axum::http::HeaderMap;
///
/// let mut headers = HeaderMap::new();
/// headers.insert("cf-connecting-ip", "203.0.113.7".parse().unwrap());
/// assert_eq!(ocre::remote_ip(&headers).unwrap().to_string(), "203.0.113.7");
/// assert_eq!(ocre::remote_ip(&HeaderMap::new()), None);
/// ```
pub fn remote_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers.get("cf-connecting-ip")?.to_str().ok()?.trim().parse().ok()
}

/// Extractor for an identifier of the request, to correlate log lines and error reports.
///
/// Cloudflare's `CF-Ray` id (e.g. `8c2f1a0b9d3e4f5a-CDG`) when present, which
/// is also shown in the Cloudflare dashboard and in Workers Logs; otherwise
/// the client's `X-Request-Id` when it is 1 to 64 letters, digits, `-` or
/// `_` (Loco's `request_id`); otherwise 16 random hex characters. Never
/// rejects; no binding call.
///
/// # Examples
///
/// ```no_run
/// use ocre::RequestId;
///
/// async fn checkout(RequestId(id): RequestId) -> &'static str {
///     worker::console_log!("[{id}] checkout started");
///     "OK"
/// }
/// # let _ = checkout;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(pub String);

impl<S: Send + Sync> FromRequestParts<S> for RequestId {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(request_id(&parts.headers)))
    }
}

fn request_id(headers: &HeaderMap) -> String {
    let valid = |id: &&str| {
        (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    ["cf-ray", "x-request-id"]
        .iter()
        .find_map(|name| headers.get(*name)?.to_str().ok().filter(valid).map(str::to_owned))
        .unwrap_or_else(|| crate::token::random_bytes::<8>().iter().map(|b| format!("{b:02x}")).collect())
}

/// Response format a client asks for in its `Accept` header, for actions that answer HTML or JSON (Rails' `respond_to`).
///
/// The media type with the highest `q` wins (first listed on ties):
/// `text/html` and `application/xhtml+xml` are [`Html`](Self::Html),
/// `application/json` and any `+json` type [`Json`](Self::Json),
/// `application/xml`, `text/xml` and `+xml` types [`Xml`](Self::Xml),
/// `text/plain` [`Text`](Self::Text), `*/*` and `text/*` the first format
/// of the list above. A missing or empty `Accept` header is `Html`, like
/// Rails; types Ocre does not know are [`Other`](Self::Other), usually
/// answered with `406 Not Acceptable`. Never rejects; no binding call.
///
/// # Examples
///
/// ```no_run
/// use axum::{extract::{Path, State}, response::{Html, IntoResponse, Response}};
/// use ocre::{Ctx, Format, Json, OptionExt, Result, params};
///
/// async fn show(State(ctx): State<Ctx>, format: Format, Path(id): Path<i64>) -> Result<Response> {
///     let post: serde_json::Value =
///         ctx.db()?.first("SELECT id, title FROM posts WHERE id = ?1", params![id]).await?.or_404()?;
///     Ok(match format {
///         Format::Json => Json(post).into_response(),
///         _ => Html(format!("<h1>{}</h1>", post["title"])).into_response(),
///     })
/// }
/// # let _ = show;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `text/html`: a page.
    Html,
    /// `application/json`.
    Json,
    /// `application/xml` or `text/xml`, e.g. an RSS or Atom feed.
    Xml,
    /// `text/plain`.
    Text,
    /// Only types Ocre does not know, e.g. `application/pdf`.
    Other,
}

impl Format {
    /// Reads the `Accept` header; see [`Format`] for the rules.
    ///
    /// # Examples
    ///
    /// ```
    /// use axum::http::HeaderMap;
    /// use ocre::Format;
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("accept", "text/html;q=0.9, application/json".parse().unwrap());
    /// assert_eq!(Format::from_headers(&headers), Format::Json);
    /// assert_eq!(Format::from_headers(&HeaderMap::new()), Format::Html);
    /// ```
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let Some(accept) = headers.get(header::ACCEPT).and_then(|value| value.to_str().ok()) else {
            return Self::Html;
        };
        let mut best: Option<(f32, Self)> = None;
        for item in accept.split(',') {
            let mut parts = item.split(';');
            let media = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
            if media.is_empty() {
                continue;
            }
            let q = parts
                .filter_map(|param| param.trim().strip_prefix("q="))
                .find_map(|q| q.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            if q <= 0.0 {
                continue;
            }
            let format = Self::from_media(&media);
            if best.is_none_or(|(best_q, _)| q > best_q) {
                best = Some((q, format));
            }
        }
        best.map_or(Self::Html, |(_, format)| format)
    }

    fn from_media(media: &str) -> Self {
        match media {
            "text/html" | "application/xhtml+xml" | "*/*" | "text/*" => Self::Html,
            "application/json" => Self::Json,
            "application/xml" | "text/xml" => Self::Xml,
            "text/plain" => Self::Text,
            _ if media.ends_with("+json") => Self::Json,
            _ if media.ends_with("+xml") => Self::Xml,
            _ => Self::Other,
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Format {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self::from_headers(&parts.headers))
    }
}

/// Redirects to the page the request came from, or to `fallback` (Rails' `redirect_back_or_to`).
///
/// Uses the `Referer` header only when it points to this app (same host as
/// the request's `Host` header), so a link from another site cannot turn
/// the app into an open redirect; the redirect keeps the referring path and
/// query string. Answers `303 See Other`, like [`Redirect::to`]. Browsers
/// send `Referer` for same-origin requests under Ocre's default
/// `Referrer-Policy`.
///
/// # Examples
///
/// ```
/// use axum::{http::HeaderMap, response::IntoResponse};
///
/// let mut headers = HeaderMap::new();
/// headers.insert("host", "blog.example".parse().unwrap());
/// headers.insert("referer", "https://blog.example/posts?page=2".parse().unwrap());
/// let response = ocre::redirect_back(&headers, "/").into_response();
/// assert_eq!(response.headers()["location"], "/posts?page=2");
///
/// headers.insert("referer", "https://evil.example/".parse().unwrap());
/// let response = ocre::redirect_back(&headers, "/").into_response();
/// assert_eq!(response.headers()["location"], "/");
/// ```
pub fn redirect_back(headers: &HeaderMap, fallback: &str) -> Redirect {
    let text = |name| headers.get(name).and_then(|value: &axum::http::HeaderValue| value.to_str().ok());
    let back = (|| {
        let (host, referer) = (text(header::HOST)?, text(header::REFERER)?);
        let rest = referer.strip_prefix("https://").or_else(|| referer.strip_prefix("http://"))?;
        let (referer_host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let path = if path.is_empty() { "/" } else { path };
        let path = path.split('#').next().unwrap_or(path);
        (referer_host.eq_ignore_ascii_case(host) && !path.starts_with("//")).then_some(path)
    })();
    Redirect::to(back.unwrap_or(fallback))
}

#[cfg(test)]
#[path = "../tests/request.rs"]
mod tests;
