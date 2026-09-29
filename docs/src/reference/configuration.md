# Configuration

This page describes every configuration file of an Ocre app (`wrangler.toml`, `.dev.vars`, `Cargo.toml`, `rust-toolchain.toml`), the binding names Ocre requires, and each Worker variable and secret the `ocre` crate reads, with its format, default, where to set it in development and production, and the error when it is missing.

## Before you start

- An Ocre app created with `ocre new` (see [CLI commands](cli.md#ocre-new)). The files below are shown as `ocre new` and the generators write them; `docs-app` stands for your app name.
- Setting production secrets needs a Cloudflare login (`ocre login` or `npx wrangler login`).
- Values of the free-plan limits quoted in comments are dated September 2026; see [Free-plan limits](limits.md) for the sources.

## Files at a glance

| File | Written by | Committed | Purpose |
|---|---|---|---|
| `wrangler.toml` | `ocre new`, then generators (`ocre g job`, `schedule`, `cache`, `auth`, `--realtime`, attachments) and `ocre deploy` (KV ids) | yes | Worker name, build command, logs, bindings (D1, R2, KV, Queues, Durable Objects, rate limiting, email), plain-text variables, cron schedules |
| `.dev.vars` | `ocre new` | no (`.gitignore`) | Secrets and variable overrides for `ocre dev` only |
| `Cargo.toml` | `ocre new`, then `ocre g api --graphql` and `--realtime` (features) | yes | Rust dependencies, Ocre features, API-only mode |
| `rust-toolchain.toml` | `ocre new` | yes | Stable Rust with the `wasm32-unknown-unknown` target |

## wrangler.toml

`ocre new docs-app` writes:

```toml
name = "docs-app"
main = "build/index.js"
compatibility_date = "2026-09-01"

# `ocre dev` sets OCRE_BUILD=--dev (fast, unoptimized); deploys build --release.
[build]
command = "cargo install -q \"worker-build@^0.8\" && worker-build ${OCRE_BUILD:---release}"

# Files in public/ (robots.txt, images, CSS...) are served by Cloudflare before
# the Worker runs: free, and not counted as Worker requests. public/_headers
# sets their headers (e.g. long caching); a single-page app would add
# not_found_handling = "single-page-application".
[assets]
directory = "public"

# Workers Logs: every request (method, URL, status, CPU time) and every
# console line, searchable in the dashboard. Free plan: 200,000 events a day,
# kept 3 days; beyond that, logs are sampled, never billed.
[observability]
enabled = true

# The first `ocre deploy` creates this database; no database_id needed.
[[d1_databases]]
binding = "DB"
database_name = "docs-app"
migrations_dir = "migrations"

# Plain-text Worker variables. Secrets go in .dev.vars for `ocre dev` and in
# `npx wrangler secret put NAME` for production; .dev.vars overrides [vars] locally.
[vars]
# Sender for `ocre::mail::send`: "noreply@yourdomain.com" or "Name <noreply@yourdomain.com>".
MAIL_FROM = "docs-app <noreply@example.com>"
# How production sends mail (`ocre dev` only prints it: MAIL_ADAPTER=log in .dev.vars):
# "resend" needs the RESEND_API_KEY secret (free: 100 emails/day, 3,000/month);
# "cloudflare" needs the [[send_email]] binding below (any recipient needs the
# Workers Paid plan; the free plan only reaches verified addresses of the account).
# MAIL_ADAPTER = "resend"

# Cloudflare Email Service binding, for MAIL_ADAPTER = "cloudflare". `ocre dev`
# simulates it and prints each message.
# [[send_email]]
# name = "EMAIL"

# Receiving email: run `ocre g mailbox`, deploy, then in the Cloudflare dashboard
# Email Routing > Routing rules, send an address to this Worker. Nothing to add here.
```

### Top-level keys

| Key | Value | Notes |
|---|---|---|
| `name` | the app name | The Worker name; the URL is `https://<name>.<your-subdomain>.workers.dev`. `ocre deploy` also names the KV namespaces after it (`<name>-cache`). |
| `account_id` | a 32-character account id | Written right after `name` when you pass `ocre new --account-id <id>` or pick an account in the interactive `ocre new`; needed when your login has several accounts. Without it, wrangler uses the logged-in account. |
| `main` | `build/index.js` | The JavaScript shim `worker-build` writes next to the WebAssembly module. |
| `compatibility_date` | `2026-09-01` | The workerd behavior the Worker runs with. Change it only on purpose. |

### [build]

`command` builds the Rust crate to WebAssembly with `worker-build` (installed with `cargo install` on first use). `${OCRE_BUILD:---release}` makes the mode depend on the `OCRE_BUILD` environment variable:

| Command | `OCRE_BUILD` | Build |
|---|---|---|
| `ocre dev` | `--dev` | Unoptimized, fast to compile |
| `ocre deploy` and every other `ocre` command that runs wrangler | `--release` | `lto = true`, `opt-level = "z"` (from `Cargo.toml`) |
| `npx wrangler deploy` run by hand | unset | `--release` (the default after `:-`) |

### [assets]

`directory = "public"`: files in `public/` are served by Workers Static Assets before the Worker runs. Requests that match a file cost no Worker request and no CPU ([billing](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/)). Limits on the free plan: 20,000 files per Worker version, 25 MiB per file ([limits](https://developers.cloudflare.com/workers/platform/limits/#static-assets), September 2026).

### [observability]

`enabled = true` turns on [Workers Logs](https://developers.cloudflare.com/workers/observability/logs/workers-logs/): each request (method, URL, status, CPU time) and each line the Worker logs, including Ocre's `[ocre]` error lines, is kept and searchable in the dashboard (Workers & Pages > your Worker > Logs). On the free plan: 200,000 log events a day, kept 3 days; above that, events are sampled, never billed (September 2026). Remove the table or set `enabled = false` to keep only `npx wrangler tail` (live logs, nothing stored). Ocre reads nothing from it. See [Deployment](../guides/deployment.md#logs).

### [[d1_databases]]

| Key | Value |
|---|---|
| `binding` | `DB`, required: `ctx.db()` looks it up by this name, and every `ocre` database command looks for it |
| `database_name` | the app name; `ocre deploy` creates the database on the first deploy when `wrangler d1 list` does not have it |
| `migrations_dir` | `migrations`, the numbered SQL files `ocre migrate` applies |

There is no `database_id`: wrangler resolves the database by name. Without an entry whose `binding` is `DB`, every `ocre` command that needs the database stops before running wrangler:

```text
error: wrangler.toml has no D1 database with binding "DB"
hint: add:
[[d1_databases]]
binding = "DB"
database_name = "<app-name>"
migrations_dir = "migrations"
```

If the Worker runs without the binding anyway (a hand-written `wrangler deploy`), `ctx.db()?` fails with a 500 and logs ``[ocre] D1 binding `DB` is missing (...). Fix: add a [[d1_databases]] entry with binding = "DB" to wrangler.toml``.

### [vars]

Plain-text Worker variables, deployed with the code on every `ocre deploy`. `ocre new` sets [`MAIL_FROM`](#mail_from) and leaves [`MAIL_ADAPTER`](#mail_adapter) commented out. Add [`ALLOWED_ORIGINS`](#allowed_origins) here when another site calls the app, [`ALLOWED_HOSTS`](#allowed_hosts) to answer only on your own host names, and your own variables (read them with `ctx.env().var("NAME")`, see [Reading your own variables](#reading-your-own-variables)). Never put secrets under `[vars]`: the file is committed.

### [[send_email]]

```toml
[[send_email]]
name = "EMAIL"
```

The Cloudflare Email Service binding used when `MAIL_ADAPTER = "cloudflare"`. `ocre new` writes it commented out; uncomment it to use it. The name must be `EMAIL`. Without it, sending fails with a 500 whose log says ``cannot send email: the send_email binding `EMAIL` is missing (...). Fix: add a [[send_email]] entry with name = "EMAIL" to wrangler.toml``. See [Email](../guides/email.md).

### [[durable_objects.bindings]] and [[migrations]]

Added by the first `ocre g scaffold <Model> ... --realtime`:

```toml
# Realtime channels (`ocre::realtime`): one Durable Object per channel holds the
# browsers' WebSockets, hibernated between broadcasts so idle connections cost
# no duration. Deploying creates the namespace from the migration below; the
# free plan only accepts SQLite-backed classes (`new_sqlite_classes`).
[[durable_objects.bindings]]
name = "CHANNELS"
class_name = "OcreChannel"

[[migrations]]
tag = "ocre-realtime-v1"
new_sqlite_classes = ["OcreChannel"]
```

`name` must be `CHANNELS` and `class_name` `OcreChannel` (the class Ocre's `realtime` feature exports). `wrangler deploy` creates the namespace from the migration; never edit or remove an applied `[[migrations]]` entry. Without the binding, `ocre::realtime::broadcast` and `WebSocketUpgrade::connect` fail with a 500 naming both entries. See [Realtime](../guides/realtime.md).

### [[r2_buckets]]

Added by the first generator with an `attachment` field:

```toml
# Files (`ocre::storage`, `attachment` fields): an R2 bucket. `ocre dev` keeps a
# local copy under .wrangler/state; `ocre deploy` creates the bucket if needed.
# Free plan: 10 GB stored, 1M writes and 10M reads a month; deletes are free.
[[r2_buckets]]
binding = "STORAGE"
bucket_name = "docs-app-storage"
```

`binding` must be `STORAGE`. `ocre deploy` runs `wrangler r2 bucket info` for each `bucket_name` and `wrangler r2 bucket create` for the missing ones. R2 must be enabled once in the dashboard (it asks for a payment method even for the free tier); when it is not, the deploy stops with a hint saying so (Cloudflare API code 10042). See [File storage](../guides/files.md).

### [[queues.producers]] and [[queues.consumers]]

Added by the first `ocre g job`:

```toml
# Background jobs (`ocre g job`): the Worker sends jobs to this queue and runs
# them (src/jobs/). `ocre deploy` creates both queues. Free plan: 10,000
# Queues operations a day (a job costs 3: write, read, delete; a retry one more
# read), messages kept 24 hours, 128 KB each.
[[queues.producers]]
binding = "JOBS"
queue = "docs-app-jobs"

# Up to 10 messages per run, waiting at most 5 s to fill a batch. Each run is
# one Worker request with 10 ms of CPU on the free plan: lower max_batch_size
# for CPU-heavy jobs. A failing job is retried with a growing delay (30 s,
# 1 min, 3 min, 9 min, 27 min), then moved to docs-app-jobs-failed, where it
# stays 24 hours (dashboard: Queues > docs-app-jobs-failed).
[[queues.consumers]]
queue = "docs-app-jobs"
max_batch_size = 10
max_batch_timeout = 5
max_retries = 5
dead_letter_queue = "docs-app-jobs-failed"
```

| Key | Value | Meaning |
|---|---|---|
| `binding` | `JOBS` | Required name: `ocre::jobs::enqueue` looks it up |
| `queue` | `<app>-jobs` | The same queue for producer and consumer: the app's Worker sends and runs its own jobs |
| `max_batch_size` | `10` | Messages per consumer run (Cloudflare allows up to 100) |
| `max_batch_timeout` | `5` | Seconds to wait to fill a batch (up to 60) |
| `max_retries` | `5` | Deliveries after the first before the message goes to the dead-letter queue |
| `dead_letter_queue` | `<app>-jobs-failed` | Where failed messages are kept (24 hours on the free plan) |

`ocre deploy` runs `wrangler queues info` for every queue named here (producers, consumers and the dead-letter queue) and `wrangler queues create` for the missing ones, before deploying. Without the producer binding, `enqueue` fails with a 500 whose log says ``the queue binding `JOBS` is missing (...). Fix: run `ocre g job <Name>` once; it adds [[queues.producers]] binding = "JOBS" and the consumer to wrangler.toml``. See [Background jobs and schedules](../guides/jobs.md).

### [triggers]

Added by the first `ocre g schedule`; each later schedule adds its expression to the list:

```toml
# Cron Triggers (`ocre g schedule`), in UTC, run by src/schedules/. Free plan: 5 per account.
[triggers]
crons = ["0 3 * * *"]
```

Cron expressions are in UTC. The free plan allows 5 Cron Triggers per account (all Workers together); `ocre g schedule` warns when the app goes past 5.

### [[kv_namespaces]]

Added by `ocre g cache`:

```toml
# Cached values for `ocre::cache` (Workers KV), added by `ocre g cache`. The
# first `ocre deploy` creates the namespace and writes its id here; `ocre dev`
# uses a local one. Free plan: 100,000 reads and 1,000 writes a day, 1 GB.
[[kv_namespaces]]
binding = "CACHE"
```

`binding` must be `CACHE`. For each `[[kv_namespaces]]` entry without an `id`, `ocre deploy` looks for a namespace titled `<name>-<binding>` (lowercased, `_` becomes `-`: `docs-app-cache`), creates it when missing, and writes `id = "<id>"` after the `binding` line. Commit that change so every machine deploys to the same namespace. Without the binding, `ocre::cache` functions fail with a 500 whose log says ``KV binding `CACHE` is missing (...). Fix: run `ocre g cache`, which adds [[kv_namespaces]] binding = "CACHE" to wrangler.toml``. See [Caching](../guides/caching.md).

### [[ratelimits]]

Added by `ocre g auth`:

```toml
# `ocre g auth`: login, sign-up, token and emailed-link routes allow 10 attempts
# a minute per IP address and Cloudflare location (Workers Rate Limiting,
# free plan, no storage used). `period` is 10 or 60 seconds.
[[ratelimits]]
name = "AUTH_RATE_LIMITER"
namespace_id = "2964407"
simple = { limit = 10, period = 60 }
```

| Key | Value | Meaning |
|---|---|---|
| `name` | `AUTH_RATE_LIMITER` | The binding the generated `throttle` (in `src/auth_api.rs`) passes to `ocre::security::rate_limit` |
| `namespace_id` | an integer, derived from the app name | Bindings with the same id share counters across the account's Workers: keep it unique per app |
| `simple.limit` | `10` | Requests allowed per key and period; the next one gets `429 Too Many Requests` |
| `simple.period` | `60` | The window in seconds: `10` or `60` only |

Add more `[[ratelimits]]` entries with other names for your own routes and call `ocre::security::rate_limit(&ctx, "NAME", &key).await?` (see [Sessions, flash and security](../guides/security.md#rate-limiting)). The binding is on the free plan and uses no D1, KV or Durable Object operation; counters are per Cloudflare location and approximate; `ocre dev` simulates it. Without the binding, `rate_limit` fails with a 500 whose log says ``rate limiting binding `AUTH_RATE_LIMITER` is missing (...). Fix: add to wrangler.toml`` followed by the entry to add.

### Binding names Ocre requires

| Binding | wrangler.toml entry | Used by | Added by |
|---|---|---|---|
| `DB` | `[[d1_databases]]` | `ctx.db()`, models, `ocre migrate`, `ocre sql`, `ocre db ...` | `ocre new` |
| `STORAGE` | `[[r2_buckets]]` | `ocre::storage`, `attachment` fields | first generator with an `attachment` field |
| `JOBS` | `[[queues.producers]]` | `ocre::jobs::enqueue`, `enqueue_in`, `ocre::mail::deliver_later` | first `ocre g job` |
| `CACHE` | `[[kv_namespaces]]` | `ocre::cache` | `ocre g cache` |
| `CHANNELS` | `[[durable_objects.bindings]]` (class `OcreChannel`) | `ocre::realtime` | first `--realtime` scaffold |
| `EMAIL` | `[[send_email]]` | `ocre::mail` with `MAIL_ADAPTER = "cloudflare"` | commented out by `ocre new` |

The names are constants of the crate ([`ocre::storage::STORAGE_BINDING`](/api/ocre/storage/constant.STORAGE_BINDING.html), [`ocre::jobs::QUEUE_BINDING`](/api/ocre/jobs/constant.QUEUE_BINDING.html), [`ocre::cache::CACHE_BINDING`](/api/ocre/cache/constant.CACHE_BINDING.html), [`ocre::realtime::CHANNELS_BINDING`](/api/ocre/realtime/constant.CHANNELS_BINDING.html), [`ocre::mail::EMAIL_BINDING`](/api/ocre/mail/constant.EMAIL_BINDING.html)); they cannot be renamed. Every missing-binding error is an `Error::Internal`: the visitor gets a 500 page (or `{"error": {"status": 500, "message": ...}}` from JSON handlers), and the Worker log gets the full message prefixed with `[ocre]`, including the fix.

Rate limiting bindings are the exception: `ocre::security::rate_limit` takes the binding name as an argument, so `AUTH_RATE_LIMITER` is only the name the `ocre g auth` code uses (`RATE_LIMITER` in `src/auth_api.rs`).

## .dev.vars

`ocre new` writes a git-ignored `.dev.vars` with a random local secret and the `log` mail adapter:

```text
SECRET_KEY_BASE=2db19cad9ab790ae4ef78858a74ecbc0300efd7e700b6513da53bcbd89e2b69e4eb28523c9cd31ea53617afc4495996fac76e80bc29233a1ae1dd464bfe3dd80
MAIL_ADAPTER=log
```

- Format: one `NAME=value` per line (dotenv).
- Only `ocre dev` (`wrangler dev`) reads it; it never reaches Cloudflare. A name in both `.dev.vars` and `[vars]` takes the `.dev.vars` value locally, which is how `MAIL_ADAPTER=log` keeps development from sending real mail.
- `.gitignore` lists `.dev.vars` and `.dev.vars.*`. Each clone needs its own: copy the lines above with a value from `ocre secret`.
- `ocre dev` prints what the Worker receives; secrets are hidden (output of an app named `ftapp` with an attachment field):

```text
Using secrets defined in .dev.vars
Your Worker has access to the following bindings:
Binding                                                  Resource                  Mode
env.DB (ftapp)                                           D1 Database               local
env.STORAGE (ftapp-storage)                              R2 Bucket                 local
env.MAIL_FROM ("ftapp <noreply@example.com>")            Environment Variable      local
env.SECRET_KEY_BASE ("(hidden)")                         Environment Variable      local
env.MAIL_ADAPTER ("(hidden)")                            Environment Variable      local
```

## Variables and secrets

Cloudflare gives a Worker two kinds of text values, read the same way in code:

| | Variables (`[vars]`) | Secrets |
|---|---|---|
| Where | `wrangler.toml`, committed | Encrypted on Cloudflare; never in the repository |
| Set in production | edit `wrangler.toml`, then `ocre deploy` | `npx wrangler secret put NAME` (prompts for the value) |
| Set for `ocre dev` | `[vars]`, or `.dev.vars` to override | `.dev.vars` |
| Use for | Sender address, adapter names, allowed origins and hosts | `SECRET_KEY_BASE`, API keys, OAuth client secrets |

```sh
npx wrangler secret put RESEND_API_KEY      # paste the key when asked
npx wrangler secret list                     # names only, never values
ocre secret | npx wrangler secret put SECRET_KEY_BASE   # rotate the app secret (signs everyone out)
```

The free plan allows 64 variables and secrets per Worker, 5 KB each ([limits](https://developers.cloudflare.com/workers/platform/limits/#environment-variables), September 2026).

The `ocre` crate reads the following names.

### SECRET_KEY_BASE

- Kind: secret. Required as soon as a request touches the session, the flash or a JWT.
- Meaning: the root key of the app. The session cookie's AES-256-GCM key is derived from it, and `ocre::jwt` derives its HS256 signing key from it with a different label, so one secret covers cookies and tokens.
- Format: at least 64 characters. `ocre secret` prints a new random value of 128 hex characters (`--json`: `{"command":"secret","ok":true,"secret":"..."}`).
- Development: `ocre new` writes one to `.dev.vars`.
- Production: `ocre deploy` checks `wrangler secret list`; when the Worker has no `SECRET_KEY_BASE` (first deploy), it generates one and uploads it with the deploy (`wrangler deploy --secrets-file`, from a temporary `.wrangler/ocre-secrets.env` readable only by you and deleted afterwards) and prints `Created the SECRET_KEY_BASE secret on Cloudflare`. An existing secret is never replaced. If `wrangler secret list` fails for another reason than a missing Worker, the deploy stops rather than risk overwriting it.
- Rotation: `ocre secret | npx wrangler secret put SECRET_KEY_BASE` alone makes every session cookie and JWT signed with the old value invalid: everyone is signed out. To rotate without signing anyone out, keep the old value in [`SECRET_KEY_BASE_PREVIOUS`](#secret_key_base_previous) first.
- Errors: when it is missing or shorter than 64 characters, requests that read an existing session cookie or change the session answer 500 and log (captured from `ocre dev` with the line removed from `.dev.vars`):

```text
✘ [ERROR] [ocre] the SECRET_KEY_BASE secret is not set. Fix: run `ocre secret`, put the value in .dev.vars as SECRET_KEY_BASE=... for `ocre dev` (`ocre new` does this), and deploy with `ocre deploy`, which uploads it
```

The short-value message is `SECRET_KEY_BASE is shorter than 64 characters.` followed by the same fix. Requests that do not use the session (no cookie, no flash) still work, which is why a missing secret can go unnoticed until the first form submission. See [Sessions, flash and security](../guides/security.md).

### SECRET_KEY_BASE_PREVIOUS

- Kind: secret. Optional; set only while rotating `SECRET_KEY_BASE`.
- Meaning: previous `SECRET_KEY_BASE` values (Rails' `cookies_rotations`). A session cookie encrypted with one of them is still read, then re-encrypted with the current key in the same response; a JWT signed with one of them still verifies until it expires.
- Format: comma-separated, newest first; spaces around values are ignored; each value at least 64 characters.
- Production: Worker secrets cannot be read back, so upload the value you are about to replace first:

```sh
npx wrangler secret put SECRET_KEY_BASE_PREVIOUS   # paste the current SECRET_KEY_BASE
ocre secret | npx wrangler secret put SECRET_KEY_BASE
```

- Removal: `npx wrangler secret delete SECRET_KEY_BASE_PREVIOUS` once the longest session lifetime (two weeks for `ocre g auth`) and the JWT lifetime (one hour) have passed; visitors who did not come back by then start with an empty session.
- Errors: a value shorter than 64 characters makes every request that uses the session or a JWT answer 500 and log `SECRET_KEY_BASE_PREVIOUS has a value shorter than 64 characters. Fix: list old SECRET_KEY_BASE values, comma-separated, newest first`.

### ALLOWED_ORIGINS

- Kind: variable (`[vars]`). Optional.
- Meaning: other origins (a separate frontend, an admin app) allowed to call this app from a browser. Listed origins get CORS headers (methods GET, POST, PUT, PATCH, DELETE; headers `Content-Type`, `Authorization`, `Accept`; credentials allowed) and pass the CSRF check that otherwise refuses cross-site unsafe requests with 403.
- Format: comma-separated origins, scheme and host (and port if any). Spaces and a trailing `/` are ignored; invalid entries are skipped.

```toml
[vars]
ALLOWED_ORIGINS = "https://app.example.com, https://admin.example.com"
```

- Default: unset or empty, no CORS layer at all: only same-origin browser requests may change data.
- Development: add it to `[vars]` (or `.dev.vars`) with the frontend's local origin, such as `http://localhost:5173`.
- Errors: none; a malformed entry is dropped silently, so check the spelling when a request still gets 403. See [Sessions, flash and security](../guides/security.md).

### ALLOWED_HOSTS

- Kind: variable (`[vars]`). Optional.
- Meaning: the host names the app answers to (Rails' `config.hosts`). A request for any other `Host` gets a plain-text `403 Forbidden: blocked host. Add it to ALLOWED_HOSTS to allow it.` before the session, CSRF check or any handler runs.
- Format: comma-separated host names, case-insensitive, without scheme or port. An entry starting with `.` also allows every subdomain: `.example.com` allows `example.com` and `www.example.com`.

```toml
[vars]
ALLOWED_HOSTS = "example.com, .example.com"
```

- Default: unset or empty, every host is allowed.
- Always allowed: `localhost`, `127.0.0.1` and `[::1]`, with any port, so `ocre dev` keeps working.
- Why on Workers: Cloudflare only routes your own host names to the Worker, but the same Worker also answers on `<name>.<subdomain>.workers.dev` and on preview URLs. Listing only your custom domain keeps visitors and search engines on it (list the `workers.dev` host too while you still use it).
- Errors: none at startup; a typo blocks your own domain with the 403 above.

### OAuth client secrets

- Names: `GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET`, `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET` (`client_id_secret` and `client_secret_secret` of `ocre::oauth::GITHUB` and `GOOGLE`).
- Kind: secrets. Required for each provider given to `ocre g auth --oauth`.
- Values: the client id and secret of the OAuth app you register with the provider, with the callback URL `https://<your host>/auth/<provider>/callback` (and `http://localhost:8787/auth/<provider>/callback` for `ocre dev`, in a second OAuth app for GitHub).
- Development: `ocre g auth --oauth` appends commented lines to `.dev.vars`; uncomment them and fill in the values.
- Production: `npx wrangler secret put GITHUB_CLIENT_ID` and `npx wrangler secret put GITHUB_CLIENT_SECRET`, or `ocre secrets push GITHUB_CLIENT_ID GITHUB_CLIENT_SECRET --file <env file>`; `ocre secrets list` shows which are set.
- Errors (500, logged) when one is missing: pressing "Continue with GitHub" logs ``the GITHUB_CLIENT_ID secret is not set. Fix: add it to .dev.vars, and `npx wrangler secret put GITHUB_CLIENT_ID` ``; the callback logs ``the GITHUB_CLIENT_SECRET secret is not set. Fix: put it in .dev.vars for `ocre dev` and run `npx wrangler secret put GITHUB_CLIENT_SECRET` for production``. See [Authentication](../guides/authentication.md#continue-with-github-or-google).

### MAIL_ADAPTER

- Kind: variable. Required to send mail (`ocre::mail::send`, `deliver_later`, the auth emails).
- Values: `log` (print the whole email to the Worker log between `[ocre mail]` lines, send nothing), `resend` (Resend's HTTP API, needs [`RESEND_API_KEY`](#resend_api_key)), `cloudflare` (Email Service through the [`[[send_email]]`](#send_email) binding `EMAIL`). Surrounding spaces are ignored.
- Default: none. Nothing is guessed from which keys exist, so a development machine holding a real key never sends by accident.
- Development: `ocre new` writes `MAIL_ADAPTER=log` to `.dev.vars`.
- Production: uncomment `MAIL_ADAPTER = "resend"` (or `"cloudflare"`) under `[vars]`, then `ocre deploy`.
- Errors (500, logged):
  - unset: ``cannot send email: MAIL_ADAPTER is not set. Fix: set MAIL_ADAPTER to "resend" (with the RESEND_API_KEY secret) or "cloudflare" (with a [[send_email]] binding named EMAIL) under [vars] in wrangler.toml; `ocre new` puts MAIL_ADAPTER=log in .dev.vars so `ocre dev` only logs mail``
  - another value, for example `smtp`: `cannot send email: unknown MAIL_ADAPTER "smtp" (expected log, resend or cloudflare). Fix: ...` (same fix).

See [Email](../guides/email.md).

### MAIL_FROM

- Kind: variable. Required to send mail, with every adapter.
- Format: `noreply@yourdomain.com` or `Name <noreply@yourdomain.com>` (the name may be quoted). With Resend or Cloudflare, the domain must be verified with that provider.
- Default: `ocre new` writes `"<app> <noreply@example.com>"` under `[vars]`; replace `example.com` with your domain before sending real mail.
- Errors (500, logged): unset: `cannot send email: MAIL_FROM is not set. Fix: add MAIL_FROM = "App <noreply@yourdomain.com>" under [vars] in wrangler.toml`; unparsable: `cannot send email: MAIL_FROM "<value>" is not an address. Fix: use "noreply@yourdomain.com" or "App <noreply@yourdomain.com>"`.

### RESEND_API_KEY

- Kind: secret. Required when `MAIL_ADAPTER = "resend"`.
- Format: the API key from [resend.com/api-keys](https://resend.com/api-keys), sent as `Authorization: Bearer <key>` to `https://api.resend.com/emails`.
- Production: `npx wrangler secret put RESEND_API_KEY`.
- Development: not needed with `MAIL_ADAPTER=log`. To send real mail from `ocre dev`, put `RESEND_API_KEY=...` and `MAIL_ADAPTER=resend` in `.dev.vars`.
- Errors (500, logged) when missing or blank: ``cannot send email: the RESEND_API_KEY secret is not set. Fix: create a key at https://resend.com/api-keys and run `npx wrangler secret put RESEND_API_KEY` (and put it in .dev.vars to send from `ocre dev`)``.
- Free plan of Resend: 100 emails a day, 3,000 a month ([quotas](https://resend.com/docs/knowledge-base/account-quotas-and-limits), September 2026).

### Reading your own variables

`ctx.env()` gives the raw Workers environment for any other variable or secret:

```rust,check
// src/support.rs
use axum::{Router, extract::State, routing::get};
use ocre::{Ctx, Result};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/support", get(support))
}

/// `SUPPORT_EMAIL` under [vars] in wrangler.toml; a default when unset.
async fn support(State(ctx): State<Ctx>) -> Result<String> {
    let email = ctx
        .env()
        .var("SUPPORT_EMAIL")
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "support@example.com".to_owned());
    Ok(format!("Write to {email}"))
}
```

Register the module in `src/lib.rs` (`mod support;` under `// ocre:modules`, `.merge(support::routes())` under `// ocre:routes`). For a secret, use `ctx.env().secret("NAME")` the same way.

## Environment variables of the ocre CLI

These are read by the `ocre` command (and wrangler) on your machine, not by the Worker:

| Name | Read by | Meaning |
|---|---|---|
| `OCRE_BUILD` | the `[build] command` | `--dev` or `--release`; set by `ocre` for every wrangler run (see [[build]](#build)) |
| `CLOUDFLARE_API_TOKEN` | wrangler | Authenticates without `ocre login` (CI); the CLI's hints mention it when a Cloudflare call fails |
| `CLOUDFLARE_ACCOUNT_ID` | wrangler | Picks the account with an API token, instead of `account_id` in `wrangler.toml` |

## Cargo.toml

`ocre new docs-app` (full-stack, with `--ocre-path`) writes:

```toml
[package]
name = "docs-app"
version = "0.1.0"
edition = "2024"
publish = false

# Standalone: the app builds even when created inside another Cargo workspace.
[workspace]

[lib]
crate-type = ["cdylib"]

[dependencies]
ocre = { path = "/path/to/ocre/crates/ocre" }
worker = { version = "0.8.7", features = ["http", "axum", "d1"] }
axum = { version = "0.8.9", default-features = false, features = ["form", "json", "query"] }
askama = "0.16.1"
serde = { version = "1.0", features = ["derive"] }

# `strip` breaks wasm-bindgen ("externref table required"); keep symbols.
[profile.release]
lto = true
codegen-units = 1
opt-level = "z"
```

Without `--ocre-path`, the dependency is `ocre = { git = "https://github.com/tgeselle/ocre.rs" }`.

### Ocre features

| Feature | Default | Enables | Turned on by |
|---|---|---|---|
| `html` | yes | askama templates, `render`, HTML error pages, the `Htmx` extractor | on in full-stack apps; off in API-only apps (`default-features = false`) |
| `graphql` | no | `ocre::graphql` (async-graphql, GraphiQL) | `ocre g api <Model> ... --graphql`, which also adds the `async-graphql` dependency |
| `realtime` | no | `ocre::realtime` and the `OcreChannel` Durable Object class | the first `ocre g scaffold <Model> ... --realtime` |

After both generators, the dependency lines read:

```toml
ocre = { path = "/path/to/ocre/crates/ocre", features = ["realtime", "graphql"] }
async-graphql = { version = "7.2.1", default-features = false, features = ["graphiql", "custom-error-conversion"] }
```

GraphQL adds about 1.1 MB of WebAssembly and 20 to 60 ms of CPU each time a Worker instance starts (see [Cost model](../explanations/cost-model.md)); only turn it on when a client needs it.

### API-only mode

`ocre new docs-app --api` writes Ocre without its default `html` feature, no `askama`, and this table at the end:

```toml
[dependencies]
ocre = { git = "https://github.com/tgeselle/ocre.rs", default-features = false }
worker = { version = "0.8.7", features = ["http", "axum", "d1"] }
axum = { version = "0.8.9", default-features = false, features = ["form", "json", "query"] }
serde = { version = "1.0", features = ["derive"] }
...

[package.metadata.ocre]
# JSON only: `ocre g scaffold` generates APIs, Ocre's `html` feature is off.
mode = "api"
```

The generators read `mode = "api"`: `ocre g scaffold` then generates a JSON API (like `ocre g api`), and `ocre g mailer` builds the email text with `format!` instead of askama templates.

## rust-toolchain.toml

```toml
[toolchain]
channel = "stable"
targets = ["wasm32-unknown-unknown"]
```

rustup reads it in the app directory and installs the stable toolchain with the WebAssembly target on first use. A Rust installed without rustup (for example Homebrew's `rust`) ignores it and has no wasm target; `ocre dev`, `ocre deploy` and `ocre new --deploy` then stop with ``the wasm32-unknown-unknown target is not installed for rustc at <sysroot>`` and the hint ``use a rustup toolchain (Homebrew's `rust` has no wasm target) and run `rustup target add wasm32-unknown-unknown` ``. See [Installation](../getting-started/installation.md).

## See also

- [CLI commands](cli.md): [`ocre dev`](cli.md#ocre-dev), [`ocre deploy`](cli.md#ocre-deploy), [`ocre secret`](cli.md#ocre-secret).
- [Generators](generators.md): which generator adds which `wrangler.toml` entry.
- [Deployment](../guides/deployment.md): the first deploy, secrets and remote migrations.
- [Free-plan limits](limits.md) and [Cost model](../explanations/cost-model.md).
- [Cloudflare: Wrangler configuration](https://developers.cloudflare.com/workers/wrangler/configuration/) for every key wrangler accepts.
