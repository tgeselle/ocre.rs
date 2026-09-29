use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};
use worker::{
    Env, MessageBatch, MessageBuilder, MessageExt, QueueContentType, QueueRetryOptionsBuilder, ScheduledEvent,
    send::SendFuture,
};

use super::Ctx;
use crate::{
    Result,
    jobs::{
        CRON_LOG_PREFIX, LOG_PREFIX, Payload, QUEUE_BINDING, decode, decode_job, delay_seconds, encode, job_name,
        job_payload, missing_queue, retry_delay,
    },
    now,
};

/// Sends `job` to the `JOBS` queue, to run in the background through [`consume`](crate::jobs::consume).
///
/// Returns once Cloudflare stored the message; the `queue` event runs it
/// within seconds (`max_batch_timeout = 5` in wrangler.toml). The job is
/// serialized to JSON and wrapped as `{"at": <now>, "job": ...}`; the whole
/// message must fit in 128 KB, so pass ids, not records. The returned future
/// is `Send`, so axum handlers can await it.
///
/// Free-plan cost: each job is one message, 3 of the 10,000 daily Queues
/// operations (a write now, a read and a delete when consumed), plus one
/// read per retry and one write if it ends in the dead-letter queue: about
/// 3,300 jobs a day.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the job does not
/// serialize to JSON, the message is over the 128 KB limit, the
/// `[[queues.producers]] binding = "JOBS"` entry is missing from wrangler.toml
/// (run `ocre g job <Name>` once), or Queues refuses the message.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::State;
/// use ocre::{Ctx, Result};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// #[serde(rename_all = "snake_case")]
/// enum Job {
///     SendWelcome { user_id: i64 },
/// }
///
/// async fn create(State(ctx): State<Ctx>) -> Result<&'static str> {
///     ocre::jobs::enqueue(&ctx, &Job::SendWelcome { user_id: 1 }).await?;
///     Ok("created")
/// }
/// ```
pub fn enqueue<J: Serialize>(ctx: &Ctx, job: &J) -> impl Future<Output = Result<()>> + Send + use<J> {
    send(ctx, job_payload(job), Duration::ZERO)
}

/// Sends `job` to the `JOBS` queue like [`enqueue`](crate::jobs::enqueue), to run after `delay`.
///
/// The delay is whole seconds, 24 hours at most
/// ([`MAX_DELAY`](crate::jobs::MAX_DELAY)); for later work, enqueue from a
/// scheduled task or store the due time in D1. Costs the same Queues
/// operations as [`enqueue`](crate::jobs::enqueue).
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when `delay` is over
/// 24 hours, plus every error of [`enqueue`](crate::jobs::enqueue).
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use axum::extract::State;
/// use ocre::{Ctx, Result};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// #[serde(rename_all = "snake_case")]
/// enum Job {
///     SendReminder { user_id: i64 },
/// }
///
/// async fn remind(State(ctx): State<Ctx>) -> Result<&'static str> {
///     let reminder = Job::SendReminder { user_id: 1 };
///     ocre::jobs::enqueue_in(&ctx, &reminder, Duration::from_secs(3600)).await?;
///     Ok("reminder set")
/// }
/// ```
pub fn enqueue_in<J: Serialize>(
    ctx: &Ctx,
    job: &J,
    delay: Duration,
) -> impl Future<Output = Result<()>> + Send + use<J> {
    send(ctx, job_payload(job), delay)
}
/// Sends a message as JSON text, due after `delay`.
pub(crate) fn send(
    ctx: &Ctx,
    payload: Result<Payload>,
    delay: Duration,
) -> impl Future<Output = Result<()>> + Send + use<> {
    let env = ctx.env().clone();
    SendFuture::new(async move {
        let delay = delay_seconds(delay)?;
        let body = encode(payload?, now() + i64::from(delay))?;
        let queue = env.queue(QUEUE_BINDING).map_err(|err| missing_queue(&err.to_string()))?;
        let message = MessageBuilder::new(body).content_type(QueueContentType::Text).delay_seconds(delay).build();
        queue.send(message).await?;
        Ok(())
    })
}

/// Runs a batch of queue messages through the app's `perform`; the Worker's `queue` entry point.
///
/// Each job goes to `perform(ctx, job)`, each email from
/// [`deliver_later`](crate::mail::deliver_later) to
/// [`mail::send`](crate::mail::send). Messages run one after the other in one
/// Worker invocation (10 per batch with the generated `max_batch_size`, all
/// sharing its CPU limit: 10 ms on Free).
///
/// - `Ok` acknowledges the message and logs `[ocre jobs] <job> done`.
/// - `Err` logs `[ocre jobs] <job> failed, retrying in <n> s: <error>` and
///   retries the message after twice the time since it was due, between 30 s
///   and 24 hours (30 s, 1 min, 3 min, 9 min, 27 min), until `max_retries = 5`
///   sends it to the dead-letter queue `<app>-jobs-failed`.
/// - A message that does not decode (not an Ocre message, or a job renamed
///   or changed while messages were queued) is logged as
///   `[ocre jobs] dropped message <id>: <reason>` and acknowledged, never retried.
///
/// Delivery is at-least-once, so a job can run twice: make it safe to repeat.
///
/// Free-plan cost: reading and acknowledging a message are 2 Queues
/// operations (of 10,000 a day); each retry is one more read, a dead-lettered
/// message one more write.
///
/// # Errors
///
/// Never returns `Err` itself: job errors are logged and retried.
///
/// # Examples
///
/// `ocre g job` writes the entry point in `src/lib.rs` and `perform` in `src/jobs/mod.rs`:
///
/// ```no_run
/// mod jobs {
///     use ocre::{Ctx, Result};
///     use serde::Deserialize;
///
///     #[derive(Deserialize)]
///     #[serde(rename_all = "snake_case")]
///     pub enum Job {
///         SendWelcome { user_id: i64 },
///     }
///
///     pub async fn perform(_ctx: Ctx, job: Job) -> Result<()> {
///         match job {
///             Job::SendWelcome { user_id } => {
///                 # let _ = user_id;
///                 Ok(())
///             }
///         }
///     }
/// }
///
/// #[worker::event(queue)]
/// async fn queue(batch: worker::MessageBatch<String>, env: worker::Env, _ctx: worker::Context) -> worker::Result<()> {
///     ocre::jobs::consume(batch, env, jobs::perform).await
/// }
/// # fn main() {}
/// ```
pub async fn consume<J, F, Fut>(batch: MessageBatch<String>, env: Env, perform: F) -> worker::Result<()>
where
    J: DeserializeOwned,
    F: Fn(Ctx, J) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let ctx = Ctx::new(env);
    for message in batch.raw_iter() {
        let envelope = match decode(message.body().as_string()) {
            Ok(envelope) => envelope,
            Err(reason) => {
                worker::console_error!("{LOG_PREFIX} dropped message {}: {reason}", message.id());
                message.ack();
                continue;
            }
        };
        let (name, result) = match envelope.payload {
            Payload::Mail(email) => ("mail".to_owned(), super::mail::deliver(ctx.env(), email).await),
            Payload::Job(value) => {
                let name = job_name(&value).to_owned();
                match decode_job::<J>(value) {
                    Ok(job) => (name, perform(ctx.clone(), job).await),
                    Err(reason) => {
                        worker::console_error!("{LOG_PREFIX} dropped message {}: {reason}", message.id());
                        message.ack();
                        continue;
                    }
                }
            }
        };
        match result {
            Ok(()) => {
                worker::console_log!("{LOG_PREFIX} {name} done");
                message.ack();
            }
            Err(err) => {
                let delay = retry_delay(now(), envelope.at);
                worker::console_error!("{LOG_PREFIX} {name} failed, retrying in {delay} s: {err}");
                message.retry_with_options(&QueueRetryOptionsBuilder::new().with_delay_seconds(delay).build());
            }
        }
    }
    Ok(())
}

/// Runs the app's task for the Cron Trigger that fired; the Worker's `scheduled` entry point.
///
/// Calls `run(ctx, cron)`, where `cron` is the expression from
/// `[triggers] crons` in wrangler.toml (UTC), and logs
/// `[ocre cron] <cron> done` or `[ocre cron] <cron> failed: <error>`.
/// Cloudflare does not retry a failed run; the next one comes at the next
/// scheduled time, so enqueue jobs from the task for work that must not be
/// lost. The Free plan allows 5 Cron Triggers per account and 10 ms of CPU
/// per run: run several tasks from one cron, and move heavy work to jobs.
///
/// # Examples
///
/// `ocre g schedule` writes the entry point in `src/lib.rs` and `run` in `src/schedules/mod.rs`:
///
/// ```no_run
/// mod schedules {
///     use ocre::{Ctx, Error, Result};
///
///     pub async fn run(_ctx: Ctx, cron: String) -> Result<()> {
///         match cron.as_str() {
///             "0 3 * * *" => Ok(()), // nightly_cleanup
///             other => Err(Error::internal(format!("no task for cron {other}"))),
///         }
///     }
/// }
///
/// #[worker::event(scheduled)]
/// async fn scheduled(event: worker::ScheduledEvent, env: worker::Env, _ctx: worker::ScheduleContext) {
///     ocre::jobs::cron(event, env, schedules::run).await
/// }
/// # fn main() {}
/// ```
pub async fn cron<F, Fut>(event: ScheduledEvent, env: Env, run: F)
where
    F: FnOnce(Ctx, String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let cron = event.cron();
    match run(Ctx::new(env), cron.clone()).await {
        Ok(()) => worker::console_log!("{CRON_LOG_PREFIX} {cron} done"),
        Err(err) => worker::console_error!("{CRON_LOG_PREFIX} {cron} failed: {err}"),
    }
}
