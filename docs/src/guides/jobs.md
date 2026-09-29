# Background jobs and schedules

This guide moves slow or retryable work out of requests with background jobs on Cloudflare Queues (`ocre g job`, `ocre::jobs::enqueue`), explains retries, the dead-letter queue and idempotency, and runs tasks on a timetable with Cron Triggers (`ocre g schedule`), all within the Workers free plan.

## Before you start

- An app created with [`ocre new`](../reference/cli.md#ocre-new). Jobs use [Cloudflare Queues](https://developers.cloudflare.com/queues/), which the [Workers Free plan includes since February 2026](https://developers.cloudflare.com/changelog/post/2026-02-04-queues-free-plan/); schedules use [Cron Triggers](https://developers.cloudflare.com/workers/configuration/cron-triggers/).
- Nothing to create by hand: the first `ocre g job` adds the queue to `wrangler.toml`, `ocre dev` runs it locally, and [`ocre deploy`](../reference/cli.md#ocre-deploy) creates it on Cloudflare.
- The examples build on `ocre g auth` (users) and `ocre g mailer User welcome` (the welcome email); see [Authentication](authentication.md) and [Email](email.md).
- The outputs below come from an app created with `ocre new shop --starter blog`, served by `ocre dev` on the default port 8787.

## How jobs run

The app's Worker is both the producer and the consumer of one queue, `<app>-jobs`, bound as `JOBS`:

```text
handler ── ocre::jobs::enqueue ──> <app>-jobs queue ──> queue event (src/lib.rs)
                                                          └─> ocre::jobs::consume ──> jobs::perform (src/jobs/mod.rs) ──> SendWelcome::perform
```

A handler enqueues a job and answers as soon as Cloudflare stored the message. Moments later Cloudflare calls the Worker's `queue` event with a batch of messages; Ocre decodes each one into the app's `Job` enum and calls `perform`. Dispatch is a plain `match` in app code, not a registry.

## Generate a job

The arguments are fields, with the same types as models (`integer` is `i64`, `string` is `String`...):

```sh
ocre g job SendWelcome user_id:integer
```

```text
  create  src/jobs/send_welcome.rs
  create  src/jobs/mod.rs
  update  src/lib.rs
  update  wrangler.toml

Next:
  enqueue it from a handler: ocre::jobs::enqueue(&ctx, &jobs::Job::SendWelcome(jobs::SendWelcome { user_id })).await?
  ocre dev (jobs run locally; look for `[ocre jobs]` lines in the output)
  ocre deploy creates the queue shop-jobs and its dead-letter queue
```

The first job wires everything; later ones only create their file and add a variant to `src/jobs/mod.rs`.

| File | What the generator writes |
|---|---|
| `src/jobs/send_welcome.rs` | `pub struct SendWelcome { pub user_id: i64 }` (serde) and `async fn perform(self, ctx: &Ctx) -> Result<()>`, which does nothing yet |
| `src/jobs/mod.rs` | The `Job` enum (one variant per job) and `perform`, which matches on it; keep the `// ocre:jobs`, `// ocre:job-variants` and `// ocre:job-dispatch` markers |
| `src/lib.rs` | `mod jobs;` and the `queue` entry point (first job only) |
| `wrangler.toml` | The `JOBS` producer and the consumer settings (first job only) |

`src/jobs/mod.rs` after the first job:

```rust
// ocre:jobs
pub mod send_welcome;
pub use send_welcome::SendWelcome;

/// Every job of the app. A queue message holds one as JSON:
/// `{"send_welcome": {"user_id": 1}}`. Renaming a variant or changing its
/// fields makes messages already queued undecodable (they are logged and
/// dropped), so change jobs when the queue is empty or add a new variant.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Job {
    // ocre:job-variants
    SendWelcome(SendWelcome),
}

/// Runs one job; called by `ocre::jobs::consume` for each queue message.
/// `Ok` acknowledges it; `Err` retries it later.
pub async fn perform(ctx: Ctx, job: Job) -> Result<()> {
    match job {
        // ocre:job-dispatch
        Job::SendWelcome(job) => job.perform(&ctx).await,
    }
}
```

The entry point added to `src/lib.rs`:

```rust
/// Background jobs from the `JOBS` queue, run by `jobs::perform` (src/jobs/mod.rs).
#[worker::event(queue)]
async fn queue(batch: worker::MessageBatch<String>, env: worker::Env, _ctx: worker::Context) -> worker::Result<()> {
    ocre::jobs::consume(batch, env, jobs::perform).await
}
```

And the queue in `wrangler.toml` (for an app named `shop`):

```toml
[[queues.producers]]
binding = "JOBS"
queue = "shop-jobs"

# Up to 10 messages per run, waiting at most 5 s to fill a batch. Each run is
# one Worker request with 10 ms of CPU on the free plan: lower max_batch_size
# for CPU-heavy jobs. ...
[[queues.consumers]]
queue = "shop-jobs"
max_batch_size = 10
max_batch_timeout = 5
max_retries = 5
dead_letter_queue = "shop-jobs-failed"
```

## Write perform

A job's fields are its arguments, stored in the queue message as JSON (128 KB at most): pass ids and small values, and load records in `perform`. The record may have changed or disappeared since the job was enqueued, so decide what that means. This job sends the welcome email of the `User` mailer to a user:

```rust,check
// src/jobs/send_welcome.rs
use ocre::{Ctx, Result};
use serde::{Deserialize, Serialize};

use crate::{mailers, models::user};

/// The job's arguments: an id, not the user record.
#[derive(Debug, Serialize, Deserialize)]
pub struct SendWelcome {
    pub user_id: i64,
}

impl SendWelcome {
    /// `Err` retries the job later; `Ok` acknowledges it.
    pub async fn perform(self, ctx: &Ctx) -> Result<()> {
        // Deleted since the job was enqueued: nothing to do, done.
        let Some(user) = user::find(ctx, self.user_id).await? else {
            return Ok(());
        };
        ocre::mail::send(ctx, mailers::user::welcome(&user.email)?).await
    }
}
```

Returning `Ok(())` acknowledges the message; returning `Err` (here, a failed D1 query or a provider error from `send`) retries it later. Return `Ok` for conditions that retrying cannot fix, such as a deleted record, or the job will be retried five times for nothing.

Jobs run outside any request: there is no session, no signed-in user and no request URL. Pass what `perform` needs (an id, a locale, an absolute base URL) as fields.

## Enqueue a job

`ocre::jobs::enqueue(&ctx, &job).await?` sends the job now; `ocre::jobs::enqueue_in(&ctx, &job, delay).await?` makes it due after `delay` (whole seconds, 24 hours at most). This module lets a signed-in user ask for the welcome email again; register it with `mod welcome;` under `// ocre:modules` and `.merge(welcome::routes())` under `// ocre:routes` in `src/lib.rs`:

```rust,check
// src/welcome.rs
use std::time::Duration;

use axum::{Router, extract::State, response::Redirect, routing::post};
use ocre::{Ctx, Result, Session};

use crate::{
    auth::CurrentUser,
    jobs::{Job, SendWelcome},
};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/welcome", post(resend)).route("/welcome/later", post(resend_later))
}

/// Answers as soon as Cloudflare stored the message; the email goes out moments later.
async fn resend(State(ctx): State<Ctx>, session: Session, CurrentUser(user): CurrentUser) -> Result<Redirect> {
    ocre::jobs::enqueue(&ctx, &Job::SendWelcome(SendWelcome { user_id: user.id })).await?;
    session.flash("notice", "The welcome email is on its way.")?;
    Ok(Redirect::to("/account"))
}

/// Same job, run in 10 minutes (24 hours at most).
async fn resend_later(State(ctx): State<Ctx>, CurrentUser(user): CurrentUser) -> Result<Redirect> {
    let job = Job::SendWelcome(SendWelcome { user_id: user.id });
    ocre::jobs::enqueue_in(&ctx, &job, Duration::from_secs(600)).await?;
    Ok(Redirect::to("/account"))
}
```

With a signed-in cookie jar:

```sh
curl -s -c jar.txt -b jar.txt -X POST http://localhost:8787/welcome -o /dev/null -w '%{http_code} %{redirect_url}\n'
```

```text
303 http://localhost:8787/account
```

`ocre dev` runs the queue in-process; within about 5 seconds (`max_batch_timeout`) the job runs and its email is printed:

```text
[ocre mail] not sent (MAIL_ADAPTER = "log")
From: shop <noreply@example.com>
To: ada@example.com
Subject: Welcome
...
[ocre mail] end
[ocre jobs] send_welcome done
...
[wrangler:info] QUEUE shop-jobs 2/3 (13ms)
```

`QUEUE shop-jobs 2/3` is wrangler's summary of one consumer run: 2 of the 3 messages of that batch were acknowledged (this job and a `deliver_later` email; the third was the failing job of [Retries](#retries-and-the-dead-letter-queue)).

`enqueue` and `enqueue_in` fail with `Error::Internal` (500, the log names the fix) when the job does not serialize, the message is over 128 KB, the delay is over 24 hours, the `JOBS` binding is missing from `wrangler.toml`, or Queues refuses the message. For later work than 24 hours, enqueue from a [scheduled task](#schedules) or store the due time in D1.

### The message

Each job is one queue message, JSON text:

```json
{"at": 1790656502, "job": {"send_welcome": {"user_id": 1}}}
```

`at` is the Unix time the job is due (now, or now plus the delay); `job` is the serde form of the `Job` enum, whose variant names are snake_case. An email from [`deliver_later`](email.md#send-from-the-background-deliver_later) is `{"at": ..., "mail": {...}}` on the same queue.

## Retries and the dead-letter queue

When `perform` returns `Err`, Ocre logs the error and asks Queues to deliver the message again after twice the time since it was due, 30 seconds at least. For a job processed right away that gives 30 s, 1 min, 3 min, 9 min and 27 min. After `max_retries = 5` retries, Cloudflare moves the message to the dead-letter queue `<app>-jobs-failed`, where it stays 24 hours; inspect it in the dashboard (Queues > `<app>-jobs-failed`). The last retry comes about 40 minutes after the job was due.

A job that always fails (its `perform` returns `Err(ocre::Error::internal(format!("{} answered 503", self.url)))`), as logged by `ocre dev`:

```text
✘ [ERROR] [ocre jobs] import_feed failed, retrying in 30 s: internal error: https://example.com/feed.xml answered 503
...
✘ [ERROR] [ocre jobs] import_feed failed, retrying in 80 s: internal error: https://example.com/feed.xml answered 503
```

The second delay is 80 s, not 60 s: the retry was processed 40 s after the job was due (30 s of delay plus the batch wait), and the next delay is twice that.

The log lines, all prefixed `[ocre jobs]`:

| Line | Meaning |
|---|---|
| `[ocre jobs] <job> done` | `perform` returned `Ok`; the message is acknowledged |
| `[ocre jobs] <job> failed, retrying in <n> s: <error>` | `perform` returned `Err`; the message comes back after `n` seconds |
| `[ocre jobs] mail done` | A `deliver_later` email was sent |
| `[ocre jobs] dropped message <id>: <reason>` | The message did not decode; it is acknowledged and never retried |

### Changing jobs safely

A message is dropped when it is not an Ocre message, or when its job no longer matches the `Job` enum: a renamed variant, a renamed field, a new field without a default, a field whose type changed. (Fields that old messages have and the struct no longer has are ignored.) Messages wait in the queue for up to 24 hours, so:

- Add a new variant (`SendWelcomeV2`) instead of changing one while messages may be queued, and remove the old one a day later.
- New fields can be added safely with `#[serde(default)]`: old messages decode with the default.
- Never rename a variant while its messages may be queued.

## Make jobs safe to repeat

Queues deliver at least once: a job can run twice, for example when a Worker is evicted after `perform` succeeded but before the acknowledgement, or when a batch is retried. Write `perform` so a second run does no harm:

- Prefer statements that are naturally idempotent: `UPDATE ... SET status = 'sent'`, `INSERT ... ON CONFLICT DO NOTHING`, `DELETE`.
- Record that the work was done, and check first. For example, with a `welcomed_at` column on `users`:

  ```rust
  let claimed = ctx.db()?
      .execute("UPDATE users SET welcomed_at = datetime('now') WHERE id = ?1 AND welcomed_at IS NULL", params![self.user_id])
      .await?;
  if claimed == 0 {
      return Ok(()); // already welcomed
  }
  ```

  Claiming before sending means an email whose sending fails after the claim is not retried; claiming after sending means a crash in between sends it twice. Pick the failure you prefer for each job.
- Calls to other APIs: pass an idempotency key when the API accepts one (a value stored in the job's fields, so every run sends the same one).

## Schedules

A scheduled task runs on the deployed Worker at times given by a cron expression, in UTC:

```sh
ocre g schedule nightly_cleanup "0 3 * * *"
```

```text
  create  src/schedules/nightly_cleanup.rs
  create  src/schedules/mod.rs
  update  wrangler.toml
  update  src/lib.rs

Next:
  ocre dev, then: curl 'http://localhost:8787/cdn-cgi/local/scheduled?cron=0+3+*+*+*'
  ocre deploy (Cron Triggers only fire on the deployed Worker, in UTC)
```

| File | What the generator writes |
|---|---|
| `src/schedules/nightly_cleanup.rs` | `pub async fn run(ctx: &Ctx) -> Result<()>`, which does nothing yet |
| `src/schedules/mod.rs` | `run(ctx, cron)`, which matches the expression that fired to a task; keep the `// ocre:schedules` and `// ocre:schedule-dispatch` markers |
| `wrangler.toml` | The expression in `[triggers] crons = ["0 3 * * *"]` |
| `src/lib.rs` | `mod schedules;` and the `scheduled` entry point (first schedule only) |

The entry point calls `ocre::jobs::cron`, which runs the task and logs the result:

```rust
/// Cron Triggers (`[triggers] crons` in wrangler.toml), run by `schedules::run` (src/schedules/mod.rs).
#[worker::event(scheduled)]
async fn scheduled(event: worker::ScheduledEvent, env: worker::Env, _ctx: worker::ScheduleContext) {
    ocre::jobs::cron(event, env, schedules::run).await
}
```

### Cron expressions

Five fields, in UTC: minute, hour, day of month, month, day of week. The generator accepts letters, digits and `* , - / #` and normalizes spaces; Cloudflare validates the rest on deploy (see its [supported expressions](https://developers.cloudflare.com/workers/configuration/cron-triggers/#supported-cron-expressions)).

| Expression | Runs |
|---|---|
| `0 3 * * *` | Every day at 03:00 UTC |
| `*/15 * * * *` | Every 15 minutes |
| `0 9 * * MON` | Mondays at 09:00 UTC |
| `0 0 1 * *` | The first day of each month at midnight UTC |

One task per expression: `ocre g schedule` refuses an expression already in `[triggers] crons` (run the new work from the existing task, or pick another minute). The free plan allows 5 Cron Triggers per account, across all Workers; past 5 in one app, the generator adds a warning to its `Next:` steps. Run several tasks from one cron when you need more.

### Keep tasks short: enqueue jobs

A scheduled run has the same CPU limit as a request (10 ms on the free plan), and a failed run is only logged (`[ocre cron] <cron> failed: <error>`), never retried: the task runs again at its next time. So a task should do a few queries and enqueue one job per item; the jobs do the slow work, with retries. This task deletes expired sign-in tokens and enqueues a welcome email for each user who signed up in the last day:

```rust,check
// src/schedules/nightly_cleanup.rs
use ocre::{Ctx, Result, params};
use serde::Deserialize;

use crate::jobs::{Job, SendWelcome};

#[derive(Deserialize)]
struct UserId {
    id: i64,
}

/// Runs at 03:00 UTC. Two queries and a few enqueues fit in 10 ms of CPU;
/// the per-user work (sending email) runs in the jobs.
pub async fn run(ctx: &Ctx) -> Result<()> {
    let db = ctx.db()?;
    let expired = db.execute("DELETE FROM auth_tokens WHERE expires_at <= datetime('now')", vec![]).await?;
    worker::console_log!("nightly_cleanup: {expired} expired tokens deleted");

    let new_users: Vec<UserId> = db
        .all("SELECT id FROM users WHERE created_at >= datetime('now', '-1 day') ORDER BY id LIMIT ?1", params![20])
        .await?;
    for UserId { id } in new_users {
        ocre::jobs::enqueue(ctx, &Job::SendWelcome(SendWelcome { user_id: id })).await?;
    }
    Ok(())
}
```

The `LIMIT` bounds the work of one run; each `enqueue` is one Queues write.

### Run a task locally

Cron Triggers do not fire in `ocre dev`. While it runs, trigger one by passing its expression (spaces as `+`):

```sh
curl 'http://localhost:8787/cdn-cgi/local/scheduled?cron=0+3+*+*+*'
```

```text
ok
```

The `ocre dev` output, with the task above and two users created that day (their jobs run a few seconds later):

```text
nightly_cleanup: 0 expired tokens deleted
[ocre cron] 0 3 * * * done
...
[ocre mail] not sent (MAIL_ADAPTER = "log")
From: shop <noreply@example.com>
To: ada@example.com
Subject: Welcome
...
[ocre mail] end
[ocre jobs] send_welcome done
[ocre mail] not sent (MAIL_ADAPTER = "log")
From: shop <noreply@example.com>
To: grace@example.com
Subject: Welcome
...
[ocre mail] end
[ocre jobs] send_welcome done
[wrangler:info] QUEUE shop-jobs 2/2 (5ms)
```

An expression with no task in `src/schedules/mod.rs` fails, and the error names the fix:

```sh
curl 'http://localhost:8787/cdn-cgi/local/scheduled?cron=*/5+*+*+*+*'
```

```text
✘ [ERROR] [ocre cron] */5 * * * * failed: internal error: no scheduled task for cron `*/5 * * * *`. Fix: add it to the match in src/schedules/mod.rs, or remove it from [triggers] crons in wrangler.toml
```

## Deploy

`ocre deploy` handles the queues and crons of `wrangler.toml`:

- Before deploying, it runs `wrangler queues info` for every queue named there (producers, consumers and dead-letter queues) and `wrangler queues create` for the missing ones, because a consumer of a missing queue fails the deploy. For each queue it creates, it prints `Created queue <name> on Cloudflare` (for example `Created queue shop-jobs on Cloudflare` and `Created queue shop-jobs-failed on Cloudflare`); with `--json` they are listed in `provisioned` as `"queue shop-jobs"`.
- `wrangler deploy` then registers the consumer and the `[triggers] crons`. Crons fire only on the deployed Worker.

Follow the deployed Worker's job and cron lines with `npx wrangler tail`. See [Deployment](deployment.md) for the rest of the deploy.

## Free-plan budget

Limits of the Workers Free plan (September 2026) and what Ocre does about them:

| Limit | Value | What Ocre does |
|---|---|---|
| [Queues operations](https://developers.cloudflare.com/queues/platform/pricing/) | 10,000 a day; a message costs 3 (write, read, delete), each retry 1 more read, a dead-lettered message 1 more write | One message per job; about 3,300 jobs a day |
| [Retention](https://developers.cloudflare.com/queues/platform/limits/) | 24 hours on Free (not configurable) | Retries stop long before: the last one comes after about 40 minutes |
| [Message size](https://developers.cloudflare.com/queues/platform/limits/) | 128 KB | `enqueue` refuses larger jobs with an error naming the fix (pass ids) |
| [Delay](https://developers.cloudflare.com/queues/configuration/batching-retries/#delay-messages) | 24 hours, on send and on retry | `enqueue_in` refuses longer delays |
| [Batches](https://developers.cloudflare.com/queues/configuration/batching-retries/) | Up to 100 messages, 60 s wait | `max_batch_size = 10`, `max_batch_timeout = 5`: one consumer run (one Worker invocation) per 10 jobs |
| [CPU](https://developers.cloudflare.com/workers/platform/limits/#cpu-time) | 10 ms per invocation, for requests, cron runs and consumer batches | Jobs should be I/O (D1, mail, `fetch`); the jobs of a batch share one invocation, so lower `max_batch_size` for CPU-heavy jobs |
| [Cron Triggers](https://developers.cloudflare.com/workers/platform/limits/) | 5 per account | `ocre g schedule` warns past 5 in the app; run several tasks from one cron |

## See also

- [`ocre g job`](../reference/generators.md#ocre-g-job) and [`ocre g schedule`](../reference/generators.md#ocre-g-schedule) in the generators reference.
- [Email](email.md): `deliver_later` sends email through this queue.
- [Free-plan limits](../reference/limits.md) and [Cost model](../explanations/cost-model.md).
- [Deployment](deployment.md) and [`ocre deploy`](../reference/cli.md#ocre-deploy).
- Rust API: [`ocre::jobs`](/api/ocre/jobs/index.html), or the [API index](../api-index.md).
