use crate::{Ctx, Error, Result};

/// Counts one request for `key` against the Workers Rate Limiting binding `binding`; 429 when over the limit.
///
/// Rails' `rate_limit to: 10, within: 1.minute, by: ...`: the limit and the
/// period are set on the binding in `wrangler.toml` (`period` is 10 or 60
/// seconds), the key is chosen per call, e.g. `login:<client ip>` or
/// `api:<user id>`. `ocre g auth` adds an `AUTH_RATE_LIMITER` binding and
/// calls this (through the generated `throttle`) in every route that checks
/// a password or sends an email: login, sign-up, magic link, password reset,
/// email confirmation, token and account deletion.
///
/// ```toml
/// [[ratelimits]]
/// name = "AUTH_RATE_LIMITER"
/// namespace_id = "1001"   # any integer unique in your account
/// simple = { limit = 10, period = 60 }
/// ```
///
/// Counters are per Cloudflare location and eventually consistent: a limit,
/// not an exact count. `ocre dev` simulates the binding locally.
///
/// # Free plan
///
/// The Rate Limiting binding is available on the free plan and costs no
/// D1, KV or Durable Object operation; the call does not wait on the
/// network (counters are cached on the machine running the Worker).
///
/// # Errors
///
/// - [`Error::TooManyRequests`] (429) when `key` is over the limit.
/// - [`Error::Internal`] when the binding is missing from `wrangler.toml`
///   (message names the fix) or the call fails.
///
/// # Examples
///
/// ```no_run
/// use axum::{extract::State, http::HeaderMap};
/// use ocre::{Ctx, Result, security::rate_limit};
///
/// async fn login(State(ctx): State<Ctx>, headers: HeaderMap) -> Result<&'static str> {
///     let ip = headers.get("cf-connecting-ip").and_then(|ip| ip.to_str().ok()).unwrap_or("unknown");
///     rate_limit(&ctx, "AUTH_RATE_LIMITER", &format!("login:{ip}")).await?; // 429 when over
///     Ok("checked the password")
/// }
/// # let _ = login;
/// ```
pub async fn rate_limit(ctx: &Ctx, binding: &str, key: &str) -> Result<()> {
    let limiter = ctx.env().rate_limiter(binding).map_err(|err| {
        Error::internal(format!(
            "rate limiting binding `{binding}` is missing ({err}). Fix: add to wrangler.toml\n\
             [[ratelimits]]\nname = \"{binding}\"\nnamespace_id = \"1001\"\nsimple = {{ limit = 10, period = 60 }}"
        ))
    })?;
    let outcome = limiter.limit(key.to_owned()).await?;
    if outcome.success { Ok(()) } else { Err(Error::TooManyRequests) }
}
