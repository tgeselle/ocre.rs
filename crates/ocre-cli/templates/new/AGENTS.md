# __APP_NAME__

Ocre app: Rust compiled to WebAssembly, running on Cloudflare Workers (free
plan) with a D1 (SQLite) database. Full-stack apps render HTML with askama and
htmx; API-only apps (`mode = "api"` in Cargo.toml) serve JSON only.

## Commands

Run from the app root. Add `--json` to any `ocre` command for one JSON object
on stdout (`"ok": true|false`, plus `error` and `hint` on failure).

| Task | Command |
|---|---|
| Model only: table, validations, queries, associations | `ocre g model Author name:string^ bio:text?` |
| CRUD resource (HTML pages; JSON in API-only apps) | `ocre g scaffold Post title:string body:text published:boolean author:references` |
| JSON REST resource, `/api/posts` | `ocre g api Post title:string body:text` |
| Same, also on `/graphql` (costs CPU, see below) | `ocre g api Post title:string --graphql` |
| Add columns (SQL inferred from the name) | `ocre g migration add_slug_to_posts slug:string?` |
| Remove a column | `ocre g migration remove_slug_from_posts` |
| Empty migration (data changes, custom SQL) | `ocre g migration backfill_slugs` |
| Authentication (users, login, magic link, password reset, JWT, API keys; once) | `ocre g auth` |
| Emails to send (one function per email) | `ocre g mailer User welcome password_reset` |
| Receive email (Email Routing) | `ocre g mailbox` |
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
`author_id`, a foreign key deleted with its parent). Suffix `?` makes a field
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
templates/          askama templates, compiled into the binary
public/             static files (CSS, images, robots.txt), served by Cloudflare before the Worker runs
migrations/         numbered D1 SQL migrations, applied in order
wrangler.toml       Cloudflare config; the D1 binding must be named DB; MAIL_FROM under [vars]
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
  `#[serde(deserialize_with = "ocre::bool_from_sql")]`.
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
  `deserialize_with = "ocre::patch"` (missing keeps the value, `null` clears it).
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

## Free-plan limits (design for them)

- 10 ms CPU per request. Awaiting D1 or `fetch` does not count. No CPU-heavy
  work in handlers: no argon2/bcrypt (use WebCrypto PBKDF2), no large parsing.
  Put static files in `public/`: they cost no Worker request or CPU.
- D1: daily read/write row quotas. Avoid N+1 queries: one query with `JOIN` or
  `WHERE id IN (...)` instead of a query per row.
- 100,000 requests per day.
- GraphQL adds ~1.1 MB of WebAssembly and 20-60 ms of CPU when a Worker
  instance starts. Only add `--graphql` when a client needs it.

## Runtime constraints

- Target is `wasm32-unknown-unknown`: no tokio, no threads, no filesystem, no
  raw sockets. Crates that need them (sqlx, reqwest with native TLS, tokio)
  do not compile. Use `worker::Fetch` for outgoing HTTP.
- Rust must come from rustup with the wasm32 target (Homebrew `rust` lacks it).
