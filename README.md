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
| `ocre g scaffold <Model> field:type...` | Model (unless it exists) plus HTML CRUD: handlers, routes, templates; registers modules in `src/lib.rs`. In an API-only app: same as `ocre g api` |
| `ocre g api <Model> field:type... [--graphql]` | Model (unless it exists) plus JSON REST resource under `/api/<plural>`; `--graphql` also exposes it on `/graphql` |
| `ocre g migration <name> [field:type...]` | Numbered migration; `create_<table>`, `add_<x>_to_<table>` and `remove_<x>_from_<table>` get their SQL from the name and fields |
| `ocre migrate [--remote]` | Apply D1 migrations |
| `ocre migrate --status [--remote]` | Show wrangler's pending-migrations table; `--json` lists them in `pending` |
| `ocre db seed [--remote]` | Run `db/seeds.sql` |
| `ocre db reset` | Local only: delete `.wrangler/state/v3/d1`, apply migrations, run `db/seeds.sql` if present |
| `ocre sql "<query>" [--remote]` | Run SQL and print the rows as a table; `--json` returns wrangler's results in `rows` |
| `ocre dev [--port N]` | Local migrations, then `wrangler dev` |
| `ocre deploy` | Existing database: migrate, then deploy. New database: deploy (creates it), then migrate |

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
key, `ON DELETE CASCADE`). Suffixes: `?` optional (NULL allowed), `^` unique.
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

## Rules the framework enforces

- Handlers are plain [axum](https://docs.rs/axum) handlers. Every Ocre type is
  `Send`, so `#[worker::send]` is never needed.
- One way to do each thing: SQL with `?N` placeholders and `params![...]`,
  askama templates compiled at build time, htmx for interactivity.
- Errors name the fix (for example a missing `DB` binding says which
  `wrangler.toml` entry to add). Internal errors are logged, never shown to users.

## Layout

```
crates/ocre/        framework crate (features: html [default], graphql)
crates/ocre-cli/    `ocre` command-line tool and app templates
examples/blog/      example app (D1 + askama + htmx)
```

Unit tests live next to each module in a separate file (`src/sql.rs` →
`src/sql/tests.rs`, declared with `#[cfg(test)] mod tests;`), so they can test
private items without mixing with production code.

## API

| Item | Use |
|---|---|
| `ocre::serve(routes(), req, env)` | Worker entry point (`#[worker::event(fetch)]`) |
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

## Tests

```sh
cargo test --workspace                           # unit, CLI and terminal tests: ~2 s
cargo test -p ocre-cli --test e2e -- --ignored   # real `wrangler dev`: ~16 s cold, ~6 s warm
cargo llvm-cov --workspace --exclude blog \
  --ignore-filename-regex 'crates/ocre/src/runtime/' --fail-under-lines 100
```

| Suite | What it runs |
|---|---|
| Unit (`src/**`) | Pure logic: params, errors, extractors, names, generators |
| `crates/ocre-cli/tests/cli.rs` | The `ocre` binary with a fake wrangler (`tests/common/fake_npx.sh`): every command, `--json` contract, every error hint |
| `crates/ocre-cli/tests/wizard.rs` | `ocre new` in a pseudo-terminal: questions, keys, cancel |
| `crates/ocre-cli/tests/e2e.rs` | Generated app built to WebAssembly (dev build, shared `target/e2e-app`), served by `wrangler dev`, full CRUD over HTTP |

CI runs lint and coverage as parallel jobs and requires 100% line coverage.
The e2e test is not part of CI (it needs Node.js and a full WebAssembly
build); run it locally before merging changes to the runtime or generators.
`crates/ocre/src/runtime/` calls the Workers JavaScript runtime and only runs
inside workerd, where it cannot be instrumented; it is excluded from the
measurement and exercised by the e2e test.

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
