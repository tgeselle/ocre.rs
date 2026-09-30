//! Background jobs (Cloudflare Queues) and scheduled tasks (Cron Triggers).
//!
//! Rails' Active Job, on [Cloudflare Queues](https://developers.cloudflare.com/queues/)
//! (in the Workers Free plan since February 2026). The app's Worker is both
//! the producer and the consumer of its queues: `<app>-jobs`, bound as
//! [`QUEUE_BINDING`], plus one per named queue (`<app>-jobs-urgent`, bound
//! as `JOBS_URGENT`, see [`queue`]). A job is a serde value, usually the
//! app's `Job` enum (`src/jobs/mod.rs`, written by `ocre g job`), and
//! dispatch is a plain `match` in the app's `perform` function, not a registry.
//!
//! A handler enqueues with [`enqueue`], [`enqueue_in`] or [`enqueue_all`]
//! (generated jobs wrap it as `job.perform_later(&ctx)`) and returns as soon
//! as Cloudflare stored the message; the Worker's `queue` event runs it
//! moments later through [`consume`]. Scheduled tasks run from the
//! `scheduled` event through [`cron`]:
//!
//! ```no_run
//! mod jobs {
//!     use ocre::{Ctx, Result};
//!     use serde::{Deserialize, Serialize};
//!
//!     #[derive(Serialize, Deserialize)]
//!     #[serde(rename_all = "snake_case")]
//!     pub enum Job {
//!         SendWelcome { user_id: i64 },
//!     }
//!
//!     pub async fn perform(_ctx: Ctx, job: Job) -> Result<()> {
//!         match job {
//!             Job::SendWelcome { user_id } => {
//!                 # let _ = user_id;
//!                 Ok(())
//!             }
//!         }
//!     }
//! }
//!
//! mod schedules {
//!     pub async fn run(_ctx: ocre::Ctx, cron: String) -> ocre::Result<()> {
//!         # let _ = cron;
//!         Ok(())
//!     }
//! }
//!
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
//!
//! // A handler
//! async fn create(axum::extract::State(ctx): axum::extract::State<ocre::Ctx>) -> ocre::Result<&'static str> {
//!     ocre::jobs::enqueue(&ctx, &jobs::Job::SendWelcome { user_id: 1 }).await?;
//!     Ok("created")
//! }
//! # fn main() {}
//! ```
//!
//! A message is JSON text, `{"at": <due unix time>, "job": {"send_welcome": {"user_id": 1}}}`.
//! [`consume`] acknowledges a job that returns `Ok`; drops, with a log line,
//! one that returns an error another try cannot fix (a 4xx
//! [`Error`] such as `NotFound`: Rails' `discard_on`); retries any other
//! `Err` with a growing delay (30 s, 1 min, 3 min, 9 min, 27 min: twice the
//! time since it was due); and drops a message it cannot decode (an unknown
//! or changed job), so it is never retried forever. After `maxRetries: 5`
//! (cloudflare.config.ts) Cloudflare moves a failing message to the dead-letter
//! queue `<app>-jobs-failed`, kept 24 hours. Delivery is at-least-once: write
//! jobs to be safe to repeat. Every line Ocre logs starts with
//! [`LOG_PREFIX`] or [`CRON_LOG_PREFIX`].
//!
//! # Free-plan budget (September 2026)
//!
//! - **Queues operations**: 10,000 a day. A message costs 3 (write, read,
//!   delete), each retry 1 more read, a dead-lettered message 1 more write.
//!   Ocre sends one message per job: about 3,300 jobs a day.
//! - **Retention**: 24 hours on Free; the last retry comes after about 40 minutes.
//! - **Message size**: 128 KB; [`enqueue`] refuses larger jobs (pass ids).
//!   [`enqueue_all`] sends 100 messages (256 KB) per call.
//! - **Delay**: 24 hours at most, on send and on retry;
//!   [`enqueue_in`] refuses longer delays
//!   ([`MAX_DELAY`]).
//! - **Batches**: up to 100 messages and 60 s wait; the generated
//!   `max_batch_size = 10`, `max_batch_timeout = 5` make one consumer run
//!   (one Worker invocation) per 10 jobs.
//! - **CPU**: 10 ms per invocation, consumer batches and cron runs included:
//!   jobs should be I/O (D1, mail, `fetch`); lower `max_batch_size` for CPU-heavy jobs.
//! - **Cron Triggers**: 5 per account; run several tasks from one cron.

use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

pub use crate::runtime::jobs::{Queue, consume, cron, enqueue, enqueue_all, enqueue_in, queue};

use crate::{Error, Result, mail::Email};

/// Name of the queue producer binding every Ocre app sends jobs to.
///
/// `ocre g job` adds `JOBS: bindings.queue({ name: "<app>-jobs" })` (and the
/// consumer trigger) to cloudflare.config.ts; without it, enqueueing is an
/// [`Error::Internal`] naming that entry.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::jobs::QUEUE_BINDING, "JOBS");
/// ```
pub const QUEUE_BINDING: &str = "JOBS";

/// Name of the queue [`enqueue`] and [`enqueue_in`] use: `default`, bound as [`QUEUE_BINDING`].
///
/// Other queues (`ocre g job <Name> --queue urgent`) are bound as
/// `JOBS_<NAME>` and used through [`queue`].
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::jobs::DEFAULT_QUEUE, "default");
/// ```
pub const DEFAULT_QUEUE: &str = "default";

/// Prefix of every line Ocre logs about jobs, e.g. `[ocre jobs] send_welcome done`.
///
/// [`consume`] logs `<prefix> <job> done`,
/// `<prefix> <job> failed, retrying in <n> s: <error>`,
/// `<prefix> <job> discarded, not retried: <error>` and
/// `<prefix> dropped message <id>: <reason>`.
///
/// # Examples
///
/// ```
/// assert!("[ocre jobs] send_welcome done".starts_with(ocre::jobs::LOG_PREFIX));
/// ```
pub const LOG_PREFIX: &str = "[ocre jobs]";

/// Prefix of every line Ocre logs about Cron Triggers, e.g. `[ocre cron] 0 3 * * * done`.
///
/// [`cron`] logs `<prefix> <cron> done` or `<prefix> <cron> failed: <error>`.
///
/// # Examples
///
/// ```
/// assert!("[ocre cron] 0 3 * * * done".starts_with(ocre::jobs::CRON_LOG_PREFIX));
/// ```
pub const CRON_LOG_PREFIX: &str = "[ocre cron]";

/// Longest delay Cloudflare Queues accepts: 24 hours, for [`enqueue_in`] and retries.
///
/// A longer delay makes [`enqueue_in`] fail; retry
/// delays are capped to it.
///
/// # Examples
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Payload {
    /// An app job, as serialized by the app (`{"send_welcome": {"user_id": 1}}`).
    Job(Value),
    /// An email to send with [`mail::send`](crate::mail::send).
    Mail(Box<Email>),
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

/// Error for a missing queue producer binding.
pub(crate) fn missing_queue(binding: &str, detail: &str) -> Error {
    let fix = if binding == QUEUE_BINDING {
        "run `ocre g job <Name>` once; it adds `JOBS: bindings.queue(...)` and its `triggers.queue(...)` consumer"
            .to_owned()
    } else {
        let name = binding.trim_start_matches("JOBS_").to_ascii_lowercase().replace('_', "-");
        format!(
            "run `ocre g job <Name> --queue {name}`; it adds `{binding}: bindings.queue(...)` and its `triggers.queue(...)` consumer"
        )
    };
    Error::internal(format!("the queue binding `{binding}` is missing ({detail}). Fix: {fix} to cloudflare.config.ts"))
}

/// The producer binding of a named queue: `default` is `JOBS`, `urgent` is
/// `JOBS_URGENT`, `low-priority` is `JOBS_LOW_PRIORITY`. A name that is not
/// lowercase letters, digits and `-` is an error.
pub(crate) fn binding(queue: &str) -> Result<String> {
    let valid = !queue.is_empty()
        && queue.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !queue.starts_with('-')
        && !queue.ends_with('-');
    if !valid {
        return Err(Error::internal(format!(
            "invalid queue name {queue:?}. Fix: use lowercase letters, digits and `-`, e.g. \"urgent\", as in \
             `ocre g job <Name> --queue urgent`"
        )));
    }
    Ok(match queue {
        DEFAULT_QUEUE => QUEUE_BINDING.to_owned(),
        name => format!("{QUEUE_BINDING}_{}", name.to_ascii_uppercase().replace('-', "_")),
    })
}

/// Most messages in one `sendBatch` call.
pub(crate) const MAX_BATCH_MESSAGES: usize = 100;
/// Most bytes in one `sendBatch` call: Queues' 256 KB, minus room for metadata.
pub(crate) const MAX_BATCH_BYTES: usize = 250_000;

/// Splits message bodies into `sendBatch` calls: 100 messages and 256 KB at most each, in order.
pub(crate) fn batches(bodies: Vec<String>) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut size = 0;
    for body in bodies {
        let full = out
            .last()
            .is_none_or(|batch| batch.len() == MAX_BATCH_MESSAGES || size + body.len() + 100 > MAX_BATCH_BYTES);
        if full {
            out.push(Vec::new());
            size = 0;
        }
        size += body.len() + 100;
        out.last_mut().expect("pushed above").push(body);
    }
    out
}

/// Whether a failed job is dropped instead of retried: errors that another
/// try cannot fix (a missing record, bad input, a refused permission), like
/// Rails' `discard_on`. `Internal` and `TooManyRequests` are retried.
pub(crate) fn discards(err: &Error) -> bool {
    matches!(
        err,
        Error::NotFound
            | Error::BadRequest(_)
            | Error::Unauthorized
            | Error::Forbidden
            | Error::Invalid(_)
            | Error::PayloadTooLarge(_)
            | Error::Conflict(_)
    )
}

/// The first 200 characters, for log lines.
fn preview(text: &str) -> String {
    match text.char_indices().nth(200) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_owned(),
    }
}

/// Development endpoint listing recent jobs, served by `ocre dev` only, for
/// tests (Rails' `assert_enqueued_with` and `assert_performed_jobs`).
///
/// `GET /ocre/dev/jobs.json` answers the last 50 jobs this Worker instance
/// enqueued and the last 50 it ran, oldest first:
/// `{"enqueued": [{"id": 1, "queue": "JOBS", "job": {"send_welcome": {"user_id": 7}}}],
/// "performed": [{"id": 2, "job": "send_welcome", "outcome": "done"}]}`. The
/// outcome is `done`, `discarded` or `retried`, as [`consume`] logs it.
/// `ocre::testing::Client::jobs` reads it.
///
/// Debug builds only (`ocre dev`); release builds (`ocre deploy`) get an
/// empty router, so it is a 404 in production. The first `ocre g job`
/// merges it into `routes()`. It uses no billed resource: the lists live
/// in the Worker's memory.
///
/// # Examples
///
/// ```
/// use axum::Router;
/// use ocre::Ctx;
///
/// fn routes() -> Router<Ctx> {
///     Router::new().merge(ocre::jobs::dev_routes())
/// }
/// # let _ = routes;
/// ```
pub fn dev_routes<S: Clone + Send + Sync + 'static>() -> axum::Router<S> {
    #[cfg(not(debug_assertions))]
    {
        axum::Router::new()
    }
    #[cfg(debug_assertions)]
    axum::Router::new().route("/ocre/dev/jobs.json", axum::routing::get(|| async { axum::Json(dev::snapshot()) }))
}

/// Remembers a job sent to the queue bound as `binding` (debug builds only; emails are listed by the mail pages).
pub(crate) fn record_enqueued(binding: &str, payload: &Payload) {
    #[cfg(debug_assertions)]
    if let Payload::Job(job) = payload {
        dev::enqueued(binding, job);
    }
    #[cfg(not(debug_assertions))]
    let _ = (binding, payload);
}

/// Remembers how a job run ended: `done`, `discarded` or `retried` (debug builds only).
pub(crate) fn record_performed(job: &str, outcome: &str) {
    #[cfg(debug_assertions)]
    dev::performed(job, outcome);
    #[cfg(not(debug_assertions))]
    let _ = (job, outcome);
}

#[cfg(debug_assertions)]
mod dev {
    use std::sync::{Mutex, PoisonError};

    use serde::Serialize;
    use serde_json::Value;

    /// How many jobs each list keeps.
    const KEEP: usize = 50;

    #[derive(Debug, Clone, Serialize)]
    pub(super) struct Enqueued {
        id: u64,
        queue: String,
        job: Value,
    }

    #[derive(Debug, Clone, Serialize)]
    pub(super) struct Performed {
        id: u64,
        job: String,
        outcome: String,
    }

    #[derive(Debug, Clone, Serialize)]
    pub(super) struct Snapshot {
        enqueued: Vec<Enqueued>,
        performed: Vec<Performed>,
    }

    static JOBS: Mutex<(u64, Snapshot)> = Mutex::new((1, Snapshot { enqueued: Vec::new(), performed: Vec::new() }));

    fn keep<T>(list: &mut Vec<T>, item: T) {
        list.push(item);
        if list.len() > KEEP {
            list.remove(0);
        }
    }

    pub(super) fn enqueued(queue: &str, job: &Value) {
        let mut jobs = JOBS.lock().unwrap_or_else(PoisonError::into_inner);
        let id = jobs.0;
        jobs.0 += 1;
        keep(&mut jobs.1.enqueued, Enqueued { id, queue: queue.to_owned(), job: job.clone() });
    }

    pub(super) fn performed(job: &str, outcome: &str) {
        let mut jobs = JOBS.lock().unwrap_or_else(PoisonError::into_inner);
        let id = jobs.0;
        jobs.0 += 1;
        keep(&mut jobs.1.performed, Performed { id, job: job.to_owned(), outcome: outcome.to_owned() });
    }

    pub(super) fn snapshot() -> Snapshot {
        JOBS.lock().unwrap_or_else(PoisonError::into_inner).1.clone()
    }
}

#[cfg(test)]
#[path = "../tests/jobs.rs"]
mod tests;
