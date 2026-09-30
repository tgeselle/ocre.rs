use std::{sync::Arc, time::Duration};

use serde::{Serialize, de::DeserializeOwned};
use worker::{Env, KvError, KvStore, send::SendFuture};

use super::{Ctx, ctx::Memo};
#[cfg(feature = "html")]
use crate::cache::{Fragment, fragment_key};
use crate::{
    Error, Result,
    cache::{
        CACHE_BINDING, LOG_PREFIX, STORE_VAR, Store, binding_error, check_key, decode, encode, log_failure, store_kind,
        ttl_seconds,
    },
};

/// The KV namespace, or `None` when `CACHE_STORE` is `"null"` (caching off).
fn store(env: &Env) -> Result<Option<KvStore>> {
    let kind = env.var(STORE_VAR).ok().map(|value| value.to_string());
    match store_kind(kind.as_deref())? {
        Store::Null => Ok(None),
        Store::Kv => env.kv(CACHE_BINDING).map(Some).map_err(|err| binding_error(&err)),
    }
}

/// The text under `key`: from this request's memory when it was read or
/// written already (Rails' local cache), else one KV read.
async fn get_text(memo: &Memo, kv: &KvStore, key: &str) -> std::result::Result<Option<String>, KvError> {
    if let Some(text) = memo.kv(key) {
        return Ok(text);
    }
    let text = kv.get(key).text().await?;
    memo.remember_kv(key, text.clone());
    Ok(text)
}

/// One KV write, remembered for the rest of the request.
async fn put_text(memo: &Memo, kv: &KvStore, key: &str, text: String, ttl: u64) -> std::result::Result<(), KvError> {
    kv.put(key, text.as_str())?.expiration_ttl(ttl).execute().await?;
    memo.remember_kv(key, Some(text));
    Ok(())
}

fn kv_error(operation: &str, key: &str, err: &KvError) -> Error {
    Error::internal(format!("{LOG_PREFIX} {operation} `{key}` failed: {err}"))
}

/// Read-through cache, like Rails' `Rails.cache.fetch`: the value under `key` in KV, or else the result of `compute`.
///
/// On a hit, the stored JSON is decoded as `T` and `compute` does not run. On
/// a miss, `compute` runs and its value is stored for `ttl` (at least
/// [`MIN_TTL`](crate::cache::MIN_TTL), truncated to whole seconds). KV
/// failures (over a daily limit, a value from an older deploy that no longer
/// decodes as `T`) are logged with [`LOG_PREFIX`](crate::cache::LOG_PREFIX)
/// and the value is computed, so the handler keeps working.
///
/// Stored values are the JSON of `T`; put a version in the key (`:v1`) and
/// bump it when `T` changes. The returned future is `Send`, so it can be
/// awaited in axum handlers. A key already read or written during the same
/// request is answered from memory (Rails' local cache). With
/// [`STORE_VAR`](crate::cache::STORE_VAR) set to `"null"`, `compute` always
/// runs and nothing is stored.
///
/// Free plan (September 2026): one KV read per call (100,000 a day), plus one
/// KV write on a miss (**1,000 a day**; one per second per key). A key
/// refreshed every `ttl` seconds costs up to `86,400 / ttl` writes a day.
///
/// # Errors
///
/// - [`Error::Internal`] (500) when the `CACHE` binding is missing, naming the
///   fix: `ocre g cache` adds `CACHE: bindings.kv(),` to cloudflare.config.ts.
/// - [`Error::Internal`] when `key` is empty or longer than 512 bytes, or
///   `ttl` is below 60 seconds.
/// - [`Error::Internal`] when the computed value does not serialize to JSON.
/// - Any error returned by `compute`, unchanged (nothing is stored).
///
/// KV read and write failures are not errors: they are logged.
///
/// # Examples
///
/// ```rust,no_run
/// use std::time::Duration;
///
/// use axum::{Json, extract::State};
/// use ocre::{Ctx, Result};
/// # #[derive(serde::Serialize, serde::Deserialize)]
/// # struct Post { title: String }
/// # async fn recent(_ctx: &Ctx) -> Result<Vec<Post>> { Ok(vec![]) }
///
/// async fn index(State(ctx): State<Ctx>) -> Result<Json<Vec<Post>>> {
///     let posts: Vec<Post> = ocre::cache::fetch(&ctx, "posts:recent:v1", Duration::from_secs(600), || async {
///         recent(&ctx).await // e.g. a D1 query
///     })
///     .await?;
///     Ok(Json(posts))
/// }
/// ```
pub fn fetch<'a, T, F, Fut>(
    ctx: &Ctx,
    key: &'a str,
    ttl: Duration,
    compute: F,
) -> impl Future<Output = Result<T>> + Send + use<'a, T, F, Fut>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let env = ctx.env().clone();
    let memo = Arc::clone(ctx.memo());
    SendFuture::new(async move {
        check_key(key)?;
        let ttl = ttl_seconds(ttl)?;
        let Some(kv) = store(&env)? else {
            return compute().await;
        };
        match get_text(&memo, &kv, key).await {
            Ok(Some(text)) => {
                if let Some(value) = decode(key, &text) {
                    return Ok(value);
                }
            }
            Ok(None) => {}
            Err(err) => log_failure("read", key, &err),
        }
        let value = compute().await?;
        let json = encode(key, &value)?;
        if let Err(err) = put_text(&memo, &kv, key, json, ttl).await {
            log_failure("write", key, &err);
        }
        Ok(value)
    })
}

/// The value under `key`, or `None` when it is absent, expired, unreadable as `T`, or when KV fails.
///
/// Never computes nor stores anything; pair it with [`write`](crate::cache::write)
/// when [`fetch`](crate::cache::fetch)'s closure form does not fit. KV
/// failures and values that no longer decode as `T` are logged with
/// [`LOG_PREFIX`](crate::cache::LOG_PREFIX) and read as `None`.
///
/// Free plan: one KV read (100,000 a day), none when the request already
/// read or wrote `key`, and none with [`STORE_VAR`](crate::cache::STORE_VAR)
/// set to `"null"` (always `None`).
///
/// # Errors
///
/// - [`Error::Internal`] (500) when the `CACHE` binding is missing, naming the
///   fix: `ocre g cache` adds `CACHE: bindings.kv(),` to cloudflare.config.ts.
/// - [`Error::Internal`] when `key` is empty or longer than 512 bytes.
///
/// # Examples
///
/// ```rust,no_run
/// use axum::extract::State;
/// use ocre::{Ctx, Result};
/// # #[derive(serde::Deserialize)]
/// # struct Rates { eur: f64 }
///
/// async fn rate(State(ctx): State<Ctx>) -> Result<String> {
///     let rates: Option<Rates> = ocre::cache::read(&ctx, "rates:v1").await?;
///     Ok(rates.map_or_else(|| "unknown".to_owned(), |rates| rates.eur.to_string()))
/// }
/// ```
pub fn read<'a, T: DeserializeOwned>(
    ctx: &Ctx,
    key: &'a str,
) -> impl Future<Output = Result<Option<T>>> + Send + use<'a, T> {
    let env = ctx.env().clone();
    let memo = Arc::clone(ctx.memo());
    SendFuture::new(async move {
        check_key(key)?;
        let Some(kv) = store(&env)? else {
            return Ok(None);
        };
        match get_text(&memo, &kv, key).await {
            Ok(text) => Ok(text.and_then(|text| decode(key, &text))),
            Err(err) => {
                log_failure("read", key, &err);
                Ok(None)
            }
        }
    })
}

/// Stores `value` as JSON under `key` for `ttl`, replacing any previous value.
///
/// `ttl` must be at least [`MIN_TTL`](crate::cache::MIN_TTL) (60 seconds)
/// and is truncated to whole seconds. Other locations may still read the old
/// value for up to 60 seconds. Unlike [`fetch`](crate::cache::fetch), a KV
/// failure is an error, not a log line.
///
/// Free plan: one KV write (**1,000 a day**; one per second per key); none
/// with [`STORE_VAR`](crate::cache::STORE_VAR) set to `"null"`.
///
/// # Errors
///
/// - [`Error::Internal`] (500) when the `CACHE` binding is missing, naming the
///   fix: `ocre g cache` adds `CACHE: bindings.kv(),` to cloudflare.config.ts.
/// - [`Error::Internal`] when `key` is empty or longer than 512 bytes, `ttl`
///   is below 60 seconds, or `value` does not serialize to JSON.
/// - [`Error::Internal`] when KV rejects the write, e.g. past the daily limit
///   (message prefixed with [`LOG_PREFIX`](crate::cache::LOG_PREFIX)).
///
/// # Examples
///
/// ```rust,no_run
/// use std::time::Duration;
///
/// use axum::extract::State;
/// use ocre::{Ctx, Result};
/// # #[derive(serde::Serialize)]
/// # struct Rates { eur: f64 }
///
/// async fn refresh(State(ctx): State<Ctx>) -> Result<()> {
///     let rates = Rates { eur: 0.92 }; // e.g. from a third-party API
///     ocre::cache::write(&ctx, "rates:v1", &rates, Duration::from_secs(3600)).await
/// }
/// ```
pub fn write<'a, T: Serialize + ?Sized>(
    ctx: &Ctx,
    key: &'a str,
    value: &T,
    ttl: Duration,
) -> impl Future<Output = Result<()>> + Send + use<'a, T> {
    let env = ctx.env().clone();
    let memo = Arc::clone(ctx.memo());
    let json = encode(key, value);
    SendFuture::new(async move {
        check_key(key)?;
        let ttl = ttl_seconds(ttl)?;
        let json = json?;
        let Some(kv) = store(&env)? else {
            return Ok(());
        };
        put_text(&memo, &kv, key, json, ttl).await.map_err(|err| kv_error("write", key, &err))
    })
}

/// Removes `key`, e.g. after the data behind it changed (Rails' `Rails.cache.delete`).
///
/// Deleting an absent key succeeds. Other locations may still read the old
/// value for up to 60 seconds.
///
/// Free plan: counts as one KV write (**1,000 a day**); none with
/// [`STORE_VAR`](crate::cache::STORE_VAR) set to `"null"`.
///
/// # Errors
///
/// - [`Error::Internal`] (500) when the `CACHE` binding is missing, naming the
///   fix: `ocre g cache` adds `CACHE: bindings.kv(),` to cloudflare.config.ts.
/// - [`Error::Internal`] when `key` is empty or longer than 512 bytes.
/// - [`Error::Internal`] when KV rejects the delete, e.g. past the daily limit
///   (message prefixed with [`LOG_PREFIX`](crate::cache::LOG_PREFIX)).
///
/// # Examples
///
/// ```rust,no_run
/// use axum::extract::{Path, State};
/// use ocre::{Ctx, Result};
///
/// async fn update(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<()> {
///     # let _ = id;
///     // post::update(&ctx, id, changes).await?;
///     ocre::cache::delete(&ctx, "posts:recent:v1").await
/// }
/// ```
pub fn delete<'a>(ctx: &Ctx, key: &'a str) -> impl Future<Output = Result<()>> + Send + use<'a> {
    let env = ctx.env().clone();
    let memo = Arc::clone(ctx.memo());
    SendFuture::new(async move {
        check_key(key)?;
        let Some(kv) = store(&env)? else {
            return Ok(());
        };
        kv.delete(key).await.map_err(|err| kv_error("delete", key, &err))?;
        memo.remember_kv(key, None);
        Ok(())
    })
}

/// Deletes up to `limit` cached values whose key starts with `prefix` (Rails'
/// `Rails.cache.clear`, bounded): `""` for everything, `"views/"` for
/// fragments, `"posts/"` for one family of keys. Call it again while
/// [`Cleared::more`](crate::cache::Cleared::more) is true, e.g. from a
/// scheduled task after a deploy that changed the cached data's shape.
///
/// Free plan: one KV list (of 1,000 a day) plus one KV write per deleted
/// key (**1,000 writes a day**), so keep `limit` small; with
/// [`STORE_VAR`](crate::cache::STORE_VAR) set to `"null"` nothing is deleted.
///
/// # Errors
///
/// - [`Error::Internal`] (500) when the `CACHE` binding is missing (`ocre g cache` adds it).
/// - [`Error::Internal`] when KV rejects the list or a delete, e.g. past the daily limit.
///
/// # Examples
///
/// ```rust,no_run
/// use ocre::{Ctx, Result};
///
/// async fn nightly(ctx: Ctx) -> Result<()> {
///     let cleared = ocre::cache::clear(&ctx, "views/", 200).await?;
///     ctx.log().info(format_args!("cleared {} fragments, more: {}", cleared.deleted, cleared.more));
///     Ok(())
/// }
/// # let _ = nightly;
/// ```
pub fn clear<'a>(
    ctx: &Ctx,
    prefix: &'a str,
    limit: usize,
) -> impl Future<Output = Result<crate::cache::Cleared>> + Send + use<'a> {
    let env = ctx.env().clone();
    let memo = Arc::clone(ctx.memo());
    SendFuture::new(async move {
        let Some(kv) = store(&env)? else {
            return Ok(crate::cache::Cleared::default());
        };
        let page = kv.list().prefix(prefix.to_owned()).limit(limit.clamp(1, 1000) as u64).execute().await;
        let page = page.map_err(|err| kv_error("list", prefix, &err))?;
        for key in &page.keys {
            kv.delete(&key.name).await.map_err(|err| kv_error("delete", &key.name, &err))?;
            memo.remember_kv(&key.name, None);
        }
        Ok(crate::cache::Cleared { deleted: page.keys.len(), more: !page.list_complete })
    })
}

/// Fragment caching, like Rails' `<% cache post do %>`: the HTML stored under `key`, or else the template `build` returns, rendered and stored.
///
/// askama templates cannot wait for KV, so the handler caches the costly
/// part of the page and passes the [`Fragment`] to the page template, which
/// writes it with `{{ fragment }}` (no `|safe` needed). `key` comes from
/// [`cache::key`](crate::cache::key) with the records' ids and
/// `updated_at`, the locale if translated, and a version to bump when the
/// template changes (Rails derives it from a template digest; Ocre keeps it
/// explicit, so a deploy does not rewrite every fragment against the daily
/// write quota). The KV key is `key` prefixed with
/// [`FRAGMENT_PREFIX`](crate::cache::FRAGMENT_PREFIX). Conditional caching
/// (`cache_if`) is an `if` around the call, rendering the template directly
/// otherwise.
///
/// Failures behave like [`fetch`](crate::cache::fetch): KV errors are
/// logged and the template is rendered; with
/// [`STORE_VAR`](crate::cache::STORE_VAR) set to `"null"` it always renders.
/// The HTML is stored as is (no JSON), and remembered for the rest of the
/// request.
///
/// Free plan: one KV read (100,000 a day), plus one KV write on a miss
/// (**1,000 a day**). Worth it for fragments that take milliseconds of CPU
/// to render (long lists, Markdown), read far more often than their records
/// change; a cheap fragment costs more quota than the CPU it saves.
///
/// # Errors
///
/// - [`Error::Internal`] (500) when the `CACHE` binding is missing, naming the
///   fix: `ocre g cache` adds `CACHE: bindings.kv(),` to cloudflare.config.ts.
/// - [`Error::Internal`] when the KV key is longer than 512 bytes or `ttl` is
///   below 60 seconds.
/// - [`Error::Internal`] when the template fails to render.
///
/// # Examples
///
/// ```rust,no_run
/// use std::time::Duration;
///
/// use askama::Template;
/// use axum::{extract::{Path, State}, response::Html};
/// use ocre::{Ctx, Result, cache::{self, Fragment}, render};
///
/// struct Post { id: i64, title: String, updated_at: String }
///
/// #[derive(Template)]
/// #[template(source = "<article><h2>{{ post.title }}</h2></article>", ext = "html")]
/// struct Card<'a> { post: &'a Post }
///
/// #[derive(Template)]
/// #[template(source = "<main>{{ card }}</main>", ext = "html")]
/// struct Show { card: Fragment }
///
/// async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Html<String>> {
///     let post = Post { id, title: "Hello".into(), updated_at: "2026-09-29 14:05:00".into() }; // from D1
///     let key = cache::key(&[&"posts", &post.id, &post.updated_at, &"card-v1"]);
///     let card = cache::fragment(&ctx, &key, Duration::from_secs(86_400), || Card { post: &post }).await?;
///     render(&Show { card })
/// }
/// ```
#[cfg(feature = "html")]
pub fn fragment<T, F>(
    ctx: &Ctx,
    key: &str,
    ttl: Duration,
    build: F,
) -> impl Future<Output = Result<Fragment>> + Send + use<T, F>
where
    T: askama::Template,
    F: FnOnce() -> T,
{
    let env = ctx.env().clone();
    let memo = Arc::clone(ctx.memo());
    let key = fragment_key(key);
    SendFuture::new(async move {
        let key = key?;
        let ttl = ttl_seconds(ttl)?;
        let Some(kv) = store(&env)? else {
            return Ok(Fragment::new(build().render()?));
        };
        match get_text(&memo, &kv, &key).await {
            Ok(Some(html)) => return Ok(Fragment::new(html)),
            Ok(None) => {}
            Err(err) => log_failure("read", &key, &err),
        }
        let html = build().render()?;
        if let Err(err) = put_text(&memo, &kv, &key, html.clone(), ttl).await {
            log_failure("write", &key, &err);
        }
        Ok(Fragment::new(html))
    })
}

/// Collection caching, like Rails' `render collection:, cached: true`: one [`Fragment`] per item, read from KV in bulk.
///
/// `key` gives each item's cache key (see [`cache::key`](crate::cache::key));
/// all keys are read with KV bulk reads (up to 100 keys per read, one
/// subrequest each), then only the missing items are rendered with `build`
/// and stored, one KV write each. The fragments come back in the order of
/// `items`. Everything else works as in [`fragment`](crate::cache::fragment).
///
/// Free plan: KV bills a bulk read as one read per key (100,000 a day) and
/// each miss as one write (**1,000 a day**): a list of 20 posts costs 20
/// reads per view, and up to 20 writes after the posts change.
///
/// # Errors
///
/// - [`Error::Internal`] (500) when the `CACHE` binding is missing, naming the
///   fix: `ocre g cache` adds `CACHE: bindings.kv(),` to cloudflare.config.ts.
/// - [`Error::Internal`] when a KV key is longer than 512 bytes or `ttl` is
///   below 60 seconds.
/// - [`Error::Internal`] when a template fails to render.
///
/// # Examples
///
/// ```rust,no_run
/// use std::time::Duration;
///
/// use askama::Template;
/// use axum::{extract::State, response::Html};
/// use ocre::{Ctx, Result, cache::{self, Fragment}, render};
///
/// struct Post { id: i64, title: String, updated_at: String }
///
/// #[derive(Template)]
/// #[template(source = "<li>{{ post.title }}</li>", ext = "html")]
/// struct Row<'a> { post: &'a Post }
///
/// #[derive(Template)]
/// #[template(source = "<ul>{% for row in rows %}{{ row }}{% endfor %}</ul>", ext = "html")]
/// struct Index { rows: Vec<Fragment> }
///
/// async fn index(State(ctx): State<Ctx>) -> Result<Html<String>> {
///     let posts: Vec<Post> = Vec::new(); // e.g. post::all(&ctx, page).await?
///     let rows = cache::fragments(
///         &ctx,
///         &posts,
///         Duration::from_secs(86_400),
///         |post| cache::key(&[&"posts", &post.id, &post.updated_at, &"row-v1"]),
///         |post| Row { post },
///     )
///     .await?;
///     render(&Index { rows })
/// }
/// ```
#[cfg(feature = "html")]
pub fn fragments<'a, I, T, K, F>(
    ctx: &Ctx,
    items: &'a [I],
    ttl: Duration,
    key: K,
    build: F,
) -> impl Future<Output = Result<Vec<Fragment>>> + Send + use<'a, I, T, K, F>
where
    T: askama::Template,
    K: Fn(&I) -> String,
    F: Fn(&'a I) -> T,
{
    let env = ctx.env().clone();
    let memo = Arc::clone(ctx.memo());
    let keys: Result<Vec<String>> = items.iter().map(|item| fragment_key(&key(item))).collect();
    SendFuture::new(async move {
        let keys = keys?;
        let ttl = ttl_seconds(ttl)?;
        let Some(kv) = store(&env)? else {
            return items.iter().map(|item| Ok(Fragment::new(build(item).render()?))).collect();
        };
        let mut unknown: Vec<&String> = keys.iter().filter(|key| memo.kv(key).is_none()).collect();
        unknown.sort_unstable();
        unknown.dedup();
        for chunk in unknown.chunks(100) {
            match kv.get_bulk(chunk).text().await {
                Ok(found) => {
                    for (key, text) in found {
                        memo.remember_kv(&key, text);
                    }
                }
                Err(err) => log_failure("read", chunk[0], &err),
            }
        }
        let mut fragments = Vec::with_capacity(items.len());
        for (item, key) in items.iter().zip(&keys) {
            if let Some(Some(html)) = memo.kv(key) {
                fragments.push(Fragment::new(html));
                continue;
            }
            let html = build(item).render()?;
            if let Err(err) = put_text(&memo, &kv, key, html.clone(), ttl).await {
                log_failure("write", key, &err);
            }
            fragments.push(Fragment::new(html));
        }
        Ok(fragments)
    })
}
