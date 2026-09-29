# __APP_NAME__

Ocre app: Rust compiled to WebAssembly, running on Cloudflare Workers (free
plan) with a D1 (SQLite) database. Full-stack apps render HTML with askama and
htmx; API-only apps (`mode = "api"` in Cargo.toml) serve JSON only.

## Documentation

Ocre's documentation is written for agents: fetch
`__DOCS_URL__/llms.txt` (every page with a one-line description) and then
the pages you need as Markdown by adding `.md` to their URL, for example
`__DOCS_URL__/guides/models.md`, `__DOCS_URL__/reference/generators.md`,
`__DOCS_URL__/reference/limits.md`. `__DOCS_URL__/llms-full.txt` is every
page in one file; `__DOCS_URL__/api-index.md` lists every public item of the
`ocre` crate. Pages are self-contained and their examples compile. This file
wins when they disagree about this app.

## Commands

Run from the app root. Add `--json` to any `ocre` command for one JSON object
on stdout (`"ok": true|false`, plus `error` and `hint` on failure).

| Task | Command |
|---|---|
| Model only: table, validations, queries, associations | `ocre g model Author name:string^ bio:text?` |
| CRUD resource (HTML pages; JSON in API-only apps) | `ocre g scaffold Post title:string body:text published:boolean author:references` |
| Same, index page updated live in every open browser (WebSockets) | `ocre g scaffold Post title:string --realtime` |
| Resource with an uploaded file (R2; adds the `STORAGE` binding) | `ocre g scaffold Photo title:string image:attachment notes:attachment?` |
| JSON REST resource, `/api/posts` | `ocre g api Post title:string body:text` |
| Same, also on `/graphql` (costs CPU, see below) | `ocre g api Post title:string --graphql` |
| Add columns (SQL inferred from the name) | `ocre g migration add_slug_to_posts slug:string?` |
| Remove a column | `ocre g migration remove_slug_from_posts` |
| Empty migration (data changes, custom SQL) | `ocre g migration backfill_slugs` |
| Authentication (users, login, magic link, password reset, JWT, API keys; once) | `ocre g auth` |
| Emails to send (one function per email) | `ocre g mailer User welcome password_reset` |
| Receive email (Email Routing) | `ocre g mailbox` |
| Background job (Cloudflare Queues) | `ocre g job SendWelcome user_id:integer` |
| Scheduled task (Cron Trigger, UTC) | `ocre g schedule nightly_cleanup "0 3 * * *"` |
| Run a scheduled task now (`ocre dev` running) | `curl 'http://localhost:8787/cdn-cgi/local/scheduled?cron=0+3+*+*+*'` |
| Cache values in Workers KV (adds the `CACHE` binding) | `ocre g cache` |
| Translations: set up (first code = default), add a locale | `ocre g locale en fr`, then `ocre g locale de` |
| Keys missing from a locale (fails if any) | `ocre i18n missing` |
| Apply migrations locally | `ocre migrate` |
| Pending migrations | `ocre migrate --status` |
| Load seed data (`db/seeds.sql`) | `ocre db seed` |
| Recreate local database (migrations + seeds) | `ocre db reset` |
| Query the database (JSON rows with `--json`) | `ocre sql "SELECT * FROM posts LIMIT 5"` |
| Run locally (http://localhost:8787) | `ocre dev` |
| List routes (method, path, handler) | `ocre routes` (or `ocre routes posts`) |
| New secret value | `ocre secret` |
| Cloudflare login (browser; once) | `ocre login` |
| Deploy + remote migrations | `ocre deploy` |
| Type-check | `cargo check --target wasm32-unknown-unknown` |

Field types: `string`, `text`, `integer`, `float`, `boolean`, `date`
(`YYYY-MM-DD`), `datetime`, `references` (`author:references` adds
`author_id`, a foreign key deleted with its parent), `attachment` (a file in
R2 stored as `<name>_key`, `_filename`, `_content_type`, `_size` columns; must
be optional `?` in JSON APIs), `json` (any JSON value as
`ocre::serde_json::Value`, stored as JSON text; build values with
`ocre::serde_json::json!`; cannot be unique `^`). Suffix `?` makes a field
optional (NULL allowed), `^` unique. Integers must stay within
±`ocre::MAX_SAFE_INTEGER` (2^53 - 1): D1 returns numbers as JavaScript
numbers; generated validations already reject larger values.

## Layout

```
src/lib.rs          entry point and router; keep the `// ocre:` marker comments
src/models/<model>.rs  the model: struct, New<Model>/<Model>Changes, validate(),
                    all/count/find/find_many/create/update/delete, associations
src/<plural>.rs     HTML resource: form parsing, handlers, routes (calls the model)
src/<plural>_api.rs JSON resource: REST handlers and GraphQL resolvers (call the model)
src/graphql.rs      GraphQL schema (when used); keep the `// ocre:graphql-*` markers
src/auth.rs         after `ocre g auth`: CurrentUser/OptionalUser extractors, sign_in/sign_out (HTML apps)
src/auth_api.rs     after `ocre g auth`: BearerUser extractor, /api/auth/* (token, me, keys)
src/mailers/<name>.rs  functions building `ocre::mail::Email`; templates in templates/mailers/<name>/<action>.{txt,html}
src/mailbox.rs      incoming email handler, called by the `email` event in src/lib.rs
src/jobs/<name>.rs  a job: serde struct of arguments + `perform(self, ctx)`
src/jobs/mod.rs     `Job` enum and `perform` match (dispatch); keep the `// ocre:job*` markers
src/schedules/<name>.rs  a scheduled task: `run(ctx)`
src/schedules/mod.rs  `run` matches the cron expression to a task; keep the `// ocre:schedule*` markers
src/realtime.rs     after `--realtime`: `GET /realtime/{channel}` and which channels may be opened; keep `// ocre:channels`
templates/<plural>/_row.html  after `--realtime`: one row with `id="<singular>_<id>"`, used by the index and broadcasts
templates/          askama templates, compiled into the binary
locales/<code>.yml  translations (after `ocre g locale`), declared in `ocre::locales!(..)` in src/lib.rs
public/             static files (CSS, images, robots.txt), served by Cloudflare before the Worker runs
migrations/         numbered D1 SQL migrations, applied in order
wrangler.toml       Cloudflare config; the D1 binding must be named DB; MAIL_FROM under [vars]
                    the JOBS queue ([[queues.*]]) and [triggers] crons are added by the generators;
                    [[kv_namespaces]] CACHE by `ocre g cache` (`ocre deploy` writes its id: commit it)
.dev.vars           local secrets and overrides for `ocre dev` (SECRET_KEY_BASE, MAIL_ADAPTER=log); never commit it
```

## Rules

- Handlers are plain axum handlers taking `State(ctx): State<Ctx>`. Do not add
  `#[worker::send]`; Ocre types are already `Send`.
- SQL goes through `ctx.db()?` with `?1, ?2` placeholders and `params![...]`.
  Never build SQL with `format!` from user input.
- Read rows with `db.all::<T>`, `db.first::<T>` (`INSERT ... RETURNING *` to get
  the new row), write with `db.execute` (returns rows changed).
- Booleans are INTEGER 0/1 in SQLite: read them with
  `#[serde(deserialize_with = "ocre::bool_from_sql")]`. JSON columns hold
  JSON text: bind a `serde_json::Value` with `params![value]` and read it
  with `deserialize_with = "ocre::json_from_sql"` (`optional_json_from_sql`
  for `Option`); HTML forms parse the text with `v.json(field, &text)`.
- Missing record: `.or_404()?`. Bad input: `Error::bad_request("...")`.
  Unexpected failure: `Error::internal("...")` (logged, not shown to users).
- Validation: collect every problem with `ocre::Validator` (`required`,
  `max_length`, `range`, `email`, `inclusion`, `date`, `check`, ...) and end
  with `v.finish()?`, which returns `Error::Invalid` (422). JSON answers
  `{"error": {"fields": {"title": ["can't be blank"]}}}`; generated HTML forms
  re-render with the messages and the typed values.
- Data rules live in the model (`src/models/<model>.rs`): `validate()` for
  checks without the database, `create`/`update` for uniqueness and foreign
  keys. Handlers and GraphQL resolvers only call model functions. After a
  migration that changes columns, update the model struct, `New<Model>`,
  `<Model>Changes`, `validate()` and the SQL in `create`/`update`.
- Load associations for many rows with `find_many(ctx, &ids)` (one query),
  never `find` in a loop.
- Templates escape `{{ value }}` by default; never mark user input `|safe`.
- Change the schema only with a new migration file; never edit an applied one.
- New module: generators register it in `src/lib.rs`. By hand, add
  `mod name;` under `// ocre:modules` and `.merge(name::routes())` under
  `// ocre:routes`. Keep `// ocre:models` and `// ocre:associations` too.
- JSON handlers return `ApiResult<T>` (errors become `{"error": {"status",
  "message"}}`), take bodies with `ocre::Json<T>` and lists with `Page`.
  Optional fields use `#[serde(default, deserialize_with = "ocre::optional")]`
  (empty or `null` = `None`) and, in `<Model>Changes`,
  `deserialize_with = "ocre::patch"` (missing keeps the value, `null` clears it);
  optional `json` fields use `#[serde(default)]` and `ocre::patch_json`, so a
  JSON string stays a string.
- Sessions: take `session: ocre::Session` in a handler; `session.insert("user_id", id)?`,
  `session.get::<i64>("user_id")?`, `session.remove(..)?`, `session.clear()?`. The
  session is an encrypted cookie (4 KB max): store ids, never records or secrets.
- Flash: `session.flash("notice", "Post was successfully created.")?` before a
  redirect; the next page takes `flash: ocre::Flash` and its template shows
  `flash.notice()` / `flash.alert()`.
- CSRF protection is automatic: browsers' cross-site POST/PUT/PATCH/DELETE get
  403. Forms need no token. Another site (a separate frontend) that must call
  the app: list its origin in `ALLOWED_ORIGINS` under `[vars]` in
  wrangler.toml (comma-separated); that also enables CORS for it.
- Security headers (nosniff, SAMEORIGIN framing, referrer policy, HSTS on
  HTTPS) are added to every response; a handler that sets one keeps its value.
- `GET /up` is the health check; keep it cheap (no database).
- Email: build it in a mailer (`src/mailers/`), send it from the handler with
  `ocre::mail::send(&ctx, mailers::user::welcome(&address)?).await?`. Validate
  user-typed addresses first with `v.email(..)`. `MAIL_ADAPTER` picks the
  delivery: `log` (`ocre dev`: the email is printed in the dev output between
  `[ocre mail]` lines, links included; read them there), `resend` (secret
  `RESEND_API_KEY`; free plan: any recipient, 100/day) or `cloudflare`
  (`[[send_email]]` binding `EMAIL`; free plan: only verified addresses of the
  account). Unset in production = `send` fails with an error naming the fix.
  Put data in the template structs; `.txt` templates are not HTML-escaped.
- Background jobs: anything slow, retryable or not needed for the response
  (emails, calls to other APIs, bulk updates) goes in a job. Enqueue with
  `ocre::jobs::enqueue(&ctx, &Job::SendWelcome(SendWelcome { user_id })).await?`
  (`use crate::jobs::{Job, SendWelcome};`), or `enqueue_in(&ctx, &job,
  Duration::from_secs(n))` (24 h max). Job fields are the arguments: pass ids
  and small values (128 KB max), load records in `perform`. `perform` returns
  `Err` to retry (30 s, 1 min, 3 min, 9 min, 27 min, then the
  `<app>-jobs-failed` dead-letter queue); jobs may run twice, so make them
  safe to repeat. Never rename a `Job` variant or change its fields while
  messages may be queued: they would be dropped (`[ocre jobs] dropped message`).
  In `ocre dev` jobs run within ~5 s; read the `[ocre jobs]` lines.
- Email from a request: prefer `ocre::mail::deliver_later(&ctx, email).await?`
  (sent by the jobs queue, retried) once the app has a job; `send` otherwise.
- Scheduled tasks: `ocre g schedule`; the task `run(ctx)` must stay within
  10 ms CPU: query ids, then enqueue one job per item. Crons are UTC; a failed
  run is logged (`[ocre cron]`), not retried.
- Incoming email: `src/mailbox.rs` `receive(ctx, email)`; `email.to()`,
  `subject()`, `text()`, `header(..)`; `email.reject("reason")` bounces,
  `email.forward("verified@address").await?` forwards; an `Err` bounces.
  Test locally with `curl 'http://localhost:8787/cdn-cgi/local/email?from=a@example.com&to=b@example.com' --data-binary @message.eml`
  (the message needs a `Message-ID` header).

- Authentication (after `ocre g auth`):
  - Protect an HTML page: take `CurrentUser(user): CurrentUser` (from
    `crate::auth`); visitors are redirected to /login and come back after.
    Optional: `OptionalUser(user): OptionalUser` gives `Option<User>`.
  - Protect a JSON route: take `BearerUser(user): BearerUser` (from
    `crate::auth_api`); it accepts `Authorization: Bearer <JWT or API key>`
    and answers 401 JSON otherwise. Put it before `Json(..)` in the arguments.
  - Ownership: filter queries by `user.id` (`WHERE user_id = ?1`) and return
    `Error::NotFound` (or `Error::Forbidden`) for other users' records.
  - Sign in only through `auth::sign_in(&session, &user)?` (it resets the
    session) and out with `auth::sign_out(&session)?`; never store more than
    the user id in the session.
  - Passwords: `ocre::password::hash` / `verify` only (PBKDF2 via WebCrypto,
    about 5 ms CPU per call: one per request at most). Never log or return
    passwords, tokens or digests; `User` never serializes `password_digest`.
  - Secret tokens (links, keys): `ocre::token::generate()`, store only
    `ocre::token::digest(&token)`, look rows up by digest.
  - JWT: `POST /api/auth/token` with `{"email", "password"}`; issue others
    with `ocre::jwt::encode(&ctx, &Claims::new(user.id.to_string(), ttl))?`.
    The key comes from `SECRET_KEY_BASE`; no other secret is needed.
  - API keys: `POST /api/auth/keys` `{"name"}` with a Bearer token returns
    `{"key", "api_key"}`: the key is shown once. List with `GET`, revoke with
    `DELETE /api/auth/keys/{id}`. In code: `models::api_key::create(&ctx, user.id, NewApiKey { name })`.
  - Magic-link and reset emails print in the `ocre dev` output
    (`[ocre mail]`); open the link from there.
  - No rate limiting: before going public, add Cloudflare rate limiting rules
    for /login, /signup, /magic_link, /passwords and /api/auth/*.

- Files (`attachment` fields, `ocre::storage`, R2 binding `STORAGE`):
  - Rules live in the model: `pub const IMAGE: Rules { max_bytes, content_types }`
    (exact types; no `image/*`, it would admit SVG). `validate()` checks
    uploads with `v.file("image", &upload, &IMAGE)`; forms add `max_bytes`
    to `FORM_LIMIT` (the `Multipart<FORM_LIMIT>` extractor answers 413 above it).
  - Set a file through the model only: `NewPhoto { image: Some(upload), .. }`,
    `PhotoChanges { image: Some(upload), .. }` (optional files:
    `Some(Some(upload))` replaces, `Some(None)` removes). `create`/`update`/`delete`
    store and delete R2 objects; never write the `*_key` columns by hand.
  - In a handler: `Multipart(mut form): Multipart<LIMIT>`, `form.form::<T>()?`
    for text fields, `form.file("image")` for an `Upload` (`None` when no file was chosen).
  - Serve with `storage::serve(&ctx, &photo.image(), &headers, Disposition::Inline).await`
    (streams R2 without CPU; ETag/304, Range, safe `Content-Disposition`).
    Check the user may see the record first. `Disposition::Download` forces a download.
  - JSON: `PUT /api/<plural>/{id}/<name>` with `curl -X PUT -F <name>=@file`,
    `DELETE` removes it; JSON bodies cannot carry files.
  - Other code: `storage::store(&ctx, "prefix", upload)`, `store_bytes`,
    `store_body` (raw body with Content-Length), `read`, `delete`.
  - Files of rows deleted by `ON DELETE CASCADE` stay in R2: delete them in
    the parent's `delete` if that matters.

- Realtime (after `ocre g scaffold ... --realtime`):
  - Send to every open page: `ocre::realtime::broadcast(&ctx, "posts", &html).await`
    from any handler or job. Build `html` with `realtime::prepend("posts", &row)`
    (top of `<tbody id="posts">`), `append`, `update(id, html)` (contents),
    `remove("post_12")`, or send an element with an `id` to replace it.
    Broadcast after the database write succeeded.
  - Pages subscribe with `<div hx-ext="ws" ws-connect="/realtime/posts">` and
    the htmx ws script (see `templates/posts/index.html`); no custom JavaScript.
  - New channel: add `"name" => {}` under `// ocre:channels` in
    `src/realtime.rs`; unknown channels are 404. Names: letters, digits,
    `_ - . :` (e.g. `post:12`). Private channel: check the user in `connect`
    (take `CurrentUser`) and return `Err(Error::Forbidden)`; use one channel
    per user or record, never a shared channel for private data.
  - Messages are rendered HTML: use askama templates (escaping) as for pages.
    Client messages are ignored; use forms/htmx requests to send data.
  - Needs `features = ["realtime"]` on `ocre` in Cargo.toml and the
    `CHANNELS` Durable Object + `[[migrations]]` in wrangler.toml (the
    generator adds them; `ocre deploy` creates the namespace).

- Caching (after `ocre g cache`):
  - Slow or costly results (aggregates, external APIs):
    `ocre::cache::fetch(&ctx, "stats:v1", Duration::from_secs(3600), || async { compute(&ctx).await }).await?`.
    The value is JSON (`Serialize + Deserialize`); bump the key's `:v1` when
    its type changes. `ocre::cache::delete(&ctx, key).await?` after changing
    the data behind it; `read`/`write` for explicit use.
  - Never cache per-user data under a shared key; put the user id in the key.
  - TTL is at least 60 s. KV is eventually consistent (up to 60 s).
  - Pages: take `conditional: ocre::cache::Conditional` and return
    `conditional.fresh_when(ETag::of(&(&post, i18n.locale()))?, CacheControl::no_cache(), || render(&view))`:
    304 without rendering when the browser's copy is current. Put everything
    the page shows in the ETag (records, locale, user id, flash).
  - `CacheControl::public(..)` only for responses identical for every
    visitor (no session data, locale in the path).
- Translations (after `ocre g locale en ...`):
  - Strings go in `locales/<code>.yml` under the root key `<code>:`, as nested
    keys indented by 2 spaces (no lists, no `{}`/`|`). Quote values starting with `%`
    or containing `: `. `%{name}` is filled by `.arg("name", value)`; plural
    keys have `one:`/`other:` children (Russian/Polish also `few:`/`many:`)
    and are picked by `.count(n)`, which also fills `%{count}`.
  - Handlers take `i18n: ocre::i18n::I18n` and pass it to the template struct:
    `{{ i18n.t("posts.title") }}`, `<html lang="{{ i18n.locale() }}">`. In
    handlers: `i18n.t("posts.created").to_string()`. Outside requests:
    `LOCALES.locale("fr")`.
  - Locale: `{locale}` path segment (`.nest("/{locale}", routes)`), else the
    `locale` cookie (`i18n.cookie()` is its `Set-Cookie` value), else
    `Accept-Language`, else the default (first code in `ocre::locales!`).
  - After adding keys, run `ocre i18n missing` and translate what it lists.
    `ocre dev` shows `translation missing: fr.key` in pages; production falls
    back to the default locale.
  - Keep `.layer(ocre::i18n::layer(&LOCALES))` last in `routes()`.

## Free-plan limits (design for them)

- 10 ms CPU per request. Awaiting D1 or `fetch` does not count. No CPU-heavy
  work in handlers: no argon2/bcrypt (use WebCrypto PBKDF2), no large parsing.
  Put static files in `public/`: they cost no Worker request or CPU.
- D1: daily read/write row quotas. Avoid N+1 queries: one query with `JOIN` or
  `WHERE id IN (...)` instead of a query per row.
- 100,000 requests per day.
- Queues: 10,000 operations per day (a job costs 3; each retry 1 more), so
  about 3,000 jobs per day; messages expire after 24 hours. Each consumer
  batch (up to 10 jobs) is one Worker request with 10 ms CPU. Cron Triggers:
  5 per account.
- GraphQL adds ~1.1 MB of WebAssembly and 20-60 ms of CPU when a Worker
  instance starts. Only add `--graphql` when a client needs it.
- Realtime: every WebSocket connection and every broadcast is one Durable
  Object request (100,000 a day, shared with other Durable Object use).
  Broadcast once per change, not per row in a loop; messages to browsers are free.
- KV (`ocre::cache`): 100,000 reads and 1,000 writes per day, 1 GB. Every
  `fetch` is a read; each miss and each `write`/`delete` is a write: a key
  refreshed every `ttl` seconds costs up to 86,400 / ttl writes per day. Use
  TTLs of an hour or more and few keys; past the limit `fetch` still works
  (computes every time) but gets slower.
- R2 (`ocre::storage`): 10 GB stored, 1M uploads (class A) and 10M downloads
  or 304s (class B) per month; deletes are free. The whole upload is in memory
  (128 MB per Worker; Cloudflare refuses bodies over 100 MB): keep `max_bytes`
  in the tens of MB. R2 must be enabled once in the dashboard (it asks for a
  payment method even for the free tier).

## Runtime constraints

- Target is `wasm32-unknown-unknown`: no tokio, no threads, no filesystem, no
  raw sockets. Crates that need them (sqlx, reqwest with native TLS, tokio)
  do not compile. Use `worker::Fetch` for outgoing HTTP.
- Rust must come from rustup with the wasm32 target (Homebrew `rust` lacks it).
