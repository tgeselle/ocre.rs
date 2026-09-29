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

/// Sends `job` to the `JOBS` queue; the `queue` event runs it within seconds
/// (`max_batch_timeout` in wrangler.toml). Returns once Cloudflare stored the
/// message. The job must serialize to at most 128 KB of JSON: pass ids, not
/// records.
///
/// ```ignore
/// use crate::jobs::{Job, SendWelcome};
///
/// async fn create(State(ctx): State<Ctx>, Form(form): Form<NewUser>) -> ocre::Result<Redirect> {
///     let user = models::user::create(&ctx, form).await?;
///     ocre::jobs::enqueue(&ctx, &Job::SendWelcome(SendWelcome { user_id: user.id })).await?;
///     Ok(Redirect::to("/"))
/// }
/// ```
///
/// Each job costs 3 of the free plan's 10,000 daily Queues operations
/// (write, read, delete), plus one read per retry.
pub fn enqueue<J: Serialize>(ctx: &Ctx, job: &J) -> impl Future<Output = Result<()>> + Send + use<J> {
    send(ctx, job_payload(job), Duration::ZERO)
}

/// Like [`enqueue`], but the job runs after `delay` (24 hours at most, see
/// [`MAX_DELAY`](crate::jobs::MAX_DELAY)).
///
/// ```ignore
/// let reminder = Job::SendReminder(SendReminder { user_id: user.id });
/// ocre::jobs::enqueue_in(&ctx, &reminder, Duration::from_secs(3600)).await?;
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

/// Runs a batch of queue messages: each job through `perform`, each email
/// from [`deliver_later`](crate::mail::deliver_later) through
/// [`mail::send`](crate::mail::send). Call it from the Worker's queue entry
/// point (`ocre g job` writes this):
///
/// ```ignore
/// #[worker::event(queue)]
/// async fn queue(batch: worker::MessageBatch<String>, env: worker::Env, _ctx: worker::Context) -> worker::Result<()> {
///     ocre::jobs::consume(batch, env, jobs::perform).await
/// }
/// ```
///
/// Messages run one after the other. `Ok` acknowledges the message and logs
/// `[ocre jobs] <job> done`; `Err` logs the error and retries the message
/// after twice the time since it was due (30 s at least), until
/// `max_retries` sends it to the dead-letter queue. A message that does not
/// decode (not an Ocre message, or a job the app no longer knows) is logged
/// and acknowledged, never retried.
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

/// Runs the task for the Cron Trigger that fired: `run(ctx, cron)`, where
/// `cron` is the expression from `[triggers] crons` in wrangler.toml. Call
/// it from the Worker's scheduled entry point (`ocre g schedule` writes this):
///
/// ```ignore
/// #[worker::event(scheduled)]
/// async fn scheduled(event: worker::ScheduledEvent, env: worker::Env, _ctx: worker::ScheduleContext) {
///     ocre::jobs::cron(event, env, schedules::run).await
/// }
/// ```
///
/// Logs `[ocre cron] <cron> done`, or `[ocre cron] <cron> failed: <error>`.
/// Cloudflare does not retry a failed run; the next one comes at the next
/// scheduled time. Enqueue jobs from the task for work that must not be lost.
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
