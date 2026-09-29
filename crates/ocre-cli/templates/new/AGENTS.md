# __APP_NAME__

Ocre app: Rust compiled to WebAssembly, running on Cloudflare Workers (free
plan) with a D1 (SQLite) database. Full-stack apps render HTML with askama and
htmx; API-only apps (`mode = "api"` in Cargo.toml) serve JSON only.

## Commands

Run from the app root. Add `--json` to any `ocre` command for one JSON object
on stdout (`"ok": true|false`, plus `error` and `hint` on failure).

| Task | Command |
|---|---|
| CRUD resource (HTML pages; JSON in API-only apps) | `ocre g scaffold Post title:string body:text published:boolean` |
| JSON REST resource, `/api/posts` | `ocre g api Post title:string body:text` |
| Same, also on `/graphql` (costs CPU, see below) | `ocre g api Post title:string --graphql` |
| Empty migration | `ocre g migration add_slug_to_posts` |
| Apply migrations locally | `ocre migrate` |
| Run locally (http://localhost:8787) | `ocre dev` |
| Cloudflare login (browser; once) | `ocre login` |
| Deploy + remote migrations | `ocre deploy` |
| Type-check | `cargo check --target wasm32-unknown-unknown` |

Scaffold field types: `string`, `text`, `integer`, `float`, `boolean`.
Integers must stay within ±`ocre::MAX_SAFE_INTEGER` (2^53 - 1): D1 returns
numbers as JavaScript numbers. Scaffolded forms already reject larger values.

## Layout

```
src/lib.rs          entry point and router; keep the `// ocre:` marker comments
src/<plural>.rs     HTML resource: model, form, routes, handlers
src/<plural>_api.rs JSON resource: list/find/create/update/delete, REST handlers, GraphQL resolvers
src/graphql.rs      GraphQL schema (when used); keep the `// ocre:graphql-*` markers
templates/          askama templates, compiled into the binary
migrations/         numbered D1 SQL migrations, applied in order
wrangler.toml       Cloudflare config; the D1 binding must be named DB
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
- Templates escape `{{ value }}` by default; never mark user input `|safe`.
- Change the schema only with a new migration file; never edit an applied one.
- New module: `ocre g scaffold` registers it in `src/lib.rs`. By hand, add
  `mod name;` under `// ocre:modules` and `.merge(name::routes())` under
  `// ocre:routes`.
- JSON handlers return `ApiResult<T>` (errors become `{"error": {"status",
  "message"}}`), take bodies with `ocre::Json<T>` and lists with `Page`.
  Put logic in the module's `list`/`find`/`create`/`update`/`delete`
  functions so REST and GraphQL share it.

## Free-plan limits (design for them)

- 10 ms CPU per request. Awaiting D1 or `fetch` does not count. No CPU-heavy
  work in handlers: no argon2/bcrypt (use WebCrypto PBKDF2), no large parsing.
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
