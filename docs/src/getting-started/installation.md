# Installation

This page installs the tools an Ocre app needs (Rust through rustup with the WebAssembly target, Node.js for wrangler, and the `ocre` CLI), checks the installation, and creates a first app with `ocre new`, either through the guided setup or with flags.

## Before you start

You need:

- macOS, Linux or Windows with a terminal, and `git` if you want `ocre new --git` or to install from a clone.
- About 2 GB of disk space for the Rust toolchain and the Cargo build cache.
- A [Cloudflare account](https://dash.cloudflare.com/sign-up) only when you deploy. Everything on this page, and `ocre dev`, works without one.

## Install Rust with rustup

Ocre apps compile to WebAssembly (`wasm32-unknown-unknown`), so the Rust toolchain must be able to add that target. Install Rust with [rustup](https://rustup.rs):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add wasm32-unknown-unknown
```

Every generated app also has a `rust-toolchain.toml` that asks rustup for the stable toolchain with this target:

```toml
[toolchain]
channel = "stable"
targets = ["wasm32-unknown-unknown"]
```

Do not use Homebrew's `rust` formula (`brew install rust`). It installs `rustc` and `cargo` without rustup: they ship only the standard library of your own machine, cannot add the `wasm32-unknown-unknown` target, and ignore `rust-toolchain.toml`. `ocre dev` and `ocre deploy` detect it and stop (see [Troubleshooting](#troubleshooting)). On macOS, `brew install rustup` is fine: it is rustup, and its `rustup` and `cargo` commands work as above once `$(brew --prefix rustup)/bin` is on your `PATH`.

## Install Node.js

The CLI runs Cloudflare's [wrangler](https://developers.cloudflare.com/workers/wrangler/) for local development, migrations and deploys, always as `npx --yes wrangler@4`. Install [Node.js](https://nodejs.org) 20 or newer, which provides `npx`. You do not install wrangler yourself: `npx` downloads wrangler 4 the first time a command needs it and caches it.

## Install the ocre CLI

Install the `ocre` binary from the Git repository:

```sh
cargo install --git https://github.com/tgeselle/ocre.rs ocre-cli
```

The package is `ocre-cli`; the command it installs is `ocre` (in `~/.cargo/bin`, which rustup puts on your `PATH`). From a clone of the repository, install the same binary with:

```sh
git clone https://github.com/tgeselle/ocre.rs
cd ocre.rs
cargo install --path crates/ocre-cli
```

Run the same command again to upgrade. Ocre has no release on crates.io yet; apps depend on the `ocre` crate from the same Git repository.

## Check the installation

```sh
ocre --version
```

```text
ocre 0.1.0
```

```sh
ocre --help
```

```text
Ocre: Rails-like Rust web framework for Cloudflare Workers, free plan first.

Usage: ocre [OPTIONS] <COMMAND>

Commands:
  new       Create a new app in ./<NAME>. In a terminal, asks for anything not given by flags
  login     Log in to Cloudflare (opens a browser) unless already logged in
  generate  Generate code (alias: `g`)
  migrate   Apply D1 migrations (local database unless --remote)
  db        Database tasks: seed, reset
  sql       Run SQL on the D1 database (local unless --remote) and print the rows
  dev       Apply local migrations, then run the app with `wrangler dev`
  deploy    Deploy to Cloudflare and apply remote migrations
  secret    Print a new random secret, like `rails secret`: a value for SECRET_KEY_BASE
  routes    List the app's HTTP routes, read from its source (no build)
  i18n      Translations: checks of the locale files (see `ocre g locale`)
  help      Print this message or the help of the given subcommand(s)

Options:
      --json     Print one JSON object on stdout; send tool logs to stderr. Never prompts
  -h, --help     Print help
  -V, --version  Print version
```

`ocre <command> --help` describes each command and its flags; the [CLI reference](../reference/cli.md) documents them all.

## Create an app with the guided setup

In a terminal, `ocre new` asks for everything its flags did not answer:

```sh
ocre new
```

The questions, in order (each one is skipped when the matching flag was given):

| Question | Answers | Flag that skips it |
|---|---|---|
| What is your app called? | A name, checked as you type: lowercase letters, digits and dashes, starting with a letter, at most 63 characters, and no existing directory of that name | `ocre new <name>` |
| What are you building? | Full-stack app (HTML pages with askama and htmx) or API only (JSON endpoints, no HTML) | `--full-stack`, `--api` |
| Pick a starter | Empty (a home page, or a status endpoint in API mode) or Blog (posts with title, body and published, full CRUD) | `--starter empty`, `--starter blog` |
| Connect your Cloudflare account? | Log in now (opens your browser) or Later (run `ocre login` when you are ready). Asked only when `wrangler whoami` finds no login; otherwise the setup prints `Logged in to Cloudflare as <email>` | `--login`, `--no-login` |
| Which Cloudflare account should host it? | One of the accounts of your login, when it has several | `--account-id <id>` |
| Initialize a git repository? | Yes (default) or no | `--git`, `--no-git` |
| Deploy it now? The first build takes about a minute. | Yes (default) or no. Asked only when you are logged in | `--deploy`, `--no-deploy` |

The setup then creates the app, deploys it if you said yes, and ends with a "Next steps" note (`cd <name>`, `ocre dev`, `ocre deploy`, plus `ocre login` when you skipped the login) and either `Your app is live at <url>` or `Happy building!`. Esc or Ctrl-C cancels with `error: cancelled`.

## Create an app with flags (agents and scripts)

The guided setup only runs when stdin and stdout are a terminal and neither `--yes` nor `--json` was given. Otherwise `ocre new` never prompts: flags decide, and anything not given takes an opt-in default (full-stack, empty starter, no Cloudflare login, no git, no deploy).

| Flag | Effect | Default without prompts |
|---|---|---|
| `<name>` | App name and directory (`./<name>`) | required |
| `--api` / `--full-stack` | API only (JSON, no templates, Ocre's `html` feature off), or HTML pages | full-stack |
| `--starter empty\|blog` | `blog` adds a `Post` resource (title, body, published) | `empty` |
| `--login` / `--no-login` | Log in to Cloudflare if needed (opens a browser) | no login |
| `--account-id <id>` | Account to deploy to; required when the login has several | none |
| `--git` / `--no-git` | Run `git init` | no git |
| `--deploy` / `--no-deploy` | Deploy right away (implies `--login`) | no deploy |
| `--yes`, `-y` | Never prompt, even in a terminal | |
| `--ocre-path <dir>` | Depend on a local `crates/ocre` checkout instead of the Git repository | Git dependency |
| `--json` | Print one JSON object on stdout; tool output goes to stderr | |

```sh
ocre new blog --yes
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

Next:
  cd blog
  ocre dev
  ocre deploy
```

With `--json`, the same result is one JSON object on stdout, and the exit code is 0 on success, 1 on failure:

```sh
ocre new other --json --yes
```

```json
{"command":"new","created":["other/Cargo.toml","other/wrangler.toml","other/rust-toolchain.toml","other/.gitignore","other/AGENTS.md","other/migrations/.gitkeep","other/public/robots.txt","other/src/lib.rs","other/templates/layout.html","other/templates/home.html","other/.dev.vars"],"next":["cd other","ocre dev","ocre deploy"],"ok":true}
```

`--starter blog` also runs the scaffold generator for `Post title:string body:text published:boolean` and lists its files (`src/models/post.rs`, `migrations/0001_create_posts.sql`, `src/posts.rs`, `templates/posts/*.html`...). `--api` writes an API-only `src/lib.rs`, no `templates/`, and adds `[package.metadata.ocre] mode = "api"` to `Cargo.toml`.

## What ocre new creates

| File | Contents |
|---|---|
| `Cargo.toml` | The app crate (`cdylib`), depending on `ocre`, `worker`, `axum`, `askama` (full-stack only) and `serde`; a standalone `[workspace]`; release profile tuned for size |
| `wrangler.toml` | The Worker: `name`, the `[build]` command (installs `worker-build` and compiles to WebAssembly), `public/` as static assets, the `DB` D1 database, and `MAIL_FROM` under `[vars]` |
| `rust-toolchain.toml` | Stable Rust with the `wasm32-unknown-unknown` target |
| `.gitignore` | Build output, `.wrangler/` (local database) and `.dev.vars` |
| `AGENTS.md` | Conventions, commands and free-plan limits for AI agents working on the app |
| `migrations/` | D1 SQL migrations, applied in order (empty for now) |
| `public/robots.txt` | Static files, served by Cloudflare before the Worker runs |
| `src/lib.rs` | The Worker entry point and router: `GET /` (home page) and `GET /up` (health check) |
| `templates/layout.html`, `templates/home.html` | askama templates: the page layout (with htmx) and the home page |
| `.dev.vars` | Local-only settings for `ocre dev`: a random `SECRET_KEY_BASE` and `MAIL_ADAPTER=log`. Never commit it |

The app has no Cloudflare resources yet: the first `ocre deploy` creates the D1 database and the `SECRET_KEY_BASE` secret. The [tutorial](tutorial.md) continues from here.

## Troubleshooting

These are the errors the CLI prints for a broken setup, with the hint it gives. With `--json` they come as `{"ok": false, "error": ..., "hint": ...}`.

Homebrew's `rust`, or any toolchain without the WebAssembly target (`ocre dev`, `ocre deploy`):

```text
error: the wasm32-unknown-unknown target is not installed for rustc at /opt/homebrew/Cellar/rust/1.90.0
hint: use a rustup toolchain (Homebrew's `rust` has no wasm target) and run `rustup target add wasm32-unknown-unknown`
```

Fix: `brew uninstall rust`, install rustup as above, then `rustup target add wasm32-unknown-unknown`. `which rustc` should print a path under `~/.cargo/bin` (or Homebrew's `rustup` prefix).

No Rust at all:

```text
error: rustc not found
hint: install Rust with rustup: https://rustup.rs
```

No Node.js (any command that runs wrangler: `ocre dev`, `ocre migrate`, `ocre deploy`, `ocre login`...):

```text
error: could not run npx: No such file or directory (os error 2)
hint: install Node.js 20 or newer (it provides npx)
```

An app command run outside an app:

```text
error: no wrangler.toml found in this directory or its parents
hint: run this command inside an Ocre app, or create one with `ocre new <name>`
```

`ocre new` errors: an invalid name (here `ocre new Blog --yes`), no name without a terminal (`ocre new --yes`), and a directory that already exists (the error shows its absolute path):

```text
error: invalid app name `Blog`
hint: use lowercase letters, digits and dashes, starting with a letter (max 63), e.g. `my-blog`
```

```text
error: missing app name
hint: run `ocre new <name>`, or run `ocre new` in a terminal for the guided setup
```

```text
error: `/path/to/blog` already exists
hint: choose another name or remove the directory
```

## See also

- [Tutorial: a blog](tutorial.md): build, run and deploy a first app.
- [CLI commands](../reference/cli.md#ocre-new): every command and flag, including `ocre new`.
- [Deployment](../guides/deployment.md): what `ocre deploy` creates on Cloudflare.
- [Configuration](../reference/configuration.md): `wrangler.toml`, `.dev.vars`, variables and secrets.
