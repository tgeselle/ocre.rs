# __APP_NAME__

Ocre app: Rust compiled to WebAssembly, running on Cloudflare Workers (free
plan) with a D1 (SQLite) database, askama templates and htmx.

## Commands

Run from the app root. Add `--json` to any `ocre` command for one JSON object
on stdout (`"ok": true|false`, plus `error` and `hint` on failure).

| Task | Command |
|---|---|
| CRUD resource | `ocre g scaffold Post title:string body:text published:boolean` |
| Empty migration | `ocre g migration add_slug_to_posts` |
| Apply migrations locally | `ocre migrate` |
| Run locally (http://localhost:8787) | `ocre dev` |
| Deploy + remote migrations | `ocre deploy` |
| Type-check | `cargo check --target wasm32-unknown-unknown` |

Scaffold field types: `string`, `text`, `integer`, `float`, `boolean`.

## Layout

```
src/lib.rs          entry point and router; keep the `// ocre:` marker comments
src/<plural>.rs     one module per resource: model, form, routes, handlers
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

## Free-plan limits (design for them)

- 10 ms CPU per request. Awaiting D1 or `fetch` does not count. No CPU-heavy
  work in handlers: no argon2/bcrypt (use WebCrypto PBKDF2), no large parsing.
- D1: daily read/write row quotas. Avoid N+1 queries: one query with `JOIN` or
  `WHERE id IN (...)` instead of a query per row.
- 100,000 requests per day.

## Runtime constraints

- Target is `wasm32-unknown-unknown`: no tokio, no threads, no filesystem, no
  raw sockets. Crates that need them (sqlx, reqwest with native TLS, tokio)
  do not compile. Use `worker::Fetch` for outgoing HTTP.
- Rust must come from rustup with the wasm32 target (Homebrew `rust` lacks it).
