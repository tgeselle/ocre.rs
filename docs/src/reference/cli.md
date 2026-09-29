# CLI commands

This page documents every `ocre` command and flag, what each one does step by step, its human and `--json` output, and the errors it reports with their hints. Code generators (`ocre g ...`) have their own page, [Generators](generators.md).

## Before you start

- The `ocre` CLI, installed with `cargo install --git https://github.com/tgeselle/ocre.rs ocre-cli` (see [Installation](../getting-started/installation.md)).
- Node.js 20 or newer: every command that touches the database, the dev server or Cloudflare runs `npx --yes wrangler@4` (the CLI pins wrangler's major version).
- A rustup toolchain with the `wasm32-unknown-unknown` target for `ocre dev` and `ocre deploy`.
- Except `ocre new`, `ocre login`, `ocre secret` and `ocre help`, commands run inside an Ocre app: the CLI walks up from the current directory to the nearest `wrangler.toml`, and reads the `database_name` of its `[[d1_databases]]` entry with `binding = "DB"`.
- Commands with `--remote`, `ocre login` and `ocre deploy` need a Cloudflare account (free) and a login (`ocre login`) or the `CLOUDFLARE_API_TOKEN` environment variable that wrangler reads.

The examples below were run with `ocre 0.1.0` and wrangler 4.143.0 on apps created by `ocre new ... --starter blog`. Commands that need Cloudflare (`login`, `deploy`, `--remote`, `new --login/--deploy`) were run against the fake wrangler of the CLI's integration tests (`crates/ocre-cli/tests/support/fake_npx.sh`), so the lines wrangler prints in those examples are the fake's, while the lines and JSON printed by `ocre` itself are real.

## Summary

| Command | What it does |
|---|---|
| [`ocre new [NAME]`](#ocre-new) | Creates an app in `./NAME`; in a terminal, asks for anything flags did not answer |
| [`ocre login`](#ocre-login) | Logs in to Cloudflare in the browser, unless already logged in |
| [`ocre generate` / `ocre g`](#ocre-generate) | Generates code: see [Generators](generators.md) |
| [`ocre migrate`](#ocre-migrate) | Applies D1 migrations (local unless `--remote`); `--status` lists pending ones |
| [`ocre db seed`](#ocre-db-seed) | Runs `db/seeds.sql` (local unless `--remote`) |
| [`ocre db reset`](#ocre-db-reset) | Local only: deletes the local database, applies every migration, runs the seeds |
| [`ocre sql QUERY`](#ocre-sql) | Runs SQL on D1 and prints the rows |
| [`ocre dev`](#ocre-dev) | Applies local migrations, then runs the app with `wrangler dev` |
| [`ocre deploy`](#ocre-deploy) | Creates missing Cloudflare resources, deploys, applies remote migrations |
| [`ocre secret`](#ocre-secret) | Prints a new random value for `SECRET_KEY_BASE` |
| [`ocre routes [FILTER]`](#ocre-routes) | Lists the app's HTTP routes, read from its source |
| [`ocre i18n missing`](#ocre-i18n-missing) | Checks the locale files; fails on missing keys or invalid files |
| [`ocre help`](#ocre-help), [`ocre --version`](#ocre-version) | Help text and version |

## Global flag: --json

`--json` is accepted by every command, before or after the subcommand (`ocre --json routes` and `ocre routes --json` are the same). It is the contract for scripts and AI agents:

- stdout carries exactly one JSON object, on one line, printed when the command ends.
- The command never prompts (`ocre new` skips its wizard).
- Output of the tools the CLI runs (wrangler, and cargo through wrangler's build) goes to stderr instead of stdout.

On success the object has `"ok": true`, `command`, and only the keys that apply (empty lists, `false` flags and absent values are omitted):

| Key | Type | Set by | Meaning |
|---|---|---|---|
| `ok` | boolean | every command | `true` |
| `command` | string | every command | `new`, `login`, `migrate`, `db seed`, `db reset`, `sql`, `dev`, `deploy`, `secret`, `routes`, `i18n missing`, or `generate <generator>` (e.g. `generate scaffold`) |
| `created` | string[] | `new`, generators | Files created, relative to the app root (to the current directory for `ocre new`, so they start with the app name) |
| `updated` | string[] | generators | Existing files changed |
| `url` | string | `dev`, `deploy`, `new --deploy` | `http://localhost:<port>`, or the `https://....workers.dev` URL found in wrangler's deploy output |
| `email` | string | `login`, `new --login`, `new --deploy` | Email of the Cloudflare login |
| `pending` | string[] | `migrate --status` | Migration files not applied yet |
| `ran` | string[] | `db seed`, `db reset`, `i18n missing` | Steps performed, in order |
| `rows` | array | `sql` | Wrangler's JSON: one object per statement, with `results` (the rows), `success` and `meta` |
| `routes` | object[] | `routes` | `{"method", "path", "handler"}`, sorted by path then method |
| `remote` | `true` | `migrate --status --remote`, `db seed --remote`, `sql --remote` | The command used the production database |
| `secret` | string | `secret` | 128 lowercase hex characters |
| `secret_created` | `true` | `deploy`, `new --deploy` | The deploy uploaded a new `SECRET_KEY_BASE` because the Worker had none |
| `provisioned` | string[] | `deploy` | Cloudflare resources created because they were missing, e.g. `queue blog-jobs` |
| `next` | string[] | `new`, `migrate --status`, generators | Commands or actions to run next, in order |

On failure the object is `{"ok": false, "error": "...", "hint": "..."}`. `hint` names the fix; it is `null` for the few errors without one (for example I/O errors).

```json
{"error":"no wrangler.toml found in this directory or its parents","hint":"run this command inside an Ocre app, or create one with `ocre new <name>`","ok":false}
```

Without `--json`, the same result is printed for humans: `  create  <path>` and `  update  <path>` lines, the steps, the route table, the URL, then a `Next:` list. Failures print `error: <message>` and `hint: <hint>` on stderr.

### Exit codes

| Code | When | stdout with `--json` |
|---|---|---|
| `0` | Success | `{"ok": true, ...}` |
| `1` | The command failed | `{"ok": false, "error", "hint"}` |
| `2` | Invalid arguments (unknown flag, missing required argument), detected by the argument parser before the command runs | Nothing: the usage error is plain text on stderr, even with `--json` |

```sh
ocre g model Thing --json
```

```text
error: the following required arguments were not provided:
  <FIELDS>...

Usage: ocre generate model --json <NAME> <FIELDS>...

For more information, try '--help'.
```

(exit code 2, printed on stderr).

### Errors shared by several commands

| Error | Hint | Cause |
|---|---|---|
| `no wrangler.toml found in this directory or its parents` | ``run this command inside an Ocre app, or create one with `ocre new <name>` `` | Run outside an app |
| `wrangler.toml is not valid TOML: ...` | none | Syntax error in `wrangler.toml` |
| `wrangler.toml has no D1 database with binding "DB"` | the `[[d1_databases]]` block to add, with `binding = "DB"`, `database_name` and `migrations_dir = "migrations"` | The `DB` binding was removed or renamed |
| `could not run npx: ...` | `install Node.js 20 or newer (it provides npx)` | No `npx` on `PATH` |
| `` `wrangler <arguments>` failed (exit status: N) `` | `read the wrangler output above; the first error line names the cause` | Wrangler failed; its own error is on stderr just above |
| `the wasm32-unknown-unknown target is not installed for rustc at <sysroot>` | ``use a rustup toolchain (Homebrew's `rust` has no wasm target) and run `rustup target add wasm32-unknown-unknown` `` | `ocre dev`, `ocre deploy`, `ocre new --deploy` with a toolchain that cannot build WebAssembly |
| `rustc not found` | `install Rust with rustup: https://rustup.rs` | No Rust on `PATH` |
| `invalid locale files:` followed by one line per problem | ``fix each line named above (quote values with "..." when in doubt); `ocre i18n missing` checks them again`` | `ocre dev` and `ocre deploy` refuse locale files the Worker could not load (see [ocre i18n missing](#ocre-i18n-missing)) |

## ocre new

```text
ocre new [OPTIONS] [NAME]
```

Creates a new app in `./NAME`. When stdin and stdout are both terminals and neither `--json` nor `--yes` is given, a wizard asks for everything the flags did not answer. Otherwise the flags alone decide, and anything not given takes its default: this is the mode for scripts and agents.

| Argument or flag | Default (flag mode) | Effect |
|---|---|---|
| `NAME` | required in flag mode | App name, also the Worker and D1 database name: lowercase letters, digits and dashes, starting with a letter, not ending with a dash, at most 63 characters |
| `--api` | off | API-only app: JSON endpoints, no HTML templates, no askama; Ocre's default features (`html`) are off |
| `--full-stack` | on | HTML pages with askama and htmx, plus JSON APIs when generated. `--api` and `--full-stack` override each other; the last one wins |
| `--starter <STARTER>` | `empty` | `empty`: home page (status endpoint in API mode) only. `blog`: adds a `Post` resource (`title:string body:text published:boolean`) with CRUD pages, or a JSON API in an API-only app |
| `--login` / `--no-login` | no login | Log in to Cloudflare (opens a browser) if not logged in yet |
| `--account-id <ACCOUNT_ID>` | none | Cloudflare account to deploy to, written as `account_id` into `wrangler.toml`. Required with `--login`/`--deploy` when the login has several accounts |
| `--git` / `--no-git` | no git | Run `git init` in the new app |
| `--deploy` / `--no-deploy` | no deploy | Deploy right after creating the app; implies `--login` |
| `-y`, `--yes` | off | Never prompt, even in a terminal |
| `--ocre-path <OCRE_PATH>` | git dependency | Use a local checkout of the `ocre` crate (`crates/ocre` of the Ocre repository) instead of `git = "https://github.com/tgeselle/ocre.rs"` |

What it does, in flag mode:

1. Checks the name and that `./NAME` does not exist, resolves `--ocre-path`, and checks that `git` runs when `--git` is given. Nothing is written if any check fails.
2. With `--login` or `--deploy`: runs `wrangler whoami --json`, runs `wrangler login` if not logged in, and picks the account (`--account-id` must be one of the login's accounts; with one account none is needed).
3. Writes the app: `Cargo.toml`, `wrangler.toml`, `rust-toolchain.toml`, `.gitignore`, `AGENTS.md`, `migrations/.gitkeep`, `public/robots.txt`, `src/lib.rs`, and in full-stack apps `templates/layout.html` and `templates/home.html`. An API-only app's `Cargo.toml` has `ocre = { ..., default-features = false }`, no askama, and `[package.metadata.ocre] mode = "api"`, which generators read.
4. Writes `.dev.vars` (git-ignored) with a new random `SECRET_KEY_BASE` and `MAIL_ADAPTER=log`, used by `ocre dev` only.
5. With `--starter blog`: runs the equivalent of `ocre g scaffold Post title:string body:text published:boolean` (`ocre g api` in an API-only app).
6. With `--git`: runs `git init --quiet`.
7. With `--deploy`: runs the same steps as [`ocre deploy`](#ocre-deploy), without the locale check.

The wizard asks, in order: the app name, "What are you building?" (full-stack or API only), "Pick a starter", then checks the Cloudflare login and offers it ("Log in now" or "Later"), asks which account when the login has several, "Initialize a git repository?" (default yes), and "Deploy it now?" (default yes, only when logged in). Wrangler's output is hidden behind a spinner and included in error messages. Each question is skipped when its flag was given. When the user chose not to log in, `ocre login` is added to the next steps. Esc or Ctrl-C cancels with the error `cancelled`.

Example, flag mode:

```sh
ocre new blog --starter blog --yes
```

```text
  create  blog/Cargo.toml
  create  blog/wrangler.toml
  create  blog/rust-toolchain.toml
  create  blog/.gitignore
  create  blog/AGENTS.md
  create  blog/migrations/.gitkeep
  create  blog/public/robots.txt
  create  blog/src/lib.rs
  create  blog/templates/layout.html
  create  blog/templates/home.html
  create  blog/.dev.vars
  create  blog/src/models/mod.rs
  create  blog/migrations/0001_create_posts.sql
  create  blog/src/models/post.rs
  create  blog/src/posts.rs
  create  blog/templates/posts/index.html
  create  blog/templates/posts/show.html
  create  blog/templates/posts/new.html
  create  blog/templates/posts/edit.html
  create  blog/templates/posts/_form.html

Next:
  cd blog
  ocre dev
  ocre deploy
```

(The example ran with `--ocre-path` pointing at a local checkout, which only changes the `ocre` line of `Cargo.toml`.)

An API-only app, with `--json`:

```sh
ocre new shop --api --json
```

```json
{"command":"new","created":["shop/Cargo.toml","shop/wrangler.toml","shop/rust-toolchain.toml","shop/.gitignore","shop/AGENTS.md","shop/migrations/.gitkeep","shop/public/robots.txt","shop/src/lib.rs","shop/.dev.vars"],"next":["cd shop","ocre dev","ocre deploy"],"ok":true}
```

Creating and deploying in one command (fake wrangler; the account has two Cloudflare accounts, hence `--account-id`):

```sh
ocre new two --deploy --account-id def456 --git
```

```text
Uploaded app
  https://app.example.workers.dev
Migrations applied to two (--remote)
  create  two/Cargo.toml
  ...
  create  two/.dev.vars
Logged in to Cloudflare as ada@example.com
Created the SECRET_KEY_BASE secret on Cloudflare

https://app.example.workers.dev

Next:
  cd two
  ocre dev
```

The same with `--json` prints wrangler's lines on stderr and this on stdout:

```json
{"command":"new","created":["three/Cargo.toml","three/wrangler.toml","three/rust-toolchain.toml","three/.gitignore","three/AGENTS.md","three/migrations/.gitkeep","three/public/robots.txt","three/src/lib.rs","three/templates/layout.html","three/templates/home.html","three/.dev.vars"],"email":"ada@example.com","next":["cd three","ocre dev"],"ok":true,"secret_created":true,"url":"https://app.example.workers.dev"}
```

Errors:

| Error | Hint |
|---|---|
| `missing app name` | ``run `ocre new <name>`, or run `ocre new` in a terminal for the guided setup`` |
| ``invalid app name `Bad_Name` `` | ``use lowercase letters, digits and dashes, starting with a letter (max 63), e.g. `my-blog` `` |
| `` `<path>` already exists `` | `choose another name or remove the directory` |
| `--ocre-path <path>: <OS error>` | ``pass the directory of the `ocre` crate (crates/ocre in the Ocre repository)`` |
| `git is not installed` | ``install git, or create the app without `--git` `` |
| `` `git init` failed (<status>) `` | none |
| `Cloudflare login did not complete` | ``run `ocre login` and approve access in the browser, or set CLOUDFLARE_API_TOKEN`` |
| `this Cloudflare login has several accounts` | `pass --account-id with one of: abc123 (Ada), def456 (Work)` (the login's accounts) |
| ``account `zzz` is not available to this Cloudflare login`` | `use one of: abc123 (Ada), def456 (Work)` |
| `this Cloudflare login has no accounts` | ``create an account at https://dash.cloudflare.com/sign-up, then run `ocre login` again`` |
| `cancelled` (wizard only) | ``run `ocre new <name>` with flags (see `ocre new --help`) to skip the questions`` |

With `--deploy`, the errors of [ocre deploy](#ocre-deploy) apply too. The app directory is already written when a deploy fails; fix the cause, `cd` into it and run `ocre deploy`.

## ocre login

```text
ocre login [--json]
```

Logs in to Cloudflare. It runs `wrangler whoami --json`; when not logged in, it runs `wrangler login`, which opens a browser to approve access, then checks `wrangler whoami --json` again. Already logged in, it only prints the login. It takes no flags besides `--json`.

Example (fake wrangler, not logged in yet; `Successfully logged in.` is wrangler's line):

```sh
ocre login
```

```text
Successfully logged in.
Logged in to Cloudflare as ada@example.com
```

```sh
ocre login --json
```

```json
{"command":"login","email":"ada@example.com","ok":true}
```

`email` is absent when the session has none (an API token). Errors: `Cloudflare login did not complete` (hint: ``run `ocre login` and approve access in the browser, or set CLOUDFLARE_API_TOKEN``), `` unexpected `wrangler whoami --json` output: ... ``, and the shared wrangler errors.

## ocre generate

```text
ocre generate <GENERATOR> [ARGS]...
ocre g <GENERATOR> [ARGS]...
```

`g` is an alias. The generators are `model`, `scaffold`, `api`, `auth`, `migration`, `mailer`, `mailbox`, `job`, `schedule`, `cache` and `locale`; each one is documented with its arguments, files, output and errors in [Generators](generators.md).

## ocre migrate

```text
ocre migrate [--remote] [--status] [--json]
```

Applies the pending SQL files of `migrations/` to the app's D1 database, in file-name order, with `wrangler d1 migrations apply <database> --local` (or `--remote`). Wrangler records applied migrations in the database's `d1_migrations` table, so each file runs once.

| Flag | Default | Effect |
|---|---|---|
| `--remote` | local | Use the production database on Cloudflare instead of the local one in `.wrangler/state/v3/d1` |
| `--status` | off | List pending migrations instead of applying them (see below) |

Example, local:

```sh
ocre migrate
```

```text
...
Migrations to be applied:
┌───────────────────────┐
│ name                  │
├───────────────────────┤
│ 0001_create_posts.sql │
└───────────────────────┘
? About to apply 1 migration(s)
Your database may not be available to serve requests during the migration, continue?
...
```

All of it is wrangler's output (wrangler answers its own question with its non-interactive default, yes). With `--json`, wrangler's output goes to stderr and stdout gets:

```json
{"command":"migrate","ok":true}
```

The object is the same with `--remote`: the success report of `ocre migrate` has no `remote` key.

### ocre migrate --status

Runs `wrangler d1 migrations list <database> --local` (or `--remote`), shows wrangler's table, and reads the pending file names from it.

```sh
ocre migrate --status
```

```text
...
Migrations to be applied:
┌───────────────────────┐
│ Name                  │
├───────────────────────┤
│ 0001_create_posts.sql │
└───────────────────────┘

Next:
  ocre migrate
```

```sh
ocre migrate --status --json
```

```json
{"command":"migrate","next":["ocre migrate"],"ok":true,"pending":["0001_create_posts.sql"]}
```

With nothing pending, wrangler prints `No migrations to apply!` and the report is `{"command":"migrate","ok":true}`: no `pending`, no `next`.

### ocre migrate --remote

Applies (or with `--status`, lists) migrations on the production D1 database. It needs a Cloudflare login and a database that exists: the first [`ocre deploy`](#ocre-deploy) creates it, and every deploy applies remote migrations itself, so `ocre migrate --remote` is only needed to migrate without deploying. With `--status`, the human output ends with `Target: remote D1 database on Cloudflare` and the JSON has `"remote": true` (fake wrangler):

```json
{"command":"migrate","next":["ocre migrate --remote"],"ok":true,"pending":["0002_create_comments.sql"],"remote":true}
```

Errors: the shared ones. A failing migration is reported as `` `wrangler d1 migrations apply blog --remote` failed (exit status: 1) `` with wrangler's error above it.

## ocre db seed

```text
ocre db seed [--remote] [--json]
```

Runs `db/seeds.sql` with `wrangler d1 execute <database> --file db/seeds.sql --local --yes` (`--remote` for production). `--yes` answers wrangler's "database unavailable during import" question, so remote seeding never waits for input. The file is plain SQL, typically `INSERT` statements; it is not tracked, so running it twice inserts the rows twice.

| Flag | Default | Effect |
|---|---|---|
| `--remote` | local | Seed the production database on Cloudflare |

```sql
-- db/seeds.sql
INSERT INTO posts (title, body, published) VALUES ('Hello', 'First post', 1);
INSERT INTO posts (title, body, published) VALUES ('Draft', 'Not yet', 0);
```

```sh
ocre db seed
```

```text
...
[
  {
    "results": [],
    "success": true,
    "meta": {
      "duration": 0
    }
  },
  ...
]
  loaded db/seeds.sql (--local)
```

```sh
ocre db seed --json
```

```json
{"command":"db seed","ok":true,"ran":["loaded db/seeds.sql (--local)"]}
```

With `--remote`, `ran` is `["loaded db/seeds.sql (--remote)"]`, the JSON has `"remote": true` and the human output ends with `Target: remote D1 database on Cloudflare`.

Errors: `db/seeds.sql not found in the app` (hint: ``create db/seeds.sql with INSERT statements, then run `ocre db seed` ``), and the shared ones.

## ocre db reset

```text
ocre db reset [--json]
```

Local only. Deletes `.wrangler/state/v3/d1` (the local D1 databases) when it exists, applies every migration with `wrangler d1 migrations apply <database> --local`, then loads `db/seeds.sql` when the file exists. It takes no `--remote`: production data is never reset.

```sh
ocre db reset
```

```text
...
  deleted .wrangler/state/v3/d1
  applied migrations (--local)
  loaded db/seeds.sql (--local)
```

```sh
ocre db reset --json
```

```json
{"command":"db reset","ok":true,"ran":["deleted .wrangler/state/v3/d1","applied migrations (--local)","loaded db/seeds.sql (--local)"]}
```

`ran` lists only the steps that happened: no `deleted ...` without a local database, no `loaded ...` without seeds.

## ocre sql

```text
ocre sql [--remote] [--json] <QUERY>
```

Runs one or more SQL statements separated by `;` with `wrangler d1 execute <database> --command <QUERY> --local --json` (or `--remote`) and prints each statement's rows. Quote the query for the shell.

| Argument or flag | Default | Effect |
|---|---|---|
| `QUERY` | required | SQL statements separated by `;` |
| `--remote` | local | Run on the production database on Cloudflare |

Wrangler's output is captured, not printed, in both modes: the human output is one table per statement, each followed by its row count, with `NULL` for null values.

```sh
ocre sql "SELECT id, title, published FROM posts"
```

```text
id | title | published
---+-------+----------
1  | Hello | 1
2  | Draft | 0
(2 rows)
```

```sh
ocre sql "SELECT COUNT(*) AS n FROM posts; SELECT * FROM posts WHERE id = 99"
```

```text
n
-
2
(1 row)

(0 rows)
```

With `--json`, `rows` is wrangler's JSON unchanged, one object per statement:

```sh
ocre sql "SELECT id, title FROM posts WHERE published = 1; SELECT COUNT(*) AS n FROM posts" --json
```

```json
{"command":"sql","ok":true,"rows":[{"meta":{"duration":1},"results":[{"id":1,"title":"Hello"},{"id":3,"title":"Hello"}],"success":true},{"meta":{"duration":0},"results":[{"n":4}],"success":true}]}
```

With `--remote`, the JSON has `"remote": true` and the human output ends with `Target: remote D1 database on Cloudflare`.

Errors: a failing statement is reported with wrangler's captured output in the message:

```sh
ocre sql "SELECT * FROM nope" --json
```

```json
{"error":"`wrangler d1 execute demo --command SELECT * FROM nope --local --json` failed (exit status: 1):\n\n{\n  \"error\": {\n    \"text\": \"no such table: nope: SQLITE_ERROR\"\n  }\n}","hint":"the wrangler output above names the cause","ok":false}
```

Also `` unexpected `wrangler d1 execute --json` output: ... `` when wrangler's output is not the expected JSON, and the shared errors.

## ocre dev

```text
ocre dev [--port <PORT>] [--json]
```

Runs the app locally, at `http://localhost:<PORT>`, until stopped (Ctrl-C).

| Flag | Default | Effect |
|---|---|---|
| `--port <PORT>` | `8787` | Port of the local server |

Steps:

1. Checks the locale files (see [ocre i18n missing](#ocre-i18n-missing)); a file the Worker could not load stops here.
2. Checks that rustc has the `wasm32-unknown-unknown` target.
3. Applies local migrations (`wrangler d1 migrations apply <database> --local`).
4. Runs `wrangler dev --port <PORT>`. Wrangler builds the app with the `[build]` command of `wrangler.toml`; `ocre dev` sets `OCRE_BUILD=--dev`, so `worker-build` makes an unoptimized build, much faster to compile than the `--release` build of deploys. The first build compiles every dependency (about a minute); later ones are incremental. Local D1, R2, KV, Queues and Durable Objects are simulated by wrangler under `.wrangler/state`, and `.dev.vars` provides the local secrets.

Abridged output of a real run on a new blog app:

```sh
ocre dev --port 8943
```

```text
...
[custom build] Running: cargo install -q "worker-build@^0.8" && worker-build ${OCRE_BUILD:---release}
...
[custom build]     Finished `dev` profile [unoptimized + debuginfo] target(s) in 37.28s
...
Using secrets defined in .dev.vars
Your Worker has access to the following bindings:
Binding                                                 Resource                  Mode
env.DB (demo)                                           D1 Database               local
env.MAIL_FROM ("demo <noreply@example.com>")            Environment Variable      local
env.SECRET_KEY_BASE ("(hidden)")                        Environment Variable      local
env.MAIL_ADAPTER ("(hidden)")                           Environment Variable      local
...
[wrangler:info] Ready on http://localhost:8943
```

The report is printed after `wrangler dev` exits successfully; with `--json` it is:

```json
{"command":"dev","ok":true,"url":"http://localhost:8787"}
```

and every line above goes to stderr. Errors: the shared ones (locale files, wasm target, wrangler). `` `wrangler dev --port 8787` failed `` usually means the build failed: the compiler errors are in the output above it.

## ocre deploy

```text
ocre deploy [--json]
```

Builds the app in release mode and deploys it to Cloudflare as the Worker named in `wrangler.toml`, creating what it needs on the way. It needs a Cloudflare login ([`ocre login`](#ocre-login)) or `CLOUDFLARE_API_TOKEN` (with `CLOUDFLARE_ACCOUNT_ID` when the token reaches several accounts). Steps, in order:

1. Checks the locale files and the wasm target, like `ocre dev`.
2. Queues: for each queue `wrangler.toml` names (`[[queues.producers]]` `queue`, `[[queues.consumers]]` `queue` and `dead_letter_queue`), runs `wrangler queues info <name>` and, when the queue does not exist, `wrangler queues create <name>`. A consumer of a missing queue would fail the deploy.
3. KV namespaces: for each `[[kv_namespaces]]` entry without an `id` (the `CACHE` binding added by `ocre g cache`), finds the namespace titled `<worker>-<binding>` (lowercase, `_` as `-`, e.g. `blog-cache`) in `wrangler kv namespace list`, creates it when missing, and writes its `id` into `wrangler.toml`. Commit that change: later deploys reuse the namespace.
4. R2 buckets: for each `bucket_name` of `[[r2_buckets]]` (the `STORAGE` bucket added by the first `attachment` field), runs `wrangler r2 bucket info <name> --json` and creates the bucket when missing.
5. `SECRET_KEY_BASE`: runs `wrangler secret list --format json`. When the Worker does not exist yet or has no `SECRET_KEY_BASE`, a new random value is written to `.wrangler/ocre-secrets.env` (mode 0600, deleted afterwards) and uploaded with `wrangler deploy --secrets-file`. An existing secret is never replaced (that would sign every user out); any other failure of `secret list` stops the deploy rather than risk it.
6. Database: when `wrangler d1 list --json` has the database, applies remote migrations first, then runs `wrangler deploy`, so the new code never runs on the old schema. When it does not exist, runs `wrangler deploy` first (which creates the D1 database), then applies the migrations.
7. Reports the `https://...workers.dev` URL found in wrangler's output.

`wrangler deploy` builds with `OCRE_BUILD=--release`. Durable Object namespaces (realtime channels) need no step: wrangler creates them from the `[[migrations]]` of `wrangler.toml`.

First deploy of an app with a job, a cache, attachments (fake wrangler; the lines before `Created the SECRET_KEY_BASE secret` are wrangler's):

```sh
ocre deploy
```

```text
Created queue blog-jobs
Created queue blog-jobs-failed
Creating namespace with title "blog-cache"
Created bucket 'blog-storage' with default storage class of Standard.
Uploaded app
  https://app.example.workers.dev
Migrations applied to blog (--remote)
Created the SECRET_KEY_BASE secret on Cloudflare
Created queue blog-jobs on Cloudflare
Created queue blog-jobs-failed on Cloudflare
Created KV namespace blog-cache (id written to wrangler.toml) on Cloudflare
Created R2 bucket blog-storage on Cloudflare

https://app.example.workers.dev
```

The same first deploy with `--json`:

```json
{"command":"deploy","ok":true,"provisioned":["queue blog-jobs","queue blog-jobs-failed","KV namespace blog-cache (id written to wrangler.toml)","R2 bucket blog-storage"],"secret_created":true,"url":"https://app.example.workers.dev"}
```

A later deploy, with everything in place:

```json
{"command":"deploy","ok":true,"url":"https://app.example.workers.dev"}
```

Free-plan note (September 2026): R2 must be enabled once in the Cloudflare dashboard, which asks for a payment method even for use within the free tier of 10 GB stored, 1M writes and 10M reads a month ([R2 pricing](https://developers.cloudflare.com/r2/pricing/)). `ocre deploy` detects an account without R2 (API code 10042) and says so in its hint.

Errors (besides the shared ones):

| Error | Hint |
|---|---|
| `` `wrangler queues info <queue>` failed: ... `` | ``log in with `ocre login`, or set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID`` |
| `` `wrangler kv namespace list` failed: ... `` | same |
| `KV namespace <title> was created but is not listed` | ``run `ocre deploy` again; it links the namespace once Cloudflare lists it`` |
| `` `wrangler r2 bucket info <bucket>` failed: ... `` | R2 not enabled (code 10042): ``enable R2 once in the Cloudflare dashboard (Storage & databases > R2; the free plan asks for a payment method but charges nothing within 10 GB, 1M writes and 10M reads a month), then run `ocre deploy` again``; otherwise the login hint above |
| `` `wrangler secret list` failed: ... `` | ``log in with `ocre login`, or set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID; Ocre only creates SECRET_KEY_BASE when sure the Worker has none`` |
| `` `wrangler d1 list` failed: ... `` | the login hint above |
| `` `wrangler deploy` failed (exit status: 1) `` | `read the wrangler output above; the first error line names the cause` |
| `` unexpected `wrangler ...` output: ... `` | none |

Resources created before a failure stay created; running `ocre deploy` again skips them.

## ocre secret

```text
ocre secret [--json]
```

Prints a new random value for `SECRET_KEY_BASE`, like `rails secret`: 64 bytes from the operating system's random number generator, as 128 lowercase hex characters. It works anywhere (no app needed) and writes nothing. `ocre new` already puts one in `.dev.vars` and the first `ocre deploy` uploads one; use `ocre secret` to replace a leaked secret (which signs every user out):

```sh
ocre secret | npx wrangler secret put SECRET_KEY_BASE
```

```sh
ocre secret
```

```text
951f4f0c57883adb8f82c2f6f1b63b21001acb6427c56a6e8ac3b471545d79da61252e7027db0b7a72b13c64bdaa7e949d2ceb9ccf85cbf0a4f5a9be35fc08d7
```

```sh
ocre secret --json
```

```json
{"command":"secret","ok":true,"secret":"f40f8e050bae938dc4b2475e7ac01e92be3bb8f6bf3d08b9d674efea068ec69e75e915ad156b3d61a6596d6953c81ec73313167965fde14b17720434301625ba"}
```

See [SECRET_KEY_BASE](configuration.md#secret_key_base).

## ocre routes

```text
ocre routes [FILTER] [--json]
```

Lists the app's HTTP routes without building it, like `rails routes`. It reads `src/lib.rs`, finds `.route("<path>", get(handler).post(handler)...)` calls, follows each `.merge(<module>::routes())` into `src/<module>.rs` (or `src/<module>/mod.rs`), and knows that `ocre::graphql::routes(...)` serves `GET` and `POST /graphql`. It is a tolerant scanner, not a Rust parser: routes built another way (closures as handlers, routers from functions not named `routes`, macros) are skipped. Routes are sorted by path, then by method (`GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, `OPTIONS`).

| Argument | Default | Effect |
|---|---|---|
| `FILTER` | none | Keep routes whose method, path or handler contains this text, case-insensitive |

```sh
ocre routes comments
```

```text
METHOD  PATH                   HANDLER
GET     /comments              comments::index
POST    /comments              comments::create
GET     /comments/new          comments::new
GET     /comments/{id}         comments::show
POST    /comments/{id}         comments::update
POST    /comments/{id}/delete  comments::delete
GET     /comments/{id}/edit    comments::edit
```

```sh
ocre routes comments --json
```

```json
{"command":"routes","ok":true,"routes":[{"handler":"comments::index","method":"GET","path":"/comments"},{"handler":"comments::create","method":"POST","path":"/comments"},{"handler":"comments::new","method":"GET","path":"/comments/new"},{"handler":"comments::show","method":"GET","path":"/comments/{id}"},{"handler":"comments::update","method":"POST","path":"/comments/{id}"},{"handler":"comments::delete","method":"POST","path":"/comments/{id}/delete"},{"handler":"comments::edit","method":"GET","path":"/comments/{id}/edit"}]}
```

The filter is a substring match on all three columns: `ocre routes up` also returns `/signup` and every `::update` handler. With no match, the human output is `No routes.` and the JSON has `"routes": []`. Error: `cannot read src/lib.rs: ...` (hint: ``` `ocre routes` reads the router built in src/lib.rs; run it inside an Ocre app ```).

## ocre i18n missing

```text
ocre i18n missing [--json]
```

Checks the translation files set up by [`ocre g locale`](generators.md#ocre-g-locale). It reads the codes declared in `ocre::locales!(...)` in `src/lib.rs` (the first is the default locale) and parses `locales/<code>.yml` with the same parser the Worker uses. It fails when any of these is found:

- a declared locale without its file (a compile error in the app);
- a file in `locales/` that `src/lib.rs` does not declare (never loaded);
- a file the Worker could not load, with the line number;
- a key of the default locale missing from another locale, including the plural forms that language needs (`one`/`other`, plus `few`/`many`... for some languages).

```sh
ocre i18n missing
```

```text
  every locale (en, fr) has every key of locales/en.yml
```

```sh
ocre i18n missing --json
```

```json
{"command":"i18n missing","ok":true,"ran":["every locale (en, fr) has every key of locales/en.yml"]}
```

A failing check (exit code 1):

```text
error: 3 locale problems:
  locales/de.yml: not declared; add "de" to ocre::locales!(...) in src/lib.rs
  locales/fr.yml: missing app.posts.one
  locales/fr.yml: missing app.posts.other
hint: add each missing key at the same path as in locales/en.yml, translated (plural keys need the forms listed); then run `ocre i18n missing` again
```

An invalid file is reported with its line, e.g. ``locales/fr.yml:6: quote values starting with `%`, e.g. "%oops"``. In an app without translations the error is `src/lib.rs declares no locales (ocre::locales!(...) not found)` with the hint ``run `ocre g locale en` to set up translations, then `ocre g locale fr` for each other language``.

`ocre dev` and `ocre deploy` run the loading part of this check (missing files, invalid files) but not the missing-keys part: at runtime, a missing key falls back to the default locale in release builds (see [Translations](../guides/i18n.md)).

## ocre help

```text
ocre help [COMMAND]...
ocre <COMMAND> --help
```

Prints the help of `ocre` or of a command (`ocre help db seed`, `ocre g model --help`). `-h` prints a shorter summary where the long help has examples. The help of `ocre g model` lists the field types.

## ocre --version

```text
ocre --version
ocre -V
```

```text
ocre 0.1.0
```

## See also

- [Generators](generators.md): every `ocre g` generator.
- [Configuration](configuration.md): `wrangler.toml`, `.dev.vars`, secrets and variables.
- [Deployment](../guides/deployment.md): deploying, custom domains, production data.
- [Models and migrations](../guides/models.md): writing migrations and seeds.
- [Free-plan limits](limits.md).
