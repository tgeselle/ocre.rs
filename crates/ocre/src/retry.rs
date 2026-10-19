//! Retries of the transient failures Cloudflare asks clients to retry (D1,
//! R2's binding and its S3 API): which errors qualify, which statements may
//! run again, and the backoff between attempts.
//!
//! A call runs at most [`RETRIES`] more times, after about 100 ms then
//! 400 ms (jittered). Each retry is one more subrequest (50 per invocation
//! on the free plan, 1,000 on paid plans).

use std::time::Duration;

/// Attempts after the first one, at most.
pub(crate) const RETRIES: u32 = 2;

/// D1 messages meaning the statement never reached the database (the request
/// was queued for too long or could not be routed): safe to send again,
/// writes included.
const D1_NEVER_RAN: &[&str] = &[
    // "D1 DB is overloaded. Requests queued for too long." and "... Too many requests queued."
    "D1 DB is overloaded",
    "Cannot resolve D1 DB due to transient issue on remote node",
];

/// D1 messages Cloudflare says to retry, but after which a write may or may
/// not have been applied: only read-only statements run again.
const D1_OUTCOME_UNKNOWN: &[&str] = &[
    "Network connection lost",
    "Replica disconnected from primary",
    // "Internal error in D1 DB storage caused object to be reset." and
    // "Internal error while starting up D1 DB storage caused object to be reset."
    "storage caused object to be reset",
    "reset because its code was updated",
];

/// Whether a failed D1 statement may run again: any statement when the error
/// proves it never ran, a read-only one (see [`crate::cache::is_read_query`])
/// on any transient error Cloudflare lists as "Retry the operation".
pub(crate) fn d1_retryable(message: &str, read_only: bool) -> bool {
    D1_NEVER_RAN.iter().any(|known| message.contains(known))
        || (read_only && D1_OUTCOME_UNKNOWN.iter().any(|known| message.contains(known)))
}

/// Whether an R2 binding error is transient: `InternalError` (10001) or
/// `ServiceUnavailable` (10043), which R2's error reference says to retry.
/// The binding puts the code in its messages: `get: We encountered an
/// internal error. Please try again. (10001)`.
pub(crate) fn r2_retryable(message: &str) -> bool {
    message.contains("(10001)") || message.contains("(10043)")
}

/// Whether a call to R2's S3 API may be sent again: R2 answered a transient
/// status (500, 502, 503, 504: `InternalError`, `ServiceUnavailable`), or the
/// request failed without an answer (connection lost). Not after a timeout,
/// which already waited the request's whole timeout.
pub(crate) fn s3_retryable(result: &crate::Result<crate::http::Response>) -> bool {
    match result {
        Ok(response) => matches!(response.status, 500 | 502 | 503 | 504),
        Err(err) => !err.to_string().contains(crate::http::TIMED_OUT),
    }
}

/// The wait before retry `retry` (1 or 2): about 100 ms then 400 ms, between
/// half and one and a half times that, by `random`.
pub(crate) fn backoff(retry: u32, random: u8) -> Duration {
    let base = 100 * 4u64.pow(retry.saturating_sub(1));
    Duration::from_millis(base / 2 + base * u64::from(random) / 256)
}

/// Runs `call`, and again (at most [`RETRIES`] times, waiting [`backoff`]
/// with `sleep`) while `retryable` accepts its outcome (an error, or an
/// HTTP answer with a transient status). `on_retry` hears of each retry
/// before the wait: its number and the outcome given up on.
pub(crate) async fn with_retries<R, F, S>(
    mut call: impl FnMut() -> F,
    retryable: impl Fn(&R) -> bool,
    mut on_retry: impl FnMut(u32, &R),
    sleep: impl Fn(Duration) -> S,
) -> R
where
    F: Future<Output = R>,
    S: Future<Output = ()>,
{
    let mut retry = 0;
    loop {
        let outcome = call().await;
        if retry == RETRIES || !retryable(&outcome) {
            return outcome;
        }
        retry += 1;
        on_retry(retry, &outcome);
        sleep(backoff(retry, crate::token::random_bytes::<1>()[0])).await;
    }
}

#[cfg(test)]
#[path = "../tests/retry.rs"]
mod tests;
