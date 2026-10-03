//! Caching: read-through values in Workers KV, HTTP `Cache-Control`/`ETag` and 304 responses.
//!
//! Two opt-in tools, both chosen for the Workers free plan.
//!
//! **Values** ([`fetch`], [`read`], [`write`](fn@write), [`delete`]) are JSON in the
//! `CACHE` KV namespace (`ocre g cache` adds the binding to `cloudflare.config.ts`;
//! `ocre deploy` creates the namespace). Use them for results that are slow or
//! costly to compute and read far more often than they change: an external API
//! call, a D1 aggregate over many rows. [`fetch`] is Rails'
//! `Rails.cache.fetch`; the other three are the explicit forms.
//!
//! ```rust,no_run
//! use std::time::Duration;
//!
//! use axum::{Json, extract::State};
//! use ocre::{Ctx, Result};
//! # #[derive(serde::Serialize, serde::Deserialize)]
//! # struct Stats { posts: i64 }
//! # impl Stats { async fn compute(_ctx: &Ctx) -> Result<Self> { Ok(Stats { posts: 0 }) } }
//!
//! async fn stats(State(ctx): State<Ctx>) -> Result<Json<Stats>> {
//!     // One KV read; a miss runs the closure and costs one KV write.
//!     let stats: Stats = ocre::cache::fetch(&ctx, "stats:v1", Duration::from_secs(3600), || async {
//!         Stats::compute(&ctx).await
//!     })
//!     .await?;
//!     Ok(Json(stats))
//! }
//! ```
//!
//! Free plan (September 2026): 100,000 KV reads and **1,000 writes a day**
//! (writes and deletes to different keys; one write per second per key),
//! 1 GB stored. Each [`fetch`] costs one read; a miss adds one write. A key
//! refreshed every `ttl` seconds costs up to `86,400 / ttl` writes a day: a
//! one-hour TTL is 24 writes per key, so about 40 hot keys fit in the budget.
//! KV accepts TTLs of [`MIN_TTL`] (60 seconds) or more. KV is eventually
//! consistent: other locations may see an old value for up to 60 seconds
//! after a write or delete. Past a daily limit, KV operations fail; [`fetch`]
//! then logs the failure (prefixed with [`LOG_PREFIX`]) and computes the
//! value, so pages keep working.
//!
//! Values are the JSON of the type; put a version in the key (`stats:v1`) and
//! change it when the type changes. A stored value that no longer decodes is
//! logged and treated as a miss.
//!
//! **HTTP**: [`CacheControl`] and [`ETag`] are response parts, and
//! [`Conditional`] answers `304 Not Modified` without rendering when the
//! browser's copy is current, like Rails' `fresh_when`. That saves CPU (no
//! template rendering) and bandwidth, never KV operations; the database query
//! still runs.
//!
//! The Workers Cache API (`caches.default`) is not wrapped: it is a no-op on
//! `*.workers.dev`, where Ocre apps deploy by default, and still runs the
//! Worker on every request. To have Cloudflare serve whole pages without
//! running the Worker, see [Caching](https://ocre.rs/guides/caching): Workers Cache honors the
//! [`CacheControl::public`] header (hits still count toward the 100,000
//! requests a day).

use std::{fmt, time::Duration};

use axum::{
    extract::FromRequestParts,
    http::{HeaderValue, Method, StatusCode, header, request::Parts},
    response::{IntoResponse, IntoResponseParts, Response, ResponseParts},
};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

pub use crate::runtime::cache::{clear, delete, fetch, read, write};
#[cfg(feature = "html")]
#[cfg_attr(docsrs, doc(cfg(feature = "html")))]
pub use crate::runtime::cache::{fragment, fragments};

/// Name of the KV namespace binding holding cached values: `CACHE`.
///
/// `ocre g cache` adds `CACHE: bindings.kv(),` to
/// `cloudflare.config.ts`. Without it, [`fetch`], [`read`], [`write`](fn@write) and [`delete`]
/// fail with [`Error::Internal`] naming that entry.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::cache::CACHE_BINDING, "CACHE");
/// ```
pub const CACHE_BINDING: &str = "CACHE";

/// Shortest TTL KV accepts (`expirationTtl`): 60 seconds.
///
/// A shorter TTL passed to [`fetch`] or [`write`](fn@write) is an [`Error::Internal`].
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::cache::MIN_TTL.as_secs(), 60);
/// ```
pub const MIN_TTL: Duration = Duration::from_secs(60);

/// Longest key KV accepts, in bytes.
const MAX_KEY_BYTES: usize = 512;

/// Prefix of every line Ocre logs about the cache: `[ocre cache]`.
///
/// Survived KV failures are logged as ``[ocre cache] <operation> `<key>` failed: <error>``
/// (operation `read`, `write` or `decode`); search the Worker logs for it.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::cache::LOG_PREFIX, "[ocre cache]");
/// ```
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
         `{CACHE_BINDING}: bindings.kv(),` to worker.env in cloudflare.config.ts"
    ))
}

/// Logs a KV failure that the caller survives (over the daily limit, a
/// value from an older deploy...).
pub(crate) fn log_failure(operation: &str, key: &str, err: &dyn fmt::Display) {
    crate::error::log_internal(&format!("{LOG_PREFIX} {operation} `{key}` failed: {err}"));
}

/// Name of the Worker variable choosing the cache store: `CACHE_STORE`.
///
/// `"kv"` (or no variable) stores values in the `CACHE` KV namespace;
/// `"null"` turns caching off without code changes (Rails' `:null_store`):
/// [`fetch`] and [`fragment`] always compute, [`read`] finds nothing,
/// [`write`](fn@write) and [`delete`] do nothing, and no KV operation is
/// made. Set it in `.dev.vars` (`CACHE_STORE=null`) to develop without the
/// cache, like Rails' `bin/rails dev:cache`; any other value is an
/// [`Error::Internal`] naming the two valid ones.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::cache::STORE_VAR, "CACHE_STORE");
/// ```
pub const STORE_VAR: &str = "CACHE_STORE";

/// The store [`STORE_VAR`] selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Store {
    Kv,
    Null,
}

/// Reads the value of [`STORE_VAR`] (`None` when the variable is absent).
pub(crate) fn store_kind(value: Option<&str>) -> Result<Store> {
    match value.map(str::trim) {
        None | Some("kv") => Ok(Store::Kv),
        Some("null") => Ok(Store::Null),
        Some(other) => Err(Error::internal(format!(
            "{STORE_VAR} is `{other}`; it must be \"kv\" (the CACHE namespace, the default) or \"null\" (caching off). \
             Fix: change it in .dev.vars or in worker.env of cloudflare.config.ts"
        ))),
    }
}

/// What [`clear`] deleted.
///
/// # Examples
///
/// ```
/// let cleared = ocre::cache::Cleared { deleted: 200, more: true };
/// assert!(cleared.more, "call clear again");
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cleared {
    /// How many values were deleted.
    pub deleted: usize,
    /// Whether keys with the prefix remain.
    pub more: bool,
}

/// Prefix of the KV keys holding [`fragment`]s: `views/`, as in Rails.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::cache::FRAGMENT_PREFIX, "views/");
/// ```
pub const FRAGMENT_PREFIX: &str = "views/";

/// Keys longer than this are hashed by [`key`].
const MAX_PLAIN_KEY: usize = 256;

/// Builds a cache key from its parts joined by `/`, like Rails' `cache_key_with_version`.
///
/// Put in it everything the cached value depends on: the table, the id and
/// `updated_at` of each record (so an update makes a new key and the old
/// value expires with its TTL, no delete needed), the locale when the text
/// is translated, and a version (`v1`) to bump when the template or type
/// changes. Nested fragments compose keys (Russian doll caching): the outer
/// key includes the newest `updated_at` of the records inside.
///
/// Keys longer than 256 bytes become `sha256/<64 hex characters>`, so any
/// key fits KV's 512-byte limit. Pure: no KV operation. Parts are `Sync`,
/// so a key built inline in an awaited call keeps handler futures `Send`.
///
/// # Examples
///
/// ```
/// use ocre::cache::key;
///
/// let (id, updated_at) = (12, "2026-09-29 14:05:00");
/// assert_eq!(key(&[&"posts", &id, &updated_at, &"v1"]), "posts/12/2026-09-29 14:05:00/v1");
///
/// let long = "x".repeat(300);
/// assert!(key(&[&long]).starts_with("sha256/"));
/// assert_eq!(key(&[&long]).len(), 7 + 64);
/// ```
pub fn key(parts: &[&(dyn fmt::Display + Sync)]) -> String {
    let mut key = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            key.push('/');
        }
        key.push_str(&part.to_string());
    }
    if key.len() <= MAX_PLAIN_KEY {
        return key;
    }
    let mut hashed = String::from("sha256/");
    push_hex(&mut hashed, &Sha256::digest(key.as_bytes()));
    hashed
}

fn push_hex(out: &mut String, bytes: &[u8]) {
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).expect("a nibble is a hex digit"));
        out.push(char::from_digit(u32::from(byte & 0xf), 16).expect("a nibble is a hex digit"));
    }
}

/// A cached piece of HTML, from [`fragment`] or [`fragments`].
///
/// It was rendered by an askama template, which escaped its values, so
/// templates write it as is: `{{ row }}` needs no `|safe` (it implements
/// askama's `HtmlSafe` with feature `html`). [`Display`](fmt::Display)
/// writes the HTML.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use askama::Template;
/// use ocre::{Ctx, Result, cache::{self, Fragment}};
///
/// #[derive(Template)]
/// #[template(source = "<li>{{ title }}</li>", ext = "html")]
/// struct Row<'a> {
///     title: &'a str,
/// }
///
/// async fn row(ctx: &Ctx) -> Result<Fragment> {
///     let key = cache::key(&[&"posts", &12, &"2026-09-29 14:05:00", &"v1"]);
///     cache::fragment(ctx, &key, Duration::from_secs(86_400), || Row { title: "Hello" }).await
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment(String);

impl Fragment {
    #[cfg_attr(not(feature = "html"), allow(dead_code))]
    pub(crate) fn new(html: String) -> Self {
        Self(html)
    }

    /// The HTML.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn row(ctx: &ocre::Ctx) -> ocre::Result<ocre::cache::Fragment> { unimplemented!() }
    /// # async fn example(ctx: &ocre::Ctx) -> ocre::Result<()> {
    /// let row = row(ctx).await?;
    /// assert!(row.as_str().starts_with('<'));
    /// # Ok(()) }
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The HTML as a `String`, e.g. for a [`realtime`](crate::realtime) broadcast.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn row(ctx: &ocre::Ctx) -> ocre::Result<ocre::cache::Fragment> { unimplemented!() }
    /// # async fn example(ctx: &ocre::Ctx) -> ocre::Result<()> {
    /// let html: String = row(ctx).await?.into_string();
    /// # let _ = html; Ok(()) }
    /// ```
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for Fragment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Fragments are HTML rendered (and escaped) by askama templates.
#[cfg(feature = "html")]
impl askama::filters::HtmlSafe for Fragment {}

/// The KV key of the fragment `key`.
#[cfg_attr(not(feature = "html"), allow(dead_code))]
pub(crate) fn fragment_key(key: &str) -> Result<String> {
    let full = format!("{FRAGMENT_PREFIX}{key}");
    check_key(&full)?;
    Ok(full)
}

/// Most results the per-request query cache keeps; it is emptied when full.
pub(crate) const QUERY_CACHE_LIMIT: usize = 100;

/// Whether the query cache may serve `sql`: a statement starting with
/// `SELECT` (case-insensitive, after whitespace and `--`/`/* */` comments).
/// Anything else may write, so it empties the cache.
pub(crate) fn is_read_query(sql: &str) -> bool {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(line) = rest.strip_prefix("--") {
            rest = line.split_once('\n').map_or("", |(_, next)| next);
        } else if let Some(block) = rest.strip_prefix("/*") {
            rest = block.split_once("*/").map_or("", |(_, next)| next);
        } else {
            break;
        }
    }
    rest.get(..6).is_some_and(|word| word.eq_ignore_ascii_case("select"))
        && !rest[6..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
}

/// The query cache key of a statement: binding, SQL and parameter values.
pub(crate) fn query_key(binding: &str, sql: &str, params: &[crate::Param]) -> String {
    format!("{binding}\u{0}{sql}\u{0}{params:?}")
}

/// A `Cache-Control` response header, built from one of four policies.
///
/// Add it to a response tuple: `(CacheControl::public(Duration::from_secs(300)), Html(page))`.
/// It implements [`IntoResponseParts`] and [`Display`](fmt::Display) (the header value).
/// Durations are truncated to whole seconds. Free: no KV or D1 operation.
///
/// | Constructor | Header | Use for |
/// |---|---|---|
/// | [`no_store`](Self::no_store) | `no-store` | secrets, one-time tokens |
/// | [`no_cache`](Self::no_cache) | `private, no-cache` | per-visitor pages, with an [`ETag`] |
/// | [`private`](Self::private) | `private, max-age=N` | per-visitor data that may be stale for N seconds |
/// | [`public`](Self::public) | `public, max-age=N` | responses identical for every visitor |
///
/// # Examples
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
///
/// // As a response part:
/// use axum::response::IntoResponse;
/// let response = (CacheControl::public(Duration::from_secs(300)), "hello").into_response();
/// assert_eq!(response.headers()["cache-control"], "public, max-age=300");
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
    /// A `no-store` policy: never keep a copy (pages with secrets, one-time tokens).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::cache::CacheControl::no_store().to_string(), "no-store");
    /// ```
    pub fn no_store() -> Self {
        Self { kind: Kind::NoStore, max_age: 0, stale_while_revalidate: None }
    }

    /// A `private, no-cache` policy: the browser keeps a copy but asks every time.
    ///
    /// With an [`ETag`] and [`Conditional`] the answer is usually a cheap
    /// 304. The right default for pages that depend on the visitor (session,
    /// locale, flash).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::cache::CacheControl::no_cache().to_string(), "private, no-cache");
    /// ```
    pub fn no_cache() -> Self {
        Self { kind: Kind::NoCache, max_age: 0, stale_while_revalidate: None }
    }

    /// A `private, max-age=N` policy: the browser reuses its copy for `max_age` without asking.
    ///
    /// Shared caches (Cloudflare) never store it. `max_age` is truncated to
    /// whole seconds.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// assert_eq!(ocre::cache::CacheControl::private(Duration::from_secs(60)).to_string(), "private, max-age=60");
    /// ```
    pub fn private(max_age: Duration) -> Self {
        Self { kind: Kind::Private, max_age: max_age.as_secs(), stale_while_revalidate: None }
    }

    /// A `public, max-age=N` policy: browsers **and Cloudflare** may reuse it for every visitor.
    ///
    /// Only for responses identical for everyone: no session data, no locale
    /// unless it is in the path. With Workers Cache enabled (`cache: { enabled:
    /// true }` in `worker` of cloudflare.config.ts) such responses are served without running the
    /// Worker (no CPU, but each hit still counts toward the free plan's 100,000
    /// requests a day). `max_age` is truncated to whole seconds.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// assert_eq!(ocre::cache::CacheControl::public(Duration::from_secs(3600)).to_string(), "public, max-age=3600");
    /// ```
    pub fn public(max_age: Duration) -> Self {
        Self { kind: Kind::Public, max_age: max_age.as_secs(), stale_while_revalidate: None }
    }

    /// Adds `stale-while-revalidate`, serving the old copy for up to `window` while fetching a new one.
    ///
    /// Applies after `max-age` runs out. Ignored by
    /// [`no_store`](Self::no_store) and [`no_cache`](Self::no_cache), which
    /// have no `max-age`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use ocre::cache::CacheControl;
    ///
    /// let policy = CacheControl::private(Duration::from_secs(60)).stale_while_revalidate(Duration::from_secs(600));
    /// assert_eq!(policy.to_string(), "private, max-age=60, stale-while-revalidate=600");
    /// assert_eq!(CacheControl::no_store().stale_while_revalidate(Duration::from_secs(600)).to_string(), "no-store");
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

/// An entity tag naming the version of what a page shows: weak (`W/"<hash>"`, the default) or strong (`"<hash>"`).
///
/// Build it from everything the page displays, so it changes when the page
/// would: the records, plus the locale, the signed-in user and the flash when
/// the page shows them. The value is the first 128 bits of a SHA-256, as 32
/// hex characters. It implements [`IntoResponseParts`] (sets `ETag`); pass it
/// to [`Conditional::fresh_when`] to answer 304s. Like Rails, tags are weak
/// unless built with [`strong`](Self::strong).
///
/// # Examples
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
    /// A tag for a version string you build, e.g. `format!("{}-{}", post.id, post.updated_at)`.
    ///
    /// Cheaper than [`of`](Self::of) for large data: only `version` is hashed.
    ///
    /// # Examples
    ///
    /// ```
    /// let etag = ocre::cache::ETag::new("v1");
    /// assert_eq!(etag.as_str().len(), 36); // W/" + 32 hex + "
    /// ```
    pub fn new(version: impl AsRef<[u8]>) -> Self {
        Self::hashed("W/", version.as_ref())
    }

    /// A strong tag (`"<hash>"`, Rails' `strong_etag:`) for a version string you build.
    ///
    /// A strong tag promises that two responses with the same tag are
    /// byte-for-byte identical, which caches and range requests rely on; use
    /// it for a file or an exact body, not for a page whose HTML may vary
    /// (nonces, CSRF tokens). `If-None-Match` compares weakly, so a strong
    /// tag also answers 304s through [`Conditional`].
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::cache::ETag;
    ///
    /// let etag = ETag::strong("report-2026-09.csv");
    /// assert!(etag.as_str().starts_with('"'));
    /// assert_eq!(etag.as_str().len(), 34); // " + 32 hex + "
    /// assert!(etag.is_strong() && !ETag::new("x").is_strong());
    /// ```
    pub fn strong(version: impl AsRef<[u8]>) -> Self {
        Self::hashed("", version.as_ref())
    }

    fn hashed(prefix: &str, version: &[u8]) -> Self {
        let digest = Sha256::digest(version);
        let mut tag = String::with_capacity(36);
        tag.push_str(prefix);
        tag.push('"');
        push_hex(&mut tag, &digest[..16]);
        tag.push('"');
        Self(tag)
    }

    /// Whether the tag is strong (built with [`strong`](Self::strong)).
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(!ocre::cache::ETag::new("v1").is_strong());
    /// ```
    pub fn is_strong(&self) -> bool {
        !self.0.starts_with("W/")
    }

    /// A tag for any serializable data, hashed as its JSON: `ETag::of(&(&posts, i18n.locale()))?`.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] (500) when `data` does not serialize to JSON (e.g.
    /// a map with non-string keys).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::cache::ETag;
    /// assert_eq!(ETag::of(&("Ada", 1))?, ETag::of(&("Ada", 1))?);
    /// assert_ne!(ETag::of(&("Ada", 1))?, ETag::of(&("Ada", 2))?);
    /// # Ok::<(), ocre::Error>(())
    /// ```
    pub fn of<T: Serialize + ?Sized>(data: &T) -> Result<Self> {
        Self::from_json(serde_json::to_vec(data))
    }

    fn from_json(json: serde_json::Result<Vec<u8>>) -> Result<Self> {
        let json = json
            .map_err(|err| Error::internal(format!("cannot compute an ETag: the data does not serialize ({err})")))?;
        Ok(Self::new(json))
    }

    /// The header value, `W/"<32 hex characters>"` (weak) or `"<32 hex characters>"` (strong).
    ///
    /// # Examples
    ///
    /// ```
    /// let etag = ocre::cache::ETag::new("v1");
    /// assert!(etag.as_str().starts_with("W/\"") && etag.as_str().ends_with('"'));
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

/// Extractor for the request's `If-None-Match`, to answer `304 Not Modified` without rendering.
///
/// Like Rails' `fresh_when`. Only GET and HEAD requests are considered; for
/// other methods (and when the header is absent or not ASCII) the client is
/// never fresh. Extraction never fails. Comparison is weak (RFC 9110):
/// `W/"x"` matches `"x"`, and `*` matches any tag. [`Default`] is a request
/// without the header.
///
/// The handler still runs its database queries to build the [`ETag`]; a 304
/// skips only rendering and the body, saving CPU and bandwidth.
///
/// # Examples
///
/// ```
/// use axum::response::Response;
/// use ocre::{Result, cache::{CacheControl, Conditional, ETag}};
///
/// async fn show(conditional: Conditional) -> Result<Response> {
///     let post = ("Hello", 1767225600); // e.g. post::find(&ctx, id).await?.or_404()?
///     let etag = ETag::of(&post)?; // everything the page shows
///     conditional.fresh_when(etag, CacheControl::no_cache(), || Ok(format!("<h1>{}</h1>", post.0)))
/// }
///
/// let response = pollster::block_on(show(Conditional::default()))?;
/// assert_eq!(response.status(), 200);
/// # Ok::<(), ocre::Error>(())
/// ```
#[derive(Debug, Clone, Default)]
pub struct Conditional {
    if_none_match: Option<String>,
}

impl Conditional {
    /// Whether the client already has the version `etag`.
    ///
    /// Always `false` for requests other than GET and HEAD, and when the
    /// request had no `If-None-Match`.
    ///
    /// # Examples
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

    /// Answers `304 Not Modified` when the client's copy is current, or else renders the page.
    ///
    /// When [`is_fresh`](Self::is_fresh), the response is a 304 with the
    /// `ETag` and `Cache-Control` headers and no body, and `render` is not
    /// called. Otherwise `render` runs and its response gets both headers.
    ///
    /// # Errors
    ///
    /// Whatever `render` returns; a 304 never fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use axum::{extract::FromRequestParts, http::Request};
    /// use ocre::cache::{CacheControl, Conditional, ETag};
    ///
    /// let response = Conditional::default().fresh_when(ETag::new("v1"), CacheControl::no_cache(), || Ok("page"))?;
    /// assert_eq!(response.status(), 200);
    /// assert_eq!(response.headers()["cache-control"], "private, no-cache");
    ///
    /// let etag = ETag::new("v1");
    /// let (mut parts, ()) = Request::get("/").header("If-None-Match", etag.as_str()).body(()).unwrap().into_parts();
    /// let conditional = pollster::block_on(Conditional::from_request_parts(&mut parts, &())).unwrap();
    /// let response = conditional.fresh_when(etag, CacheControl::no_cache(), || -> ocre::Result<&str> {
    ///     unreachable!("not rendered")
    /// })?;
    /// assert_eq!(response.status(), 304);
    /// # Ok::<(), ocre::Error>(())
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
