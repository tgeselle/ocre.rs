use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};
use worker::{Env, KvStore, send::SendFuture};

use super::Ctx;
use crate::{
    Error, Result,
    cache::{CACHE_BINDING, LOG_PREFIX, binding_error, check_key, decode, encode, log_failure, ttl_seconds},
};

fn store(env: &Env) -> Result<KvStore> {
    env.kv(CACHE_BINDING).map_err(|err| binding_error(&err))
}

fn kv_error(operation: &str, key: &str, err: &worker::KvError) -> Error {
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
/// awaited in axum handlers.
///
/// Free plan (September 2026): one KV read per call (100,000 a day), plus one
/// KV write on a miss (**1,000 a day**; one per second per key). A key
/// refreshed every `ttl` seconds costs up to `86,400 / ttl` writes a day.
///
/// # Errors
///
/// - [`Error::Internal`] (500) when the `CACHE` binding is missing, naming the
///   fix: `ocre g cache` adds `[[kv_namespaces]] binding = "CACHE"` to wrangler.toml.
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
    SendFuture::new(async move {
        check_key(key)?;
        let ttl = ttl_seconds(ttl)?;
        let store = store(&env)?;
        match store.get(key).text().await {
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
        let written = match store.put(key, json) {
            Ok(put) => put.expiration_ttl(ttl).execute().await,
            Err(err) => Err(err),
        };
        if let Err(err) = written {
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
/// Free plan: one KV read (100,000 a day).
///
/// # Errors
///
/// - [`Error::Internal`] (500) when the `CACHE` binding is missing, naming the
///   fix: `ocre g cache` adds `[[kv_namespaces]] binding = "CACHE"` to wrangler.toml.
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
    SendFuture::new(async move {
        check_key(key)?;
        match store(&env)?.get(key).text().await {
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
/// Free plan: one KV write (**1,000 a day**; one per second per key).
///
/// # Errors
///
/// - [`Error::Internal`] (500) when the `CACHE` binding is missing, naming the
///   fix: `ocre g cache` adds `[[kv_namespaces]] binding = "CACHE"` to wrangler.toml.
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
    let json = encode(key, value);
    SendFuture::new(async move {
        check_key(key)?;
        let ttl = ttl_seconds(ttl)?;
        let put = store(&env)?.put(key, json?).map_err(|err| kv_error("write", key, &err))?;
        put.expiration_ttl(ttl).execute().await.map_err(|err| kv_error("write", key, &err))?;
        Ok(())
    })
}

/// Removes `key`, e.g. after the data behind it changed (Rails' `Rails.cache.delete`).
///
/// Deleting an absent key succeeds. Other locations may still read the old
/// value for up to 60 seconds.
///
/// Free plan: counts as one KV write (**1,000 a day**).
///
/// # Errors
///
/// - [`Error::Internal`] (500) when the `CACHE` binding is missing, naming the
///   fix: `ocre g cache` adds `[[kv_namespaces]] binding = "CACHE"` to wrangler.toml.
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
    SendFuture::new(async move {
        check_key(key)?;
        store(&env)?.delete(key).await.map_err(|err| kv_error("delete", key, &err))?;
        Ok(())
    })
}
