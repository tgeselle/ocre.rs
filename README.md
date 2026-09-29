# Ocre

Rails-like Rust web framework for Cloudflare Workers, designed to run on the
Workers **free plan** and to be written by **AI agents**.

Status: early. The core crate, the `ocre` CLI and an example app run with
`wrangler dev` and in production on the free plan; jobs, auth and realtime are
not built yet.

## Quick start

```sh
cargo install --git https://github.com/tgeselle/ocre.rs ocre-cli   # installs `ocre`
ocre new      # guided setup: name, starter, Cloudflare login, git, first deploy
```

The guided setup runs in a terminal. Every question has a flag, so the same
app can be created without prompts (agents, scripts, CI):

```sh
ocre new my-app --starter blog --git --deploy --json
cd my-app
ocre g scaffold Comment author:string body:text
ocre dev       # applies local migrations, serves http://localhost:8787
ocre deploy    # deploys, creates the D1 database if needed, applies remote migrations
```

Each generated app has an `AGENTS.md` with the conventions, commands and
free-plan limits an agent needs.

## CLI

| Command | Effect |
|---|---|
| `ocre new [name]` | App skeleton. In a terminal, asks for anything flags did not answer |
| `ocre login` | Cloudflare login in the browser, unless already logged in |
| `ocre g model <Model> field:type...` | Migration and `src/models/<model>.rs`: struct, validations, queries, associations |
| `ocre g scaffold <Model> field:type... [--realtime]` | Model (unless it exists) plus HTML CRUD: handlers, routes, templates; registers modules in `src/lib.rs`. `--realtime`: the index page updates live in every open browser (see [Realtime](#realtime)). In an API-only app: same as `ocre g api` |
| `ocre g api <Model> field:type... [--graphql]` | Model (unless it exists) plus JSON REST resource under `/api/<plural>`; `--graphql` also exposes it on `/graphql` |
| `ocre g auth` | Authentication generated into the app: users, sign-up/login/logout pages, password reset and magic links by email, JWTs and API keys (see [Authentication](#authentication)). Runs once |
| `ocre g migration <name> [field:type...]` | Numbered migration; `create_<table>`, `add_<x>_to_<table>` and `remove_<x>_from_<table>` get their SQL from the name and fields |
| `ocre g mailer <Name> action...` | `src/mailers/<name>.rs`, one function per action returning an `ocre::mail::Email`, with `templates/mailers/<name>/<action>.{txt,html}` (text built with `format!` in API-only apps) |
| `ocre g mailbox` | `src/mailbox.rs` for incoming email, wired to the Worker's `email` event in `src/lib.rs` |
| `ocre g job <Name> [field:type...]` | `src/jobs/<name>.rs` (arguments + `perform`), added to the `Job` enum and `perform` match in `src/jobs/mod.rs`; the first job wires the `JOBS` queue and the `queue` event (see [Background jobs](#background-jobs-and-scheduled-tasks)) |
| `ocre g schedule <name> "<cron>"` | `src/schedules/<name>.rs`, run by a Cron Trigger added to `[triggers] crons`, dispatched by cron in `src/schedules/mod.rs`; the first one wires the `scheduled` event |
| `ocre g cache` | Adds the `CACHE` Workers KV binding to `wrangler.toml` for `ocre::cache::fetch` (see [Caching](#caching)) |
| `ocre g locale <code>...` | `locales/<code>.yml` per code, declared in `ocre::locales!(...)` in `src/lib.rs`; the first run makes its first code the default locale and adds the `I18n` layer to `routes()` (see [Translations](#translations)) |
| `ocre migrate [--remote]` | Apply D1 migrations |
| `ocre migrate --status [--remote]` | Show wrangler's pending-migrations table; `--json` lists them in `pending` |
| `ocre db seed [--remote]` | Run `db/seeds.sql` |
| `ocre db reset` | Local only: delete `.wrangler/state/v3/d1`, apply migrations, run `db/seeds.sql` if present |
| `ocre sql "<query>" [--remote]` | Run SQL and print the rows as a table; `--json` returns wrangler's results in `rows` |
| `ocre dev [--port N]` | Checks locale files, applies local migrations, then `wrangler dev` |
| `ocre deploy` | Existing database: migrate, then deploy. New database: deploy (creates it), then migrate. Uploads a new `SECRET_KEY_BASE` only when the Worker has none (an existing one is never rotated). Creates the queues `wrangler.toml` names when missing, the KV namespaces without an `id` (then writes the id into `wrangler.toml`), and the R2 buckets of `[[r2_buckets]]` when missing. Refuses locale files the Worker could not load |
| `ocre i18n missing` | Keys of the default locale missing from other locales (with the plural forms each language needs), undeclared or invalid locale files; fails when there is any |
| `ocre routes [filter]` | The app's routes (method, path, handler), read from `src/lib.rs` and the modules it merges; `--json` returns `routes` |
| `ocre secret` | New random `SECRET_KEY_BASE` value (128 hex characters), like `rails secret` |

`ocre new` flags:

| Flag | Effect | Default without prompts |
|---|---|---|
| `--api` / `--full-stack` | API only: JSON, no templates, Ocre's `html` feature off (like `rails new --api`) | full-stack |
| `--starter empty\|blog` | `blog` adds a `Post` resource (title, body, published) | `empty` |
| `--login` / `--no-login` | Log in to Cloudflare if needed (opens a browser) | no login |
| `--account-id <id>` | Account to deploy to; required when the login has several | none |
| `--git` / `--no-git` | `git init` | no git |
| `--deploy` / `--no-deploy` | Deploy right away (implies `--login`) | no deploy |
| `--yes`, `-y` | Never prompt, even in a terminal | |
| `--ocre-path <dir>` | Use a local `crates/ocre` instead of the git dependency | git |

Field types: `string`, `text`, `integer`, `float`, `boolean`, `date`,
`datetime`, `references` (`author:references` adds `author_id` with a foreign
key, `ON DELETE CASCADE`), `attachment` (a file in R2, see [Files](#files)). Suffixes: `?` optional (NULL allowed), `^` unique.
Scaffold routes: `GET /posts`, `GET /posts/new`, `POST /posts`,
`GET /posts/{id}`, `GET /posts/{id}/edit`, `POST /posts/{id}` (update),
`POST /posts/{id}/delete`.

## Models

Models are plain generated Rust, not derive macros: `ocre g model Post
title:string^ author:references` writes `src/models/post.rs` with the `Post`
struct, `NewPost` and `PostChanges` (the create and update inputs), their
`validate()`, and `all`, `count`, `find`, `find_many`, `create`, `update`,
`delete`, plus `post.author(&ctx)` and, in `author.rs`, `author.posts(&ctx,
page)`. Every query and rule is visible in one file, so people and agents can
read, grep and change it.

Validation collects every error before answering, with Rails' messages:
`create` and `update` add uniqueness ("has already been taken") and
foreign-key ("must exist") checks. A failed validation is `Error::Invalid`,
status 422: HTML forms re-render with the messages and the typed values, JSON
answers `{"error": {"status": 422, "message": "Validation failed", "fields":
{"title": ["can't be blank"]}}}`, GraphQL puts `fields` in `extensions`.

## JSON APIs

`ocre g api Post title:string body:text` generates `src/posts_api.rs`:

| Route | Effect |
|---|---|
| `GET /api/posts?limit=&offset=` | List, newest first; `limit` 1-100 (default 50) |
| `GET /api/posts/{id}` | One record |
| `POST /api/posts` | Create; every field required; `201` |
| `PATCH /api/posts/{id}` | Update only the fields sent |
| `DELETE /api/posts/{id}` | Delete; `204` |

Errors are JSON: `{"error": {"status": 404, "message": "Not found"}}`;
internal details are logged, never returned. The handlers and the GraphQL
resolvers call the model, so both share its rules. When the model already
exists (after `ocre g scaffold` or `ocre g model`), it is reused.

With `--graphql`, the same resource gets `posts(limit, offset)`, `post(id)`,
`createPost(input)`, `updatePost(id, patch)` and `deletePost(id)` on
`POST /graphql`, and GraphiQL on `GET /graphql`. GraphQL is opt-in because it
costs on the free plan: about 1.1 MB more WebAssembly, and 20-60 ms of CPU
each time a new Worker instance starts, measured with `wrangler tail` (the
free plan allows 10 ms per request, with tolerance for infrequent overruns).

An API-only app (`ocre new --api`) has no templates and no askama; `ocre g
scaffold` generates JSON APIs there. The mode is stored in `Cargo.toml` as
`[package.metadata.ocre] mode = "api"`.

Contract for agents: with `--json` (or without a terminal) commands never
prompt, and stdout carries exactly one JSON object, `{"ok": true, "command",
"created", "updated", "url", "email", "next"}` or `{"ok": false, "error",
"hint"}`; wrangler output goes to stderr. Exit code is 0 on success, 1 on
failure. Generators never overwrite files.

## Web

`ocre::serve` wraps every app with:

- **Sessions** in an encrypted cookie (AES-256-GCM, key derived from the
  `SECRET_KEY_BASE` secret), like Rails' cookie store: no database rows or KV
  operations. Handlers take `session: ocre::Session` (`get`, `insert`,
  `remove`, `clear`). `ocre new` writes a local secret to `.dev.vars` (git
  ignored); `ocre deploy` creates the production one.
- **Flash**: `session.flash("notice", "...")` before a redirect; the next page
  takes `flash: ocre::Flash`. Scaffolds show "Post was successfully created."
- **CSRF protection** without tokens: unsafe requests (POST, PUT, PATCH,
  DELETE) and WebSocket handshakes that a browser sends from another site
  (`Sec-Fetch-Site`, or `Origin` against `Host` for older browsers) get 403,
  the check Go 1.25 ships as `http.CrossOriginProtection`. Cookies are `SameSite=Lax`.
- **CORS** for the origins in the `ALLOWED_ORIGINS` Worker variable
  (comma-separated), which are also trusted by the CSRF check.
- **Security headers**: `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: SAMEORIGIN`, `Referrer-Policy:
  strict-origin-when-cross-origin`, `X-XSS-Protection: 0`,
  `X-Permitted-Cross-Domain-Policies: none`, and HSTS on HTTPS. A handler's own
  value wins.

Generated apps also have `GET /up` (health check) and `public/`, served by
Workers Static Assets before the Worker runs, so static files cost no Worker
request or CPU.

## Email

`ocre::mail::send(&ctx, Email::new(to, subject, text).html(html)).await?`
sends from the `MAIL_FROM` variable (`noreply@yourdomain.com` or
`Name <noreply@yourdomain.com>`; `ocre new` puts a placeholder under `[vars]`
in `wrangler.toml`). The `MAIL_ADAPTER` variable names the adapter; nothing is
guessed from which keys happen to be set, so a development machine holding a
real API key still never sends by accident:

| `MAIL_ADAPTER` | Delivery | Configuration | Free-plan limits (September 2026) |
|---|---|---|---|
| `log` | Prints the whole email (headers, text, HTML) to the Worker console between `[ocre mail]` lines, like Rails' letter_opener. `ocre new` writes `MAIL_ADAPTER=log` to `.dev.vars`, which overrides `[vars]` in `ocre dev` | none | none |
| `resend` | `POST https://api.resend.com/emails` | `RESEND_API_KEY` secret (`npx wrangler secret put RESEND_API_KEY`), `MAIL_FROM` on a domain verified in Resend | [Resend free plan](https://resend.com/docs/knowledge-base/account-quotas-and-limits): 100 emails a day, 3,000 a month, one domain; any recipient |
| `cloudflare` | Cloudflare Email Service through the `EMAIL` [send_email binding](https://developers.cloudflare.com/email-service/api/send-emails/workers-api/) (uncomment `[[send_email]]` in `wrangler.toml`; `ocre dev` simulates it) | `MAIL_FROM` on a domain onboarded to Email Service | [Workers Free](https://developers.cloudflare.com/email-service/platform/pricing/): only verified destination addresses of the account (fine for mail to yourself); any recipient needs Workers Paid (3,000 a month included, then $0.35 per 1,000) |

With `MAIL_ADAPTER` unset, `send` fails with an internal error that names the
fix, so a production Worker never drops mail silently. For signup and
password-reset mail on the free plan, use Resend. An invalid recipient
(`Validator::email`'s rule) is a 400; a missing `MAIL_FROM`, key or binding is
a 500 whose log says what to add.

`ocre g mailer User welcome password_reset` writes `src/mailers/user.rs` with
`welcome(to) -> Result<Email>` and `password_reset(to)`, rendering
`templates/mailers/user/<action>.txt` (not HTML-escaped) and `.html` with
askama; add fields to the template structs for the data an email needs.

Receiving uses [Email Routing](https://developers.cloudflare.com/email-service/local-development/routing/)
(free and unlimited on every plan): `ocre g mailbox` writes `src/mailbox.rs`
and this entry point in `src/lib.rs`:

```rust
#[worker::event(email)]
async fn email(message: worker::ForwardableEmailMessage, env: worker::Env, _ctx: worker::Context) -> worker::Result<()> {
    ocre::mail::receive(message, env, mailbox::receive).await
}
```

`mailbox::receive(ctx, email)` gets an `InboundEmail`: envelope `from()` and
`to()`, decoded `subject()`, `header(name)`, `headers()`, `text()` and
`html()` (parsed from multipart, quoted-printable, base64 and RFC 2047
headers; attachments are skipped), `raw()` bytes, plus `reject(reason)`
(bounce) and `forward(address).await` (to a verified destination address).
An `Err` from the handler is logged and bounces the email. In the dashboard,
Email Routing > Routing rules sends an address to the Worker. Locally, while
`ocre dev` runs, POST a raw message (it needs a `Message-ID` header):

```sh
curl 'http://localhost:8787/cdn-cgi/local/email?from=ada@example.com&to=support@example.com' \
  --data-binary $'From: ada@example.com\r\nTo: support@example.com\r\nSubject: Hi\r\nMessage-ID: <1@example.com>\r\n\r\nHello'
```

## Files

Uploads are stored in [R2](https://developers.cloudflare.com/r2/) and
described by four columns of the record that owns them, like Active Storage
without its extra tables. `ocre g scaffold Photo title:string image:attachment
notes:attachment?` generates:

| Piece | What it does |
|---|---|
| `image_key`, `image_filename`, `image_content_type`, `image_size` | Columns (NULL-able for `?`); `photo.image()` returns an `ocre::storage::Attachment` (`photo.notes()` an `Option`) |
| `pub const IMAGE: Rules` in the model | Largest file (10 MB) and allowed content types (PNG, JPEG, GIF, WebP, PDF, plain text); `validate()` checks each upload with `v.file(..)`, before anything is stored |
| `create` / `update` / `delete` | Store new files under `photos/image/<random>`, then write the row; files are deleted again if the write fails, replaced or removed files after it succeeds, and a deleted record's files with it |
| HTML forms | `enctype="multipart/form-data"`, a file input per attachment, "Remove notes" on the edit page for optional files; the form's request limit is the sum of its files' limits plus 1 MB (larger: 413) |
| `GET /photos/{id}/image` | Streams the file: `Content-Type`, `Content-Length`, `Content-Disposition` with the original name, `ETag` and 304, `Range` (206/416), `Cache-Control: private, no-cache` |
| `wrangler.toml` | `[[r2_buckets]] binding = "STORAGE"`, `bucket_name = "<app>-storage"`, added by the first generator that needs it |

`ocre g api Document name:string file:attachment?` adds `GET`, `PUT`
(multipart, `curl -X PUT -F file=@spec.pdf`) and `DELETE` on
`/api/documents/{id}/file`; JSON cannot carry a file, so attachments of a JSON
API must be optional. GraphQL exposes the four columns; files go through REST.

The framework API, `ocre::storage`: the `Multipart<LIMIT>` extractor
(`form.form::<T>()` for text fields, `form.file("image")` for an `Upload`),
`Validator::file`, `store` / `store_bytes` / `store_body` (a request body
streamed in with its `Content-Length`), `read`, `delete`,
`delete_attachments` and `serve(&ctx, &attachment, &headers, Disposition::Inline)`.

Choices, for the free plan:

- **Costs** ([R2 pricing](https://developers.cloudflare.com/r2/pricing/),
  September 2026, free every month): 10 GB-month stored, 1M class A
  operations (each upload is one), 10M class B (each download or 304 is one),
  deletes free, no egress fees. R2 has to be enabled once in the dashboard
  (Storage & databases > R2), which asks for a payment method even for the
  free tier; `ocre deploy` says so when the account lacks it (API code 10042).
- **CPU**: downloads never pass through WebAssembly. `storage::serve` hands
  R2's stream to `ocre::serve`, which answers with it directly (that is why
  the Worker's `fetch` returns `worker::web_sys::Response`); axum bodies are
  otherwise copied chunk by chunk into and out of WebAssembly and lose
  `Content-Length`. Uploads are read into memory and split with a
  substring search: 1.2 ms per 10 MB in WebAssembly (memchr, measured in V8),
  plus a copy to R2 (0.15 ms per 10 MB).
- **Memory and size**: a Worker has 128 MB, and Cloudflare refuses request
  bodies over 100 MB on the Free plan, so keep limits in the tens of MB; the
  whole request is in memory while it is stored. A Worker can instead stream
  a raw body of known length into R2 with `store_body`.
- **Safety**: keys are random (128 bits), never derived from file names.
  File names lose directories and control characters. Only types that cannot
  run scripts (raster images, PDF, plain text, audio, video) are shown
  inline; HTML, SVG, XML and JavaScript are sent as downloads of type
  `application/octet-stream`. The content type comes from the browser: the
  allowlist limits it, nothing sniffs file contents. Serving routes are as
  protected as the handler you put around them.
- **Local development**: `ocre dev` keeps objects in `.wrangler/state` (wrangler's R2 simulation).

Not included: presigned URLs and direct browser-to-R2 uploads (they need R2
S3 API credentials and SigV4 signing), public buckets and custom domains
(served by Cloudflare without the Worker; set up in the dashboard), image
resizing, and cleanup of files whose rows are removed by `ON DELETE CASCADE`.

## Background jobs and scheduled tasks

Jobs run on [Cloudflare Queues](https://developers.cloudflare.com/queues/),
which the [Workers Free plan includes since February 2026](https://developers.cloudflare.com/changelog/post/2026-02-04-queues-free-plan/).
The app's Worker is both the producer and the consumer of one queue,
`<app>-jobs`, bound as `JOBS`.

`ocre g job SendWelcome user_id:integer` writes `src/jobs/send_welcome.rs`
(a serde struct with the arguments and `async fn perform(self, ctx: &Ctx)`)
and adds it to `src/jobs/mod.rs`: a `Job` enum and a `perform` function that
matches on it, so dispatch is plain code, not a registry. The first job also
adds the queue to `wrangler.toml` and this entry point to `src/lib.rs`:

```rust
#[worker::event(queue)]
async fn queue(batch: worker::MessageBatch<String>, env: worker::Env, _ctx: worker::Context) -> worker::Result<()> {
    ocre::jobs::consume(batch, env, jobs::perform).await
}
```

Enqueue from a handler; the request returns as soon as Cloudflare stored the
message:

```rust
use crate::jobs::{Job, SendWelcome};

ocre::jobs::enqueue(&ctx, &Job::SendWelcome(SendWelcome { user_id: user.id })).await?;
ocre::jobs::enqueue_in(&ctx, &job, Duration::from_secs(3600)).await?; // 24 hours at most
```

A message is JSON text, `{"at": <due unix time>, "job": {"send_welcome": {"user_id": 1}}}`.
`consume` runs the messages of a batch one after the other: `Ok` acknowledges
the message (`[ocre jobs] send_welcome done` in the log); `Err` logs the error
and retries the message after twice the time since it was due, 30 s at least
(30 s, 1 min, 3 min, 9 min, 27 min). Queues' own `retry()` counter stops
after `max_retries = 5`, then moves the message to the dead-letter queue
`<app>-jobs-failed`, kept 24 hours for inspection in the dashboard. A message
that does not decode (not an Ocre message, or a job renamed or changed while
messages were queued) is logged as `[ocre jobs] dropped message ...` and
acknowledged, never retried. Jobs can run twice (at-least-once delivery):
write them to be safe to repeat.

`ocre::mail::deliver_later(&ctx, email).await?` is Rails' `deliver_later`:
it checks the email and the mail configuration like `send` (a bad address is
still a 400), enqueues it, and the consumer sends it with `send`, retrying
provider failures. It needs the queue that the first `ocre g job` wires.

`ocre g schedule nightly_cleanup "0 3 * * *"` writes
`src/schedules/nightly_cleanup.rs` (`async fn run(ctx: &Ctx)`), adds the
expression to `[triggers] crons` in `wrangler.toml` and a match arm to
`src/schedules/mod.rs`; the first schedule adds the `scheduled` entry point,
which calls `ocre::jobs::cron(event, env, schedules::run)`. Cron times are UTC
([syntax](https://developers.cloudflare.com/workers/configuration/cron-triggers/#supported-cron-expressions));
a failed run is logged (`[ocre cron] ... failed`) and not retried. Locally,
`wrangler dev` runs the queue in-process, and a cron fires on request:

```sh
curl 'http://localhost:8787/cdn-cgi/local/scheduled?cron=0+3+*+*+*'
```

`ocre deploy` runs `wrangler queues info` for every queue named in
`wrangler.toml` and `wrangler queues create` for the missing ones, before
deploying (a consumer of a missing queue fails the deploy); `--json` lists them
in `provisioned`.

Free-plan budget (September 2026):

| Limit | Value | What Ocre does |
|---|---|---|
| [Queues operations](https://developers.cloudflare.com/queues/platform/pricing/) | 10,000 a day; a message costs 3 (write, read, delete), each retry 1 more read, a dead-lettered message 1 more write | One message per job; about 3,300 jobs a day |
| [Retention](https://developers.cloudflare.com/queues/platform/limits/) | 24 hours on Free (not configurable) | Retries stop long before: the last one comes after about 40 minutes |
| [Message size](https://developers.cloudflare.com/queues/platform/limits/) | 128 KB | `enqueue` refuses larger jobs with an error naming the fix (pass ids) |
| [Delay](https://developers.cloudflare.com/queues/configuration/batching-retries/#delay-messages) | 24 hours, on send and on retry | `enqueue_in` refuses longer delays |
| [Batches](https://developers.cloudflare.com/queues/configuration/batching-retries/) | up to 100 messages, 60 s wait | `max_batch_size = 10`, `max_batch_timeout = 5`: one consumer run (one Worker request) per 10 jobs |
| [CPU](https://developers.cloudflare.com/workers/platform/limits/#cpu-time) | 10 ms per invocation on Free, for requests, cron runs and (like any Worker invocation) consumer batches | Jobs should be I/O (D1, mail, `fetch`); lower `max_batch_size` for CPU-heavy jobs |
| [Cron Triggers](https://developers.cloudflare.com/workers/platform/limits/) | 5 per account on Free | `ocre g schedule` warns past 5 in the app; run several tasks from one cron |

## Authentication

`ocre g auth` writes authentication into the app, like Rails 8's
authentication generator: every query and rule is app code an agent can read
and change, and the framework only provides small primitives.

| File | Full-stack | API-only | Contents |
|---|---|---|---|
| `migrations/*_create_users.sql` | yes | yes | `users`: `email` (unique, `COLLATE NOCASE`), `password_digest` |
| `migrations/*_create_auth_tokens.sql` | yes | | single-use emailed tokens (`purpose`, `digest`, `expires_at`) |
| `migrations/*_create_api_keys.sql` | yes | yes | `api_keys`: `user_id`, `name`, `digest` (unique), `last_used_at` |
| `src/models/user.rs` | yes | yes | `User`, `NewUser` (email format, password 8 to 128 characters), `create`, `authenticate`, `update_password`; emails trimmed and lowercased |
| `src/models/auth_token.rs` | yes | | `issue`, `peek`, `consume` (15 minutes, single use) |
| `src/models/api_key.rs` | yes | yes | `create` (returns the key once), `for_user`, `revoke`, `authenticate` |
| `src/auth.rs` | yes | | `CurrentUser` (redirects to `/login`, then back), `OptionalUser`, `sign_in`, `sign_out` |
| `src/registrations.rs` | yes | | `GET/POST /signup`, `GET /account` (an example protected page) |
| `src/sessions.rs` | yes | | `GET/POST /login`, `POST /logout`, `GET/POST /magic_link`, `GET/POST /magic_link/{token}` |
| `src/passwords.rs` | yes | | `GET /passwords/new`, `POST /passwords`, `GET/POST /passwords/{token}` |
| `templates/auth/*.html` | yes | | the pages |
| `src/auth_api.rs` | yes | yes | `BearerUser`; `POST /api/auth/signup`, `POST /api/auth/token` (JWT, 1 hour), `GET /api/auth/me`, `GET/POST /api/auth/keys`, `DELETE /api/auth/keys/{id}` |

```rust
use crate::auth::CurrentUser;        // HTML: visitors are redirected to /login
use crate::auth_api::BearerUser;     // JSON: 401 without a valid JWT or API key

async fn dashboard(CurrentUser(user): CurrentUser) -> ocre::Result<Html<String>> { ... }
async fn my_posts(BearerUser(user): BearerUser, State(ctx): State<Ctx>) -> ApiResult<Json<Vec<Post>>> { ... }
```

Framework primitives: `ocre::password::{hash, verify}`,
`ocre::token::{generate, digest, constant_time_eq}`,
`ocre::jwt::{encode, decode, Claims}`, `ocre::now()` and
`Error::Unauthorized` (401, with `WWW-Authenticate: Bearer` in JSON) /
`Error::Forbidden` (403).

Security choices:

- **Passwords**: PBKDF2-HMAC-SHA256 through WebCrypto
  (`crypto.subtle.deriveBits`), which runs natively in workerd instead of in
  WebAssembly; bcrypt or argon2 compiled to WebAssembly would not fit in 10 ms.
  100,000 iterations, the most Workers accept (OWASP recommends 600,000 for
  this algorithm; the cap is the platform's). Digests are self-describing,
  `pbkdf2_sha256$100000$<salt>$<hash>` (16-byte random salt, base64), so the
  count can grow later; `ocre::password::iterations(digest)` reads it back.
  Native builds compute the same function in pure Rust (`pbkdf2` crate) for
  tests. Comparison is constant-time; passwords are never logged. A login
  with an unknown email runs a hash too, so timing does not reveal accounts.
- **Measured cost of one hash**: 5.5 ms (100 hashes in 550 ms, timed with
  `Date.now()` around `crypto.subtle.deriveBits` in `wrangler dev`, workerd on
  an Apple M5 Max). A login request takes 9.5 ms in `wrangler dev` against 3.5
  ms for `GET /up`. Sign-up, login and password changes therefore use about
  half of the free plan's 10 ms CPU budget, and only those requests hash.
- **Sessions**: the encrypted cookie holds only `user_id`. Login empties the
  session before storing the id (no state carries over; with a cookie store
  there is no server-side session id to fixate), logout clears it.
  `CurrentUser` remembers the page (GET only, local paths only: no open
  redirect) and returns there after login.
- **Emailed tokens** (password reset, magic link): 256 random bits,
  URL-safe; the database keeps only their SHA-256 digest; valid 15 minutes;
  consumed with one `DELETE ... RETURNING` so they work once, even under
  concurrent use; a new request cancels the previous link. The link opens a
  page with a button that POSTs, so mail scanners that follow links do not use
  it up. The request forms answer the same whether or not the email has an
  account. Links use the request's origin; Cloudflare routes by host name, so
  it is always one of the app's own hosts. Mail goes through `ocre::mail`: in
  `ocre dev` the link appears in the console; in production set
  `MAIL_ADAPTER` (see [Email](#email)) or these forms answer 500.
- **JWT**: HS256 only (a token naming `none` or any other `alg` is refused),
  `sub` = user id, `iat`, `exp`. The key is derived from `SECRET_KEY_BASE`
  (HMAC-SHA256 with a fixed label, so it differs from the cookie key) rather
  than a separate `JWT_SECRET`: one secret to create, upload and rotate, and
  rotating it signs everyone out of sessions and tokens at once. JWTs cannot
  be revoked before they expire (1 hour); use API keys for long-lived access.
- **API keys**: 256 random bits shown once; stored as SHA-256 digests (a fast
  hash is enough for random secrets and costs no CPU), revocable,
  `last_used_at` written at most once an hour to save D1 writes.

Not included yet: **rate limiting** (login, sign-up, token and email routes
accept unlimited attempts; put [Cloudflare rate limiting
rules](https://developers.cloudflare.com/waf/rate-limiting-rules/) in front of
them before going public), email confirmation, "sign out everywhere" (sessions
last until logout or until `SECRET_KEY_BASE` changes) and roles
(`Error::Forbidden` is there for app checks).

## Realtime

Live pages like Rails' Action Cable and Turbo Streams, on the free plan:
`ocre g scaffold Post title:string --realtime` makes `/posts` show the posts
other visitors create, edit and delete, without custom JavaScript.

```
browser ──WebSocket──> GET /realtime/posts ──> src/realtime.rs connect (who may listen)
                                                   └─> OcreChannel "posts" (Durable Object, holds the sockets)
POST /posts ──> create ──> ocre::realtime::broadcast(&ctx, "posts", html) ──> every socket
```

- **Server**: `ocre::realtime::broadcast(&ctx, channel, message)` sends a
  message (HTML or JSON text) to every browser on `channel`, from any handler
  or job. The helpers `prepend(target_id, html)`, `append`, `update` (inner
  HTML) and `remove(id)` build htmx out-of-band swaps; an element with an `id`
  on its own replaces the page element with that id. Several can go in one
  message.
- **Client**: htmx's [WebSocket extension](https://htmx.org/extensions/ws/)
  connects (`<div hx-ext="ws" ws-connect="/realtime/posts">`), reconnects
  with backoff, and swaps each message in by id
  ([`hx-swap-oob`](https://htmx.org/attributes/hx-swap-oob/)).
- **Authorization**: `src/realtime.rs` in the app routes `GET
  /realtime/{channel}` to `connect`, which lists the channels anyone may open
  (unknown ones are 404) before `WebSocketUpgrade::connect`. Add checks there
  (`CurrentUser` works: browsers send the session cookie with the handshake),
  or use a channel per record or user (`post:12`). Handshakes from other
  sites are refused (403), like forms.
- **What `--realtime` generates**: `templates/<plural>/_row.html` (one row
  with `id="post_12"`, shared by the index and broadcasts), the index wrapped
  in `ws-connect`, a broadcast after create (`prepend`), update (the row) and
  delete (`remove`) in the controller, and on first use Ocre's `realtime`
  feature in `Cargo.toml`, `src/realtime.rs` and this in `wrangler.toml`:

```toml
[[durable_objects.bindings]]
name = "CHANNELS"
class_name = "OcreChannel"

[[migrations]]
tag = "ocre-realtime-v1"
new_sqlite_classes = ["OcreChannel"]
```

`ocre deploy` needs no extra step: `wrangler deploy` creates the Durable
Object namespace from the migration. `ocre dev` runs it locally (workerd
supports Durable Objects and WebSocket Hibernation).

How it runs, and why it fits the free plan (limits of September 2026):
one Durable Object per channel name (class `OcreChannel`, shipped by Ocre)
accepts the sockets with the [WebSocket Hibernation
API](https://developers.cloudflare.com/durable-objects/best-practices/websockets/),
so between broadcasts it is evicted from memory while browsers stay
connected. It stores nothing.

| Resource | Free plan | Realtime use |
|---|---|---|
| [Durable Object requests](https://developers.cloudflare.com/durable-objects/platform/pricing/) | 100,000 a day | 1 per connection (and reconnection), 1 per broadcast; incoming messages count 1/20 (subscribers send none); messages to browsers are free |
| Duration | 13,000 GB-s a day (128 MB objects: about 28 hours awake) | only while handling a connection or broadcast, a few milliseconds; hibernated sockets cost nothing |
| Worker requests | 100,000 a day | 1 per connection; broadcasts are subrequests of the request that sends them |
| [Durable Object limits](https://developers.cloudflare.com/durable-objects/platform/limits/) | SQLite-backed classes only; 32,768 WebSockets per object | `new_sqlite_classes`; one object per channel |

Broadcasts wait for the channel object (one subrequest, little CPU). The
generated controller treats them as best effort: a failure is logged
(`[ocre realtime] broadcast to posts failed: ...`) and the request still
succeeds. Clients only listen; what they send is ignored. API-only apps can
use the same pieces by hand: turn on the `realtime` feature, add the
`wrangler.toml` entries above and a `connect` route, and broadcast JSON.

## Caching

Two tools, both opt-in, chosen for the free plan:

| | KV values (`ocre::cache::fetch`) | HTTP (`CacheControl`, `ETag`, `Conditional`) |
|---|---|---|
| Saves | Slow or costly work: D1 aggregates, third-party APIs | Rendering and bandwidth: `304 Not Modified` |
| Where | Workers KV, global, eventually consistent (up to 60 s) | The browser (and Cloudflare with Workers Cache, below) |
| Free plan (September 2026) | 100,000 reads and **1,000 writes** a day, 1 GB ([limits](https://developers.cloudflare.com/kv/platform/limits/)) | free |

```rust
use std::time::Duration;

// One KV read per call; a miss runs the closure and costs one KV write.
let stats: Stats = ocre::cache::fetch(&ctx, "stats:v1", Duration::from_secs(3600), || async {
    Stats::compute(&ctx).await
})
.await?;
ocre::cache::delete(&ctx, "stats:v1").await?; // after a change; also a write
```

`ocre g cache` adds `[[kv_namespaces]] binding = "CACHE"` to
`wrangler.toml`; `ocre dev` uses a local namespace and `ocre deploy` creates
`<app>-cache` (or links an existing one with that title) and writes its `id`
into `wrangler.toml`. The binding is not in `ocre new` apps because KV writes
are the scarcest free resource: a key refreshed every `ttl` seconds costs up to
`86,400 / ttl` writes a day (a one-hour TTL is 24 writes per key, so about 40
hot keys fit), and KV accepts TTLs of 60 seconds or more. Values are JSON; put a
version in the key (`stats:v1`) and change it when the type changes (an
undecodable value is logged and recomputed). `fetch` never fails because of
KV: past a daily limit it logs `[ocre cache] ... failed` and computes the
value. `read`, `write` and `delete` are the explicit forms.

For pages, `Conditional` answers `304` without rendering when the browser
already has the current version, like Rails' `fresh_when`:

```rust
async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>, i18n: I18n, conditional: Conditional) -> Result<Response> {
    let post = post::find(&ctx, id).await?.or_404()?;
    let etag = ETag::of(&(&post, i18n.locale()))?;   // everything the page shows
    conditional.fresh_when(etag, CacheControl::no_cache(), || render(&ShowView { post, i18n }))
}
```

The database query still runs; the template does not. Pages that show a
flash message or the signed-in user must put them in the `ETag` too.

**Serving pages without running the Worker.** Two Cloudflare caches can do
this, and Ocre wraps neither:

- The [Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/)
  (`caches.default`) only works on custom domains: on `*.workers.dev`, where
  Ocre apps deploy by default, `put` does nothing. It is also local to one data
  center and the Worker still runs for every request.
- [Workers Cache](https://developers.cloudflare.com/workers/cache/)
  (`[cache] enabled = true` in `wrangler.toml`, Wrangler 4.69+) works on
  `workers.dev` too and serves `CacheControl::public(..)` responses from
  Cloudflare's tiered cache: hits use no CPU. But on the free plan every hit
  still counts toward the 100,000 requests a day, and turning it on also counts
  static asset requests (`public/`), which are otherwise free
  ([pricing](https://developers.cloudflare.com/workers/cache/#pricing)). Its
  cache key ignores cookies and `Accept-Language`, so only mark responses
  `public` when they are the same for every visitor (locale in the path,
  nothing from the session); responses with `Set-Cookie` are never stored.

## Translations

Rails-style translations, compiled into the Worker:

```yaml
# locales/fr.yml (a YAML subset: nested keys and strings)
fr:
  posts:
    created: "Article créé."
    greeting: "Bonjour %{name} !"
    count:
      one: "%{count} article"     # CLDR categories: zero, one, two, few, many, other
      other: "%{count} articles"
```

```rust
// src/lib.rs, written by `ocre g locale en fr`: the first code is the default.
static LOCALES: ocre::i18n::Locales = ocre::locales!("en", "fr");
// and at the end of routes(): .layer(ocre::i18n::layer(&LOCALES))

async fn index(i18n: I18n, session: Session) -> Result<Html<String>> {
    session.flash("notice", i18n.t("posts.created").to_string())?;
    render(&IndexView { i18n, count: 3 })
}
```

```html
<html lang="{{ i18n.locale() }}">
<p>{{ i18n.t("posts.greeting").arg("name", user.name) }}</p>
<p>{{ i18n.t("posts.count").count(count) }}</p>
```

- **Cost**: `include_str!` puts the files in the binary; the first
  translation in a Worker instance parses them once, with a small parser
  written for this subset (the `toml` crate alone, parsing into a table,
  compiled to 205 KB of release WebAssembly; the app binary is about 420 KB).
  Lookups are a `BTreeMap` search; `t(..)` is written straight into the
  askama output without an intermediate `String`.
- **Locale per request**: the `I18n` extractor takes a `{locale}` path
  segment (nest routes with `.nest("/{locale}", pages())`; an unknown code is
  a 404), else the `locale` cookie (`i18n.cookie()` gives the `Set-Cookie`
  value), else `Accept-Language` (quality order; `fr-CH` matches `fr`), else
  the default locale. Outside requests (mailers, jobs):
  `LOCALES.locale(&user.locale)`.
- **Plurals**: `.count(n)` picks the CLDR form for the locale (English,
  German, Spanish, Italian, Dutch...: one/other; French, Portuguese, Hindi:
  0 and 1 are `one`; Russian, Ukrainian, Polish: one/few/many; Czech, Slovak:
  one/few; Arabic; Hebrew; Japanese, Chinese, Korean: other only) and sets
  `%{count}`; a `zero` key wins for 0.
- **Missing keys**: release builds (`ocre deploy`) fall back to the default
  locale; debug builds (`ocre dev`) show `translation missing: fr.posts.created`
  so gaps are visible. `ocre i18n missing` lists every key absent from each
  locale and fails if there is any; `ocre dev` and `ocre deploy` refuse files
  the Worker could not parse (with the line and the fix).
- **Escaping**: askama escapes translations and interpolated values; never
  mark them `|safe`.
- Generated scaffolds, auth pages and mailers still contain English strings.
  To translate one, move each string to `locales/en.yml`, add `i18n: I18n` to
  the handler and its template struct, and replace the text with
  `{{ i18n.t("posts.index.title") }}` (flash: `i18n.t(..).to_string()`).

## Rules the framework enforces

- Handlers are plain [axum](https://docs.rs/axum) handlers. Every Ocre type is
  `Send`, so `#[worker::send]` is never needed.
- One way to do each thing: SQL with `?N` placeholders and `params![...]`,
  askama templates compiled at build time, htmx for interactivity.
- Errors name the fix (for example a missing `DB` binding says which
  `wrangler.toml` entry to add). Internal errors are logged, never shown to users.

## Layout

```
crates/ocre/                 framework crate (features: html [default], graphql, realtime)
  src/                       code; src/runtime/ calls the Workers JavaScript runtime
  tests/                     unit tests, mirroring src/: tests/session.rs tests src/session.rs
crates/ocre-cli/             `ocre` command-line tool
  src/                       commands and generators
  templates/                 files `ocre new` and the generators write
  tests/                     unit tests mirroring src/ (tests/generate/model.rs, ...)
  tests/integration/         the `ocre` binary against a fake wrangler, and the terminal wizard
  tests/system/              generated apps running on workerd (`wrangler dev`)
  tests/support/             shared test helpers and the fake wrangler script
```

Unit tests sit in `tests/` at the same path as the code they test, like
RSpec's `spec/` or minitest's `test/`. Each source file includes its test file
with `#[cfg(test)] #[path = "../tests/session.rs"] mod tests;`, so the tests
can reach private items while living apart from the code. Both crates set
`autotests = false`; the integration and system suites are declared as
`[[test]]` targets in `crates/ocre-cli/Cargo.toml`.

## API

| Item | Use |
|---|---|
| `ocre::serve(routes(), req, env)` | Worker entry point (`#[worker::event(fetch)]`, returns `worker::Result<worker::web_sys::Response>`) |
| `Multipart(form): Multipart<LIMIT>` | `multipart/form-data` body up to `LIMIT` bytes; `form.form::<T>()`, `form.file(name)` |
| `ocre::storage::{store, serve, delete_attachments}` | Files in R2 (binding `STORAGE`), see [Files](#files) |
| `State(ctx): State<Ctx>` | Per-request context |
| `ctx.db()?` | D1 database bound as `DB` |
| `db.all::<T>(sql, params![..])` | All rows as `Vec<T>` |
| `db.first::<T>(sql, params![..])` | First row as `Option<T>`; use with `INSERT ... RETURNING *` |
| `db.execute(sql, params![..])` | Rows changed |
| `render(&template)` | askama template to `Html<String>` (feature `html`) |
| `Htmx(is_htmx)` | Extractor: true when `HX-Request: true` (feature `html`) |
| `option.or_404()?` | Missing record to 404 |
| `Error::bad_request(msg)` / `Error::internal(msg)` | 400 / 500; HTML page, or JSON through `ApiError` |
| `ApiResult<T>`, `ApiError` | JSON error responses; `?` converts from `Error` |
| `Json(value)`, `Created(value)` | JSON body extractor (invalid JSON is a JSON 400) and responses |
| `Page { limit, offset }` | `?limit=&offset=` extractor with bounds; `Page::new` for GraphQL |
| `ocre::graphql::routes(schema)` | `/graphql` endpoint and GraphiQL (feature `graphql`) |
| `#[serde(deserialize_with = "ocre::bool_from_sql")]` | Read SQLite INTEGER 0/1 as `bool` |
| `session: Session` | Encrypted cookie session: `get::<T>`, `insert`, `remove`, `clear`, `flash(kind, msg)` |
| `flash: Flash` | Previous request's flash: `notice()`, `alert()`, `get(kind)`, `iter()` |
| `Validator`, `FieldError` | Collect field errors; `finish()?` returns `Error::Invalid` (422) |
| `ocre::password::hash(pw).await?` / `verify(pw, digest).await?` | PBKDF2-HMAC-SHA256 digest (WebCrypto on Workers) / constant-time check |
| `ocre::token::generate()` / `digest(token)` | 256-bit URL-safe random token / SHA-256 hex to store |
| `ocre::jwt::encode(&ctx, &Claims::new(sub, ttl))?` / `decode(&ctx, token)?` | HS256 JWT signed with a key derived from `SECRET_KEY_BASE`; `decode` fails with 401 |
| `ocre::now()` | Unix seconds, on Workers and natively |
| `Error::Unauthorized` / `Error::Forbidden` | 401 / 403 |
| `ocre::mail::send(&ctx, email)` | Send an `Email` (`Email::new(to, subject, text).html(..).reply_to(..)`) with the `MAIL_ADAPTER` adapter |
| `ocre::mail::receive(message, env, handler)` | Worker `email` entry point; `handler(ctx, InboundEmail)` |
| `ocre::mail::deliver_later(&ctx, email)` | Check now, send from the jobs queue (retried on failure) |
| `ocre::jobs::enqueue(&ctx, &job)` / `enqueue_in(&ctx, &job, delay)` | Send a serde job to the `JOBS` queue (delay up to 24 h) |
| `ocre::jobs::consume(batch, env, perform)` | Worker `queue` entry point: `perform(ctx, job)`, ack on `Ok`, retry with backoff on `Err`, drop undecodable messages |
| `ocre::jobs::cron(event, env, run)` | Worker `scheduled` entry point: `run(ctx, cron)` |
| `ocre::realtime::broadcast(&ctx, channel, message)` | Send HTML (or JSON text) to every WebSocket on `channel` (feature `realtime`) |
| `realtime::prepend(target, html)` / `append` / `update` / `remove(id)` | htmx out-of-band swaps for broadcasts |
| `upgrade: WebSocketUpgrade` then `upgrade.connect(&ctx, channel)` | Extractor for WebSocket handshakes (400 otherwise); connects to the channel's `OcreChannel` Durable Object |
| `ocre::cache::fetch(&ctx, key, ttl, \|\| async { .. })` | Read-through cache of a JSON value in the `CACHE` KV namespace; also `read`, `write`, `delete` |
| `CacheControl::no_cache()` / `private(ttl)` / `public(ttl)` / `no_store()` | `Cache-Control` response part |
| `ETag::new(version)` / `ETag::of(&data)?` | Weak `ETag` response part |
| `conditional: Conditional` then `conditional.fresh_when(etag, cache_control, \|\| render(..))` | `304 Not Modified` without rendering when the client's copy is current |
| `static LOCALES: ocre::i18n::Locales = ocre::locales!("en", "fr")` | Translations from `locales/*.yml`; `.layer(ocre::i18n::layer(&LOCALES))` in `routes()` |
| `i18n: I18n` then `i18n.t(key).arg(name, value).count(n)` | Extractor: the request's locale; translation with `%{name}` values and plural forms |

## Requirements

- Rust via **rustup** with the `wasm32-unknown-unknown` target
  (`rust-toolchain.toml` installs it). A Homebrew `rust` without rustup has no
  wasm target and the build fails.
- Node.js (for `npx wrangler`).

## Try it

```sh
cargo install --path crates/ocre-cli
ocre new blog --starter blog --ocre-path "$PWD/crates/ocre" --yes
cd blog && ocre dev
```

## Tests

```sh
cargo test --workspace --all-features             # unit, CLI and terminal tests: ~5 s
cargo test -p ocre-cli --test e2e -- --ignored   # real `wrangler dev`: ~20 s warm
cargo llvm-cov --workspace --all-features \
  --ignore-filename-regex 'crates/ocre/src/runtime/' --fail-under-lines 100
```

| Suite | What it runs |
|---|---|
| Unit (`crates/*/tests/**`, mirroring `src/`) | Pure logic: params, errors, sessions, crypto formats, MIME, extractors, generators |
| `crates/ocre-cli/tests/integration/` | The `ocre` binary with a fake wrangler (`tests/support/fake_npx.sh`): every command, `--json` contract, every error hint; `ocre new` in a pseudo-terminal |
| `crates/ocre-cli/tests/system/e2e.rs` | Generated apps built to WebAssembly (dev build, shared `target/e2e-app`), served by `wrangler dev`: CRUD, sessions, CSRF, auth, email over HTTP, realtime broadcasts to WebSocket clients, background jobs and cron runs, translations by `Accept-Language`/cookie/path, KV read-through cache, 304 responses |

CI (manual trigger for now) runs lint, coverage and a generated-app build as parallel jobs, and requires
100% line coverage. The generated-app job runs `ocre new` and every generator,
then compiles the result to WebAssembly.
The e2e test is not part of CI (it needs Node.js and a full WebAssembly
build); run it locally before merging changes to the runtime or generators.
`crates/ocre/src/runtime/` calls the Workers JavaScript runtime and only runs
inside workerd, where it cannot be instrumented; it is excluded from the
measurement and exercised by the e2e test.

## Measured

Blog starter app, release build: `index_bg.wasm` 420 KB (130 KB gzipped),
`index.js` 24 KB (Workers limit: 64 MiB). Worker startup time: 4 ms. Measured
before sessions, auth and email were added.

Production, free plan (`wrangler tail`, 55 requests):

| Route | CPU median | CPU max | Wall median |
|---|---|---|---|
| `GET /` (list, 1 D1 query) | 2 ms | 23 ms (1 of 40, likely a new isolate) | 18 ms |
| `GET /posts/:id` | 2 ms | 4 ms | 16.5 ms |
| `POST /posts` (insert) | 3 ms | 6 ms | 28 ms |

Password hashing (PBKDF2-HMAC-SHA256, 100,000 iterations, WebCrypto in
workerd under `wrangler dev`, Apple M5 Max): 5.5 ms per hash.

The free-plan limit is 10 ms CPU per request; Cloudflare tolerates infrequent
overruns per isolate, and kills requests only when overruns become frequent.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
