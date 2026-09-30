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
| Many files per record (child model, show page and API routes) | `ocre g scaffold Album title:string photos:attachments` |
| Belongs to one of several models | `ocre g scaffold Comment body:text commentable:polymorphic:post,photo` |
| JSON REST resource, `/api/posts` | `ocre g api Post title:string body:text` |
| Same, also on `/graphql` (costs CPU, see below) | `ocre g api Post title:string --graphql` |
| Model + `index`/`show` actions to fill in (lighter than scaffold) | `ocre g resource Tag name:string^ color:enum:red,green,blue` |
| Pages or JSON endpoints without a model (GET actions) | `ocre g controller Pages about contact` (`--api` for JSON, `--auth` for signed-in users) |
| Add columns (SQL inferred from the name) | `ocre g migration add_slug_to_posts slug:string?` |
| Remove a column | `ocre g migration remove_slug_from_posts` |
| Add / remove an index (column names, in order) | `ocre g migration add_index_to_posts author_id created_at` (`add_unique_index_to_posts slug`, `remove_index_from_posts author_id created_at`) |
| Rename a column / a table; drop a table | `ocre g migration rename_body_to_content_in_posts`, `rename_posts_to_articles`, `drop_posts` |
| Change a column's type, NOT NULL, DEFAULT or CHECK (SQLite table rebuild) | `ocre db schema`, then `ocre g migration rebuild_posts` and edit its CREATE TABLE |
| Empty migration (data changes, custom SQL) | `ocre g migration backfill_slugs` |
| Authentication (users, login, magic link, password reset, JWT, API keys; once) | `ocre g auth` (`--db-sessions` to list/revoke devices, `--oauth github,google`) |
| Emails to send (one function per email; previews at `/ocre/dev/mailers` in `ocre dev`) | `ocre g mailer User welcome password_reset` |
| Receive email (Email Routing) | `ocre g mailbox` |
| Background job (Cloudflare Queues) | `ocre g job SendWelcome user_id:integer` |
| Job on its own queue (never waits behind others) | `ocre g job SendCode user_id:integer --queue urgent` |
| Scheduled task (Cron Trigger, UTC; English or cron) | `ocre g schedule nightly_cleanup "every day at 3am"` |
| List scheduled tasks; run one now (`ocre dev` running) | `ocre schedules`, `ocre schedules run nightly_cleanup` |
| Cache values in Workers KV (adds the `CACHE` binding) | `ocre g cache` |
| Read-only JSON data compiled into the Worker | `ocre g data countries` (`crate::data::countries::all()`) |
| Browser test (Playwright, run by `ocre test --e2e`) | `ocre g system_test signing_up` |
| Turn the cache off / back on in `ocre dev` (`CACHE_STORE=null` in `.dev.vars`) | `ocre dev --no-cache` / `ocre dev --cache` |
| GitHub Actions: checks on every push, `ocre deploy` on main (needs `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID` secrets) | `ocre g ci` |
| Installable app: web manifest, service worker, icon | `ocre g pwa` |
| Custom domain (then `ocre deploy`) | `ocre domains add www.example.com` (`ocre domains`, `ocre domains remove`) |
| Translations: set up (first code = default), add a locale | `ocre g locale en fr`, then `ocre g locale de` |
| Preview a generator without writing; overwrite / keep existing files | `ocre g scaffold Post title:string --pretend` (`--force`, `--skip`) |
| Undo a generator run (from `.ocre/generated/`; commit that directory) | `ocre destroy scaffold Post` (`--pretend`, `--force`) |
| Customize generated code: copy templates to `.ocre/templates/` | `ocre g override controller` (no argument: list) |
| App's own generator in `.ocre/generators/<name>/`, then run it | `ocre g generator service`, then `ocre g service Billing amount:integer` |
| Apply a template of ocre commands (one per line) | `ocre template setup.ocre` |
| Keys missing from a locale (fails if any) | `ocre i18n missing` |
| Apply migrations locally | `ocre migrate` |
| Pending migrations | `ocre migrate --status` |
| Load seed data (`db/fixtures/<table>.yml` named rows, local only, then `db/seeds.sql`); empty the tables first | `ocre db seed` (`ocre db seed --replant`, `--from test/fixtures`) |
| Save table rows as fixture files (never overwrites without `--force`) | `ocre db dump` (`--tables posts,users`, `--dir <dir>`) |
| Recreate local database (migrations + fixtures + seeds) | `ocre db reset` |
| Set up or update the local database (safe to repeat) | `ocre db prepare` |
| Empty every local table / delete the local database | `ocre db truncate` / `ocre db drop` |
| Last applied migration | `ocre db version` |
| Current schema of every table into `db/schema.sql` | `ocre db schema` |
| Query the database (JSON rows with `--json`) | `ocre sql "SELECT * FROM posts LIMIT 5"` |
| Run locally (http://localhost:8787) | `ocre dev` |
| List routes (method, path, handler) | `ocre routes` (or `ocre routes posts`) |
| New secret value | `ocre secret` |
| Live production logs | `ocre logs` (`--status error`, `--search text`) |
| Secret names locally and deployed; upload production values; print one local value | `ocre secrets list`, `ocre secrets push GITHUB_CLIENT_SECRET --file .prod.vars`, `ocre secrets fetch NAME --file .prod.vars` |
| Check tools, bindings, migrations, secrets, production settings, `.ocre/doctor/` scripts (fails on a problem) | `ocre doctor` |
| Everything CI checks, locally, before pushing (fmt, clippy, tests, wasm32, translations) | `ocre ci` |
| Versions and configuration; code size; TODO/FIXME comments | `ocre about`, `ocre stats`, `ocre notes` |
| Cloudflare login (browser, `cf auth login`; once) | `ocre login` |
| Deploy: creates missing resources, remote migrations, then `cf deploy` | `ocre deploy` |
| Check cloudflare.config.ts after editing it | `npx tsc -p .` |
| Find a Cloudflare command `ocre` does not wrap | `npx cf cli search "list worker versions"`, then `npx cf <command> --help` |
| Type-check | `cargo check --target wasm32-unknown-unknown` |
| Unit tests + type-check (`--e2e`: also the request tests of `tests/*.rs`, `tests/e2e.sh` and `tests/system/` against a fresh test database) | `ocre test` |

Field types: `string`, `text`, `rich_text` (HTML from the Trix editor,
sanitized on save; show it with `{{ x|rich_text }}`), `integer`, `float`, `decimal` (exact, as
text), `boolean`, `date` (`YYYY-MM-DD`), `time`, `datetime`, `uuid`,
`enum:draft,published` (a Rust enum in the model, stored as text with a
`CHECK`; cannot be unique, not with `--graphql`), `references`
(`author:references` adds `author_id`, a foreign key deleted with its parent;
`author:references?` sets it to NULL instead; `user:references^` is a
has-one; a model with only references, `Tagging post:references
tag:references`, is a join model giving `post.tags(ctx, page)`),
`attachment` (a file in R2 stored as `<name>_key`, `_filename`,
`_content_type`, `_size` columns; must be optional `?` in JSON APIs), `json`
(any JSON value as `ocre::serde_json::Value`, stored as JSON text; build
values with `ocre::serde_json::json!`; cannot be unique `^`). Suffix `?`
makes a field optional (NULL allowed), `^` unique. Integers must stay within
±`ocre::MAX_SAFE_INTEGER` (2^53 - 1): D1 returns numbers as JavaScript
numbers; generated validations already reject larger values.
`lock_version:integer` turns on optimistic locking: `update` answers
`Error::Conflict` (409) when `<Model>Changes.lock_version` is stale.

## Layout

```
src/lib.rs          entry point and router; keep the `// ocre:` marker comments
src/models/<model>.rs  the model: struct, New<Model>/<Model>Changes, validate(), query(),
                    all/count/find/find_many/create/update/delete, preload_<x>/for_<x>,
                    associations, before_/after_ create/update/delete callbacks
src/<plural>.rs     HTML resource: form parsing, handlers, routes (calls the model)
src/<plural>_api.rs JSON resource: REST handlers and GraphQL resolvers (call the model)
src/graphql.rs      GraphQL schema (when used); keep the `// ocre:graphql-*` markers
src/auth.rs         after `ocre g auth`: CurrentUser/OptionalUser extractors, sign_in/sign_out (HTML apps)
src/auth_api.rs     after `ocre g auth`: BearerUser extractor, /api/auth/* (token, me, keys)
src/mailers/<name>.rs  functions building `ocre::mail::Email`; templates in templates/mailers/<name>/<action>.{txt,html}, extending templates/mailers/layout.{txt,html}
src/mailers/mod.rs  `defaults(email)` applied to every mailer's email; `PREVIEWS` (keep `// ocre:mailer-previews`)
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
cloudflare.config.ts  Cloudflare config (bindings, triggers, exports); the D1 binding must be DB;
                    variables are `KEY: bindings.text("value")` in worker.env (MAIL_FROM);
                    generators add entries after `// ocre:env`, `// ocre:triggers`, `// ocre:exports`
                    (JOBS / JOBS_<NAME> queues, crons, CACHE, STORAGE, CHANNELS, AUTH_RATE_LIMITER);
                    `ocre deploy` writes the CACHE KV id into it: commit it
wrangler.config.ts  build command (worker-build) and assets directory, read by cf through the app's wrangler
package.json        pinned cf, wrangler, typescript (commit package-lock.json); tsconfig.json checks the .ts files
.dev.vars           local secrets and overrides for `ocre dev` (SECRET_KEY_BASE, MAIL_ADAPTER=log); never commit it
.prod.vars          production secret values for `ocre secrets push` (git-ignored); never commit it
```

## Rules

- Cloudflare: use `ocre` commands first, then Cloudflare's `cf` CLI
  (`npx cf ...`, the version pinned in package.json); never run `wrangler`
  directly (Ocre runs the app's wrangler itself where cf cannot yet). Find a
  command with `npx cf cli search "<what you want to do>"` and read its
  `--help` (or `npx cf schema <command>`). Production secrets: put
  `NAME=value` lines in `.prod.vars`, then `ocre secrets push NAME --file .prod.vars`;
  never pass secret values on the command line.
- cloudflare.config.ts: edit only its literal entries, in the canonical forms
  the generators write (`JOBS: bindings.queue({ name: "__APP_NAME__-jobs" }),`,
  `triggers.scheduled({ schedule: "0 3 * * *" }),`); Ocre reads the file
  without running it, so no variables, template strings or spreads in the
  values it reads (names, queues, buckets, crons, KV ids). Keep the
  `// ocre:env`, `// ocre:triggers` and `// ocre:exports` markers. Check with
  `npx tsc -p .` (or `ocre doctor`).
- Handlers are plain axum handlers taking `State(ctx): State<Ctx>`. Ocre types
  are `Send`; add `#[worker::send]` only to a handler that awaits another
  crate's JavaScript-backed future (reqwest, `worker::Fetch`): without it the
  route fails with "`Handler<_, _>` is not satisfied".
- Outgoing HTTP: `reqwest` with `default-features = false` (it uses `fetch` in
  WebAssembly) or `worker::Fetch`; each call is a subrequest.
- Queries: start from the model's `query()` (an `ocre::Query<T>`):
  `post::query().eq("published", true).order_desc("id").page(page).all(&ctx.db()?)`;
  `.first(&db)` (find_by), `.count`, `.exists`, `.pluck(&db, "id")`,
  `.aggregate(&db, "SUM(price)")`, `.paginate(&db, page)` (`Paginated<T>`),
  `.update_all(&db, vec![("col", v.into_param())])`, `.delete_all(&db)`.
  Conditions: `eq ne gt gte lt lte between is_in not_in is_null like contains
  starts_with ends_with`, `any(|q| ..)` for OR, `not(|q| ..)`,
  `where_sql("x > ?", params![..])`. Scopes are plain
  `fn published(q: Query<Post>) -> Query<Post>` applied with `.scope(published)`.
  Column names are `&'static str`: map user input to a fixed column with a
  `match` (sort by `Direction`, which deserializes from `asc`/`desc`).
- Raw SQL goes through `ctx.db()?` with `?1, ?2` placeholders and
  `params![...]`; never build SQL with `format!` from user input (`LIKE`
  patterns: `ocre::escape_like`). `db.all::<T>`, `db.first::<T>`
  (`INSERT ... RETURNING *` to get the new row), `db.execute` (rows changed).
- Transactions: D1 has no `BEGIN`; statements that must all apply or none go
  in one `ctx.db()?.batch(vec![Statement::new(sql, params![..]), query.update_statement(..)])`.
  Put conditions in the write (`UPDATE ... WHERE stock >= ?1`) and check the
  rows changed instead of read-then-write.
- Booleans are INTEGER 0/1 in SQLite: read them with
  `#[serde(deserialize_with = "ocre::bool_from_sql")]`. JSON columns hold
  JSON text: bind a `serde_json::Value` with `params![value]` and read it
  with `deserialize_with = "ocre::json_from_sql"` (`optional_json_from_sql`
  for `Option`); HTML forms parse the text with `v.json(field, &text)`.
- Missing record: `.or_404()?`. Bad input: `Error::bad_request("...")`.
  Unexpected failure: `Error::internal("...")` (logged, not shown to users).
- Validation: collect every problem with `ocre::Validator` (`required`,
  `min_length`/`max_length`/`length`, `range`, `greater_than`..., `email`,
  `inclusion`/`exclusion`, `format(field, v, |c| ..)`, `confirmation`,
  `acceptance`, `date`/`time`/`datetime`, `one_of::<Enum>`, `check`,
  `.message("..")` to replace the last message) and end with `v.finish()?`,
  which returns `Error::Invalid` (422). JSON answers
  `{"error": {"fields": {"title": ["can't be blank"]}}}`; generated HTML forms
  re-render with the messages and the typed values.
- Data rules live in the model (`src/models/<model>.rs`): `validate()` for
  checks without the database, `create`/`update` for uniqueness and foreign
  keys, the `before_*`/`after_*` callbacks at the end of the file for
  normalizing input and side effects (a `before_*` `Err` stops the write; an
  `after_*` `Err` does not undo it). Handlers and GraphQL resolvers only call
  model functions. After a migration that changes columns, update the model
  struct, `New<Model>`, `<Model>Changes`, `validate()` and the SQL in
  `create`/`update`.
- Load associations for many rows at once: `comment::preload_posts(ctx, &comments)`
  (HashMap by id), `comment::for_posts(ctx, &post_ids)`, `find_many(ctx, &ids)`;
  never an association or `find` in a loop. Other query helpers:
  `where_missing("comments", "post_id")`, `create_or_first(&db, || async { .. })`
  with a UNIQUE index, `batches(100, |r| r.id)` + `next(&db)` for big tables
  (from jobs: 50 queries per invocation), `explain(&db)`.
- Migrations are forward-only (no down/rollback): fix mistakes with a new
  migration. Encrypted columns: `ocre::encryption::Encrypted` (or
  `Deterministic` to look up with `eq`) as the row field type. Another D1
  database: `ctx.db_named("BINDING")?`.
- Templates escape `{{ value }}` by default; never mark user input `|safe`.
- Change the schema only with a new migration file; never edit an applied one.
- New module: generators register it in `src/lib.rs`. By hand, add
  `mod name;` under `// ocre:modules` and `.merge(name::routes())` under
  `// ocre:routes`. Keep `// ocre:models` and `// ocre:associations` too.
- URLs: each HTML controller has `pub mod paths` (`paths::index()`,
  `paths::new()`, `paths::show(id)`, `paths::edit(id)`, `paths::delete(id)`);
  redirect with `Redirect::to(&paths::show(id))`, link in its templates with
  `{{ paths::show(post.id) }}` and elsewhere `crate::posts::paths::show(id)`.
  When you change a route, change `paths` too. Nested routes use
  `Path((post_id, id)): Path<(i64, i64)>`; namespaces `.nest("/admin", admin::routes())`.
- Forms with lists or nested records (`tag_ids[]`, `lines[0][qty]`,
  `order[note]`): extract `ocre::NestedForm<T>` instead of axum's `Form`.
  Unicode route paths: `.route(&ocre::encode_path("/café"), ..)`. Streaming
  (Server-Sent Events): return `ocre::sse::stream(state, step)`, pace with
  `ocre::sleep`; links in emails: `ocre::mail::url(&ctx, path)` (`APP_URL`).
- Errors render `templates/error.html` through `error_page` in src/lib.rs;
  keep `.fallback(not_found)` and `.layer(map_response(error_page))` last in
  `routes()` (before the i18n layer).
- View helpers: `use ocre::filters;` in the controller, then
  `{{ price|number_to_currency("$") }}`, `{{ post.created_at|time_ago_in_words }} ago`,
  `{{ post.created_at|strftime("%b %-d, %Y") }}`, `number_with_delimiter`,
  `number_to_human_size`, `excerpt`, `highlight`; askama's `truncate`,
  `linebreaksbr`, `pluralize`. The same functions are in `ocre::helpers`.
- One action serving HTML and JSON: take `format: ocre::Format` (Accept
  header) and `match` it. Client IP: `ocre::RemoteIp`; request id:
  `ocre::RequestId`; back to the previous page: `ocre::redirect_back(&headers, "/")`;
  generated files: `ocre::storage::send_data(bytes, "name.csv", "text/csv", Disposition::Download)`.
- htmx: the layout boosts links and forms (`hx-boost`). For partial updates,
  answer a fragment when `Htmx(true)` and a redirect otherwise; `ocre::HxRedirect`
  for a full navigation from an htmx request. Pagination links:
  `page.previous()` / `page.next(rows.len())` and `?{{ p.query() }}`. No inline
  `<script>` or `on*=` attributes: scripts go in `public/`.
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
  403. Forms and `fetch()` calls need no token. Another site (a separate
  frontend) that must call the app: list its origin in
  `ALLOWED_ORIGINS: bindings.text("https://app.example.com")` in worker.env of
  cloudflare.config.ts (comma-separated); that also enables CORS for it.
  `ALLOWED_HOSTS: bindings.text("example.com, .example.com")` answers only on those hosts.
- Security headers (nosniff, SAMEORIGIN framing, referrer policy, HSTS on
  HTTPS) are added to every response; a handler that sets one keeps its value.
  Full-stack apps also set a Content-Security-Policy in src/lib.rs: no inline
  scripts or `onclick=`; use `ocre::security::CspNonce` for an inline script.
- Request data: read forms and JSON into structs that list only the fields a
  visitor may set (never `user_id` or roles: take them from `CurrentUser`).
  SQL: values only through `?1` + `params![..]` or `Query` methods, never
  `format!`; map a user-chosen sort onto a fixed column name. User HTML:
  `ocre::security::sanitize` before `|safe`. Redirect targets from the
  request: `ocre::security::url_from`.
- Rate limiting: `ocre::security::rate_limit(&ctx, "BINDING", &key).await?`
  (429 when over) with a `BINDING: bindings.rateLimit({ namespace: "<unique integer>", simple: { limit: 10, period: 60 } }),`
  entry in cloudflare.config.ts; key by
  `ocre::remote_ip(&headers)` or user id.
- Logging: `ctx.log().info(...)` (levels trace/debug/info/warn/error; add fields with
  `ctx.log().with("key", value)`), never `println!`. Lines carry the request id and
  go to Workers Logs as JSON in production. `LOG_LEVEL` (var) sets the level.
- Errors: return `Error::internal("...")` for unexpected failures (logged with the
  request's details and sent to `ocre::errors` subscribers, e.g. Sentry with the
  `SENTRY_DSN` secret); `ctx.errors().report(...)` for errors you handle yourself.
  `ocre dev` shows a development error page with the D1 statements the request ran.
- `GET /up` is the health check; keep it cheap (no database).
- Email: build it in a mailer (`src/mailers/`), send it from the handler with
  `ocre::mail::send(&ctx, mailers::user::welcome(&address)?).await?`. Validate
  user-typed addresses first with `v.email(..)`. `MAIL_ADAPTER` picks the
  delivery: `log` (`ocre dev`: the email is printed in the dev output between
  `[ocre mail]` lines, links included; read them there), `resend` (secret
  `RESEND_API_KEY`; free plan: any recipient, 100/day) or `cloudflare`
  (`EMAIL: bindings.sendEmail()` binding; free plan: only verified addresses of the
  account). Unset in production = `send` fails with an error naming the fix.
  Put data in the template structs; `.txt` templates are not HTML-escaped.
  More on an `Email`: `.cc(..)`, `.bcc(..)`, `.also_to(..)`, `.reply_to(..)`,
  `.header(..)`, `.attach(filename, content_type, bytes)`, `.inline(cid, ..)`
  (HTML `<img src="cid:..">`). In `ocre dev`: previews and the emails sent at
  `http://localhost:8787/ocre/dev/mailers`, as JSON at
  `/ocre/dev/mailers/sent.json` (read emailed links there in scripts).
- Background jobs: anything slow, retryable or not needed for the response
  (emails, calls to other APIs, bulk updates) goes in a job. Enqueue with
  `SendWelcome { user_id }.perform_later(&ctx).await?`
  (`use crate::jobs::SendWelcome;`), or `ocre::jobs::enqueue_in(&ctx,
  &Job::SendWelcome(..), Duration::from_secs(n))` (24 h max), or many at once
  with `ocre::jobs::enqueue_all(&ctx, &jobs)` (never a loop of enqueues). Job
  fields are the arguments: pass ids
  and small values (128 KB max), load records in `perform`. `perform` returns
  `Err(Error::NotFound)` or another 4xx error to drop the job (logged, not
  retried), any other `Err` to retry (30 s, 1 min, 3 min, 9 min, 27 min, then the
  `<app>-jobs-failed` dead-letter queue); jobs may run twice, so make them
  safe to repeat. Never rename a `Job` variant or change its fields while
  messages may be queued: they would be dropped (`[ocre jobs] dropped message`).
  In `ocre dev` jobs run within ~5 s; read the `[ocre jobs]` lines. Queues have
  no priorities: urgent jobs get their own queue (`--queue urgent`).
- Email from a request: prefer `ocre::mail::deliver_later(&ctx, email).await?`
  (sent by the jobs queue, retried; `deliver_in` to delay it) once the app has
  a job; `send` otherwise.
- Scheduled tasks: `ocre g schedule <name> "<when>"` ("every 15 minutes",
  "every weekday at 6pm", or a cron expression); the task `run(ctx)` must stay within
  10 ms CPU: query ids, then enqueue jobs (`enqueue_all`). Crons are UTC; a failed
  run is logged (`[ocre cron]`), not retried. Cron Triggers never fire in
  `ocre dev`: `ocre schedules run <name>` fires one.
- Incoming email: `src/mailbox.rs` `receive(ctx, email)`; `email.to()`,
  `subject()`, `text()`, `header(..)`; `email.reject("reason")` bounces,
  `email.forward("verified@address").await?` forwards; an `Err` bounces.
  Test locally with the form at `http://localhost:8787/ocre/dev/mailbox`, or
  `curl 'http://localhost:8787/cdn-cgi/local/email?from=a@example.com&to=b@example.com' --data-binary @message.eml`
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
  - Sign in only through `auth::sign_in(&ctx, &session, &headers, &user, remember).await?`
    (it resets the session and returns the page to go to) and out with
    `auth::sign_out(&ctx, &session).await?`; never store more than the user id
    in the session.
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
  - Every route that checks a password or sends an email calls
    `auth_api::throttle(&ctx, &headers, "<action>").await?` (binding
    `AUTH_RATE_LIMITER`, 10 a minute per IP): do the same in new ones.
  - `--db-sessions` apps: sessions are D1 rows (`/account/sessions` revokes
    them). `--oauth` apps: secrets `<PROVIDER>_CLIENT_ID` and
    `<PROVIDER>_CLIENT_SECRET` in .dev.vars and, for production, `ocre secrets push ... --file .prod.vars`.

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
    `store_body` (raw body with Content-Length), `read`, `delete`;
    `Upload::new(name, type, bytes)` for app-made files; `head`/`exists`
    (class B), `list(prefix, cursor, limit)` (class A per page: never per request).
  - Content check: `v.file_content("image", &upload)` refuses bytes that do not
    match the declared type; `storage::analyze(&bytes)` gives type, width, height.
  - Big files: `storage::direct_upload` (presigned PUT, needs `R2_ACCOUNT_ID`,
    `R2_BUCKET` vars, `R2_ACCESS_KEY_ID`/`R2_SECRET_ACCESS_KEY` secrets and a
    bucket CORS rule) then `attach_direct_upload(signed_key)`; purge abandoned
    ones from a schedule with `purge_unattached`. `serve_redirect` sends a 302
    to a presigned GET. Presigned URLs hit the real bucket, not `ocre dev`'s.
  - Resized images: `Variant::new().width(300).path("/photos/1/image")`
    (Cloudflare Image Transformations; custom domain only, not workers.dev).
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
    Client messages are ignored; use forms/htmx requests to send data. For
    chat-like relays between JavaScript clients, `upgrade.identified_by(user.id.to_string()).rebroadcast()`
    in `connect` sends what a client sends to the others as `{"from","data"}` JSON.
  - Tests: in `ocre dev`, GET `/ocre/dev/realtime/sent.json` lists recent broadcasts.
  - Needs `features = ["realtime"]` on `ocre` in Cargo.toml and the
    `CHANNELS` binding + `OcreChannel` export in cloudflare.config.ts (the
    generator adds them; `ocre deploy` creates the namespace).

- Caching (after `ocre g cache`):
  - Slow or costly results (aggregates, external APIs):
    `ocre::cache::fetch(&ctx, "stats:v1", Duration::from_secs(3600), || async { compute(&ctx).await }).await?`.
    The value is JSON (`Serialize + Deserialize`); bump the key's `:v1` when
    its type changes. `ocre::cache::delete(&ctx, key).await?` after changing
    the data behind it; `read`/`write` for explicit use.
  - Never cache per-user data under a shared key; put the user id in the key.
  - Costly HTML: `ocre::cache::fragment(&ctx, &ocre::cache::key(&[&"posts", &post.id, &post.updated_at, &"card-v1"]), ttl, || CardView { post: &post }).await?`
    returns a `Fragment`; put it in the page's view struct and write `{{ card }}` (no `|safe`).
    Lists: `ocre::cache::fragments(&ctx, &posts, ttl, |p| key.., |p| RowView { post: p })`.
    Bump the key's version when the template changes; never cache HTML holding a CSRF token or nonce.
  - Repeated identical SELECTs in one request are cached automatically (any write empties it);
    `ctx.db()?.uncached()` bypasses it. `CACHE_STORE=null` in `.dev.vars` turns KV caching off.
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
  - Defaults: `i18n.t("key").or_key("other.key").or("Text")`. Scoped keys:
    `i18n.scope("posts.index")` in the handler, then `{{ i18n.t(".title") }}`.
    Markup in a translation: `{{ i18n.t("terms").arg("name", n).html() }}`
    (text raw, values escaped); never `|safe`. Links keeping the locale:
    `{{ i18n.path("/posts") }}` (routes under `.nest("/{locale}", ..)`).
  - Localized formats: `i18n.l(date, "long")`, `i18n.number(n)`,
    `i18n.currency(amount, "€")`, `i18n.time_ago_in_words(at)`. Names:
    `i18n.model_name("post", n)` (`models.post.one/other`),
    `i18n.attribute("post", "title")` (`attributes.post.title`). Validation
    errors: `i18n.full_message("post", &error)` (keys `errors.messages.blank`...,
    built in for en, fr, de, es, it, pt, nl).
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
- 50 D1 queries and 50 subrequests (`fetch`) per invocation (request, queue
  batch, cron run). Write many rows with one statement:
  `ocre::bulk::{insert, upsert, update}` (then `db.execute(&s.sql, s.params)`).
  Long jobs: `ocre::jobs::run_steps(&Budget::new(49), cost_per_step, cursor, step)`
  returns the cursor to re-enqueue the job with when the budget runs out.
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
- Node.js 22 or newer, with the app's npm packages installed (`npm install`).
