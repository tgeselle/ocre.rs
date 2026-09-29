# Ocre

Rails-like Rust web framework for Cloudflare Workers, designed to run on the
Workers **free plan** and to be written by **AI agents**.

Status: early. The core crate, the `ocre` CLI and an example app run with
`wrangler dev` and in production on the free plan; jobs, auth and realtime are
not built yet.

## Quick start

```sh
cargo install --git https://github.com/tgeselle/ocre.rs ocre-cli   # installs `ocre`
ocre new my-app && cd my-app
ocre g scaffold Post title:string body:text published:boolean
ocre dev       # applies local migrations, serves http://localhost:8787
ocre deploy    # deploys, creates the D1 database if needed, applies remote migrations
```

Each generated app has an `AGENTS.md` with the conventions, commands and
free-plan limits an agent needs.

## CLI

| Command | Effect |
|---|---|
| `ocre new <name> [--ocre-path <dir>]` | App skeleton; `--ocre-path` uses a local `crates/ocre` instead of git |
| `ocre g scaffold <Model> field:type...` | Migration, model, form, CRUD handlers, routes, templates; registers the module in `src/lib.rs` |
| `ocre g migration <name>` | Empty numbered migration |
| `ocre migrate [--remote]` | Apply D1 migrations |
| `ocre dev [--port N]` | Local migrations, then `wrangler dev` |
| `ocre deploy` | Existing database: migrate, then deploy. New database: deploy (creates it), then migrate |

Field types: `string`, `text`, `integer`, `float`, `boolean`. Scaffold routes:
`GET /posts`, `GET /posts/new`, `POST /posts`, `GET /posts/{id}`,
`GET /posts/{id}/edit`, `POST /posts/{id}` (update), `POST /posts/{id}/delete`.

Contract for agents: commands never prompt. With `--json`, stdout carries
exactly one JSON object, `{"ok": true, "command", "created", "updated", "url",
"next"}` or `{"ok": false, "error", "hint"}`; wrangler output goes to stderr.
Exit code is 0 on success, 1 on failure. Generators never overwrite files.

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
crates/ocre-cli/    `ocre` command-line tool and app templates
examples/blog/      example app (D1 + askama + htmx)
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
| `#[serde(deserialize_with = "ocre::bool_from_sql")]` | Read SQLite INTEGER 0/1 as `bool` |

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
| `GET /` (list, 1 D1 query) | 2 ms | 23 ms (1 of 40, likely a new isolate) | 18 ms |
| `GET /posts/:id` | 2 ms | 4 ms | 16.5 ms |
| `POST /posts` (insert) | 3 ms | 6 ms | 28 ms |

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
