# Ocre

Rails-like Rust web framework for Cloudflare Workers, designed to run on the
Workers **free plan** and to be written by **AI agents**.

Generators write plain, readable Rust into your app (models with SQL, axum
handlers, askama templates, migrations); the `ocre` CLI builds, runs and
deploys it as one Worker. **Documentation: <https://ocre.rs>** (also as
Markdown for agents: [`llms.txt`](https://ocre.rs/llms.txt)).

Status: early. Apps run with `ocre dev` and in production on the free plan;
[ROADMAP.md](ROADMAP.md) lists what is missing.

## Quick start

```sh
cargo install ocre-cli          # installs `ocre`
ocre new                        # guided setup: name, starter, Cloudflare login, git, first deploy
```

Every question of the guided setup has a flag, so agents and scripts never get
a prompt:

```sh
ocre new qa --starter qa --yes  # a live Q&A app: accounts, events, realtime votes
cd qa
ocre dev                        # http://localhost:8787
ocre deploy                     # creates the D1 database, migrates, deploys
```

The [tutorial](https://ocre.rs/getting-started/tutorial.html) builds that app
step by step. Requirements: Rust through rustup with the
`wasm32-unknown-unknown` target, and Node.js 22 or newer (see
[Installation](https://ocre.rs/getting-started/installation.html)).

## What's in the box

Everything runs on Cloudflare's free plan:

| | |
|---|---|
| [Models and migrations](https://ocre.rs/guides/models.html) | D1 (SQLite), generated model files with plain SQL, `ocre::Query`, forward-only migrations, [validations](https://ocre.rs/guides/validations.html) |
| [Controllers](https://ocre.rs/guides/controllers.html), [views](https://ocre.rs/guides/views.html), [htmx](https://ocre.rs/guides/htmx.html) | axum handlers, askama templates, `hx-boost`, scaffolds with CRUD pages |
| [JSON APIs and GraphQL](https://ocre.rs/guides/json-apis.html) | REST from `ocre g api`, opt-in GraphQL; API-only apps with `ocre new --api` |
| [Authentication](https://ocre.rs/guides/authentication.html), [security](https://ocre.rs/guides/security.html) | Sign-up, login, magic links, OAuth, JWTs and API keys; encrypted cookie sessions, CSRF, CSP |
| [Email](https://ocre.rs/guides/email.html) | Mailers, Resend or Cloudflare Email Service, inbound email |
| [Jobs and schedules](https://ocre.rs/guides/jobs.html) | Cloudflare Queues and Cron Triggers |
| [Files](https://ocre.rs/guides/files.html) | R2 attachments, direct and resumable multipart uploads |
| [Realtime](https://ocre.rs/guides/realtime.html) | HTML over WebSockets, one hibernating Durable Object per channel |
| [Caching](https://ocre.rs/guides/caching.html), [translations](https://ocre.rs/guides/i18n.html) | KV read-through cache, fragment caching, ETags; YAML locales |
| [Webhooks](https://ocre.rs/guides/webhooks.html), [web push](https://ocre.rs/guides/push.html) | Signed webhooks, external jobs, push notifications |
| [Testing](https://ocre.rs/guides/testing.html), [deployment](https://ocre.rs/guides/deployment.html) | Request tests against workerd, `ocre deploy` provisions what the app needs |

Exact limits and what each feature costs against them:
[Free-plan limits](https://ocre.rs/reference/limits.html) and
[Cost model](https://ocre.rs/explanations/cost-model.html). Every command and
flag: [CLI](https://ocre.rs/reference/cli.html) and
[Generators](https://ocre.rs/reference/generators.html). Rust API:
[rustdoc](https://ocre.rs/api/ocre/) and the one-page
[API index](https://ocre.rs/api-index.html).

## Contributing

### Layout

```
crates/ocre/                 framework crate (features: html [default], graphql, realtime, push, testing)
  src/                       code; src/runtime/ calls the Workers JavaScript runtime
  tests/                     unit tests, mirroring src/: tests/session.rs tests src/session.rs
crates/ocre-cli/             `ocre` command-line tool
  src/                       commands and generators
  templates/                 files `ocre new`, the starters and the generators write
  tests/                     unit tests mirroring src/ (tests/generate/model.rs, ...)
  tests/integration/         the `ocre` binary against fake cf/wrangler/npm, and the terminal wizard
  tests/system/              generated apps running on workerd (`cf dev`)
  tests/support/             shared test helpers and the fake cf script
docs/                        documentation site (mdBook), built by `cargo docs-site`
  src/                       pages: SUMMARY.md (order), getting-started/, guides/, reference/, explanations/
  site/                      the hand-written home page
  tool/                      build/check/deploy tool (standalone crate, outside the workspace)
```

Each source file includes its test file with
`#[cfg(test)] #[path = "../tests/session.rs"] mod tests;`, so tests reach
private items while living apart from the code. Both crates set
`autotests = false`; the integration and system suites are `[[test]]` targets
in `crates/ocre-cli/Cargo.toml`.

To try local changes: `cargo install --path crates/ocre-cli`, then
`ocre new demo --starter qa --ocre-path "$PWD/crates/ocre" --yes`.

### Tests

```sh
cargo test --workspace --all-features             # unit, CLI and terminal tests
cargo test -p ocre-cli --test e2e -- --ignored   # generated apps on real `cf dev`
cargo llvm-cov --workspace --all-features \
  --ignore-filename-regex 'crates/ocre/src/runtime/' --fail-under-lines 100
```

CI requires 100% line coverage, no clippy or rustdoc warning, passing
doctests and an up-to-date `docs/api-index.md`, and builds a generated app
with every generator to WebAssembly. The e2e suite is not in CI (it needs
Node.js and full WebAssembly builds): run it before merging changes to the
runtime or the generators. `crates/ocre/src/runtime/` only runs inside
workerd, so coverage excludes it and the e2e suite exercises it.

### Documentation site

[mdBook](https://rust-lang.github.io/mdBook/) 0.5.4
(`cargo install mdbook --version 0.5.4 --locked`) plus `docs/tool`, through the
`cargo docs-site` alias:

```sh
cargo docs-site build    # docs/book/: HTML, <page>.md twins, llms.txt, llms-full.txt, /api/
cargo docs-site serve    # build, then wrangler dev
cargo docs-site check    # compile the `rust,check` examples in a generated app
cargo docs-site deploy   # build, then wrangler deploy (https://ocre.rs)
```

Pages start with `# Title` and a one-paragraph summary (it becomes the page's
line in `llms.txt`), stay self-contained (prerequisites, complete code,
commands, real output), and mark complete Rust modules as
```` ```rust,check ````. `docs/api-index.md` is generated from rustdoc JSON
with `python3 scripts/api-index.py` (nightly toolchain; `--check` fails when
it is stale).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
