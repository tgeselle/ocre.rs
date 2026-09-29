//! Caching: values in Workers KV, and HTTP caching of responses.
//!
//! **Values** ([`fetch`], [`read`], [`write`], [`delete`]) are JSON in the
//! `CACHE` KV namespace (`ocre g cache` adds the binding; `ocre deploy`
//! creates the namespace). Use them for results that are slow or costly to
//! compute and read far more often than they change: an external API call, an
//! aggregate over many rows.
//!
//! ```ignore
//! use std::time::Duration;
//!
//! let stats: Stats = ocre::cache::fetch(&ctx, "stats:v1", Duration::from_secs(3600), || async {
//!     Stats::compute(&ctx).await
//! })
//! .await?;
//! ```
//!
//! Free plan (September 2026): 100,000 KV reads and **1,000 writes a day**
//! (writes and deletes to different keys; one write per second per key),
//! 1 GB stored. Each [`fetch`] costs one read; a miss adds one write. A key
//! refreshed every `ttl` seconds costs up to `86,400 / ttl` writes a day: a
//! one-hour TTL is 24 writes per key, so about 40 hot keys fit in the budget.
//! KV is eventually consistent: other locations may see an old value for up
//! to 60 seconds after a write or delete. Past a daily limit, KV operations
//! fail; `fetch` then logs the failure and computes the value, so pages keep
//! working.
//!
//! **HTTP**: [`CacheControl`] and [`ETag`] are response parts, and
//! [`Conditional`] answers `304 Not Modified` without rendering when the
//! browser's copy is current. That saves CPU (no template rendering) and
//! bandwidth, never KV operations.
//!
//! The Workers Cache API (`caches.default`) is not wrapped: it is a no-op on
//! `*.workers.dev`, where Ocre apps deploy by default, and still runs the
//! Worker on every request. To have Cloudflare serve whole pages without
//! running the Worker, see "Workers Cache" in the README: it honors the
//! [`CacheControl::public`] header.

use std::{fmt, time::Duration};

use axum::{
    extract::FromRequestParts,
    http::{HeaderValue, Method, StatusCode, header, request::Parts},
    response::{IntoResponse, IntoResponseParts, Response, ResponseParts},
};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

pub use crate::runtime::cache::{delete, fetch, read, write};

/// Name of the KV namespace binding holding cached values.
pub const CACHE_BINDING: &str = "CACHE";

/// Shortest TTL KV accepts (`expirationTtl`).
pub const MIN_TTL: Duration = Duration::from_secs(60);

/// Longest key KV accepts, in bytes.
const MAX_KEY_BYTES: usize = 512;

/// Prefix of every line Ocre logs about the cache.
pub const LOG_PREFIX: &str = "[ocre cache]";

pub(crate) fn check_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err(Error::internal(format!(
            "cache key `{key}` is {} bytes; KV keys are 1 to {MAX_KEY_BYTES} bytes. Fix: use a short key such as \
             \"posts:index:v1\"",
            key.len()
        )));
    }
    Ok(())
}

/// The TTL in whole seconds, at least [`MIN_TTL`].
pub(crate) fn ttl_seconds(ttl: Duration) -> Result<u64> {
    if ttl < MIN_TTL {
        return Err(Error::internal(format!(
            "cache TTL {ttl:?} is below KV's minimum of 60 seconds. Fix: pass Duration::from_secs(60) or more; \
             each refresh costs a KV write (1,000 a day on the free plan)"
        )));
    }
    Ok(ttl.as_secs())
}

pub(crate) fn encode<T: Serialize + ?Sized>(key: &str, value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|err| Error::internal(format!("cannot cache `{key}`: the value does not serialize to JSON ({err})")))
}

/// The cached value, or `None` (logged) when it no longer matches `T`, e.g.
/// after a deploy changed the struct: the caller recomputes it.
pub(crate) fn decode<T: DeserializeOwned>(key: &str, text: &str) -> Option<T> {
    serde_json::from_str(text)
        .map_err(|err| log_failure("decode", key, &format!("{err}; recomputing it (change the key to avoid this)")))
        .ok()
}

pub(crate) fn binding_error(err: &dyn fmt::Display) -> Error {
    Error::internal(format!(
        "KV binding `{CACHE_BINDING}` is missing ({err}). Fix: run `ocre g cache`, which adds \
         [[kv_namespaces]] binding = \"{CACHE_BINDING}\" to wrangler.toml"
    ))
}

/// Logs a KV failure that the caller survives (over the daily limit, a
/// value from an older deploy...).
pub(crate) fn log_failure(operation: &str, key: &str, err: &dyn fmt::Display) {
    crate::error::log_internal(&format!("{LOG_PREFIX} {operation} `{key}` failed: {err}"));
}

/// `Cache-Control` for a response; add it to a response tuple:
/// `(CacheControl::public(Duration::from_secs(300)), Html(page))`.
///
/// ```
/// use std::time::Duration;
/// use ocre::cache::CacheControl;
///
/// assert_eq!(CacheControl::no_cache().to_string(), "private, no-cache");
/// assert_eq!(
///     CacheControl::public(Duration::from_secs(300)).stale_while_revalidate(Duration::from_secs(60)).to_string(),
///     "public, max-age=300, stale-while-revalidate=60"
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheControl {
    kind: Kind,
    max_age: u64,
    stale_while_revalidate: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    NoStore,
    NoCache,
    Private,
    Public,
}

impl CacheControl {
    /// `no-store`: never keep a copy (pages with secrets, one-time tokens).
    ///
    /// ```
    /// assert_eq!(ocre::cache::CacheControl::no_store().to_string(), "no-store");
    /// ```
    pub fn no_store() -> Self {
        Self { kind: Kind::NoStore, max_age: 0, stale_while_revalidate: None }
    }

    /// `private, no-cache`: the browser keeps a copy but asks every time;
    /// with an [`ETag`] the answer is usually a cheap 304. The right default
    /// for pages that depend on the visitor (session, locale, flash).
    ///
    /// ```
    /// assert_eq!(ocre::cache::CacheControl::no_cache().to_string(), "private, no-cache");
    /// ```
    pub fn no_cache() -> Self {
        Self { kind: Kind::NoCache, max_age: 0, stale_while_revalidate: None }
    }

    /// `private, max-age=N`: the browser reuses its copy for `max_age`
    /// without asking; shared caches never store it.
    ///
    /// ```
    /// use std::time::Duration;
    /// assert_eq!(ocre::cache::CacheControl::private(Duration::from_secs(60)).to_string(), "private, max-age=60");
    /// ```
    pub fn private(max_age: Duration) -> Self {
        Self { kind: Kind::Private, max_age: max_age.as_secs(), stale_while_revalidate: None }
    }

    /// `public, max-age=N`: browsers **and Cloudflare** may reuse it for
    /// every visitor. Only for responses identical for everyone: no session
    /// data, no locale unless it is in the path. With Workers Cache enabled
    /// (`[cache] enabled = true` in wrangler.toml) such responses are served
    /// without running the Worker.
    ///
    /// ```
    /// use std::time::Duration;
    /// assert_eq!(ocre::cache::CacheControl::public(Duration::from_secs(3600)).to_string(), "public, max-age=3600");
    /// ```
    pub fn public(max_age: Duration) -> Self {
        Self { kind: Kind::Public, max_age: max_age.as_secs(), stale_while_revalidate: None }
    }

    /// Adds `stale-while-revalidate`: after `max-age`, serve the old copy
    /// for up to `window` while fetching a new one. Ignored by `no_store`
    /// and `no_cache`.
    ///
    /// ```
    /// use std::time::Duration;
    /// let policy = ocre::cache::CacheControl::private(Duration::from_secs(60)).stale_while_revalidate(Duration::from_secs(600));
    /// assert_eq!(policy.to_string(), "private, max-age=60, stale-while-revalidate=600");
    /// ```
    pub fn stale_while_revalidate(self, window: Duration) -> Self {
        Self { stale_while_revalidate: Some(window.as_secs()), ..self }
    }
}

impl fmt::Display for CacheControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scope = match self.kind {
            Kind::NoStore => return f.write_str("no-store"),
            Kind::NoCache => return f.write_str("private, no-cache"),
            Kind::Private => "private",
            Kind::Public => "public",
        };
        write!(f, "{scope}, max-age={}", self.max_age)?;
        match self.stale_while_revalidate {
            Some(window) => write!(f, ", stale-while-revalidate={window}"),
            None => Ok(()),
        }
    }
}

impl IntoResponseParts for CacheControl {
    type Error = std::convert::Infallible;

    fn into_response_parts(self, mut res: ResponseParts) -> Result<ResponseParts, Self::Error> {
        let value = HeaderValue::from_str(&self.to_string()).expect("Cache-Control values are ASCII");
        res.headers_mut().insert(header::CACHE_CONTROL, value);
        Ok(res)
    }
}

/// A weak entity tag (`W/"<hash>"`): the version of what a page shows.
/// Build it from everything the page displays, so it changes when the page
/// would: the records, plus the locale, the signed-in user and the flash
/// when the page shows them.
///
/// ```
/// use ocre::cache::ETag;
///
/// let a = ETag::new("post-42-1767225600");
/// assert_eq!(a, ETag::new("post-42-1767225600"));
/// assert_ne!(a, ETag::new("post-42-1767225601"));
/// assert!(a.as_str().starts_with("W/\""));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ETag(String);

impl ETag {
    /// A tag for a version string you build, e.g. `format!("{}-{}", post.id, post.updated_at)`:
    /// cheaper than [`of`](Self::of) for large data.
    ///
    /// ```
    /// assert_eq!(ocre::cache::ETag::new("v1").as_str().len(), 36);
    /// ```
    pub fn new(version: impl AsRef<[u8]>) -> Self {
        let digest = Sha256::digest(version.as_ref());
        let mut tag = String::with_capacity(36);
        tag.push_str("W/\"");
        for byte in &digest[..16] {
            tag.push(char::from_digit(u32::from(byte >> 4), 16).expect("a nibble is a hex digit"));
            tag.push(char::from_digit(u32::from(byte & 0xf), 16).expect("a nibble is a hex digit"));
        }
        tag.push('"');
        Self(tag)
    }

    /// A tag for data, hashed as JSON: `ETag::of(&(&posts, i18n.locale()))?`.
    ///
    /// ```
    /// use ocre::cache::ETag;
    /// assert_eq!(ETag::of(&("Ada", 1)).unwrap(), ETag::of(&("Ada", 1)).unwrap());
    /// assert_ne!(ETag::of(&("Ada", 1)).unwrap(), ETag::of(&("Ada", 2)).unwrap());
    /// ```
    pub fn of<T: Serialize + ?Sized>(data: &T) -> Result<Self> {
        Self::from_json(serde_json::to_vec(data))
    }

    fn from_json(json: serde_json::Result<Vec<u8>>) -> Result<Self> {
        let json = json
            .map_err(|err| Error::internal(format!("cannot compute an ETag: the data does not serialize ({err})")))?;
        Ok(Self::new(json))
    }

    /// The header value, `W/"..."`.
    ///
    /// ```
    /// assert!(ocre::cache::ETag::new("v1").as_str().ends_with('"'));
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Weak comparison (RFC 9110): `W/"x"` matches `"x"`.
    fn matches(&self, other: &str) -> bool {
        let opaque = |tag: &str| tag.trim().trim_start_matches("W/").to_owned();
        opaque(&self.0) == opaque(other)
    }
}

impl IntoResponseParts for ETag {
    type Error = std::convert::Infallible;

    fn into_response_parts(self, mut res: ResponseParts) -> Result<ResponseParts, Self::Error> {
        let value = HeaderValue::from_str(&self.0).expect("ETags are ASCII");
        res.headers_mut().insert(header::ETAG, value);
        Ok(res)
    }
}

/// Extractor: the request's `If-None-Match`, for answering `304 Not
/// Modified` without rendering, like Rails' `fresh_when`.
///
/// ```ignore
/// async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>, conditional: Conditional) -> Result<Response> {
///     let post = post::find(&ctx, id).await?.or_404()?;
///     let etag = ETag::of(&post)?;
///     conditional.fresh_when(etag, CacheControl::no_cache(), || render(&ShowView { post }))
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct Conditional {
    if_none_match: Option<String>,
}

impl Conditional {
    /// Whether the client already has the version `etag` (GET and HEAD only).
    ///
    /// ```
    /// use axum::{extract::FromRequestParts, http::Request};
    /// use ocre::cache::{Conditional, ETag};
    ///
    /// let etag = ETag::new("v1");
    /// let (mut parts, ()) = Request::get("/").header("If-None-Match", etag.as_str()).body(()).unwrap().into_parts();
    /// let conditional = pollster::block_on(Conditional::from_request_parts(&mut parts, &())).unwrap();
    /// assert!(conditional.is_fresh(&etag));
    /// assert!(!conditional.is_fresh(&ETag::new("v2")));
    /// ```
    pub fn is_fresh(&self, etag: &ETag) -> bool {
        self.if_none_match
            .as_deref()
            .is_some_and(|header| header.trim() == "*" || header.split(',').any(|candidate| etag.matches(candidate)))
    }

    /// `304 Not Modified` (with `etag` and `cache_control`, no body) when the
    /// client's copy is current; otherwise runs `render` and adds both headers.
    ///
    /// ```
    /// use ocre::cache::{CacheControl, Conditional, ETag};
    ///
    /// let response = Conditional::default().fresh_when(ETag::new("v1"), CacheControl::no_cache(), || Ok("page")).unwrap();
    /// assert_eq!(response.status(), 200);
    /// assert_eq!(response.headers()["cache-control"], "private, no-cache");
    /// ```
    pub fn fresh_when<R: IntoResponse>(
        &self,
        etag: ETag,
        cache_control: CacheControl,
        render: impl FnOnce() -> Result<R>,
    ) -> Result<Response> {
        if self.is_fresh(&etag) {
            return Ok((StatusCode::NOT_MODIFIED, etag, cache_control, ()).into_response());
        }
        Ok((etag, cache_control, render()?).into_response())
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Conditional {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let cacheable = parts.method == Method::GET || parts.method == Method::HEAD;
        let if_none_match = parts
            .headers
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .filter(|_| cacheable)
            .map(str::to_owned);
        Ok(Self { if_none_match })
    }
}

#[cfg(test)]
#[path = "../tests/cache.rs"]
mod tests;
