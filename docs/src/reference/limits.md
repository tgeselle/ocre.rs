# Free-plan limits

This page lists the Cloudflare Workers Free plan limits that matter to an Ocre app, as published by Cloudflare in September 2026, with the source of each value and what Ocre does about it.

## Before you start

- The values apply to an app deployed with `ocre deploy` on a Cloudflare account without Workers Paid. `ocre dev` (local workerd) enforces none of them.
- Most limits are per account and per day (reset at 00:00 UTC), shared by every Worker of the account, not per app.
- Cloudflare changes its limits. Each row links to the page the value was read from (September 2026); check it before relying on a number. For how an app spends these budgets and a worked daily example, see [Cost model](../explanations/cost-model.md).

## Workers

| Limit | Free plan (September 2026) | Source | What Ocre does |
|---|---|---|---|
| Requests | 100,000 a day per account; past it, error 1027 until midnight UTC | [Workers limits](https://developers.cloudflare.com/workers/platform/limits/#daily-requests) | Files in `public/` are served by Workers Static Assets, free and unlimited, without running the Worker. A WebSocket connection counts once, not per message. |
| Static asset requests | free and unlimited; 20,000 files per version, 25 MiB per file | [Static assets billing](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/), [limits](https://developers.cloudflare.com/workers/platform/limits/#static-assets) | `[assets] directory = "public"` in `wrangler.toml` from `ocre new`. Turning on Workers Cache makes asset requests count ([Workers Cache pricing](https://developers.cloudflare.com/workers/cache/#pricing)); Ocre does not enable it. |
| CPU time | 10 ms per invocation (HTTP request, Cron Trigger, queue consumer batch); occasional overruns are tolerated per isolate, frequent ones end in error 1102 | [CPU time](https://developers.cloudflare.com/workers/platform/limits/#cpu-time) | Waiting on D1, KV, R2 or `fetch` does not count. Passwords are hashed with WebCrypto PBKDF2 (native, about 5.5 ms) instead of WebAssembly; R2 downloads are streamed without passing through WebAssembly; GraphQL (20 to 60 ms at instance start) is opt-in. |
| Memory | 128 MB per isolate, shared by the concurrent requests it runs | [Memory](https://developers.cloudflare.com/workers/platform/limits/#memory) | Uploads are read into memory: generated attachment rules default to 10 MB per file and forms refuse bodies over the sum of their files' limits plus 1 MB (413). |
| Request body size | 100 MB (Cloudflare Free plan); larger bodies get 413 before the Worker runs | [Request limits](https://developers.cloudflare.com/workers/platform/limits/#request-and-response-limits) | Keep `max_bytes` of attachment `Rules` in the tens of MB. |
| Worker size | 64 MiB uncompressed | [Worker size](https://developers.cloudflare.com/workers/platform/limits/#worker-size) | Release builds use `opt-level = "z"` and LTO; the blog starter measured 420 KB of WebAssembly (130 KB gzipped). GraphQL adds about 1.1 MB. |
| Startup time | 1 second to evaluate the global scope | [Startup time](https://developers.cloudflare.com/workers/platform/limits/#worker-startup-time) | The blog starter measured 4 ms. Translations are parsed on first use, not at startup. |
| Subrequests | 50 per invocation; 1,000 to Cloudflare services | [Subrequests](https://developers.cloudflare.com/workers/platform/limits/#subrequests) | Generated pages run one D1 query (lists, show) or a few (create/update add one check per unique field and reference). `find_many` loads many rows in one query per 100 ids instead of one per row. |
| Variables and secrets | 64 per Worker, 5 KB each | [Environment variables](https://developers.cloudflare.com/workers/platform/limits/#environment-variables) | Ocre needs at most four: `SECRET_KEY_BASE`, `MAIL_FROM`, `MAIL_ADAPTER`, `RESEND_API_KEY` (plus `ALLOWED_ORIGINS` when used). |
| Cron Triggers | 5 per account | [Account plan limits](https://developers.cloudflare.com/workers/platform/limits/#account-plan-limits) | `ocre g schedule` warns when the app has more than 5; run several tasks from one cron. |
| Workers Logs | 200,000 log events a day, kept 3 days | [Workers Logs pricing](https://developers.cloudflare.com/workers/platform/pricing/#workers-logs) | Ocre logs only errors and job/cron outcomes (`[ocre ...]` lines); the `log` mail adapter prints whole emails, so do not use it in production. |

## D1 (database, binding `DB`)

| Limit | Free plan (September 2026) | Source | What Ocre does |
|---|---|---|---|
| Rows read | 5 million a day (rows scanned, not rows returned) | [D1 pricing](https://developers.cloudflare.com/d1/platform/pricing/) | Generated lists are paginated (`Page`: default 50, at most 100) and ordered by the primary key; every `references` column gets an index, every unique field a unique index. |
| Rows written | 100,000 a day; each index on a written column adds one row written | [D1 pricing](https://developers.cloudflare.com/d1/platform/pricing/) | Sessions live in an encrypted cookie (no rows); API keys update `last_used_at` at most once an hour. |
| Storage | 5 GB per account; 500 MB per database; 10 databases per account | [D1 limits](https://developers.cloudflare.com/d1/platform/limits/) | One database per app (`database_name` = app name). Files go to R2, not D1. |
| Queries per invocation | 50 | [D1 limits](https://developers.cloudflare.com/d1/platform/limits/) | `find_many` instead of `find` in a loop. |
| Bound parameters | 100 per query | [D1 limits](https://developers.cloudflare.com/d1/platform/limits/) | `find_many` splits ids into chunks of 100. |
| Row or string size | 2,000,000 bytes (2 MB) | [D1 limits](https://developers.cloudflare.com/d1/platform/limits/) | Keep large text and files in R2 (`attachment` fields). |
| SQL statement length | 100 KB | [D1 limits](https://developers.cloudflare.com/d1/platform/limits/) | Queries use `?N` placeholders, never inlined values. |
| Numbers | D1 returns numbers as JavaScript numbers (exact up to 2^53 - 1) | [`ocre::MAX_SAFE_INTEGER`](/api/ocre/constant.MAX_SAFE_INTEGER.html) | Generated `integer` validations reject values beyond ±9,007,199,254,740,991. |

## Queues (background jobs, binding `JOBS`)

| Limit | Free plan (September 2026) | Source | What Ocre does |
|---|---|---|---|
| Operations | 10,000 a day; a delivered message costs 3 (write, read, delete), each retry 1 more read, a dead-lettered message 1 more write; each 64 KB of a message counts as one operation | [Queues pricing](https://developers.cloudflare.com/queues/platform/pricing/) | One message per job: about 3,300 jobs a day without retries. |
| Retention | 24 hours, not configurable | [Queues pricing](https://developers.cloudflare.com/queues/platform/pricing/) | Retries (30 s, 1 min, 3 min, 9 min, 27 min) end about 40 minutes after the first failure; the dead-letter queue `<app>-jobs-failed` keeps failures 24 hours. |
| Message size | 128 KB (1 KB = 1,000 bytes, including about 100 bytes of metadata) | [Queues limits](https://developers.cloudflare.com/queues/platform/limits/) | `enqueue` refuses messages over 127,000 bytes: `cannot enqueue a <n> byte message: Cloudflare Queues messages hold 128 KB at most. Fix: store large data in D1 or R2 and put its id in the job`. |
| Delay | 24 hours, on send and on retry | [Delay messages](https://developers.cloudflare.com/queues/configuration/batching-retries/#delay-messages) | `enqueue_in` refuses longer delays and names the fix (a scheduled task, or a due time in D1). |
| Batches | up to 100 messages, 60 s wait | [Queues limits](https://developers.cloudflare.com/queues/platform/limits/) | `max_batch_size = 10`, `max_batch_timeout = 5`: each consumer run handles up to 10 jobs within one invocation's 10 ms of CPU. |
| Retries | up to 100 | [Queues limits](https://developers.cloudflare.com/queues/platform/limits/) | `max_retries = 5`. |
| Queues | 10,000 per account | [Queues limits](https://developers.cloudflare.com/queues/platform/limits/) | Two per app: `<app>-jobs` and `<app>-jobs-failed`, created by `ocre deploy`. |

## R2 (files, binding `STORAGE`)

| Limit | Free plan (September 2026, per month) | Source | What Ocre does |
|---|---|---|---|
| Storage | 10 GB-month (Standard storage class) | [R2 pricing](https://developers.cloudflare.com/r2/pricing/#free-tier) | Files of deleted and replaced records are deleted; files of rows removed by `ON DELETE CASCADE` are not. |
| Class A operations | 1 million a month (`PutObject`, `ListObjects`, ...) | [R2 pricing](https://developers.cloudflare.com/r2/pricing/#class-a-operations) | One per upload. |
| Class B operations | 10 million a month (`GetObject`, `HeadObject`, ...) | [R2 pricing](https://developers.cloudflare.com/r2/pricing/#class-b-operations) | One per download or `304`. |
| Deletes, egress | free | [R2 pricing](https://developers.cloudflare.com/r2/pricing/#free-operations) | |
| Activation | R2 must be enabled once in the dashboard, which asks for a payment method even for the free tier | [R2 pricing](https://developers.cloudflare.com/r2/pricing/) | `ocre deploy` stops with a hint when the account lacks it (API code 10042). |

## Workers KV (cache, binding `CACHE`)

| Limit | Free plan (September 2026) | Source | What Ocre does |
|---|---|---|---|
| Reads | 100,000 a day | [KV limits](https://developers.cloudflare.com/kv/platform/limits/) | `ocre::cache::fetch` reads once per call. |
| Writes | 1,000 a day (to different keys); 1 per second to the same key | [KV limits](https://developers.cloudflare.com/kv/platform/limits/) | The scarcest free resource: each miss of `fetch`, each `write` and each `delete` is one. Past a limit, `fetch` logs `[ocre cache] ... failed` and computes the value instead of failing. |
| Deletes, lists | 1,000 a day each | [Workers pricing, KV](https://developers.cloudflare.com/workers/platform/pricing/#workers-kv) | `ocre::cache::delete` is one delete; Ocre never lists. |
| Storage | 1 GB per account and per namespace | [KV limits](https://developers.cloudflare.com/kv/platform/limits/) | Values are JSON; keep them small. |
| Key size | 512 bytes | [KV limits](https://developers.cloudflare.com/kv/platform/limits/) | Longer or empty keys are refused with an error naming the fix. |
| Value size | 25 MiB | [KV limits](https://developers.cloudflare.com/kv/platform/limits/) | |
| Minimum TTL | 60 seconds (`expirationTtl`) | [Write key-value pairs](https://developers.cloudflare.com/kv/api/write-key-value-pairs/) | `fetch` and `write` refuse shorter TTLs (`ocre::cache::MIN_TTL`). |
| Consistency | writes can take up to 60 seconds to be visible elsewhere | [Write key-value pairs](https://developers.cloudflare.com/kv/api/write-key-value-pairs/) | Not for data that must be read back immediately. |

## Durable Objects (realtime, binding `CHANNELS`)

| Limit | Free plan (September 2026) | Source | What Ocre does |
|---|---|---|---|
| Requests | 100,000 a day; incoming WebSocket messages count 1 per 20; outgoing messages are free | [Durable Objects pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/) | One request per connection (and reconnection) and one per broadcast. Browsers only listen, so they send no messages. |
| Duration | 13,000 GB-s a day (at 128 MB, about 28 hours awake) | [Durable Objects pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/) | `OcreChannel` uses the WebSocket Hibernation API: objects are only billed while handling a connection or a broadcast. |
| Storage backend | SQLite-backed classes only | [Durable Objects pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/) | `new_sqlite_classes = ["OcreChannel"]`; the channel stores nothing. |
| Classes, storage | 100 classes per account, 5 GB storage per account | [Durable Objects limits](https://developers.cloudflare.com/durable-objects/platform/limits/) | One class, no storage. |
| Received WebSocket message size | 32 MiB | [Durable Objects limits](https://developers.cloudflare.com/durable-objects/platform/limits/) | Client messages are ignored. |

## Email

| Limit | Free plan (September 2026) | Source | What Ocre does |
|---|---|---|---|
| Email Routing (receiving) | free and unlimited; 200 routing rules per domain, 200 destination addresses per account, 25 MiB per incoming message | [Email Service pricing](https://developers.cloudflare.com/email-service/platform/pricing/), [limits](https://developers.cloudflare.com/email-service/platform/limits/#email-routing-limits) | `ocre g mailbox` wires the Worker's `email` event; each received message is one invocation with 10 ms of CPU. |
| Email Service (sending, `MAIL_ADAPTER = "cloudflare"`) | on Workers Free, only to verified destination addresses of the account (free); any recipient needs Workers Paid (3,000 a month included, then $0.35 per 1,000); 5 MiB per message, 50 recipients | [Email Service pricing](https://developers.cloudflare.com/email-service/platform/pricing/), [limits](https://developers.cloudflare.com/email-service/platform/limits/) | Fine for mail to yourself; use Resend for sign-up and password-reset mail on the free plan. |
| Resend (sending, `MAIL_ADAPTER = "resend"`) | 100 emails a day, 3,000 a month (each recipient counts; received emails too); up to 3 verified domains | [Resend quotas](https://resend.com/docs/knowledge-base/account-quotas-and-limits) | `deliver_later` retries provider failures through the jobs queue. |

## What happens at a limit

| Resource | Behavior past the limit | Ocre's handling |
|---|---|---|
| Worker requests | Error 1027 page (or the request bypasses the Worker if its route fails open) until 00:00 UTC | none possible inside the app |
| CPU | Error 1102 "Worker exceeded resource limits" when overruns become frequent | Design: I/O in handlers, heavy work in jobs |
| D1 rows | Queries fail | Errors become 500 responses, logged with `[ocre]` |
| KV | Operations of that type fail until 00:00 UTC | `fetch` falls back to computing the value |
| Queues | Sends fail | `enqueue` returns the error; the handler decides |
| Durable Objects | Requests fail | Broadcast failures are logged (`[ocre realtime] broadcast to <channel> failed: ...`) and the request still succeeds |

## See also

- [Cost model](../explanations/cost-model.md): how a request spends these limits, with a daily budget example.
- [Configuration](configuration.md): the `wrangler.toml` entries behind each binding.
- [Background jobs and schedules](../guides/jobs.md), [File storage](../guides/files.md), [Caching](../guides/caching.md), [Realtime](../guides/realtime.md), [Email](../guides/email.md).
- [Cloudflare Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/) for the Workers Paid plan.
