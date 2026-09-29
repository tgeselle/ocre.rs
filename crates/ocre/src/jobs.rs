//! Background jobs on Cloudflare Queues, and scheduled tasks on Cron Triggers.
//!
//! A job is a serde value, usually the app's `Job` enum (`src/jobs/mod.rs`,
//! written by `ocre g job`). A handler enqueues it and returns at once; the
//! Worker's `queue` event runs it moments later:
//!
//! ```ignore
//! use crate::jobs::{Job, SendWelcome};
//!
//! ocre::jobs::enqueue(&ctx, &Job::SendWelcome(SendWelcome { user_id: user.id })).await?;
//! ocre::jobs::enqueue_in(&ctx, &job, std::time::Duration::from_secs(3600)).await?;
//! ```
//!
//! ```ignore
//! // src/lib.rs
//! #[worker::event(queue)]
//! async fn queue(batch: worker::MessageBatch<String>, env: worker::Env, _ctx: worker::Context) -> worker::Result<()> {
//!     ocre::jobs::consume(batch, env, jobs::perform).await
//! }
//!
//! #[worker::event(scheduled)]
//! async fn scheduled(event: worker::ScheduledEvent, env: worker::Env, _ctx: worker::ScheduleContext) {
//!     ocre::jobs::cron(event, env, schedules::run).await
//! }
//! ```
//!
//! [`consume`] acknowledges a job that returns `Ok`, retries one that returns
//! `Err` with a growing delay (about 30 s, 1 min, 3 min, 9 min, 27 min: twice
//! the time since it was due), and drops, with a log line, a message it
//! cannot decode (an unknown or changed job), so it is never retried forever.
//! After `max_retries` (wrangler.toml) Cloudflare moves a failing message to
//! the dead-letter queue. Every line Ocre logs starts with [`LOG_PREFIX`] or
//! [`CRON_LOG_PREFIX`].

use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

pub use crate::runtime::jobs::{consume, cron, enqueue, enqueue_in};

use crate::{Error, Result, mail::Email};

/// Name of the queue producer binding every Ocre app sends jobs to
/// (`[[queues.producers]] binding = "JOBS"` in wrangler.toml).
///
/// ```
/// assert_eq!(ocre::jobs::QUEUE_BINDING, "JOBS");
/// ```
pub const QUEUE_BINDING: &str = "JOBS";

/// Prefix of every line Ocre logs about jobs, e.g. `[ocre jobs] send_welcome done`.
///
/// ```
/// assert!("[ocre jobs] send_welcome done".starts_with(ocre::jobs::LOG_PREFIX));
/// ```
pub const LOG_PREFIX: &str = "[ocre jobs]";

/// Prefix of every line Ocre logs about Cron Triggers, e.g. `[ocre cron] 0 3 * * * done`.
///
/// ```
/// assert!("[ocre cron] 0 3 * * * done".starts_with(ocre::jobs::CRON_LOG_PREFIX));
/// ```
pub const CRON_LOG_PREFIX: &str = "[ocre cron]";

/// Longest delay Cloudflare Queues accepts, for [`enqueue_in`] and retries.
///
/// ```
/// assert_eq!(ocre::jobs::MAX_DELAY.as_secs(), 24 * 60 * 60);
/// ```
pub const MAX_DELAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Largest message Ocre sends: Queues' 128 KB limit, minus room for the
/// ~100 bytes of metadata Cloudflare adds.
pub(crate) const MAX_MESSAGE_BYTES: usize = 127_000;

/// First retry delay, in seconds.
pub(crate) const RETRY_BASE_SECONDS: u32 = 30;

/// The JSON text of a queue message: `{"at": 1727000000, "job": {...}}` or
/// `{"at": ..., "mail": {...}}` for [`deliver_later`](crate::mail::deliver_later).
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Envelope {
    /// Unix time the message was due; retries back off from it.
    pub at: i64,
    #[serde(flatten)]
    pub payload: Payload,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Payload {
    /// An app job, as serialized by the app (`{"send_welcome": {"user_id": 1}}`).
    Job(Value),
    /// An email to send with [`mail::send`](crate::mail::send).
    Mail(Email),
}

/// The job as JSON.
pub(crate) fn job_payload<J: Serialize>(job: &J) -> Result<Payload> {
    serde_json::to_value(job)
        .map(Payload::Job)
        .map_err(|err| Error::internal(format!("cannot enqueue the job: it does not serialize to JSON ({err})")))
}

/// Seconds for `delaySeconds`; at most [`MAX_DELAY`].
pub(crate) fn delay_seconds(delay: Duration) -> Result<u32> {
    if delay > MAX_DELAY {
        return Err(Error::internal(format!(
            "cannot delay a job by {} s: Cloudflare Queues delays messages by 24 hours (86400 s) at most. Fix: \
             enqueue it later from a scheduled task (`ocre g schedule`), or store the due time in D1",
            delay.as_secs()
        )));
    }
    Ok(u32::try_from(delay.as_secs()).expect("24 hours fit in u32"))
}

/// The message text, due at `at`. Too large a message is an error that names the fix.
pub(crate) fn encode(payload: Payload, at: i64) -> Result<String> {
    let text = serde_json::to_string(&Envelope { at, payload }).expect("JSON values and emails serialize");
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(Error::internal(format!(
            "cannot enqueue a {} byte message: Cloudflare Queues messages hold 128 KB at most. Fix: store large \
             data in D1 or R2 and put its id in the job",
            text.len()
        )));
    }
    Ok(text)
}

/// Reads a message body. `Err` explains why it is dropped.
pub(crate) fn decode(body: Option<String>) -> std::result::Result<Envelope, String> {
    let text = body.ok_or("the body is not text; Ocre sends jobs as JSON text with `ocre::jobs::enqueue`")?;
    serde_json::from_str(&text).map_err(|err| format!("not an Ocre job message ({err}): {}", preview(&text)))
}

/// A job's name in logs: the variant of an externally tagged enum
/// (`send_welcome` for `{"send_welcome": {...}}` or `"send_welcome"`), else `job`.
pub(crate) fn job_name(job: &Value) -> &str {
    match job {
        Value::String(name) => name,
        Value::Object(map) if map.len() == 1 => map.keys().next().expect("one key"),
        _ => "job",
    }
}

/// The app's job, from the message JSON. `Err` explains why it is dropped.
pub(crate) fn decode_job<J: DeserializeOwned>(job: Value) -> std::result::Result<J, String> {
    let text = job.to_string();
    serde_json::from_value(job).map_err(|err| {
        format!(
            "{} does not match the app's jobs ({err}); a job renamed or changed while messages were queued? \
             Fix: keep accepting the old form in src/jobs/mod.rs",
            preview(&text)
        )
    })
}

/// Delay before retrying a job due at `at` that failed at `now`: twice the
/// time already waited, between 30 s and 24 hours.
pub(crate) fn retry_delay(now: i64, at: i64) -> u32 {
    let waited = u32::try_from(now.saturating_sub(at).max(0)).unwrap_or(u32::MAX);
    let max = u32::try_from(MAX_DELAY.as_secs()).expect("24 hours fit in u32");
    waited.saturating_mul(2).clamp(RETRY_BASE_SECONDS, max)
}

/// Error for a missing `JOBS` queue binding.
pub(crate) fn missing_queue(detail: &str) -> Error {
    Error::internal(format!(
        "the queue binding `{QUEUE_BINDING}` is missing ({detail}). Fix: run `ocre g job <Name>` once; it adds \
         [[queues.producers]] binding = \"{QUEUE_BINDING}\" and the consumer to wrangler.toml"
    ))
}

/// The first 200 characters, for log lines.
fn preview(text: &str) -> String {
    match text.char_indices().nth(200) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
#[path = "../tests/jobs.rs"]
mod tests;
