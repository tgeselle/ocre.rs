# CLI commands

This page documents every `ocre` command and flag, what each one does step by step, its human and `--json` output, and the errors it reports with their hints. Code generators (`ocre g ...`) have their own page, [Generators](generators.md).

## Before you start

- The `ocre` CLI, installed with `cargo install --git https://github.com/tgeselle/ocre.rs ocre-cli` (see [Installation](../getting-started/installation.md)).
- Node.js 22 or newer, and the app's npm packages (`npm install`, which `ocre new` runs): commands that touch Cloudflare or the dev server run the app's Cloudflare [`cf` CLI](https://www.npmjs.com/package/cf) (`node_modules/.bin/cf`, pinned in `package.json`; outside an app, `npx --yes cf@1.0.0-beta.5`), and local database commands run the app's own wrangler (see [Why wrangler still appears](../guides/deployment.md#why-wrangler-still-appears)).
- A rustup toolchain with the `wasm32-unknown-unknown` target for `ocre dev` and `ocre deploy`.
- Except `ocre new`, `ocre login`, `ocre secret`, `ocre version`, `ocre doctor` and `ocre help`, commands run inside an Ocre app: the CLI walks up from the current directory to the nearest `cloudflare.config.ts`, and reads the `name` of its `DB: bindings.d1({ name })` entry (see [Configuration](configuration.md#cloudflareconfigts)). A directory with only a `wrangler.toml` (an app made by an older Ocre) is refused with a hint pointing to [Upgrading from wrangler.toml](../guides/upgrading.md).
- Commands with `--remote`, `ocre login` and `ocre deploy` need a Cloudflare account (free) and a login (`ocre login`) or the `CLOUDFLARE_API_TOKEN` environment variable that cf reads (plus `CLOUDFLARE_ACCOUNT_ID` when the token sees several accounts).

The examples below come from `ocre 0.1.0` on apps created by `ocre new ... --starter blog`; the lines printed by wrangler come from wrangler 4.143.0 (local database commands, and `cf dev`, which delegates to it). Commands that need Cloudflare (`login`, `deploy`, `--remote`, `new --login/--deploy`) are shown with the fake cf of the CLI's integration tests, so the lines cf prints there are the fake's, while the lines and JSON printed by `ocre` itself are Ocre's.

## Summary

| Command | What it does |
|---|---|
| [`ocre new [NAME]`](#ocre-new) | Creates an app in `./NAME`; in a terminal, asks for anything flags did not answer; `--template` applies an application template |
| [`ocre login`](#ocre-login) | Logs in to Cloudflare in the browser, unless already logged in |
| [`ocre generate` / `ocre g`](#ocre-generate) | Generates code: see [Generators](generators.md); `--pretend`, `--force`, `--skip` |
| [`ocre destroy` / `ocre d`](#ocre-destroy) | Undoes a recorded generator run |
| [`ocre template SOURCE`](#ocre-template) | Applies an application template (a file or URL of ocre commands) to the app |
| [`ocre migrate`](#ocre-migrate) | Applies D1 migrations (local unless `--remote`); `--status` lists pending ones |
| [`ocre db create`](#ocre-db-create) | Creates the local database, or with `--remote` the D1 database on Cloudflare |
| [`ocre db prepare`](#ocre-db-prepare) | Local, safe to repeat: applies pending migrations, seeds a new database |
| [`ocre db seed`](#ocre-db-seed) | Loads `db/fixtures`, then runs `db/seeds.sql` (local unless `--remote`); `--replant` empties the tables first |
| [`ocre db reset`](#ocre-db-reset) | Local only: deletes the local database, applies every migration, loads fixtures and seeds |
| [`ocre db drop`](#ocre-db-drop) | Local only: deletes the local database |
| [`ocre db truncate`](#ocre-db-truncate) | Local only: deletes every row, keeps tables and migrations |
| [`ocre db version`](#ocre-db-version) | Prints the last applied migration |
| [`ocre db schema`](#ocre-db-schema) | Writes the database's `CREATE` statements to `db/schema.sql` |
| [`ocre db dump`](#ocre-db-dump) | Writes table rows to fixture files `db/fixtures/<table>.yml` |
| [`ocre sql QUERY`](#ocre-sql) | Runs SQL on D1 and prints the rows |
| [`ocre dev`](#ocre-dev) | Applies local migrations, then runs the app with `cf dev` |
| [`ocre test`](#ocre-test) | Runs `cargo test`, the wasm32 check and, with `--e2e`, the request tests, `tests/e2e.sh` and the browser tests against a local server |
| [`ocre deploy`](#ocre-deploy) | Creates missing Cloudflare resources, applies remote migrations, then runs `cf deploy` |
| [`ocre logs`](#ocre-logs) | Streams the deployed Worker's live logs (`wrangler tail`) |
| [`ocre secret`](#ocre-secret) | Prints a new random value for `SECRET_KEY_BASE` |
| [`ocre secrets list` / `push` / `fetch`](#ocre-secrets) | Lists secret names locally and on the Worker; uploads values from a git-ignored file; prints one local value |
| [`ocre domains [add\|remove HOST]`](#ocre-domains) | The Worker's custom domains in `cloudflare.config.ts` |
| [`ocre routes [FILTER]`](#ocre-routes) | Lists the app's HTTP routes, read from its source |
| [`ocre schedules [run TASK]`](#ocre-schedules) | Lists the Cron Triggers and their tasks; `run` fires one on `ocre dev` |
| [`ocre i18n missing`](#ocre-i18n-missing) | Checks the locale files; fails on missing keys or invalid files |
| [`ocre doctor`](#ocre-doctor) | Checks the tools and the app's setup (production settings, `.ocre/doctor/` checks); fails when a check fails |
| [`ocre ci`](#ocre-ci) | Runs the CI steps locally (fmt, clippy, tests, wasm32 check, translations); `--signoff` runs `gh signoff` |
| [`ocre about`](#ocre-about), [`ocre version`](#ocre-version) | Versions and the app's configuration |
| [`ocre stats [DIRS]`](#ocre-stats) | Lines of code per part of the app |
| [`ocre notes`](#ocre-notes) | Lists TODO, FIXME and OPTIMIZE comments |
| [`ocre time-zones`](#ocre-time-zones) | Prints the IANA time zone names |
| [`ocre help`](#ocre-help) | Help text |

## Global flag: --json

`--json` is accepted by every command, before or after the subcommand (`ocre --json routes` and `ocre routes --json` are the same). It is the contract for scripts and AI agents:

- stdout carries exactly one JSON object, on one line, printed when the command ends.
- The command never prompts (`ocre new` skips its wizard).
- Output of the tools the CLI runs (cf, wrangler, npm, and cargo through the build) goes to stderr instead of stdout.

On success the object has `"ok": true`, `command`, and only the keys that apply (empty lists, `false` flags and absent values are omitted):

| Key | Type | Set by | Meaning |
|---|---|---|---|
| `ok` | boolean | every command | `true` |
| `command` | string | every command | The command's name, e.g. `new`, `migrate`, `db seed`, `secrets list`, `schedules run`, `destroy`, `doctor`, or `generate <generator>` (`generate scaffold`; `generate custom` for app generators) |
| `created` | string[] | `new`, generators, `db dump` | Files created, relative to the app root (to the current directory for `ocre new`, so they start with the app name) |
| `updated` | string[] | generators, `destroy`, `template`, `db schema`, `db dump --force` | Existing files changed |
| `skipped` | string[] | generators (`--skip`), `destroy` | Existing files kept: by `--skip`, or left changed by `destroy` (Cargo.toml, cloudflare.config.ts, package.json, changed lines) |
| `removed` | string[] | `destroy` | Files deleted, the generation record last |
| `pretend` | `true` | generators, `destroy` | `--pretend`: nothing was written |
| `templates` | object[] | `generate override` | `{"path", "overridden"}` per generator template |
| `about` | object | `version`, `about` | `cli`, `app`, `app_version`, `ocre`, and for `about` also `mode`, `rust_toolchain`, `compatibility_date`, `bindings`, `vars`, `features` |
| `checks` | object[] | `doctor` | `{"name", "status", "detail", "hint"}`, `status` being `ok`, `warn` or `fail` |
| `stats` | object | `stats` | `rows` (`name`, `files`, `lines`, `loc`, `functions`), `code_loc`, `test_loc` |
| `notes` | object[] | `notes` | `{"path", "line", "tag", "text"}` |
| `version` | string or `null` | `db version` | Last applied migration file, `null` when none is |
| `secrets` | object[] | `secrets list` | `{"name", "local", "deployed"}` |
| `schedules` | object[] | `schedules` | `{"cron", "task"}` |
| `url` | string | `dev`, `deploy`, `new --deploy` | `http://localhost:<port>`, or the `https://....workers.dev` URL found in `cf deploy`'s output |
| `email` | string | `login`, `new --login`, `new --deploy` | Email of the Cloudflare login |
| `pending` | string[] | `migrate --status` | Migration files not applied yet |
| `ran` | string[] | `new`, `db seed`, `db reset`, `db dump`, `i18n missing` | Steps performed, in order |
| `rows` | array | `sql` | D1's JSON: one object per statement, with `results` (the rows), `success` and `meta` |
| `routes` | object[] | `routes` | `{"method", "path", "handler"}`, sorted by path then method |
| `remote` | `true` | `migrate --status --remote`, `db seed --remote`, `db dump --remote`, `sql --remote` | The command used the production database |
| `secret` | string | `secret` | 128 lowercase hex characters |
| `secret_created` | `true` | `deploy`, `new --deploy` | The deploy uploaded a new `SECRET_KEY_BASE` because the Worker had none |
| `provisioned` | string[] | `deploy` | Cloudflare resources created because they were missing, e.g. `D1 database blog`, `queue blog-jobs` |
| `next` | string[] | `new`, `migrate --status`, generators, `destroy`, db tasks, `secrets list` | Commands or actions to run next, in order |

On failure the object is `{"ok": false, "error": "...", "hint": "..."}`. `hint` names the fix; it is `null` for the few errors without one (for example I/O errors). `ocre doctor` and `ocre test` report failed checks the same way, with the report's keys (`checks`, `ran`) alongside.

```json
{"error":"no cloudflare.config.ts found in this directory or its parents","hint":"run this command inside an Ocre app, or create one with `ocre new <name>`","ok":false}
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
| `no cloudflare.config.ts found in this directory or its parents` | ``run this command inside an Ocre app, or create one with `ocre new <name>` `` | Run outside an app |
| `<dir> uses wrangler.toml; Ocre now reads cloudflare.config.ts` | ``convert it: `npm install --save-dev --save-exact cf@1.0.0-beta.5 wrangler@4.144.0`, then `npx cf migrate --no-install`, then apply the Ocre fixes of the upgrading guide (...)`` | An app made by an older Ocre; see [Upgrading from wrangler.toml](../guides/upgrading.md) |
| ``cloudflare.config.ts has no D1 database bound to `DB` `` | ``add `DB: bindings.d1({ name: "<app>" }),` inside `worker.env` `` | The `DB` binding was removed or renamed |
| ``cloudflare.config.ts defines `<KEY>` in a form Ocre cannot read`` | the canonical entry to write, e.g. ``write `STORAGE: bindings.r2({ name: "<bucket>" }),` `` | A value Ocre needs is not a literal (see [Configuration](configuration.md#how-ocre-reads-and-edits-it)) |
| `the app's npm packages are not installed (node_modules/.bin/cf, node_modules/.bin/wrangler)` | ``run `npm install` in <app> (needs Node.js 22 or newer)`` | `ocre new --no-install`, or a fresh clone |
| `could not run cf: ...` (or `wrangler`) | ``install Node.js 22 or newer, then run `npm install` in the app`` | No Node.js on `PATH` |
| `` `cf <arguments>` failed (exit status: N) `` (or `` `wrangler <arguments>` ``) | `read the cf output above; the first error line names the cause` | The tool failed; its own error is on stderr just above |
| `` `cf <arguments>` failed: ┌ APIError ... `` | ``log in with `ocre login`, or set CLOUDFLARE_API_TOKEN (and CLOUDFLARE_ACCOUNT_ID when the token sees several accounts)`` | A Cloudflare API call made by Ocre (lists, creations) failed; cf's error box, with the API code, is in the message |
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
| `--login` / `--no-login` | no login | Log in to Cloudflare (`cf auth login`, opens a browser) if not logged in yet |
| `--account-id <ACCOUNT_ID>` | none | Cloudflare account to deploy to, written as `accountId: "<id>",` at the top of `cloudflare.config.ts`. Required with `--login`/`--deploy` when the login has several accounts |
| `--git` / `--no-git` | no git | Run `git init` in the new app |
| `--deploy` / `--no-deploy` | no deploy | Deploy right after creating the app; implies `--login` |
| `--no-install` | install | Skip `npm install` in the new app (for offline use); run `npm install` in it before `ocre dev`. Cannot be combined with `--deploy` |
| `-y`, `--yes` | off | Never prompt, even in a terminal |
| `--ocre-path <OCRE_PATH>` | git dependency | Use a local checkout of the `ocre` crate (`crates/ocre` of the Ocre repository) instead of `git = "https://github.com/tgeselle/ocre.rs"` |
| `-m`, `--template <TEMPLATE>` | none | Application template to apply once the app exists: a file or `https://` URL of ocre commands, one per line (see [ocre template](#ocre-template)). Every line is checked before the app is written |

What it does, in flag mode:

1. Checks the name and that `./NAME` does not exist, resolves `--ocre-path`, and checks that `git` runs when `--git` is given. Nothing is written if any check fails.
2. With `--login` or `--deploy`: runs `cf auth whoami`, runs `cf auth login` if not logged in, and picks the account (`--account-id` must be one of the login's accounts; with one account none is needed).
3. Writes the app: `Cargo.toml`, `cloudflare.config.ts` (with `accountId` when an account was picked), `wrangler.config.ts`, `package.json`, `tsconfig.json`, `rust-toolchain.toml`, `.gitignore`, `AGENTS.md`, `migrations/.gitkeep`, `public/robots.txt`, `src/lib.rs`, and in full-stack apps `templates/layout.html`, `templates/home.html` and `templates/error.html`. An API-only app's `Cargo.toml` has `ocre = { ..., default-features = false }`, no askama, and `[package.metadata.ocre] mode = "api"`, which generators read.
4. Writes `.dev.vars` (git-ignored) with a new random `SECRET_KEY_BASE` and `MAIL_ADAPTER=log`, used by `ocre dev` only.
5. With `--starter blog`: runs the equivalent of `ocre g scaffold Post title:string body:text published:boolean` (`ocre g api` in an API-only app).
6. With `--template`: runs the template's lines in the new app, like [`ocre template`](#ocre-template); the files they create are added to `created` and the lines to `ran`.
7. Unless `--no-install`: runs `npm install` in the app, which installs the pinned `cf`, `wrangler` and `typescript` into `node_modules/` and writes `package-lock.json` (commit it). `ran` gets `npm install (cf 1.0.0-beta.5, wrangler 4.144.0)`.
8. With `--git`: runs `git init --quiet`.
9. With `--deploy`: runs the same steps as [`ocre deploy`](#ocre-deploy), without the locale check.

The wizard asks, in order: the app name, "What are you building?" (full-stack or API only), "Pick a starter", then checks the Cloudflare login and offers it ("Log in now" or "Later"), asks which account when the login has several, "Initialize a git repository?" (default yes), and "Deploy it now?" (default yes, only when logged in). The output of cf and npm is hidden behind a spinner and included in error messages; with `--no-install`, the wizard does not offer to deploy. Each question is skipped when its flag was given. When the user chose not to log in, `ocre login` is added to the next steps. Esc or Ctrl-C cancels with the error `cancelled`.

Example, flag mode:

```sh
ocre new blog --starter blog --yes
```

```text
  create  blog/Cargo.toml
  create  blog/cloudflare.config.ts
  create  blog/wrangler.config.ts
  create  blog/package.json
  create  blog/tsconfig.json
  create  blog/rust-toolchain.toml
  create  blog/.gitignore
  create  blog/AGENTS.md
  create  blog/migrations/.gitkeep
  create  blog/public/robots.txt
  create  blog/src/lib.rs
  create  blog/templates/layout.html
  create  blog/templates/home.html
  create  blog/templates/error.html
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
  npm install (cf 1.0.0-beta.5, wrangler 4.144.0)

Next:
  cd blog
  ocre dev
  ocre deploy
```

(With `--ocre-path` pointing at a local checkout, which only changes the `ocre` line of `Cargo.toml`; `npm install`'s own output is not shown.)

An API-only app, with `--json` and without the npm install (run `npm install` in it before `ocre dev`; `ocre doctor` reminds you):

```sh
ocre new shop --api --no-install --json
```

```json
{"command":"new","created":["shop/Cargo.toml","shop/cloudflare.config.ts","shop/wrangler.config.ts","shop/package.json","shop/tsconfig.json","shop/rust-toolchain.toml","shop/.gitignore","shop/AGENTS.md","shop/migrations/.gitkeep","shop/public/robots.txt","shop/src/lib.rs","shop/.dev.vars"],"next":["cd shop","ocre dev","ocre deploy"],"ok":true}
```

Creating and deploying in one command (fake cf; the account has two Cloudflare accounts, hence `--account-id`):

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

The same with `--json` prints cf's lines on stderr and this on stdout:

```json
{"command":"new","created":["three/Cargo.toml","three/cloudflare.config.ts","three/wrangler.config.ts","three/package.json","three/tsconfig.json","three/rust-toolchain.toml","three/.gitignore","three/AGENTS.md","three/migrations/.gitkeep","three/public/robots.txt","three/src/lib.rs","three/templates/layout.html","three/templates/home.html","three/templates/error.html","three/.dev.vars"],"email":"ada@example.com","next":["cd three","ocre dev"],"ok":true,"ran":["npm install (cf 1.0.0-beta.5, wrangler 4.144.0)"],"secret_created":true,"url":"https://app.example.workers.dev"}
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
| `npm not found` | ``install Node.js 22 or newer (it provides npm), or pass --no-install and run `npm install` in the app later`` |
| `` `npm install` failed in <dir> (<status>): ... `` | ``the app is created: fix the cause above (network, Node.js 22+), then run `npm install` in it`` |
| `--deploy needs the app's npm packages` | ``drop --no-install, or deploy later with `npm install && ocre deploy` `` |
| `this Cloudflare login has no accounts` | ``create an account at https://dash.cloudflare.com/sign-up, then run `ocre login` again`` |
| `cancelled` (wizard only) | ``run `ocre new <name>` with flags (see `ocre new --help`) to skip the questions`` |

With `--deploy`, the errors of [ocre deploy](#ocre-deploy) apply too. The app directory is already written when a deploy fails; fix the cause, `cd` into it and run `ocre deploy`.

## ocre login

```text
ocre login [--json]
```

Logs in to Cloudflare. It runs `cf auth whoami`; when not logged in, it runs `cf auth login`, which opens a browser to approve access (OAuth), then checks `cf auth whoami` again. Already logged in, it only prints the login. It takes no flags besides `--json`. Inside an app it runs the app's `cf`; elsewhere `npx --yes cf@1.0.0-beta.5`.

cf keeps its own login, separate from wrangler's: after upgrading from an Ocre that used wrangler, run `ocre login` once again. On a machine with several Cloudflare accounts, `npx cf auth activate <profile> .` binds a cf profile to the app directory. CI uses `CLOUDFLARE_API_TOKEN` instead (see [Deployment](../guides/deployment.md#ci)).

Example (fake cf, not logged in yet; `Successfully logged in.` is the fake's line):

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

`email` is absent when the session has none (an API token). Errors: `Cloudflare login did not complete` (hint: ``run `ocre login` and approve access in the browser, or set CLOUDFLARE_API_TOKEN``), `` unexpected `cf auth whoami` output: ... ``, and the shared errors.

## ocre generate

```text
ocre generate <GENERATOR> [ARGS]... [--pretend] [--force | --skip]
ocre g <GENERATOR> [ARGS]...
```

`g` is an alias. The built-in generators are `model`, `scaffold`, `api`, `resource`, `controller`, `auth`, `migration`, `mailer`, `mailbox`, `job`, `schedule`, `cache`, `data`, `system_test`, `ci`, `pwa`, `locale`, `override` and `generator`; any other name runs the app's own generator in `.ocre/generators/<name>/`. Each one is documented with its arguments, files, output and errors in [Generators](generators.md).

| Flag | Effect |
|---|---|
| `--pretend` | Report the files that would be created or updated; write nothing |
| `--force` | Overwrite files that already exist |
| `--skip` | Keep files that already exist and generate the rest (conflicts with `--force`) |

Generated Rust files go through `rustfmt` with the app's `rustfmt.toml` before they are written (unchanged when rustfmt is missing), so `cargo fmt --check` and [`ocre ci`](#ocre-ci) pass on generated code. Every run that writes files is recorded in `.ocre/generated/` for [`ocre destroy`](#ocre-destroy) (see [Generation records](generators.md#generation-records-and-ocre-destroy)).

## ocre destroy

```text
ocre destroy <GENERATOR> [NAME] [--force] [--pretend] [--json]
ocre d <GENERATOR> [NAME]
```

Undoes a generator run, like `rails destroy`, from its record in `.ocre/generated/`: deletes the files it created (and the directories left empty), takes the lines it added out of existing files and puts back the lines it replaced, then deletes the record. Changes to `Cargo.toml`, `cloudflare.config.ts` and `package.json` stay (features and bindings later code may use); they are reported as `skipped`. No cf, no network.

| Argument or flag | Default | Effect |
|---|---|---|
| `GENERATOR` | required | The generator as typed after `ocre g` (`scaffold`, `controller`, or an app generator's name) |
| `NAME` | latest run | The name given to the generator (`Post`; `BlogPost`, `blog_post` and `blog-post` match each other). Without it, the latest run of that generator |
| `--force` | off | Delete the generated files even if they changed since; leave in place the added lines that changed |
| `--pretend` | off | Show what would be removed; change nothing |

```sh
ocre destroy scaffold Temp
```

```text
  update  src/models/mod.rs
  update  src/lib.rs
  remove  migrations/0004_create_temps.sql
  remove  src/models/temp.rs
  remove  src/temps.rs
  remove  templates/temps/index.html
  remove  templates/temps/show.html
  remove  templates/temps/new.html
  remove  templates/temps/edit.html
  remove  templates/temps/_form.html
  remove  .ocre/generated/0005_scaffold_temp.json

Next:
  if `ocre migrate` already applied migrations/0004_create_temps.sql, its tables and columns stay: undo them with a new migration (`ocre g migration ...`)
```

The latest `ocre g controller` run, whose `src/help.rs` was edited since, deleted anyway:

```sh
ocre destroy controller --force --json
```

```json
{"command":"destroy","ok":true,"removed":["src/help.rs","templates/help/faq.html",".ocre/generated/0008_controller_help.json"],"updated":["src/lib.rs"]}
```

A migration file is deleted, but a migration already applied to a database stays applied: the next step says so. Destroy later runs first when they build on an earlier one (a scaffold whose model a later `references` field extended).

Errors:

| Error | Hint |
|---|---|
| ``no recorded `ocre g scaffold Temp` run to destroy`` | `recorded runs: ocre g scaffold Post title:string ..., ...` (the recorded commands), or, without records, ``` `ocre destroy` undoes runs recorded in .ocre/generated/; this app has none, so remove the files by hand ``` |
| ``cannot destroy `ocre g controller Help faq`: src/help.rs changed since it was generated`` (also ``<file>: the generated `<line>` changed`` and `<file> no longer exists`) | `destroy later generator runs first (newest first), undo your edits, or pass --force to delete the generated files anyway and leave changed lines in place` |
| `.ocre/generated/<file> is not a valid generation record: ...` | ``restore it from version control, or delete it if you no longer need `ocre destroy` for it`` |

## ocre template

```text
ocre template <SOURCE> [--json]
```

Applies an application template to the current app, like `rails app:template` (`ocre new --template` does the same for a new app). `SOURCE` is a file path or an `http(s)://` URL of a text file with one ocre command per line; `#` starts a comment and the leading `ocre` is optional:

```text
# Blog comments
g scaffold Comment body:text post:references
cargo add slug
migrate
```

Only commands that change the app locally are allowed: `g`/`generate`, `d`/`destroy`, `migrate`, `db`, `sql`, `i18n`, `routes`, and `cargo add` / `cargo remove`; nothing with `--remote`, no `deploy`, `login` or shell commands. Every line is checked before the first one runs; lines then run in order, each as `ocre <line> --json` in the app (or `cargo ...`), and the first failing line stops the run. A template runs generators and `cargo add` on your machine: apply only templates you trust.

```sh
ocre template blog.ocre
```

```text
  create  migrations/0004_create_comments.sql
  create  src/models/comment.rs
  create  src/comments.rs
  create  templates/comments/index.html
  create  templates/comments/show.html
  create  templates/comments/new.html
  create  templates/comments/edit.html
  create  templates/comments/_form.html
  update  Cargo.toml
  update  src/lib.rs
  update  src/models/mod.rs
  update  src/models/post.rs
  ocre g scaffold Comment body:text post:references
  cargo add slug
  ocre migrate
```

With `--json`: `{"command": "template", "created": [...], "updated": [...], "ran": [...], "ok": true}`.

Errors:

| Error | Hint |
|---|---|
| ``blog.ocre line 1: `deploy` is not allowed in a template`` (also `targets production (--remote)`, `has an unclosed quote`) | ``template lines are ocre commands that change the app locally: g/generate, destroy, migrate, db, sql, i18n, routes, or `cargo add`/`cargo remove`, without --remote`` |
| `could not read the template missing.ocre: ...` | `pass the path of a text file of ocre commands, or an https:// URL` |
| `could not download the template <url>: ...` | `check the URL (it must serve the template as plain text), or download it and pass its path` |
| ``template line `ocre g ...` failed: <its error>`` | the line's own hint |
| ``template line `cargo add ...` failed (<status>)`` | `the cargo output above names the cause` |

## ocre migrate

```text
ocre migrate [--remote] [--status] [--json]
```

Applies the pending SQL files of `migrations/` to the app's D1 database, in file-name order. Locally it runs the app's wrangler, `wrangler d1 migrations apply DB --local -c .wrangler/ocre-d1.json --persist-to .wrangler/state` (a config Ocre derives from `cloudflare.config.ts`; see [Why wrangler still appears](../guides/deployment.md#why-wrangler-still-appears)); with `--remote` it looks the database id up with `cf d1 list --name <database>` and runs `cf d1 migrations apply <id>`. Applied migrations are recorded in the database's `d1_migrations` table, so each file runs once.

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

All of it is wrangler's output (wrangler answers its own question with its non-interactive default, yes). With `--json`, that output goes to stderr and stdout gets:

```json
{"command":"migrate","ok":true}
```

The object is the same with `--remote`: the success report of `ocre migrate` has no `remote` key.

### ocre migrate --status

Locally, runs `wrangler d1 migrations list DB --local` (same derived config), shows its table and reads the pending file names from it. With `--remote`, runs `cf d1 migrations list <id>` and reads the names from its JSON.

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

Applies (or with `--status`, lists) migrations on the production D1 database. It needs a Cloudflare login and a database that exists: the first [`ocre deploy`](#ocre-deploy) creates it, and every deploy applies remote migrations itself, so `ocre migrate --remote` is only needed to migrate without deploying. With `--status`, the human output ends with `Target: remote D1 database on Cloudflare` and the JSON has `"remote": true` (fake cf):

```json
{"command":"migrate","next":["ocre migrate --remote"],"ok":true,"pending":["0002_create_comments.sql"],"remote":true}
```

Errors: the shared ones, and `the D1 database blog does not exist on Cloudflare yet` (hint: ``run `ocre deploy` (or `ocre db create --remote`) first``) with `--remote`. A failing migration is reported as `` `wrangler d1 migrations apply DB --local` failed (exit status: 1) `` (or `` `cf d1 migrations apply <id>` failed ``) with the tool's error above it.

## ocre db seed

```text
ocre db seed [--remote] [--replant] [--from <DIR>] [--json]
```

Loads the fixtures of `db/fixtures/` (see [Fixtures](../guides/models.md#fixtures)), then runs `db/seeds.sql`; either may be missing, not both. The fixtures become one SQL file, `.wrangler/ocre-fixtures.sql` (deleted afterwards), run with `wrangler d1 execute DB --local --file .wrangler/ocre-fixtures.sql --yes`: foreign keys deferred, each fixture table emptied, then one `INSERT` per row, so loading twice gives the same rows. Fixtures load into the local database only.

`db/seeds.sql` runs locally with `wrangler d1 execute DB --file db/seeds.sql --local --yes` (the app's wrangler; `--yes` answers its "database unavailable during import" question), with `--remote` through `cf d1 query <id> --batch @.wrangler/ocre-batch.json` (a temporary file holding the SQL). The file is plain SQL, typically `INSERT` statements; it is not tracked, so running it twice inserts the rows twice.

| Flag | Default | Effect |
|---|---|---|
| `--remote` | local | Seed the production database on Cloudflare (`db/seeds.sql` only; refused when there are fixtures to load) |
| `--replant` | off | Local only: first empty every app table like [`ocre db truncate`](#ocre-db-truncate), then load the fixtures and seeds, so the data matches the files exactly. Loco's `seed --reset` |
| `--from <DIR>` | `db/fixtures` | Fixture directory, relative to the app root |

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

`ocre db seed --replant --json` reports both steps: `{"command":"db seed","ok":true,"ran":["emptied posts (--local)","loaded db/seeds.sql (--local)"]}`. Fixtures add a step before the seeds: `"loaded db/fixtures: authors, posts (--local)"`.

Errors: `db/seeds.sql not found in the app` when there is neither a fixture directory nor seeds (hint: ``create db/seeds.sql with INSERT statements or fixture files in db/fixtures/, then run `ocre db seed` ``), `<DIR> not found in the app` for a missing `--from` directory (hint: `pass the directory of the fixture files, relative to the app root`), `the fixtures of db/fixtures only load into the local database` with `--remote` (hint: `fixtures replace table rows: local only; use db/seeds.sql for remote data`), fixture file errors naming the file and label (e.g. ``db/fixtures/posts.yml: `hello`: no fixture labelled `ada` for `author` ``, hint: ``add a `ada:` row to a fixture file, or set `author_id` to an id``), ``` `ocre db seed --replant` only runs on the local database ``` for `--replant --remote` (hint: ``Ocre never deletes production data; use the Cloudflare dashboard or `cf d1 ...` for that on purpose``), and the shared ones.

## ocre db reset

```text
ocre db reset [--json]
```

Local only. Deletes `.wrangler/state/v3/d1` (the local D1 databases) when it exists, applies every migration like `ocre migrate`, then loads the fixtures of `db/fixtures/` and `db/seeds.sql` when they exist. It takes no `--remote`: production data is never reset.

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

`ran` lists only the steps that happened: no `deleted ...` without a local database, no `loaded ...` without fixtures or seeds.

## ocre db create

```text
ocre db create [--remote] [--json]
```

Creates the app's database. Locally, it runs `SELECT 1` through the app's wrangler (`wrangler d1 execute DB --local`), which creates the database file under `.wrangler/state/v3/d1`; the next step is `ocre migrate`. With `--remote`, it looks for the D1 database with `cf d1 list --name <database>` and runs `cf d1 create --name <database>` when it is missing (the first `ocre deploy` does the same on its own).

```json
{"command":"db create","next":["ocre migrate"],"ok":true,"ran":["created local database shop"]}
```

`ran` says `local database shop already exists` when it did, and `D1 database shop already exists` with `--remote`; a remote creation is listed in `provisioned` (`D1 database shop`) with `"remote": true`.

## ocre db prepare

```text
ocre db prepare [--json]
```

Local, safe to run any time (Rails' `db:prepare`): applies pending migrations to the local database and, when that database did not exist yet, loads the fixtures of `db/fixtures/` and `db/seeds.sql` if present. A good first command after cloning an app.

```json
{"command":"db prepare","ok":true,"ran":["applied migrations (--local)","loaded db/seeds.sql (--local)"]}
```

On an existing database, `ran` is only `["applied migrations (--local)"]`.

## ocre db drop

```text
ocre db drop [--json]
```

Local only: deletes `.wrangler/state/v3/d1`, the local D1 databases. The next step is `ocre db prepare`.

```text
  deleted .wrangler/state/v3/d1

Next:
  ocre db prepare
```

Without a local database, `ran` is `["no local database to delete"]`. `ocre db drop --remote` is refused: ``` `ocre db drop` only runs on the local database ``` (hint: ``Ocre never deletes production data; use the Cloudflare dashboard or `cf d1 ...` for that on purpose``).

## ocre db truncate

```text
ocre db truncate [--json]
```

Local only: deletes every row of every app table (all tables but SQLite's, D1's and `d1_migrations`) in one batch with deferred foreign keys, and resets the `AUTOINCREMENT` counters. Tables and applied migrations stay.

```json
{"command":"db truncate","ok":true,"ran":["emptied posts (--local)"]}
```

With no table, `ran` is `["no tables to empty"]`. `--remote` is refused with ``` `ocre db truncate` only runs on the local database ``` and the same hint as `ocre db drop`.

## ocre db version

```text
ocre db version [--remote] [--json]
```

Prints the last migration applied (local database unless `--remote`), read from the `d1_migrations` table.

```sh
ocre db version
```

```text
0001_create_posts.sql
```

```json
{"command":"db version","ok":true,"version":"0001_create_posts.sql"}
```

Before any migration it prints `no migration applied` (`"version": null`). With `--remote`, the human output ends with `Target: remote D1 database on Cloudflare` and the JSON has `"remote": true`.

## ocre db schema

```text
ocre db schema [--remote] [--json]
```

Writes the database's current `CREATE TABLE`, `CREATE INDEX`, view and trigger statements (from `sqlite_master`, without SQLite's and D1's own tables) to `db/schema.sql`, like Rails' `structure.sql`. It is a snapshot to read and the input of [`ocre g migration rebuild_<table>`](generators.md#ocre-g-migration); migrations stay the source of truth. Run it again after `ocre migrate`.

```sh
ocre db schema
```

```text
  create  db/schema.sql
```

```sql
-- Schema of the local D1 database, written by `ocre db schema` from sqlite_master.
-- A snapshot for reading: migrations/ are the source of truth. Run `ocre db schema` again after `ocre migrate`.

CREATE TABLE posts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    published INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

Later runs report `update  db/schema.sql` (`"updated": ["db/schema.sql"]`). Errors: the shared errors, and `unexpected D1 query output: ...`.

## ocre db dump

```text
ocre db dump [--tables <TABLES>] [--dir <DIR>] [--force] [--remote] [--json]
```

Writes the rows of every app table (the tables of `ocre db schema`), or of `--tables posts,authors`, to fixture files `<DIR>/<table>.yml` that [`ocre db seed`](#ocre-db-seed) loads back. It reads the table names from `sqlite_master`, then runs one `SELECT * FROM "<table>";` per table in a single query (locally through the app's wrangler, with `--remote` through `cf d1 query`; each row returned is a D1 row read). Each row gets the label `<table>_<id>` and keeps its `id`; strings are double-quoted YAML.

| Flag | Default | Effect |
|---|---|---|
| `--tables <TABLES>` | every app table | Comma-separated table names |
| `--dir <DIR>` | `db/fixtures` | Directory of the fixture files, relative to the app root |
| `--force` | off | Overwrite existing fixture files |
| `--remote` | local | Read the production database on Cloudflare (read-only) |

```sh
ocre db dump --tables posts
```

```yaml
# db/fixtures/posts.yml
# Rows of the local D1 database, written by `ocre db dump`; `ocre db seed` loads them.
posts_1:
  id: 1
  title: "Hello"
  published: 1
```

```json
{"command":"db dump","created":["db/fixtures/posts.yml"],"ok":true}
```

With `--force`, rewritten files are in `updated`; with no tables, `ran` is `["no tables to dump"]`. Errors: `db/fixtures/posts.yml already exist` when a file exists (hint: `pass --force to overwrite them, or --dir <dir> to dump elsewhere`), `unexpected D1 query output: ...`, and the shared ones.

## ocre sql

```text
ocre sql [--remote] [--json] <QUERY>
```

Runs one or more SQL statements separated by `;` locally with `wrangler d1 execute DB --command <QUERY> --local --json` (the app's wrangler), with `--remote` with `cf d1 query <id> --batch @.wrangler/ocre-batch.json`, and prints each statement's rows. Quote the query for the shell.

| Argument or flag | Default | Effect |
|---|---|---|
| `QUERY` | required | SQL statements separated by `;` |
| `--remote` | local | Run on the production database on Cloudflare |

The tool's output is captured, not printed, in both modes: the human output is one table per statement, each followed by its row count, with `NULL` for null values.

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

With `--json`, `rows` is D1's JSON unchanged, one object per statement:

```sh
ocre sql "SELECT id, title FROM posts WHERE published = 1; SELECT COUNT(*) AS n FROM posts" --json
```

```json
{"command":"sql","ok":true,"rows":[{"meta":{"duration":1},"results":[{"id":1,"title":"Hello"},{"id":3,"title":"Hello"}],"success":true},{"meta":{"duration":0},"results":[{"n":4}],"success":true}]}
```

With `--remote`, the JSON has `"remote": true` and the human output ends with `Target: remote D1 database on Cloudflare`.

Errors: a failing statement is reported with the tool's captured output in the message (here the local database, through the app's wrangler):

```sh
ocre sql "SELECT * FROM nope" --json
```

```json
{"error":"`wrangler d1 execute DB --local --command SELECT * FROM nope --json` failed (exit status: 1):\n\n{\n  \"error\": {\n    \"text\": \"no such table: nope: SQLITE_ERROR\"\n  }\n}","hint":"the wrangler output above names the cause","ok":false}
```

Also `unexpected D1 query output: ...` when the tool's output is not the expected JSON, and the shared errors (with `--remote`, a missing database too).

## ocre dev

```text
ocre dev [--port <PORT>] [--json]
```

Runs the app locally, at `http://localhost:<PORT>`, until stopped (Ctrl-C).

| Flag | Default | Effect |
|---|---|---|
| `--port <PORT>` | `8787` | Port of the local server |
| `--no-cache` / `--cache` | neither | Turn `ocre::cache` off (`CACHE_STORE=null` in `.dev.vars`) or back on; kept for later runs |

Steps:

1. Checks the locale files (see [ocre i18n missing](#ocre-i18n-missing)); a file the Worker could not load stops here.
2. Checks that rustc has the `wasm32-unknown-unknown` target.
3. Checks that the app's npm packages are installed (`node_modules/.bin/cf` and `wrangler`).
4. Applies local migrations, like `ocre migrate` (through the app's wrangler).
5. Runs `cf dev --port <PORT>`. cf delegates the build and the local server to the app's wrangler, which runs the `build.command` of `wrangler.config.ts`; `ocre dev` sets `OCRE_BUILD=--dev`, so `worker-build` makes an unoptimized build, much faster to compile than the `--release` build of deploys. The first build compiles every dependency (about a minute); later ones are incremental. Local D1, R2, KV, Queues and Durable Objects are simulated under `.wrangler/state`, the same state the local database commands use, and `.dev.vars` provides the local secrets.

Abridged output of a run on a new blog app (the lines come from wrangler, which cf runs):

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

The report is printed after `cf dev` exits successfully; with `--json` it is:

```json
{"command":"dev","ok":true,"url":"http://localhost:8787"}
```

and every line above goes to stderr. Errors: the shared ones (locale files, wasm target, npm packages, cf). `` `cf dev --port 8787` failed `` usually means the build failed: the compiler errors are in the output above it.

`ocre dev --no-cache` turns `ocre::cache` off in development (Rails' `dev:cache`): it writes `CACHE_STORE=null` into `.dev.vars`, so every `ocre::cache::fetch` computes its value, and the choice stays for later runs; `ocre dev --cache` removes the line. The report's `ran` says which (`caching off: CACHE_STORE=null in .dev.vars`). `ocre secrets push` refuses to upload that development value.

## ocre test

```text
ocre test [--e2e] [--port <PORT>] [--json] [-- <CARGO_TEST_ARGS>...]
```

Runs the app's checks in one command, like `bin/rails test:all`, and stops at the first failure:

1. `cargo test`, the native unit tests, with the arguments after `--` passed on (`ocre test -- models` runs the tests whose name contains `models`); request tests are `#[ignore]`d here;
2. `cargo check --target wasm32-unknown-unknown`, the type check of the real build;
3. with `--e2e`:
   - a fresh test database in `.wrangler/test-state` (migrations, then `tests/fixtures/*.yml`); the development data is untouched,
   - one server on `<PORT>` for the whole run, logging to `.wrangler/test-state/dev.log`,
   - `cargo test -- --ignored --test-threads=1`: the request tests, with `OCRE_TEST_URL`, `OCRE_TEST_STATE` and `OCRE_TEST_LOG` set for `ocre::testing`,
   - `sh tests/e2e.sh` with `BASE_URL`, when the app has it,
   - `node_modules/.bin/playwright test` with `BASE_URL`, when `tests/system/` has `*.spec.ts` files ([`ocre g system_test`](generators.md#ocre-g-system_test)),
   
   then stops the server.

| Flag | Default | Effect |
|---|---|---|
| `--e2e` | off | Also run the request tests, `tests/e2e.sh` and the browser tests against a local server (see [Testing](../guides/testing.md)) |
| `--port <PORT>` | `8788` | Port of the server started for `--e2e`, next to `ocre dev`'s 8787 |

```sh
ocre test --e2e --json
```

```json
{"command":"test","ok":true,"ran":["cargo test: ok","cargo check --target wasm32-unknown-unknown: ok","test database .wrangler/test-state: migrated, tests/fixtures loaded","cargo test -- --ignored against the test server on port 8788: ok","playwright test (tests/system) against the test server on port 8788: ok"]}
```

With `--json`, the output of cargo, wrangler and Playwright goes to stderr. A failing step ends with, for example, `error: cargo test -- --ignored failed (exit status: 101)` and a hint naming how to rerun it; the steps that passed are still listed in `ran`. Other errors: `tests/e2e.sh failed (<status>)`, `tests/system has tests but Playwright is not installed` (hint: `npm install`, then `npx playwright install chromium`), `playwright test failed (<status>)` (hint: screenshots and traces in `test-results/`), `the test server stopped or did not get ready` (hint: its output is in `.wrangler/test-state/dev.log`), a fixture file that does not parse, the missing wasm target, and `could not run cargo: ...`.

## ocre deploy

```text
ocre deploy [--json]
```

Builds the app in release mode and deploys it to Cloudflare as the Worker named in `cloudflare.config.ts`, creating what it needs first. It needs a Cloudflare login ([`ocre login`](#ocre-login)) or `CLOUDFLARE_API_TOKEN` (with `CLOUDFLARE_ACCOUNT_ID` when the token reaches several accounts), and the app's npm packages. Ocre provisions every resource itself before `cf deploy` runs. Steps, in order:

1. Checks the locale files, the wasm target and that `node_modules` has the app's `cf` and `wrangler` (hint: `npm install`).
2. Database: looks for the `DB` database with `cf d1 list --name <database>` and creates it with `cf d1 create --name <database>` when missing.
3. Queues: lists the account's queues (`cf queues list`) and creates, with `cf queues create --queue-name <name>`, each queue the config names (`bindings.queue` names, `triggers.queue` names and their `deadLetterQueue`) that is missing. A consumer of a missing queue would fail the deploy.
4. R2 buckets: for each `bindings.r2({ name })` (the `STORAGE` bucket added by the first `attachment` field), runs `cf r2 buckets get <name>` and creates the bucket (`cf r2 buckets create`) when Cloudflare answers that it does not exist (API code 10006).
5. KV namespaces: for each `bindings.kv()` entry without an `id` (the `CACHE` binding added by `ocre g cache`), finds the namespace titled `<worker>-<binding>` (lowercase, `_` as `-`, e.g. `blog-cache`) in `cf kv namespaces list`, creates it when missing, and rewrites the entry to `CACHE: bindings.kv({ id: "<id>" }),` in `cloudflare.config.ts`. Commit that change: later deploys reuse the namespace.
6. `SECRET_KEY_BASE`: runs `cf workers secrets list --worker <name>`. When the Worker does not exist yet or has no `SECRET_KEY_BASE`, a new random value is written to `.wrangler/ocre-secrets.env` (mode 0600, deleted afterwards) and uploaded with `cf deploy --secrets-file`. An existing secret is never replaced (that would sign every user out); any other failure of the secrets list stops the deploy rather than risk it.
7. Migrations: `cf d1 migrations apply <id>` on the production database, always before the new code goes live, so it never runs on the old schema.
8. Deploy: `cf deploy`, with `OCRE_BUILD=--release`; cf delegates the release build to the app's wrangler (`build.command` of `wrangler.config.ts`).
9. Reports the `https://...workers.dev` URL found in cf's output.

Durable Object namespaces (realtime channels) need no step: `cf deploy` creates them from the `exports` of `cloudflare.config.ts`.

First deploy of an app with a job, a cache, attachments (fake cf; the lines before `Created the SECRET_KEY_BASE secret` are cf's):

```sh
ocre deploy
```

```text
Migrations applied to the remote database
Uploaded app
  https://app.example.workers.dev
Created the SECRET_KEY_BASE secret on Cloudflare
Created D1 database blog on Cloudflare
Created queue blog-jobs on Cloudflare
Created queue blog-jobs-failed on Cloudflare
Created R2 bucket blog-storage on Cloudflare
Created KV namespace blog-cache (id written to cloudflare.config.ts) on Cloudflare

https://app.example.workers.dev
```

The same first deploy with `--json`:

```json
{"command":"deploy","ok":true,"provisioned":["D1 database blog","queue blog-jobs","queue blog-jobs-failed","R2 bucket blog-storage","KV namespace blog-cache (id written to cloudflare.config.ts)"],"secret_created":true,"url":"https://app.example.workers.dev"}
```

A later deploy, with everything in place:

```json
{"command":"deploy","ok":true,"url":"https://app.example.workers.dev"}
```

Free-plan note (September 2026): R2 must be enabled once in the Cloudflare dashboard, which asks for a payment method even for use within the free tier of 10 GB stored, 1M writes and 10M reads a month ([R2 pricing](https://developers.cloudflare.com/r2/pricing/)). `ocre deploy` detects an account without R2 (API code 10042) and says so in its hint.

Errors (besides the shared ones):

| Error | Hint |
|---|---|
| `` `cf d1 list --name <database>` failed: ... `` (also `cf queues list`, `cf queues create ...`, `cf kv namespaces list ...`, `cf d1 create ...`) | ``log in with `ocre login`, or set CLOUDFLARE_API_TOKEN (and CLOUDFLARE_ACCOUNT_ID when the token sees several accounts)`` |
| `` `cf d1 create --name <database>` returned no uuid: ... `` | ``run `ocre deploy` again: it finds the database by name once Cloudflare lists it`` |
| `` `cf kv namespaces create --title <title>` returned no id: ... `` | ``run `ocre deploy` again: it links the namespace once Cloudflare lists it`` |
| `` `cf r2 buckets get <bucket>` failed: ... `` | R2 not enabled (code 10042): ``enable R2 once in the Cloudflare dashboard (Storage & databases > R2; the free plan asks for a payment method but charges nothing within 10 GB, 1M writes and 10M reads a month), then run `ocre deploy` again``; otherwise the login hint above |
| `` `cf workers secrets list --worker <name>` failed: ... `` | the login hint above, then `; Ocre only creates SECRET_KEY_BASE when sure the Worker has none` |
| `` `cf d1 migrations apply <id>` failed (exit status: 1) `` | `read the cf output above; the first error line names the cause`; the previous code keeps running |
| `` `cf deploy` failed (exit status: 1) `` | same |
| `` unexpected `cf ...` output: ... `` | none |

Resources created before a failure stay created; running `ocre deploy` again skips them.

## ocre logs

Streams the deployed Worker's live logs until you stop it (Ctrl-C): each request with its outcome, its console lines (`ctx.log()` lines, Ocre's `[ocre]` errors) and uncaught exceptions. It runs the app's `wrangler tail`, because cf 1.0.0-beta.5 has no tail command, so it uses wrangler's own login (`npx wrangler login` in the app, or `CLOUDFLARE_API_TOKEN`). Stored logs are in the dashboard (Workers Logs).

```sh
ocre logs                               # pretty, every request
ocre logs --status error                # failed invocations only
ocre logs --search checkout --format json
```

| Flag | Effect |
|---|---|
| `--format pretty\|json` | `pretty` (default), or one JSON object per event |
| `--status ok\|error\|canceled` | Keep invocations with that outcome (repeatable) |
| `--search <text>` | Keep events whose console lines contain the text |

Errors: outside an app, the usual "no cloudflare.config.ts found" error; without `npm install`, the hint names it; when wrangler fails (not logged in, Worker never deployed), the error carries wrangler's output and the hint `wrangler tail uses wrangler's own login: run npx wrangler login ... the Worker must be deployed (ocre deploy)`. See [Errors, logging and debugging](../guides/debugging.md).

## ocre secret

```text
ocre secret [--json]
```

Prints a new random value for `SECRET_KEY_BASE`, like `rails secret`: 64 bytes from the operating system's random number generator, as 128 lowercase hex characters. It works anywhere (no app needed) and writes nothing. `ocre new` already puts one in `.dev.vars` and the first `ocre deploy` uploads one; use `ocre secret` to replace a leaked secret (which signs every user out): put the value in `.prod.vars` (git-ignored) as `SECRET_KEY_BASE=<value>`, then

```sh
ocre secrets push SECRET_KEY_BASE --file .prod.vars
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

## ocre secrets

```text
ocre secrets list [--json]
ocre secrets push <NAMES>... [--file <FILE>] [--json]
```

Worker secrets are Ocre's credentials (Rails' `credentials.yml.enc`): Cloudflare stores them encrypted, the Worker reads them with `ctx.env().secret(NAME)`, and nothing secret is committed. Values can be written but never read back.

`ocre secrets list` shows each secret name of `.dev.vars` (used by `ocre dev`) and of the deployed Worker (`cf workers secrets list --worker <name>`), side by side. Names set locally but not deployed get a next step; `SECRET_KEY_BASE` and `MAIL_ADAPTER` are left out of it, their `.dev.vars` values being for development only.

```text
  GITHUB_CLIENT_ID                .dev.vars
  MAIL_ADAPTER                    .dev.vars
  SECRET_KEY_BASE                 .dev.vars

Next:
  ocre secrets push GITHUB_CLIENT_ID --file <production values>
```

```json
{"command":"secrets list","next":["ocre secrets push GITHUB_CLIENT_ID --file <production values>"],"ok":true,"secrets":[{"deployed":false,"local":true,"name":"GITHUB_CLIENT_ID"},{"deployed":false,"local":true,"name":"MAIL_ADAPTER"},{"deployed":false,"local":true,"name":"SECRET_KEY_BASE"}]}
```

`ocre secrets push` uploads the named secrets to the deployed Worker, with their values read from a `NAME=value` file (`--file`, default `.dev.vars`; keep production values in `.prod.vars`, which the app's `.gitignore` lists). It writes them to a temporary JSON file under `.wrangler/` (readable only by you), runs one `cf workers secrets bulk --worker <name> --file <it>` (a new Worker version, no rebuild), and deletes the file whatever happened. Values never appear on a command line. The report's `ran` is `["uploaded GITHUB_CLIENT_ID", ...]`.

```sh
ocre secrets push GITHUB_CLIENT_ID GITHUB_CLIENT_SECRET --file .prod.vars
```

Errors:

| Error | Hint |
|---|---|
| `name the secrets to upload` | ``e.g. `ocre secrets push GITHUB_CLIENT_ID GITHUB_CLIENT_SECRET`; `ocre secrets list` shows them`` |
| `SECRET_KEY_BASE in .dev.vars is a development value` (also `MAIL_ADAPTER`) | ``production needs its own: `ocre deploy` creates SECRET_KEY_BASE; for others put the production value in another git-ignored file (e.g. .prod.vars) and pass `--file <it>` `` |
| `NOPE is not set in .prod.vars` | ``add `NOPE=<value>` to .prod.vars (a git-ignored file), then run this again`` |
| `cannot read <file>: ...` | none |

Both commands need a Cloudflare login for the deployed side, and the shared errors apply.

`ocre secrets fetch NAME [--file FILE]` prints one value of a local `NAME=value` file (default `.dev.vars`), like Rails' `credentials:fetch`, for scripts: `export STRIPE_KEY=$(ocre secrets fetch STRIPE_KEY --file .prod.vars)`. With `--json` the value is in `secret`. It never calls Cloudflare, which does not return deployed values. Error: `STRIPE_KEY is not set in .dev.vars` (hint: ``add `STRIPE_KEY=<value>` to .dev.vars; deployed values cannot be read back (Cloudflare only stores them)``).

## ocre domains

```text
ocre domains [--json]
ocre domains add <HOST> [--json]
ocre domains remove <HOST> [--json]
```

The Worker's custom domains: the `domains` list of `worker` in `cloudflare.config.ts` (cf's config schema). `add` writes the list (after `name:` the first time) and `remove` takes a host out; the next `ocre deploy` publishes the Worker on them, and Cloudflare creates the DNS record and the certificate. The host's zone must be on the Worker's Cloudflare account (free plan zones work); the token of a CI deploy then also needs the Workers Routes permission of that zone. Without a subcommand, lists them.

```sh
ocre domains add www.example.com
```

```json
{"command":"domains add","domains":["www.example.com"],"next":["ocre deploy (creates the DNS record and certificate; the zone must be on this Cloudflare account)"],"ok":true,"updated":["cloudflare.config.ts"]}
```

Errors: `` `https://x` is not a hostname Ocre can add as a custom domain `` (hint: a lowercase hostname without scheme, path or wildcard), `www.example.com is already a custom domain of the Worker`, `www.example.com is not a custom domain of the Worker`, and ``cloudflare.config.ts has a `domains` entry Ocre cannot read`` (hint: write it as a list of string literals). Routes with wildcards (`example.com/api/*`) stay hand-written `triggers.fetch({ pattern, zone })` entries.

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

## ocre schedules

```text
ocre schedules [--json]
ocre schedules run <TASK> [--port <PORT>] [--json]
```

`ocre schedules` lists the `triggers.scheduled` expressions of `cloudflare.config.ts` with the task that handles each, read from the `"<cron>" => <task>::run(...)` arms of `src/schedules/mod.rs` (see [`ocre g schedule`](generators.md#ocre-g-schedule)), without building the app. A cron no task handles fails when it fires; it is listed as `(no task: fails when it fires)`, with a next step.

```text
CRON (UTC)   TASK
0 3 * * *    src/schedules/nightly_cleanup.rs
0 9 * * MON  src/schedules/weekly_digest.rs
```

```json
{"command":"schedules","ok":true,"schedules":[{"cron":"0 3 * * *","task":"nightly_cleanup"},{"cron":"0 9 * * MON","task":"weekly_digest"}]}
```

`ocre schedules run <TASK>` fires the task's cron on the running `ocre dev`, through the dev server's local `/cdn-cgi/local/scheduled` endpoint, and returns once it answered; the task's `[ocre cron] <cron> done` (or `failed`) line is in the `ocre dev` output. Cron Triggers never fire in `ocre dev` by themselves.

| Argument | Default | Effect |
|---|---|---|
| `TASK` | required | The task, as in `src/schedules/<task>.rs` |
| `--port` | `8787` | The port `ocre dev` listens on |

```json
{"command":"schedules run","ok":true,"ran":["fired nightly_cleanup (0 3 * * *); its `[ocre cron]` line is in the `ocre dev` output"]}
```

Errors: ``no scheduled task `<task>` `` (hint: the tasks of the app, or how to create one); ``cannot reach `ocre dev` on port 8787: ...`` (hint: start `ocre dev` first, or pass `--port`); `the scheduled task answered HTTP 500` (hint: read the `[ocre cron]` line in the `ocre dev` output).

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

## ocre ci

```text
ocre ci [--signoff] [--json]
```

Runs the app's CI steps on your machine, in order, and stops at the first failure (Rails 8.1's `bin/ci`). They are the steps of the GitHub workflow that [`ocre g ci`](generators.md#ocre-g-ci) writes, so a green `ocre ci` means a green CI run:

1. `cargo fmt --check` (hint on failure: ``run `cargo fmt`, then `ocre ci` again``);
2. `cargo clippy --all-targets -- -D warnings` (hint: ``fix the warnings above (`rustup component add clippy` if cargo has no clippy command)``);
3. `cargo test`;
4. `cargo check --target wasm32-unknown-unknown` (after checking that rustc has the target);
5. `ocre i18n missing`, when `src/lib.rs` declares locales.

| Flag | Effect |
|---|---|
| `--signoff` | After a green run, runs `gh signoff`, the [basecamp/gh-signoff](https://github.com/basecamp/gh-signoff) extension of the GitHub CLI, which sets a passing status on the pushed commit (install it with `gh extension install basecamp/gh-signoff`; a branch protection rule can require it) |

```text
  cargo fmt --check: ok
  cargo clippy --all-targets -- -D warnings: ok
  cargo test: ok
  cargo check --target wasm32-unknown-unknown: ok
  ocre i18n missing: ok
```

```json
{"command":"ci","ok":true,"ran":["cargo fmt --check: ok","cargo clippy --all-targets -- -D warnings: ok","cargo test: ok","cargo check --target wasm32-unknown-unknown: ok"]}
```

With `--json`, cargo's output goes to stderr. A failing step ends with, for example, `error: cargo test failed (exit status: 101)` (`"ok": false`, the steps that passed still in `ran`). Other errors: the missing wasm target, `could not run cargo: ...`, the `ocre i18n missing` problems, `gh not found` (hint: ``install the GitHub CLI (https://cli.github.com), then `gh extension install basecamp/gh-signoff` ``) and `gh signoff failed (<status>)`.

## ocre doctor

```text
ocre doctor [--json]
```

Checks the tools and, inside an app, its setup, like `cargo loco doctor`. Each check is `ok`, `warn` or `fail`; any `fail` makes the command exit with code 1, so it can gate CI. Warnings (not logged in, pending migrations) do not. Outside an app, only the tool checks run.

| Check | Fails or warns when |
|---|---|
| `rust` | Fails: rustc has no `wasm32-unknown-unknown` target |
| `node` | Fails: `node --version` does not run or is older than 22 (cf's requirement) |
| `cloudflare login` | Warns: not logged in (`cf auth whoami`); the hint says to log in once more when a wrangler login exists, since cf keeps its own. Fails when cf errors |
| `npm packages` | In an app. Fails: `node_modules` has no `cf` or `wrangler` (run `npm install`), or wrangler is older than 4.136. Warns: the installed versions differ from the ones this CLI is tested with (cf 1.0.0-beta.5, wrangler 4.144.0) |
| `config` | In an app, with the packages installed. Fails: cf's own loader (`@cloudflare/config`) rejects `cloudflare.config.ts`, or `tsc -p .` reports type errors in it or in `wrangler.config.ts` |
| `cache binding`, `storage binding`, `jobs queue`, `cron triggers`, `realtime binding` | Only when the code uses them (`ocre::cache::`, `ocre::storage::`, the `queue` and `scheduled` events, realtime channels). Fails: `cloudflare.config.ts` lacks the `CACHE` KV binding, the `STORAGE` R2 binding, a queue binding and its `triggers.queue`, a `triggers.scheduled` entry, or the `CHANNELS` binding and `OcreChannel` export |
| `migrations` | Warns: local migrations are pending (`wrangler d1 migrations list DB --local`, through the app's wrangler) |
| `local secrets` | Fails: `.dev.vars` has no `SECRET_KEY_BASE` |
| `production secrets` | When logged in. Warns: the deployed Worker lacks `SECRET_KEY_BASE` (or `RESEND_API_KEY` with `MAIL_ADAPTER = "resend"`); ok when the Worker is not deployed yet |
| `production config` | Settings unsafe in production (Loco's production safety check). Fails: a plain-text variable of `worker.env` is named like a secret (`SECRET`, `TOKEN`, `PASSWORD`, `API_KEY`, `PRIVATE_KEY`: committed and readable; push it with `ocre secrets push`), or the app is a git repository whose `.gitignore` lacks `.dev.vars`. Warns: `MAIL_ADAPTER = "log"`, `CACHE_STORE = "null"`, `LOG_LEVEL = "debug"` or `"trace"` in `worker.env` (development values belong in `.dev.vars`) |
| Each file of `.ocre/doctor/` | The app's own checks (Loco's initializer `check()`), run in name order from the app root with no input: exit 0 is `ok`, exit 2 `warn`, anything else `fail`; the first line of stdout is the detail, the first line of stderr the hint. A file that cannot run (not executable, no `#!` line) fails |

An app check is any executable, for example `.ocre/doctor/stripe` (`chmod +x` it):

```sh
#!/bin/sh
# Fails unless .prod.vars has the Stripe key the production Worker needs.
if grep -q '^STRIPE_KEY=' .prod.vars 2>/dev/null; then
  echo "STRIPE_KEY ready in .prod.vars"
else
  echo "STRIPE_KEY missing from .prod.vars"
  echo "add STRIPE_KEY=... to .prod.vars, then ocre secrets push STRIPE_KEY --file .prod.vars" >&2
  exit 1
fi
```

```text
  ok    rust                wasm32-unknown-unknown target installed
  ok    node                node v22.23.2
  ok    cloudflare login    logged in as ada@example.com
  ok    npm packages        cf 1.0.0-beta.5, wrangler 4.144.0
  ok    config              cloudflare.config.ts is valid
  ok    storage binding     configured in cloudflare.config.ts
  warn  migrations          pending locally: 0001_create_posts.sql
                            run `ocre migrate`
  ok    local secrets       SECRET_KEY_BASE set in .dev.vars
  ok    production config   no secret in plain-text variables, no development-only setting
  ok    production secrets  not deployed yet
```

With `--json`, `checks` holds one `{"name", "status", "detail", "hint"}` object per line. A failed check ends the output with `error: 1 check(s) failed: local secrets` and `hint: fix each failed check as its hint says, then run `ocre doctor` again` (`"ok": false`, `error` and `hint` with `--json`).

## ocre about

```text
ocre about [--json]
```

Versions and the app's configuration, read from `Cargo.toml`, `cloudflare.config.ts`, `wrangler.config.ts` and `rust-toolchain.toml` without building anything (Rails' `about`, Loco's `doctor --config`). Variable values are never printed, only their names.

```text
Ocre CLI            0.1.0
App                 ci-app
App version         0.1.0
Ocre crate          git https://github.com/tgeselle/ocre.rs
Rust toolchain      stable
Compatibility date  2026-09-01
Mode                full-stack (HTML and JSON)
Bindings            D1 DB (ci-app), KV CACHE, R2 STORAGE (ci-app-storage), Queue JOBS (ci-app-jobs), Queue consumer (ci-app-jobs), Durable Object CHANNELS (OcreChannel), cron 0 3 * * *, Assets (public)
Variables           MAIL_FROM
Ocre features       realtime, graphql
```

With `--json`, the same values are in `about`: `cli`, `app`, `app_version`, `ocre`, `mode`, `rust_toolchain`, `compatibility_date`, `bindings`, `vars` and `features`.

## ocre version

```text
ocre version [--json]
ocre --version
```

`ocre version` prints the CLI's version and, inside an app, the app's name and version and its `ocre` dependency (version, git source or path):

```text
Ocre CLI            0.1.0
App                 ci-app
App version         0.1.0
Ocre crate          git https://github.com/tgeselle/ocre.rs
```

```json
{"about":{"app":"ci-app","app_version":"0.1.0","cli":"0.1.0","ocre":"git https://github.com/tgeselle/ocre.rs"},"command":"version","ok":true}
```

`ocre --version` (or `-V`) prints only `ocre 0.1.0`.

## ocre stats

```text
ocre stats [DIRS]... [--json]
```

Lines of code per part of the app, like `rails stats`: files, lines, LOC (lines neither blank nor only a comment) and Rust functions for `src/models`, `src/jobs`, `src/mailers`, `src/schedules`, the rest of `src/`, `templates/`, `migrations/` and `tests/`, then the Rust code-to-test ratio. Each extra directory (relative to the app root) gets its own row.

```text
Name                    Files   Lines     LOC Functions
Models                     19    3282    2358       273
Jobs                        4     144      64         7
Mailers                     3      85      44         3
Schedules                   3      53      21         3
Controllers and app        27    2921    2233       211
Templates                  50     787     641         0
Migrations                 26     217     186         0
Tests                       0       0       0         0

Code LOC: 4720    Test LOC: 0    Code to test ratio: 1:0.0
```

With `--json`: `"stats": {"rows": [{"name", "files", "lines", "loc", "functions"}, ...], "code_loc", "test_loc"}`. Error: `lib is not a directory of the app` (hint: ``pass directories relative to the app root, e.g. `ocre stats lib` ``).

Directories to count on every run (Rails' `CodeStatistics.register_directory`) go in `Cargo.toml`; each gets its own row, once, before those given on the command line:

```toml
[package.metadata.ocre]
stats = ["lib", "benches"]
```

## ocre notes

```text
ocre notes [--annotations TAG,TAG...] [--json]
```

Lists the `TODO`, `FIXME` and `OPTIMIZE` comments of `src/`, `templates/`, `migrations/`, `tests/`, `db/` and `locales/`, like `rails notes`. A tag counts inside a comment (`//`, `/*`, `{#`, `<!--`, `--` or `#`) as a whole word; `--annotations` replaces the tags.

```text
src/pages.rs:42: [TODO] tidy this
```

```json
{"command":"notes","notes":[{"line":42,"path":"src/pages.rs","tag":"TODO","text":"tidy this"}],"ok":true}
```

## ocre time-zones

```text
ocre time-zones [--json]
```

Prints the 418 IANA time zone names that `ocre::helpers::time_zone_options` offers, one per line (Rails' `bin/rails time:zones:all`); `--json` returns them as `time_zones`.

```sh
ocre time-zones | grep Europe/P
```

```text
Europe/Paris
Europe/Podgorica
Europe/Prague
```

## ocre help

```text
ocre help [COMMAND]...
ocre <COMMAND> --help
```

Prints the help of `ocre` or of a command (`ocre help db seed`, `ocre g model --help`). `-h` prints a shorter summary where the long help has examples. The help of `ocre g model` lists the field types.

## See also

- [Generators](generators.md): every `ocre g` generator.
- [Configuration](configuration.md): `cloudflare.config.ts`, `wrangler.config.ts`, `package.json`, `.dev.vars`, secrets and variables.
- [Deployment](../guides/deployment.md): deploying, custom domains, production data.
- [Models and migrations](../guides/models.md): writing migrations and seeds.
- [Free-plan limits](limits.md).
