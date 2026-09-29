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
templates/          askama templates, compiled into the binary
public/             static files (CSS, images, robots.txt), served by Cloudflare before the Worker runs
migrations/         numbered D1 SQL migrations, applied in order
wrangler.toml       Cloudflare config; the D1 binding must be named DB
.dev.vars           local secrets for `ocre dev` (SECRET_KEY_BASE); never commit it
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
