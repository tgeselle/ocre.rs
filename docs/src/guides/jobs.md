# Background jobs and schedules

This guide moves slow or retryable work out of requests with background jobs on Cloudflare Queues (`ocre g job`, `perform_later`, `ocre::jobs::enqueue_all`), explains retries, discarding, the dead-letter queue and idempotency, gives urgent jobs their own queue, and runs tasks on a timetable with Cron Triggers (`ocre g schedule "every day at 3am"`, `ocre schedules`), all within the Workers free plan.

## Before you start

- An app created with [`ocre new`](../reference/cli.md#ocre-new). Jobs use [Cloudflare Queues](https://developers.cloudflare.com/queues/), which the [Workers Free plan includes since February 2026](https://developers.cloudflare.com/changelog/post/2026-02-04-queues-free-plan/); schedules use [Cron Triggers](https://developers.cloudflare.com/workers/configuration/cron-triggers/).
- Nothing to create by hand: the first `ocre g job` adds the queue to `cloudflare.config.ts`, `ocre dev` runs it locally, and [`ocre deploy`](../reference/cli.md#ocre-deploy) creates it on Cloudflare.
- The examples build on `ocre g auth` (users) and `ocre g mailer User welcome` (the welcome email); see [Authentication](authentication.md) and [Email](email.md).
- The outputs below come from an app created with `ocre new shop --starter blog`, served by `ocre dev` on the default port 8787.

## How jobs run

The app's Worker is both the producer and the consumer of its queue, `<app>-jobs`, bound as `JOBS` (plus any [named queue](#urgent-jobs-named-queues)):

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
  update  cloudflare.config.ts

Next:
  enqueue it from a handler: jobs::SendWelcome { user_id }.perform_later(&ctx).await?
  ocre dev (jobs run locally; look for `[ocre jobs]` lines in the output)
  ocre deploy creates the queue shop-jobs and its dead-letter queue
```

The first job wires everything; later ones only create their file and add a variant to `src/jobs/mod.rs`.

| File | What the generator writes |
|---|---|
| `src/jobs/send_welcome.rs` | `pub struct SendWelcome { pub user_id: i64 }` (serde), `fn perform_later(self, ctx)` (sends it to the queue) and `async fn perform(self, ctx: &Ctx) -> Result<()>`, which does nothing yet |
| `src/jobs/mod.rs` | The `Job` enum (one variant per job) and `perform`, which matches on it; keep the `// ocre:jobs`, `// ocre:job-variants` and `// ocre:job-dispatch` markers |
| `src/lib.rs` | `mod jobs;` and the `queue` entry point (first job only) |
| `cloudflare.config.ts` | The `JOBS` producer binding and the consumer trigger (first job only) |

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
/// `Ok` acknowledges it; `Err(Error::NotFound)` and other 4xx errors drop
/// it (logged); other errors retry it later. Code here runs around every
/// job, like Rails' `around_perform`.
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

And the queue in `cloudflare.config.ts` (for an app named `shop`), the producer after `// ocre:env` and the consumer after `// ocre:triggers`:

```ts
// in worker.env
// Background jobs (`ocre g job`): the Worker sends jobs to this queue and runs
// them (src/jobs/). ...
JOBS: bindings.queue({ name: "shop-jobs" }),

// in worker.triggers
// Runs the jobs of shop-jobs: up to 10 messages per run, waiting at most 5 s
// to fill a batch. Each run is one Worker request with 10 ms of CPU on the free
// plan: lower maxBatchSize for CPU-heavy jobs. ...
triggers.queue({ name: "shop-jobs", deadLetterQueue: "shop-jobs-failed", maxBatchSize: 10, maxBatchTimeout: 5, maxRetries: 5 }),
```

## Write perform

A job's fields are its arguments, stored in the queue message as JSON (128 KB at most): pass ids and small values, and load records in `perform`. The record may have changed or disappeared since the job was enqueued, so decide what that means. This job sends the welcome email of the `User` mailer to a user:

```rust,check
// src/jobs/send_welcome.rs
use ocre::{Ctx, OptionExt, Result};
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
        // Deleted since the job was enqueued: `NotFound` drops the job (logged), no retry.
        let user = user::find(ctx, self.user_id).await?.or_404()?;
        ocre::mail::send(ctx, mailers::user::welcome(&user.email)?).await
    }
}
```

Returning `Ok(())` acknowledges the message. An error that another try cannot fix is dropped with a log line, like Rails' `discard_on`: `Error::NotFound` (here, a user deleted since), `BadRequest`, `Unauthorized`, `Forbidden`, `Invalid` and `PayloadTooLarge`. Any other `Err` (a failed D1 query, a provider error from `send`: `Error::Internal`, or `TooManyRequests`) retries the job later. See [Errors: retry or discard](#errors-retry-or-discard).

Jobs run outside any request: there is no session, no signed-in user and no request URL. Pass what `perform` needs (an id, a locale, an absolute base URL) as fields. Records travel as ids, like Rails' GlobalID arguments; dates as strings or Unix times; any other type works if it implements serde's `Serialize` and `Deserialize` (derive them, or use `#[serde(with = "...")]`), which replaces Rails' custom argument serializers.

## Enqueue a job

`job.perform_later(&ctx).await?` (generated in each job) sends the job now and returns as soon as Cloudflare stored it, like Rails' `perform_later`. Under it are the functions of `ocre::jobs`, for any job of the `Job` enum:

| Call | Does |
|---|---|
| `ocre::jobs::enqueue(&ctx, &job)` | Sends the job to the `default` queue, due now |
| `ocre::jobs::enqueue_in(&ctx, &job, delay)` | Due after `delay` (whole seconds, 24 hours at most), like Rails' `set(wait:)` |
| `ocre::jobs::enqueue_all(&ctx, &jobs)` | Many jobs, in one call per 100 (see [Enqueue many jobs](#enqueue-many-jobs)) |
| `ocre::jobs::queue(&ctx, "urgent").enqueue(&job)` | The same on a [named queue](#urgent-jobs-named-queues) (also `enqueue_in`, `enqueue_all`) |
| `job.perform(&ctx).await` | Runs it now, in the request, like Rails' `perform_now` |

This module lets a signed-in user ask for the welcome email again; register it with `mod welcome;` under `// ocre:modules` and `.merge(welcome::routes())` under `// ocre:routes` in `src/lib.rs`:

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
    SendWelcome { user_id: user.id }.perform_later(&ctx).await?;
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

`QUEUE shop-jobs 2/3` is the local server's summary of one consumer run (a wrangler line: `cf dev` runs wrangler): 2 of the 3 messages of that batch were acknowledged (this job and a `deliver_later` email; the third was the failing job of [Errors: retry or discard](#errors-retry-or-discard)).

`perform_later`, `enqueue`, `enqueue_in` and `enqueue_all` fail with `Error::Internal` (500, the log names the fix) when the job does not serialize, the message is over 128 KB, the delay is over 24 hours, the queue's binding (`JOBS`, `JOBS_URGENT`...) is missing from `cloudflare.config.ts`, or Queues refuses the message. For later work than 24 hours, enqueue from a [scheduled task](#schedules) or store the due time in D1.

### The message

Each job is one queue message, JSON text:

```json
{"at": 1790656502, "job": {"send_welcome": {"user_id": 1}}}
```

`at` is the Unix time the job is due (now, or now plus the delay); `job` is the serde form of the `Job` enum, whose variant names are snake_case. An email from [`deliver_later`](email.md#send-from-the-background-deliver_later) is `{"at": ..., "mail": {...}}` on the same queue.

## Enqueue many jobs

`ocre::jobs::enqueue_all(&ctx, &jobs).await?` is Rails' `perform_all_later`: it sends a list of jobs (any variants of `Job`) with one `sendBatch` call per 100 messages (256 KB), instead of one call per job. A Worker invocation may only make a limited number of calls to bindings, so use it whenever a handler or a scheduled task enqueues more than a few jobs. Each call is atomic, the list as a whole is not: if the second call fails, the first 100 jobs are queued. Every job still costs 3 Queues operations.

```rust,check
use ocre::{Ctx, Result, params};
use serde::Deserialize;

use crate::jobs::{Job, SendWelcome};

#[derive(Deserialize)]
struct UserId {
    id: i64,
}

/// One `sendBatch` call for up to 100 users.
pub async fn welcome_everyone(ctx: &Ctx) -> Result<()> {
    let users: Vec<UserId> = ctx.db()?.all("SELECT id FROM users ORDER BY id LIMIT ?1", params![100]).await?;
    let jobs: Vec<Job> = users.into_iter().map(|u| Job::SendWelcome(SendWelcome { user_id: u.id })).collect();
    ocre::jobs::enqueue_all(ctx, &jobs).await
}
```

## Urgent jobs: named queues

Cloudflare Queues has no priorities: every queue delivers its messages roughly in order, in batches. Ocre gives urgent work its own queue instead, like Rails' `queue_as` and Loco's named queues: a sign-in code must not wait behind a thousand digest emails.

```sh
ocre g job SendCode user_id:integer --queue urgent
```

```text
  create  src/jobs/send_code.rs
  update  cloudflare.config.ts
  update  src/jobs/mod.rs

Next:
  enqueue it from a handler: jobs::SendCode { user_id }.perform_later(&ctx).await?
  ocre dev (jobs run locally; look for `[ocre jobs]` lines in the output)
  ocre deploy creates the queue shop-jobs-urgent and its dead-letter queue
```

The generated `perform_later` sends to that queue (`ocre::jobs::queue(ctx, "urgent").enqueue(...)`), and `cloudflare.config.ts` gets the producer `JOBS_URGENT` and a consumer that waits at most 1 second to fill a batch:

```ts
// in worker.env
JOBS_URGENT: bindings.queue({ name: "shop-jobs-urgent" }),
// in worker.triggers
triggers.queue({ name: "shop-jobs-urgent", deadLetterQueue: "shop-jobs-urgent-failed", maxBatchSize: 10, maxBatchTimeout: 1, maxRetries: 5 }),
```

Every queue is consumed by the same `queue` event and the same `perform`: the queue changes when a job runs, not how. Queue names are lowercase letters, digits and `-`; `default` is the `JOBS` queue. Queues are free to create; a message costs the same 3 operations on any queue. Loco's worker tags (a process that only runs some jobs) have no equivalent: there are no worker processes, and a queue per kind of work gives the same isolation.

## Run code around jobs: callbacks

Rails' `before_perform`, `around_perform` and `after_perform` are code around the `match` in `perform` of `src/jobs/mod.rs`, which runs every job; `before_enqueue` and `after_enqueue` are code in a job's `perform_later`. Returning early halts, like `throw :abort`:

```rust
pub async fn perform(ctx: Ctx, job: Job) -> Result<()> {
    let started = ocre::now();
    let result = match job {
        // ocre:job-dispatch
        Job::SendWelcome(job) => job.perform(&ctx).await,
    };
    worker::console_log!("job took {} s", ocre::now() - started);
    result
}
```

Handle an error of one job there too, like Rails' `rescue_from`: `Job::ImportFeed(job) => job.perform(&ctx).await.or_else(|err| ...)`.

## Enqueue after the database commits

D1 has no transaction that stays open across `await`s: a group of writes is one `ctx.db()?.batch(statements).await?`, committed when the call returns. Enqueue after it, and a job never sees data that was rolled back, which is what Rails' `enqueue_after_transaction_commit` ensures. If enqueuing fails after the commit, the handler returns the error (500) with the data saved; make the job something a scheduled task can catch up on, or accept the rare miss.

## Limit concurrency

Cloudflare runs several consumer invocations of a queue in parallel when messages pile up. Rails' `limits_concurrency` has two Ocre forms:

- For a whole queue, add `maxConcurrency: 1` to its `triggers.queue({ ... })` entry in `cloudflare.config.ts`: one batch at a time. Combine it with a [named queue](#urgent-jobs-named-queues) for the jobs that must not overlap.
- For jobs sharing a key (one import per account), claim a row in D1 first, with an expiry in case a run dies, and return an error to retry later when it is taken:

  ```rust
  // CREATE TABLE job_locks (key TEXT PRIMARY KEY, expires_at INTEGER NOT NULL);
  let db = ctx.db()?;
  let key = format!("import:{}", self.account_id);
  let now = ocre::now();
  let claimed = db
      .execute(
          "INSERT INTO job_locks (key, expires_at) VALUES (?1, ?2) \
           ON CONFLICT (key) DO UPDATE SET expires_at = ?2 WHERE job_locks.expires_at < ?3",
          params![key.clone(), now + 300, now],
      )
      .await?;
  if claimed == 0 {
      return Err(Error::internal("another import of this account is running")); // retried later
  }
  let result = self.import(ctx).await;
  db.execute("DELETE FROM job_locks WHERE key = ?1", params![key]).await?;
  result
  ```

  Each claim and release is one D1 row written (100,000 a day on the free plan).

## Long jobs: continue in steps

Each consumer run has the CPU limit of a request (10 ms on the free plan), so a job over many rows does a slice and enqueues itself for the rest, with a cursor in its fields, like Rails' `ActiveJob::Continuable` steps:

```rust
pub async fn perform(self, ctx: &Ctx) -> Result<()> {
    let rows: Vec<Row> = ctx.db()?.all("SELECT id FROM posts WHERE id > ?1 ORDER BY id LIMIT 50", params![self.after]).await?;
    for row in &rows {
        // ... the work for one row
    }
    if let Some(last) = rows.last() {
        Reindex { after: last.id }.perform_later(ctx).await?; // the next step
    }
    Ok(())
}
```

A retried step starts again from its own cursor, so steps must be safe to repeat.

## Errors: retry or discard

What `perform` returns decides what happens to the message:

| `perform` returns | Ocre | Like Rails |
|---|---|---|
| `Ok(())` | Acknowledges it | |
| `Err(Error::NotFound)`, `BadRequest`, `Unauthorized`, `Forbidden`, `Invalid`, `PayloadTooLarge` | Logs `discarded, not retried` and acknowledges it: another try would fail the same way | `discard_on`, `ActiveJob::DeserializationError` |
| Any other `Err` (`Internal`, `TooManyRequests`) | Retries it with a growing delay | `retry_on` |

A retried job comes back after twice the time since it was due, 30 seconds at least. For a job processed right away that gives 30 s, 1 min, 3 min, 9 min and 27 min. After `maxRetries: 5` retries, Cloudflare moves the message to the dead-letter queue `<app>-jobs-failed`, where it stays 24 hours; inspect it in the dashboard (Queues > `<app>-jobs-failed`), which lists its messages. The last retry comes about 40 minutes after the job was due. The number of retries is per queue (`maxRetries` in `cloudflare.config.ts`): put jobs that need another policy on their [own queue](#urgent-jobs-named-queues).

To change the policy of one job, map its errors in `perform`: `Err(Error::internal(..))` to retry what Ocre would discard, `Ok(())` (with a log line) to drop what it would retry.

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
| `[ocre jobs] <job> failed, retrying in <n> s: <error>` | `perform` returned an error worth retrying; the message comes back after `n` seconds |
| `[ocre jobs] <job> discarded, not retried: <error>` | `perform` returned a 4xx error; the message is acknowledged |
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

A scheduled task runs on the deployed Worker at times given in plain English or as a cron expression, in UTC:

```sh
ocre g schedule nightly_cleanup "every day at 3am"
```

```text
  create  src/schedules/nightly_cleanup.rs
  create  src/schedules/mod.rs
  update  cloudflare.config.ts
  update  src/lib.rs

Next:
  ocre dev, then: ocre schedules run nightly_cleanup
  ocre deploy (Cron Triggers only fire on the deployed Worker; this one runs at `0 3 * * *`, UTC)
```

| File | What the generator writes |
|---|---|
| `src/schedules/nightly_cleanup.rs` | `pub async fn run(ctx: &Ctx) -> Result<()>`, which does nothing yet |
| `src/schedules/mod.rs` | `run(ctx, cron)`, which matches the expression that fired to a task; keep the `// ocre:schedules` and `// ocre:schedule-dispatch` markers |
| `cloudflare.config.ts` | The expression, `triggers.scheduled({ schedule: "0 3 * * *" }),` after `// ocre:triggers` |
| `src/lib.rs` | `mod schedules;` and the `scheduled` entry point (first schedule only) |

The entry point calls `ocre::jobs::cron`, which runs the task and logs the result:

```rust
/// Cron Triggers (`triggers.scheduled` in cloudflare.config.ts), run by `schedules::run` (src/schedules/mod.rs).
#[worker::event(scheduled)]
async fn scheduled(event: worker::ScheduledEvent, env: worker::Env, _ctx: worker::ScheduleContext) {
    ocre::jobs::cron(event, env, schedules::run).await
}
```

### When: English or cron

The generator turns a plain-English phrase into Cloudflare's five cron fields (minute, hour, day of month, month, day of week) and keeps the phrase in the task's comment, like Loco's English schedules. Times are UTC, 24-hour (`16:30`) or with `am`/`pm`; `midnight` and `noon` work too.

| Phrase | Cron | Runs |
|---|---|---|
| `every minute` | `* * * * *` | Every minute (the shortest interval Cron Triggers allow) |
| `every 15 minutes` | `*/15 * * * *` | Every 15 minutes |
| `every hour`, `hourly` | `0 * * * *` | At the start of every hour |
| `every 6 hours` | `0 */6 * * *` | At 00:00, 06:00, 12:00 and 18:00 |
| `every day at 3am`, `daily at 03:00`, `at 3am` | `0 3 * * *` | Every day at 03:00 |
| `midnight on tuesdays` | `0 0 * * TUE` | Tuesdays at midnight |
| `every monday and friday at 9:30` | `30 9 * * MON,FRI` | Mondays and Fridays at 09:30 |
| `every weekday at 6pm` | `0 18 * * MON-FRI` | Monday to Friday at 18:00 |
| `every weekend at noon` | `0 12 * * SAT,SUN` | Saturdays and Sundays at 12:00 |
| `weekly`, `every week` | `0 0 * * SUN` | Sundays at midnight |
| `monthly`, `every month` | `0 0 1 * *` | The first day of each month, at midnight |
| `every month at 2am` | `0 2 1 * *` | The first day of each month, at 02:00 |

Anything else is refused with both forms in the hint; seconds (`every 15 seconds`) are refused because Cron Triggers run at most once a minute. A cron expression is accepted as is: letters, digits and `* , - / #`, with spaces normalized; Cloudflare validates the rest on deploy (see its [supported expressions](https://developers.cloudflare.com/workers/configuration/cron-triggers/#supported-cron-expressions)).

One task per expression: `ocre g schedule` refuses an expression already in `[triggers] crons` (run the new work from the existing task, or pick another minute). The free plan allows 5 Cron Triggers per account, across all Workers; past 5 in one app, the generator adds a warning to its `Next:` steps. Run several tasks from one cron when you need more.

### List the schedules

`ocre schedules` reads `[triggers] crons` and the dispatch of `src/schedules/mod.rs`, without building the app:

```sh
ocre schedules
```

```text
CRON (UTC)   TASK
0 3 * * *    src/schedules/nightly_cleanup.rs
0 9 * * MON  src/schedules/weekly_digest.rs
```

A cron without a task shows `(no task: fails when it fires)` and a `Next:` step. With `--json`, the list is in `schedules`: `[{"cron": "0 3 * * *", "task": "nightly_cleanup"}, ...]`.

### Keep tasks short: enqueue jobs

A scheduled run has the same CPU limit as a request (10 ms on the free plan), and a failed run is only logged (`[ocre cron] <cron> failed: <error>`), never retried: the task runs again at its next time. So a task should do a few queries and enqueue one job per item; the jobs do the slow work, with retries. This task deletes expired sign-in tokens and enqueues a welcome email for each user who signed up in the last day:

```rust,check
// src/schedules/nightly_cleanup.rs
use ocre::{Ctx, Result, params};
use serde::Deserialize;

use crate::jobs::SendWelcome;

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
        SendWelcome { user_id: id }.perform_later(ctx).await?;
    }
    Ok(())
}
```

The `LIMIT` bounds the work of one run; each `perform_later` is one Queues write. For more than a few jobs, send them with one [`enqueue_all`](#enqueue-many-jobs).

### Run a task locally

Cron Triggers do not fire in `ocre dev`. While it runs, fire one task from another terminal, like Loco's `scheduler --name`:

```sh
ocre schedules run nightly_cleanup
```

```text
  fired nightly_cleanup (0 3 * * *); its `[ocre cron]` line is in the `ocre dev` output
```

It calls the dev server's local endpoint with the task's expression (`--port` if `ocre dev` is not on 8787); the same with curl, spaces as `+`:

```sh
curl 'http://localhost:8787/cdn-cgi/local/scheduled?cron=0+3+*+*+*'
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
✘ [ERROR] [ocre cron] */5 * * * * failed: internal error: no scheduled task for cron `*/5 * * * *`. Fix: add it to the match in src/schedules/mod.rs, or remove its `triggers.scheduled(...)` entry from cloudflare.config.ts
```

## One-off tasks

Rails' rake tasks and Loco's `cargo loco task` run app code from a terminal. A Worker has no terminal: its code only runs inside workerd, for a request, a queue batch, a cron or an email. Ocre covers the uses of tasks without opening a remote-execution endpoint:

| Need | Ocre |
|---|---|
| Change data once (backfill a column, fix rows) | A data migration (`ocre g migration`, then SQL in the file), or `ocre sql "UPDATE ..." --remote` |
| Seed data | `ocre db seed` |
| Recurring work | `ocre g schedule`; `ocre schedules run <task>` runs it on demand in `ocre dev` |
| App code once in production (send a batch of emails, reindex) | A job enqueued from a handler that only an admin can call (`ocre g auth`'s `CurrentUser`, plus your own admin check); it runs with retries, in steps if long |

## Test jobs

A job is a plain struct: build it in a unit test and check what it holds, or test the functions `perform` calls. `perform` itself needs a `Ctx`, which only exists inside workerd, so run it end to end in `ocre dev`: enqueue through a request, then read the `[ocre jobs]` lines, the database (`ocre sql`), or the emails it sent at `http://localhost:8787/ocre/dev/mailers/sent.json` (see [Email](email.md#preview-and-inspect-emails-in-development)). In a handler, `job.perform(&ctx).await` runs a job inline, Rails' `perform_now`, with the same code as the queue.

Request tests (`ocre test --e2e`) check them with `ocre::testing::Client::jobs()`, Rails' `assert_enqueued_with` and `assert_performed_jobs`: it reads `GET /ocre/dev/jobs.json`, which the first `ocre g job` merges into `routes()` (debug builds only, a 404 after `ocre deploy`), and lists the last 50 jobs enqueued (`queue`, `job` as JSON, `name()`) and the last 50 runs (`job`, `outcome`: `done`, `discarded` or `retried`):

```rust,ignore
let mut client = ocre::testing::Client::new();
client.post("/signups", &[("email", "ada@example.com")]);
assert_eq!(client.jobs().enqueued.last().unwrap().name(), Some("send_welcome"));
ocre::testing::eventually(|| client.jobs().performed.iter().any(|run| run.job == "send_welcome" && run.outcome == "done").then_some(()));
```

## Deploy

`ocre deploy` handles the queues and crons of `cloudflare.config.ts`:

- Before deploying, it lists the account's queues (`cf queues list`) and creates (`cf queues create`) every queue named there that is missing (producers, consumers and dead-letter queues), because a consumer of a missing queue fails the deploy. For each queue it creates, it prints `Created queue <name> on Cloudflare` (for example `Created queue shop-jobs on Cloudflare` and `Created queue shop-jobs-failed on Cloudflare`); with `--json` they are listed in `provisioned` as `"queue shop-jobs"`.
- `cf deploy` then registers the consumer and the `triggers.scheduled` crons. Crons fire only on the deployed Worker.

Follow the deployed Worker's job and cron lines in Workers Logs (dashboard: your Worker > Logs; search `[ocre jobs]` or `[ocre cron]`). See [Deployment](deployment.md) for the rest of the deploy.

## Free-plan budget

Limits of the Workers Free plan (September 2026) and what Ocre does about them:

| Limit | Value | What Ocre does |
|---|---|---|
| [Queues operations](https://developers.cloudflare.com/queues/platform/pricing/) | 10,000 a day; a message costs 3 (write, read, delete), each retry 1 more read, a dead-lettered message 1 more write | One message per job; about 3,300 jobs a day, whatever the queue; discarded jobs are not retried |
| [Retention](https://developers.cloudflare.com/queues/platform/limits/) | 24 hours on Free (not configurable) | Retries stop long before: the last one comes after about 40 minutes |
| [Message size](https://developers.cloudflare.com/queues/platform/limits/) | 128 KB; 100 messages and 256 KB per `sendBatch` | `enqueue` refuses larger jobs with an error naming the fix (pass ids); `enqueue_all` splits lists into batches |
| Queues | Named queues cost nothing to create | `ocre g job --queue <name>` adds one, with its own consumer |
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
