# Ocre

Rails-like Rust web framework for Cloudflare Workers, designed to run on the
Workers **free plan** and to be written by **AI agents**.

Status: feasibility stage. The core crate and an example app run with
`wrangler dev` and in production on the free plan; generators, CLI, jobs and
realtime are not built yet.

## Rules the framework enforces

- Handlers are plain [axum](https://docs.rs/axum) handlers. Every Ocre type is
  `Send`, so `#[worker::send]` is never needed.
- One way to do each thing: SQL with `?N` placeholders and `params![...]`,
  askama templates compiled at build time, htmx for interactivity.
- Errors name the fix (for example a missing `DB` binding says which
  `wrangler.toml` entry to add). Internal errors are logged, never shown to users.

## Layout

```
crates/ocre/        framework crate
examples/blog/      example app (D1 + askama + htmx)
  src/lib.rs        routes and handlers
  templates/        askama templates
  migrations/       D1 SQL migrations
  wrangler.toml     Workers config; D1 binding must be named DB
```

## API

| Item | Use |
|---|---|
| `ocre::serve(routes(), req, env)` | Worker entry point (`#[worker::event(fetch)]`) |
| `State(ctx): State<Ctx>` | Per-request context |
| `ctx.db()?` | D1 database bound as `DB` |
| `db.all::<T>(sql, params![..])` | All rows as `Vec<T>` |
| `db.first::<T>(sql, params![..])` | First row as `Option<T>`; use with `INSERT ... RETURNING *` |
| `db.execute(sql, params![..])` | Rows changed |
| `render(&template)` | askama template to `Html<String>` |
| `Htmx(is_htmx)` | Extractor: true when `HX-Request: true` |
| `option.or_404()?` | Missing record to 404 |
| `Error::bad_request(msg)` / `Error::internal(msg)` | 400 / 500 responses |

## Requirements

- Rust via **rustup** with the `wasm32-unknown-unknown` target
  (`rust-toolchain.toml` installs it). A Homebrew `rust` without rustup has no
  wasm target and the build fails.
- Node.js (for `npx wrangler`).

## Run the example

```sh
cd examples/blog
npx wrangler d1 migrations apply ocre-blog --local
npx wrangler dev
```

Deploy (free Cloudflare account; `npx wrangler login` once). The first deploy
creates the D1 database from `database_name`, so no `database_id` is needed;
migrations run after it:

```sh
npx wrangler deploy
npx wrangler d1 migrations apply ocre-blog --remote
```

## Measured

Example app, release build: `index_bg.wasm` 420 KB (130 KB gzipped),
`index.js` 24 KB (Workers limit: 64 MiB). Worker startup time: 4 ms.

Production, free plan (`wrangler tail`, 55 requests):

| Route | CPU median | CPU max | Wall median |
|---|---|---|---|
| `GET /` (list, 1 D1 query) | 2 ms | 23 ms (1 of 40, new isolate) | 18 ms |
| `GET /posts/:id` | 2 ms | 4 ms | 16.5 ms |
| `POST /posts` (insert) | 3 ms | 6 ms | 28 ms |

The free-plan limit is 10 ms CPU per request; Cloudflare tolerates infrequent
overruns per isolate, and kills requests only when overruns become frequent.
