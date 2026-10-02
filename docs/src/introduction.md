# Ocre

Ocre is a Rails-like Rust web framework for Cloudflare Workers, designed to run on the Workers free plan and to be written by AI agents: generators write plain, readable Rust (models, controllers, templates, migrations) into your app, and the `ocre` CLI builds, runs and deploys it.

An Ocre app is one Worker compiled to WebAssembly. It stores data in D1 (SQLite), renders HTML with [askama](https://docs.rs/askama) templates and [htmx](https://htmx.org), or serves JSON (and optionally GraphQL). Everything it uses is on Cloudflare's free plan: Workers, D1, Queues (background jobs), Cron Triggers, R2 (files), Durable Objects (WebSockets), KV (cache) and Email Routing.

```sh
cargo install --git https://github.com/tgeselle/ocre.rs ocre-cli
ocre new qa --starter qa --yes
cd qa
ocre dev        # http://localhost:8787: a live Q&A app
ocre deploy     # https://qa.<your-subdomain>.workers.dev
```

Status: early. The APIs described here are the ones in the repository's `main` branch; there is no stable release yet.

## How these docs are organized

| Part | Read it when | Pages |
|---|---|---|
| Getting started | You are new: install the tools, then build and deploy a live Q&A app step by step | [Installation](getting-started/installation.md), [Tutorial](getting-started/tutorial.md) |
| Guides | You need to do one thing (add a model, send email, run a job...) | One page per domain, from [Models](guides/models.md) to [Deployment](guides/deployment.md) |
| Reference | You need exact facts: every command and flag, field types, configuration keys, limits | [CLI](reference/cli.md), [Generators](reference/generators.md), [Field types](reference/field-types.md), [Configuration](reference/configuration.md), [Limits](reference/limits.md), [API index](api-index.md) |
| Explanations | You want to know why Ocre works the way it does | [Architecture](explanations/architecture.md), [Generated code](explanations/generated-code.md), [Security model](explanations/security-model.md), [Cost model](explanations/cost-model.md) |

The Rust API of the `ocre` crate is documented item by item in the [rustdoc reference](/api/ocre/), and on one page in the [API index](api-index.md).

## For AI agents

- Every page is also plain Markdown: add `.md` to its URL (`/guides/models` is `/guides/models.md`).
- [`/llms.txt`](/llms.txt) lists every page with a one-line description; [`/llms-full.txt`](/llms-full.txt) is all pages in one file.
- Pages are self-contained: prerequisites, complete code, the commands to run and their expected output. Rust examples marked as checked compile against a real generated app.
- Each generated app has an `AGENTS.md` with the conventions, commands and free-plan limits of that app; read it first, then these docs for details.
- With `--json`, every `ocre` command prints exactly one JSON object on stdout and never prompts (see [CLI commands](reference/cli.md)).
