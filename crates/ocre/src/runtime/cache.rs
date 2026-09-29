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

/// Read-through cache, like Rails' `Rails.cache.fetch`: the value under
/// `key` in KV, or else the result of `compute`, stored for `ttl` (60 seconds
/// or more). One KV read per call, plus one write on a miss (free plan:
/// 1,000 writes a day). KV failures (over a daily limit, a value from an
/// older deploy that no longer deserializes) are logged and the value is
/// computed; only a missing `CACHE` binding or an invalid key/TTL is an error.
///
/// ```ignore
/// let posts: Vec<Post> = ocre::cache::fetch(&ctx, "posts:recent:v1", Duration::from_secs(600), || async {
///     post::recent(&ctx).await
/// })
/// .await?;
/// ```
///
/// Stored values are the JSON of `T`; put a version in the key (`:v1`) and
/// bump it when `T` changes. The returned future is `Send`.
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

/// The value under `key`, or `None` when absent, expired, unreadable as `T`
/// or when KV fails (logged). One KV read.
///
/// ```ignore
/// let rates: Option<Rates> = ocre::cache::read(&ctx, "rates:v1").await?;
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

/// Stores `value` under `key` for `ttl` (60 seconds or more). One KV write
/// (1,000 a day on the free plan; one per second per key). Fails when KV
/// does, e.g. past the daily limit.
///
/// ```ignore
/// ocre::cache::write(&ctx, "rates:v1", &rates, Duration::from_secs(3600)).await?;
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

/// Removes `key`, e.g. after the data behind it changed. Counts as a KV
/// write (1,000 a day on the free plan). Other locations may still read the
/// old value for up to 60 seconds.
///
/// ```ignore
/// post::update(&ctx, id, changes).await?;
/// ocre::cache::delete(&ctx, "posts:recent:v1").await?;
/// ```
pub fn delete<'a>(ctx: &Ctx, key: &'a str) -> impl Future<Output = Result<()>> + Send + use<'a> {
    let env = ctx.env().clone();
    SendFuture::new(async move {
        check_key(key)?;
        store(&env)?.delete(key).await.map_err(|err| kv_error("delete", key, &err))?;
        Ok(())
    })
}
