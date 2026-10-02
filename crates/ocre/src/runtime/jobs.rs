use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};
use worker::{
    BatchMessageBuilder, Env, MessageBatch, MessageBuilder, MessageExt, QueueContentType, QueueRetryOptionsBuilder,
    ScheduledEvent, send::SendFuture,
};

use super::Ctx;
use crate::{
    Result,
    jobs::{
        CRON_LOG_PREFIX, DEFAULT_QUEUE, LOG_PREFIX, Payload, batches, binding, decode, decode_job, delay_seconds,
        discards, encode, job_name, job_payload, missing_queue, retry_delay,
    },
    now,
};

/// Sends `job` to the `JOBS` queue, to run in the background through [`consume`](crate::jobs::consume).
///
/// Returns once Cloudflare stored the message; the `queue` event runs it
/// within seconds (`maxBatchTimeout: 5` in cloudflare.config.ts). The job is
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
/// `JOBS: bindings.queue(...)` entry is missing from cloudflare.config.ts
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
    queue(ctx, DEFAULT_QUEUE).enqueue(job)
}

/// Sends every job of `jobs` to the `JOBS` queue in as few calls as possible, like Rails' `perform_all_later`.
///
/// One `sendBatch` call carries up to 100 messages and 256 KB, so 250 jobs
/// take 3 calls instead of 250: use it whenever a handler or a scheduled
/// task enqueues more than a few jobs (a Worker invocation may only make a
/// limited number of calls to bindings). The jobs may be different variants
/// of the app's `Job` enum; they run in any order. Each call is atomic, the
/// whole list is not: when a later call fails, the earlier jobs are queued.
/// An empty list sends nothing.
///
/// Free-plan cost: the same as [`enqueue`](crate::jobs::enqueue) for each
/// job (3 Queues operations each); only the number of calls shrinks.
///
/// # Errors
///
/// Every error of [`enqueue`](crate::jobs::enqueue); when one job cannot be
/// serialized or is over 128 KB, nothing is sent.
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// #[serde(rename_all = "snake_case")]
/// enum Job {
///     SendDigest { user_id: i64 },
/// }
///
/// async fn digests(ctx: &Ctx, user_ids: &[i64]) -> Result<()> {
///     let jobs: Vec<Job> = user_ids.iter().map(|&user_id| Job::SendDigest { user_id }).collect();
///     ocre::jobs::enqueue_all(ctx, &jobs).await
/// }
/// ```
pub fn enqueue_all<J: Serialize>(ctx: &Ctx, jobs: &[J]) -> impl Future<Output = Result<()>> + Send + use<J> {
    queue(ctx, DEFAULT_QUEUE).enqueue_all(jobs)
}

/// A named queue, for jobs that must not wait behind others: `ocre::jobs::queue(&ctx, "urgent").enqueue(&job)`.
///
/// Cloudflare Queues has no priorities: Ocre gives urgent work its own
/// queue instead, like Rails' `queue_as`/`set(queue:)` and Loco's named
/// queues. Each queue has its own consumer settings in cloudflare.config.ts
/// (`ocre g job <Name> --queue urgent` adds `<app>-jobs-urgent` with
/// `maxBatchTimeout: 1`), so a backlog of slow jobs on `default` never
/// delays it. Every queue is consumed by the same `queue` event and the
/// same `perform`. The name `default` is the `JOBS` queue of
/// [`enqueue`](crate::jobs::enqueue); `urgent` is bound as `JOBS_URGENT`.
///
/// Free-plan cost: queues are free to create; each message costs the same
/// 3 operations whatever its queue.
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// #[serde(rename_all = "snake_case")]
/// enum Job {
///     SendMagicLink { user_id: i64 },
/// }
///
/// async fn sign_in(ctx: &Ctx) -> Result<()> {
///     ocre::jobs::queue(ctx, "urgent").enqueue(&Job::SendMagicLink { user_id: 1 }).await
/// }
/// ```
pub fn queue(ctx: &Ctx, name: &'static str) -> Queue {
    Queue { env: ctx.env().clone(), name }
}

/// A job queue, from [`queue`](crate::jobs::queue): enqueue on it like on the default queue.
///
/// # Examples
///
/// ```no_run
/// # async fn f(ctx: &ocre::Ctx) -> ocre::Result<()> {
/// let urgent = ocre::jobs::queue(ctx, "urgent");
/// urgent.enqueue(&"reindex").await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Queue {
    env: Env,
    name: &'static str,
}

impl Queue {
    /// Sends `job` to this queue, like [`enqueue`](crate::jobs::enqueue) does to `default`.
    ///
    /// # Errors
    ///
    /// Those of [`enqueue`](crate::jobs::enqueue); a missing binding names
    /// the fix, `ocre g job <Name> --queue <name>`, and a queue name that is
    /// not lowercase letters, digits and `-` is an
    /// [`Error::Internal`](crate::Error::Internal).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn f(ctx: &ocre::Ctx) -> ocre::Result<()> {
    /// ocre::jobs::queue(ctx, "urgent").enqueue(&serde_json::json!({"send_code": {"user_id": 1}})).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn enqueue<J: Serialize>(&self, job: &J) -> impl Future<Output = Result<()>> + Send + use<J> {
        send(self.env.clone(), self.name, job_payload(job), Duration::ZERO)
    }

    /// Sends `job` to this queue, to run after `delay` (24 hours at most), like [`enqueue_in`](crate::jobs::enqueue_in).
    ///
    /// # Errors
    ///
    /// Those of [`enqueue_in`](crate::jobs::enqueue_in) and [`Queue::enqueue`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn f(ctx: &ocre::Ctx) -> ocre::Result<()> {
    /// let later = std::time::Duration::from_secs(60);
    /// ocre::jobs::queue(ctx, "urgent").enqueue_in(&"retry_payment", later).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn enqueue_in<J: Serialize>(
        &self,
        job: &J,
        delay: Duration,
    ) -> impl Future<Output = Result<()>> + Send + use<J> {
        send(self.env.clone(), self.name, job_payload(job), delay)
    }

    /// Sends every job of `jobs` to this queue in batches, like [`enqueue_all`](crate::jobs::enqueue_all).
    ///
    /// # Errors
    ///
    /// Those of [`enqueue_all`](crate::jobs::enqueue_all) and [`Queue::enqueue`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn f(ctx: &ocre::Ctx) -> ocre::Result<()> {
    /// ocre::jobs::queue(ctx, "urgent").enqueue_all(&["a", "b"]).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn enqueue_all<J: Serialize>(&self, jobs: &[J]) -> impl Future<Output = Result<()>> + Send + use<J> {
        let payloads: Vec<Result<Payload>> = jobs.iter().map(job_payload).collect();
        send_all(self.env.clone(), self.name, payloads)
    }
}

fn producer(env: &Env, name: &str) -> Result<worker::Queue> {
    let binding = binding(name)?;
    env.queue(&binding).map_err(|err| missing_queue(&binding, &err.to_string()))
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
    queue(ctx, DEFAULT_QUEUE).enqueue_in(job, delay)
}

/// Sends a message as JSON text to the queue `name`, due after `delay`.
pub(crate) fn send(
    env: Env,
    name: &'static str,
    payload: Result<Payload>,
    delay: Duration,
) -> impl Future<Output = Result<()>> + Send + use<> {
    SendFuture::new(async move {
        let delay = delay_seconds(delay)?;
        let payload = payload?;
        let recorded = cfg!(debug_assertions).then(|| payload.clone());
        let body = encode(payload, now() + i64::from(delay))?;
        let queue = producer(&env, name)?;
        let message = MessageBuilder::new(body).content_type(QueueContentType::Text).delay_seconds(delay).build();
        queue.send(message).await?;
        if let Some(payload) = recorded {
            crate::jobs::record_enqueued(name, &payload);
        }
        Ok(())
    })
}

fn send_all(
    env: Env,
    name: &'static str,
    payloads: Vec<Result<Payload>>,
) -> impl Future<Output = Result<()>> + Send + use<> {
    SendFuture::new(async move {
        if payloads.is_empty() {
            return Ok(());
        }
        let at = now();
        let payloads = payloads.into_iter().collect::<Result<Vec<_>>>()?;
        let bodies = payloads.iter().map(|payload| encode(payload.clone(), at)).collect::<Result<Vec<_>>>()?;
        let queue = producer(&env, name)?;
        for batch in batches(bodies) {
            let messages =
                batch.into_iter().map(|body| MessageBuilder::new(body).content_type(QueueContentType::Text).build());
            queue.send_batch(BatchMessageBuilder::new().messages(messages).build()).await?;
        }
        for payload in &payloads {
            crate::jobs::record_enqueued(name, payload);
        }
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
/// - An error that another try cannot fix, like Rails' `discard_on`
///   ([`Error::NotFound`](crate::Error::NotFound), `BadRequest`,
///   `Unauthorized`, `Forbidden`, `Invalid`, `PayloadTooLarge`: a record
///   deleted since the job was enqueued, bad input) logs
///   `[ocre jobs] <job> discarded, not retried: <error>` and acknowledges it.
/// - Any other `Err` (`Internal`, `TooManyRequests`) logs
///   `[ocre jobs] <job> failed, retrying in <n> s: <error>` and
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
/// Never returns `Err` itself: job errors are logged, then discarded or retried.
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
            Payload::Mail(email) => ("mail".to_owned(), super::mail::deliver(ctx.env(), *email).await),
            Payload::Job(value) => {
                let name = job_name(&value).to_owned();
                match decode_job::<J>(value) {
                    Ok(job) => (name, perform(ctx.fresh(), job).await),
                    Err(reason) => {
                        worker::console_error!("{LOG_PREFIX} dropped message {}: {reason}", message.id());
                        message.ack();
                        continue;
                    }
                }
            }
        };
        let outcome = match &result {
            Ok(()) => "done",
            Err(err) if discards(err) => "discarded",
            Err(_) => "retried",
        };
        crate::jobs::record_performed(&name, outcome);
        match result {
            Ok(()) => {
                worker::console_log!("{LOG_PREFIX} {name} done");
                message.ack();
            }
            Err(err) if discards(&err) => {
                worker::console_error!("{LOG_PREFIX} {name} discarded, not retried: {err}");
                report(&ctx, "ocre.job", &err, [("job", name.as_str()), ("retried", "false")]);
                message.ack();
            }
            Err(err) => {
                let delay = retry_delay(now(), envelope.at);
                worker::console_error!("{LOG_PREFIX} {name} failed, retrying in {delay} s: {err}");
                report(&ctx, "ocre.job", &err, [("job", name.as_str()), ("retried", "true")]);
                message.retry_with_options(&QueueRetryOptionsBuilder::new().with_delay_seconds(delay).build());
            }
        }
    }
    super::errors::flush(&ctx).await;
    Ok(())
}

/// Queues Ocre's own report of a failed job or cron run (already logged).
pub(crate) fn report<const N: usize>(ctx: &Ctx, source: &str, err: &crate::Error, context: [(&str, &str); N]) {
    let context = context.iter().map(|(key, value)| ((*key).to_owned(), serde_json::Value::from(*value))).collect();
    ctx.errors().report_unhandled(&err.to_string(), source, context, false);
}

/// Runs the app's task for the Cron Trigger that fired; the Worker's `scheduled` entry point.
///
/// Calls `run(ctx, cron)`, where `cron` is the expression from
/// a `triggers.scheduled({ schedule })` entry of cloudflare.config.ts (UTC), and logs
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
    let ctx = Ctx::new(env);
    match run(ctx.clone(), cron.clone()).await {
        Ok(()) => worker::console_log!("{CRON_LOG_PREFIX} {cron} done"),
        Err(err) => {
            worker::console_error!("{CRON_LOG_PREFIX} {cron} failed: {err}");
            report(&ctx, "ocre.cron", &err, [("cron", cron.as_str())]);
        }
    }
    super::errors::flush(&ctx).await;
}

/// Takes the lock `key` for `owner` until `ttl` seconds from now; `false`
/// when another owner holds it and it has not expired. The owner holding it
/// takes it again (extending it), so a job that continues in several queue
/// messages keeps its lock from step to step. One D1 write.
///
/// The `job_locks` table comes from the migration of `ocre g job --lock`
/// ([`LOCKS_TABLE_SQL`](crate::jobs::LOCKS_TABLE_SQL)).
///
/// # Errors
///
/// A D1 error (the table is missing: run the migration).
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Error, Result, jobs};
///
/// async fn import(ctx: &Ctx, account_id: i64, run: &str) -> Result<()> {
///     let db = ctx.db()?;
///     let key = format!("import:{account_id}");
///     if !jobs::lock(&db, &key, run, 600).await? {
///         return Err(Error::internal("another import of this account is running")); // retried later
///     }
///     // ... the work
///     jobs::unlock(&db, &key, run).await
/// }
/// # let _ = import;
/// ```
pub async fn lock(db: &crate::Db, key: &str, owner: &str, ttl: i64) -> Result<bool> {
    let now = crate::now();
    let taken = db
        .execute(
            "INSERT INTO job_locks (key, owner, expires_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT (key) DO UPDATE SET owner = ?2, expires_at = ?3 \
             WHERE job_locks.owner = ?2 OR job_locks.expires_at < ?4",
            crate::params![key, owner, now + ttl, now],
        )
        .await?;
    Ok(taken > 0)
}

/// Releases the lock `key` if `owner` holds it. One D1 write.
///
/// # Errors
///
/// A D1 error.
pub async fn unlock(db: &crate::Db, key: &str, owner: &str) -> Result<()> {
    db.execute("DELETE FROM job_locks WHERE key = ?1 AND owner = ?2", crate::params![key, owner]).await?;
    Ok(())
}
