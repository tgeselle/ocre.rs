# Cost model

This page explains how an Ocre app spends the Cloudflare Workers Free plan: what counts as a request, where the 10 ms of CPU per request go, what each generated query costs in D1 rows, why KV writes are the scarcest resource, what a background job costs in Queues operations, and when a growing app should move to Workers Paid.

## Before you start

- This is an explanation, not a task: read it when designing a feature or when a limit is getting close. The exact limits, with their sources, are on [Free-plan limits](../reference/limits.md) (values of September 2026).
- It assumes an Ocre app from `ocre new`, with the code the generators write: `ocre g scaffold` for pages, `ocre g job` for jobs, `ocre g cache` for KV, `--realtime` for WebSockets.
- Measured numbers come from the Ocre README (production `wrangler tail` on the free plan, and `wrangler dev` on an Apple M5 Max). Everything else is computed from Cloudflare's published rules and marked as an estimate.

## The daily budgets

| Budget | Free plan (September 2026) | Spent by |
|---|---|---|
| Worker requests | 100,000 a day per account | Every HTTP request that is not a static file, every WebSocket connection |
| CPU | 10 ms per invocation | Rust code running in WebAssembly, JSON and HTML rendering, crypto |
| D1 rows read | 5,000,000 a day | Rows scanned by queries |
| D1 rows written | 100,000 a day | Inserted, updated and deleted rows, plus one per index touched |
| KV reads / writes | 100,000 / 1,000 a day | `ocre::cache` |
| Queues operations | 10,000 a day | Background jobs: 3 per job |
| Durable Object requests | 100,000 a day | Realtime connections and broadcasts |
| R2 | 10 GB, 1M uploads, 10M downloads a month | `attachment` fields |

Daily budgets reset at 00:00 UTC and are shared by every Worker of the account.

## What counts as a request

A Worker request is one invocation of the app's `fetch` entry point. In an Ocre app:

| Traffic | Worker requests | Why |
|---|---|---|
| A file in `public/` (CSS, images, `robots.txt`) | 0 | Workers Static Assets serves it before the Worker runs; asset requests are free and unlimited ([billing](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/)) |
| An HTML page, a JSON call, `GET /up` | 1 | |
| A form submission in a scaffold | 2 | `POST /posts` answers with a redirect, and the browser then loads `GET /posts/{id}` |
| A `304 Not Modified` from `Conditional::fresh_when` | 1 | The Worker still runs (and queries D1); it only skips rendering |
| A response served by Workers Cache (`[cache] enabled = true`) | 1 | Cache hits use no CPU but still count, and enabling the cache also makes static asset requests count ([pricing](https://developers.cloudflare.com/workers/cache/#pricing)); Ocre does not enable it |
| A realtime WebSocket | 1 per connection or reconnection | Messages over an open WebSocket are not Worker requests ([pricing](https://developers.cloudflare.com/workers/platform/pricing/#workers)) |
| A queue consumer batch, a cron run, an incoming email | 1 invocation each | Each has its own 10 ms of CPU. Cloudflare's pricing page does not say whether they count toward the 100,000 requests; budget them as if they do |

Subrequests (D1 queries, KV and R2 calls, `fetch`, broadcasts to a Durable Object) are not billed as requests, but a free invocation may make at most 50 subrequests and 50 D1 queries.

## CPU per request

CPU time is the time the Worker spends computing. Waiting for D1, KV, R2 or `fetch` does not count, so a handler that runs three queries and renders a template stays well under 10 ms. Cloudflare tolerates occasional overruns per isolate and ends requests with error 1102 only when they become frequent ([CPU time](https://developers.cloudflare.com/workers/platform/limits/#cpu-time)).

Measured on the blog starter (release build, production, free plan, `wrangler tail`, 55 requests; README "Measured"):

| Route | CPU median | CPU max | Wall median |
|---|---|---|---|
| `GET /` (list, 1 D1 query) | 2 ms | 23 ms (1 of 40, likely a new isolate) | 18 ms |
| `GET /posts/:id` | 2 ms | 4 ms | 16.5 ms |
| `POST /posts` (insert) | 3 ms | 6 ms | 28 ms |

Other measured costs:

| Work | CPU | Where measured |
|---|---|---|
| Worker startup (blog starter, 420 KB of WebAssembly) | 4 ms | README "Measured" |
| One password hash (PBKDF2-HMAC-SHA256, 100,000 iterations, WebCrypto) | 5.5 ms | `wrangler dev`, 100 hashes in 550 ms |
| A login request (one hash) against `GET /up` | 9.5 ms against 3.5 ms | `wrangler dev` |
| GraphQL schema at each new Worker instance (`--graphql`) | 20 to 60 ms | `wrangler tail` |
| Splitting a 10 MB multipart upload / copying it to R2 | 1.2 ms / 0.15 ms | V8, WebAssembly |

What follows from these numbers:

- Ordinary pages use a fifth to a third of the budget. The rest is headroom for templates and validation, not for heavy computation.
- A password hash takes about half the budget, so a request must never hash twice. Sign-up, login and password changes are the only requests that hash; API keys are checked with SHA-256, which costs almost nothing. bcrypt or argon2 compiled to WebAssembly would not fit.
- GraphQL's startup cost exceeds the budget on every new instance. That is tolerated when it is infrequent, which is why `--graphql` is opt-in.
- Downloads from R2 are streamed by the runtime and cost almost no CPU; uploads are read into memory and split, about 1.2 ms per 10 MB.
- Work that does not fit goes into a job: each consumer batch is a separate invocation with its own 10 ms, and a cron task should only query ids and enqueue one job per item.

## D1 rows

D1 bills rows scanned, not rows returned, and every write to an indexed column writes the index too ([D1 pricing](https://developers.cloudflare.com/d1/platform/pricing/)). The queries a generated model runs (`src/models/<model>.rs`):

| Function | SQL | Rows (estimate from D1's rules) |
|---|---|---|
| `all(ctx, page)` | `SELECT * FROM posts ORDER BY id DESC LIMIT ?1 OFFSET ?2` | The rows SQLite walks along the primary key: up to `limit + offset`. Deep pages cost more than the first one. |
| `count(ctx)` | `SELECT COUNT(*) AS count FROM posts` | Scans the table: grows with the table. Scaffold pages do not call it. |
| `find(ctx, id)` | `SELECT * FROM posts WHERE id = ?1` | 1 read |
| `find_many(ctx, &ids)` | `SELECT * FROM posts WHERE id IN (?1, ...)`, 100 ids per query | 1 read per id found, one query per 100 ids |
| `create` | `INSERT ... RETURNING *`, after one `SELECT 1 ... LIMIT 1` per unique field and per reference | 1 row written plus 1 per index on the table; 1 read per check (indexed) |
| `update` | `UPDATE ... RETURNING *` with the same checks | 1 row written plus 1 per index whose column is written |
| `delete` | `DELETE ... RETURNING *` | 1 row written, plus the rows deleted by `ON DELETE CASCADE` and their indexes |
| `post.comments(ctx, page)` (association) | `WHERE post_id = ?1` on an indexed column | the matching comments, up to the page limit |

Every `references` column gets an index and every unique field a unique index, so lookups by them read only the matching rows. A `WHERE` on another column scans the table: add an index in a migration (`CREATE INDEX ...`) when a column is filtered often, and accept that each write then costs one more row.

The most common way to waste rows and queries is N+1: loading a list, then one record per item. Listing 50 comments with `comment.post(&ctx)` in a loop runs 51 queries, and a free invocation may run only 50. `find_many` loads the posts in one query:

```rust,check
// src/recent_comments.rs
use std::collections::HashMap;

use axum::{Router, extract::State, routing::get};
use ocre::{ApiResult, Ctx, Json, Page};
use serde::Serialize;

use crate::models::{comment, post};

#[derive(Serialize)]
pub struct RecentComment {
    author: String,
    body: String,
    post_title: Option<String>,
}

pub fn routes() -> Router<Ctx> {
    Router::new().route("/api/recent_comments", get(recent))
}

/// Two queries whatever the page size: the comments, then their posts.
async fn recent(State(ctx): State<Ctx>, page: Page) -> ApiResult<Json<Vec<RecentComment>>> {
    let comments = comment::all(&ctx, page).await?;
    let mut ids: Vec<i64> = comments.iter().map(|comment| comment.post_id).collect();
    ids.sort_unstable();
    ids.dedup();
    let posts: HashMap<i64, post::Post> =
        post::find_many(&ctx, &ids).await?.into_iter().map(|post| (post.id, post)).collect();
    let recent = comments
        .into_iter()
        .map(|comment| RecentComment {
            post_title: posts.get(&comment.post_id).map(|post| post.title.clone()),
            author: comment.author,
            body: comment.body,
        })
        .collect();
    Ok(Json(recent))
}
```

A SQL `JOIN` through `ctx.db()?.all::<T>(..)` is the other option; see [Avoid N+1 queries](../guides/models.md#avoid-n1-queries).

Sessions cost no rows: they live in an encrypted cookie. API keys write `last_used_at` at most once an hour for the same reason.

## KV writes, the scarcest resource

Workers KV allows 100,000 reads but only 1,000 writes and 1,000 deletes a day on the free plan. With `ocre::cache::fetch(&ctx, key, ttl, compute)`:

- every call is one read;
- every miss (first call, or after the TTL expired) runs `compute` and is one write;
- every `ocre::cache::delete` (after changing the data behind a key) is one delete, and every `write` one write.

A key that is read often is rewritten each time its TTL expires, so it costs up to 86,400 / ttl writes a day: 24 for a one-hour TTL, 1,440 for the one-minute minimum, which alone is more than the daily budget. With one-hour TTLs, about 40 hot keys fit. Keys per user or per record multiply this: one key per visitor is a write per visitor.

Past the limit, `fetch` keeps working: it logs `[ocre cache] ... failed` and computes the value every time, so the app gets slower rather than broken. For whole pages, HTTP caching is free: `Conditional::fresh_when` answers `304` without rendering, and `CacheControl` lets browsers keep responses. See [Caching](../guides/caching.md).

## Queues operations per job

A job is one queue message. Cloudflare counts operations per message ([Queues pricing](https://developers.cloudflare.com/queues/platform/pricing/)):

| Event | Operations |
|---|---|
| `enqueue` (write), delivery (read), acknowledgement (delete) | 3 |
| Each retry after an `Err` from `perform` | +1 read |
| Moving to the dead-letter queue after the 5th retry | +1 write |
| Each 64 KB of a message beyond the first | the counts above again |

With 10,000 operations a day, that is about 3,300 jobs a day when none fails. A job that fails every time costs 3 + 5 retries + 1 dead-letter write = 9 operations. Keep messages small (ids, not records) so each stays in one 64 KB unit; Ocre refuses messages over 128 KB. `ocre::mail::deliver_later` is one job per email.

Consumer runs take up to 10 messages (`maxBatchSize: 10`, waiting at most 5 s), and each run is one invocation with 10 ms of CPU for the whole batch. Jobs should be I/O (D1, mail, `fetch`); lower `maxBatchSize` in `cloudflare.config.ts` for CPU-heavy ones.

## Realtime and Durable Objects

Each realtime channel is one `OcreChannel` Durable Object using the WebSocket Hibernation API:

| Event | Worker requests | Durable Object requests |
|---|---|---|
| A browser opens (or reopens) the page's WebSocket | 1 | 1 |
| `ocre::realtime::broadcast` to a channel | 0 (a subrequest of the request that broadcasts) | 1, whatever the number of browsers |
| A message to each browser | 0 | 0 (outgoing messages are free) |

Between broadcasts, hibernated sockets cost no duration. Broadcast once per change, not once per row in a loop. The free plan allows 100,000 Durable Object requests and 13,000 GB-s of duration a day ([pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)).

## A worked daily budget

A small blog on the free plan, with comments, accounts, a cached statistic, email and a live comment list. Assumed traffic for one day, and the cost computed with the rules above (estimates, not measurements):

| Activity | Worker requests | D1 reads | D1 writes | Other |
|---|---|---|---|---|
| 1,000 views of the post list (50 per page) | 1,000 | 50,000 | | CSS and images from `public/`: free |
| 2,000 views of a post (`find`) | 2,000 | 2,000 | | |
| 100 comments posted (POST + redirect; reference check; insert into `comments`, which has the `post_id` index) | 200 | 200 | 200 | |
| 20 sign-ups (one password hash each; `users` has a unique `email`) | 40 | 20 | 40 | your code queues 20 welcome emails with `deliver_later` |
| 30 password resets (the generated `src/passwords.rs` sends the email directly with `mail::send`) | 60 | 60 | 60 | 30 emails on Resend |
| 20 welcome-email jobs in consumer batches | up to 20 | 20 | | 60 Queues operations; 20 emails on Resend |
| 1 nightly cron | 1 | a few | | |
| Home page statistic in KV, one-hour TTL, on the list page | | 24 recomputations | | 1,000 KV reads, 24 KV writes |
| 300 realtime connections, 100 broadcasts | 300 | | | 400 Durable Object requests |
| Total | about 3,600 of 100,000 | about 52,300 of 5,000,000 | about 300 of 100,000 | KV writes 24 of 1,000; Queues 60 of 10,000 |

Almost every budget is below 5%. The one that is not is email: 50 emails is half of Resend's 100 a day. Traffic can grow about twenty-fold before Worker requests run out, while a campaign that makes 100 people sign up in a day already uses the whole email quota. The CPU budget is per request, so it does not grow with traffic: 2 to 3 ms for pages and about 5.5 ms more for the requests that hash a password.

To see real numbers for your app, use the Cloudflare dashboard: Workers & Pages > your Worker > Metrics (requests, CPU time, errors), and Storage & databases > D1 > your database > Metrics (rows read and written).

## When to move to Workers Paid

Workers Paid costs a minimum of $5 a month per account and raises every budget at once ([Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/), September 2026):

| Resource | Free | Paid (included, then pay as you go) |
|---|---|---|
| Worker requests | 100,000 a day | 10 million a month, then $0.30 per million |
| CPU | 10 ms per invocation | 30 million CPU-ms a month, then $0.02 per million; 30 s per invocation by default, up to 5 minutes |
| Subrequests | 50 per invocation | 10,000 per invocation |
| D1 | 5 million reads, 100,000 writes a day, 5 GB | 25 billion reads, 50 million writes a month, 5 GB; then usage pricing |
| KV | 100,000 reads, 1,000 writes a day, 1 GB | 10 million reads, 1 million writes a month, 1 GB; then usage pricing |
| Queues | 10,000 operations a day, 24-hour retention | 1 million operations a month, then $0.40 per million; retention 4 days by default, up to 14 |
| Durable Objects | 100,000 requests, 13,000 GB-s a day | 1 million requests, 400,000 GB-s a month; then usage pricing |
| Cron Triggers | 5 per account | 250 per account |
| Email Service sending | verified addresses only | any recipient, 3,000 a month included, then $0.35 per 1,000 |

Move when one of these happens, not before:

- The daily request limit is reached (visitors get error 1027), or is regularly above half.
- Requests fail with error 1102 because a route needs more CPU than the free plan tolerates (large imports, image processing, many password hashes).
- `[ocre cache] ... failed` lines appear every day: KV writes run out.
- The app needs more than about 3,000 jobs a day, messages kept longer than 24 hours, or more than 5 Cron Triggers in the account.
- The app must send email to any recipient through Cloudflare (`MAIL_ADAPTER = "cloudflare"`); with Resend, its own paid plans are the alternative.

The generated app runs unchanged on Workers Paid: no Ocre setting depends on the plan. Only the defaults written for the free plan (`max_batch_size = 10`, one-hour cache TTLs, 10 MB attachment limits) become worth revisiting.

## See also

- [Free-plan limits](../reference/limits.md): every limit with its source.
- [Architecture](architecture.md): how a request flows through `ocre::serve`.
- [Caching](../guides/caching.md), [Background jobs and schedules](../guides/jobs.md), [Realtime](../guides/realtime.md), [Authentication](../guides/authentication.md).
- [Configuration](../reference/configuration.md): the `cloudflare.config.ts` settings mentioned here.
