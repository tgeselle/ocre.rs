# Configuration

This page describes every configuration file of an Ocre app (`cloudflare.config.ts`, `wrangler.config.ts`, `package.json`, `tsconfig.json`, `.dev.vars`, `Cargo.toml`, `rust-toolchain.toml`), the binding names Ocre requires, and each Worker variable and secret the `ocre` crate reads, with its format, default, where to set it in development and production, and the error when it is missing.

## Before you start

- An Ocre app created with `ocre new` (see [CLI commands](cli.md#ocre-new)). The files below are shown as `ocre new` and the generators write them; `docs-app` stands for your app name.
- Its npm packages installed: `ocre new` runs `npm install` (skip it with `--no-install`, then run `npm install` yourself). The TypeScript files import their types from `node_modules`.
- Setting production secrets needs a Cloudflare login (`ocre login`, which runs `cf auth login`).
- Values of the free-plan limits quoted in comments are dated September 2026; see [Free-plan limits](limits.md) for the sources.

## Files at a glance

| File | Written by | Committed | Purpose |
|---|---|---|---|
| `cloudflare.config.ts` | `ocre new`, then generators (`ocre g job`, `schedule`, `cache`, `auth`, `--realtime`, attachments) and `ocre deploy` (KV ids) | yes | Worker name, logs, bindings (D1, R2, KV, Queues, Durable Objects, rate limiting, email), plain-text variables, queue consumers and cron schedules, exported Durable Object classes |
| `wrangler.config.ts` | `ocre new` | yes | The build command (`worker-build`) and the static-assets directory |
| `package.json`, `package-lock.json` | `ocre new` and its `npm install` | yes | The pinned `cf`, `wrangler` and `typescript` versions |
| `tsconfig.json` | `ocre new` | yes | Type checking of the two `.ts` files, for your editor and `npx tsc -p .` |
| `.dev.vars` | `ocre new` | no (`.gitignore`) | Secrets and variable overrides for `ocre dev` only |
| `Cargo.toml` | `ocre new`, then `ocre g api --graphql` and `--realtime` (features) | yes | Rust dependencies, Ocre features, API-only mode |
| `rust-toolchain.toml` | `ocre new` | yes | Stable Rust with the `wasm32-unknown-unknown` target |

Every `ocre` command that runs inside an app looks for the nearest `cloudflare.config.ts` in the current directory or its parents. An app that still has only a `wrangler.toml` is refused with a hint pointing to [Upgrading from wrangler.toml](../guides/upgrading.md).

## cloudflare.config.ts

The app's Cloudflare configuration, read by Cloudflare's [`cf` CLI](https://www.npmjs.com/package/cf) (`cf dev`, `cf deploy`) and by Ocre. `ocre new docs-app` writes:

```ts
// The app's Cloudflare configuration: bindings, triggers, exported classes.
// Types come from `cf/config`, so your editor shows every option; check the
// file with `npx tsc -p .` (or `ocre doctor`). `ocre g ...` adds entries at the
// `// ocre:` markers: keep them, and keep Ocre's entries as literals.
import { bindings, defineConfig, exports, triggers } from "cf/config";

export default defineConfig({
	worker: {
		name: "docs-app",
		compatibilityDate: "2026-09-01",
		// Built by worker-build (see wrangler.config.ts); `ocre dev` builds with --dev.
		entrypoint: "build/index.js",
		// Workers Logs: every request (method, URL, status, CPU time) and every
		// console line, searchable in the dashboard. Free plan: 200,000 events a
		// day, kept 3 days; beyond that, logs are sampled, never billed.
		observability: { enabled: true },
		// Files in public/ (robots.txt, images, CSS...) are served by Cloudflare
		// before the Worker runs: free, and not counted as Worker requests.
		// public/_headers sets their headers (e.g. long caching); a single-page
		// app would add `assets: { notFoundHandling: "single-page-application" },`.
		env: {
			// The first `ocre deploy` creates this database; no id needed.
			DB: bindings.d1({ name: "docs-app" }),
			// Plain-text variables. Secrets go in .dev.vars for `ocre dev` and in
			// `ocre secrets push NAME --file <file>` for production; .dev.vars
			// overrides these locally.
			// Sender for `ocre::mail::send`: "noreply@yourdomain.com" or "Name <noreply@yourdomain.com>".
			MAIL_FROM: bindings.text("docs-app <noreply@example.com>"),
			// How production sends mail (`ocre dev` only prints it: MAIL_ADAPTER=log in .dev.vars):
			// "resend" needs the RESEND_API_KEY secret (free: 100 emails/day, 3,000/month);
			// "cloudflare" needs the EMAIL binding below (any recipient needs the
			// Workers Paid plan; the free plan only reaches verified addresses of the account).
			// MAIL_ADAPTER: bindings.text("resend"),
			// Cloudflare Email Service binding, for MAIL_ADAPTER "cloudflare". `ocre dev`
			// simulates it and prints each message.
			// EMAIL: bindings.sendEmail(),
			// ocre:env
		},
		// Receiving email: run `ocre g mailbox`, deploy, then in the Cloudflare
		// dashboard Email Routing > Routing rules, send an address to this Worker.
		triggers: [
			// ocre:triggers
		],
		exports: {
			// ocre:exports
		},
	},
});
```

`defineConfig`, `bindings`, `triggers` and `exports` come from the `cf/config` module of the `cf` package, so an editor with TypeScript support completes and checks every option once `npm install` has run.

### How Ocre reads and edits it

- **Markers.** Generators insert their entries on the line right after one of three marker comments, which must stay on their own line:

  | Marker | Inside | Entries added there |
  |---|---|---|
  | `// ocre:env` | `worker.env` | bindings and variables: `DB`, `JOBS`, `STORAGE`, `CACHE`, `CHANNELS`, `AUTH_RATE_LIMITER`, `EMAIL` |
  | `// ocre:triggers` | `worker.triggers` | queue consumers (`ocre g job`) and cron schedules (`ocre g schedule`) |
  | `// ocre:exports` | `worker.exports` | the `OcreChannel` Durable Object class (first `--realtime` scaffold) |

  A generator that needs a marker which is gone stops with ``cloudflare.config.ts is missing the `// ocre:env` marker`` (or the other marker) and a hint saying where to put it back.
- **Literals only.** Ocre reads the file itself, without running Node: it looks for the `KEY: bindings.<kind>(...)`, `triggers.<kind>(...)` and `KEY: exports.<kind>(...)` calls inside `defineConfig({ worker: { ... } })` and parses their arguments as object literals (unquoted keys and trailing commas are fine; comments are skipped). Write the values Ocre needs (the worker `name`, `DB`'s `name`, queue, bucket and cron values, KV ids) as string or number literals. A variable, a template string or a spread in one of them is an error naming the canonical form, for example ``cloudflare.config.ts defines `DB` in a form Ocre cannot read`` with the hint `` write `DB: bindings.d1({ name: "docs-app" }),` ``. Values Ocre does not read can be any TypeScript.
- **Checking.** `npx tsc -p .` type-checks both `.ts` files against cf's types (a wrong `period: 30` on a rate limiter gives `Type '30' is not assignable to type '10 | 60'`). [`ocre doctor`](cli.md#ocre-doctor) runs it too, and validates the file with cf's own loader.

### Canonical entries

What each generator inserts, for an app named `docs-app` (most come with a comment on the free-plan limits, inserted above them):

| Added by | Marker | Entry |
|---|---|---|
| `ocre new` | (written in place) | `DB: bindings.d1({ name: "docs-app" }),` |
| `ocre g cache` | `// ocre:env` | `CACHE: bindings.kv(),`; `ocre deploy` rewrites it to `CACHE: bindings.kv({ id: "<id>" }),` |
| first `ocre g job` | `// ocre:env` | `JOBS: bindings.queue({ name: "docs-app-jobs" }),` |
| first `ocre g job` | `// ocre:triggers` | `triggers.queue({ name: "docs-app-jobs", deadLetterQueue: "docs-app-jobs-failed", maxBatchSize: 10, maxBatchTimeout: 5, maxRetries: 5 }),` |
| `ocre g job <Name> --queue urgent` | both | `JOBS_URGENT: bindings.queue({ name: "docs-app-jobs-urgent" }),` and its own `triggers.queue({ name: "docs-app-jobs-urgent", deadLetterQueue: "docs-app-jobs-urgent-failed", maxBatchSize: 10, maxBatchTimeout: 1, maxRetries: 5 }),` |
| `ocre g schedule` | `// ocre:triggers` | `triggers.scheduled({ schedule: "0 3 * * *" }),`, one per expression |
| first `attachment` field | `// ocre:env` | `STORAGE: bindings.r2({ name: "docs-app-storage" }),` |
| first `--realtime` scaffold | `// ocre:env` | `CHANNELS: bindings.durableObject({ worker: "docs-app", exportName: "OcreChannel" }),` |
| first `--realtime` scaffold | `// ocre:exports` | `OcreChannel: exports.durableObject({ storage: "sqlite" }),` |
| `ocre g auth` | `// ocre:env` | `AUTH_RATE_LIMITER: bindings.rateLimit({ namespace: "2964407", simple: { limit: 10, period: 60 } }),` |
| you | `// ocre:env` | `EMAIL: bindings.sendEmail(),` (uncomment it for `MAIL_ADAPTER` `"cloudflare"`) |
| you | `// ocre:env` | `KEY: bindings.text("value"),` for a plain-text variable |

`ocre g mailbox` adds nothing to the file. `ocre destroy` never edits `cloudflare.config.ts` (like `Cargo.toml`): remove the entries yourself.

### Top-level and worker keys

| Key | Value | Notes |
|---|---|---|
| `accountId` (top level) | a 32-character account id | Written before `worker:` as `accountId: "<id>",` when you pass `ocre new --account-id <id>` or pick an account in the interactive `ocre new`; needed when your login has several accounts. Without it, cf uses the logged-in account (or `CLOUDFLARE_ACCOUNT_ID`). |
| `worker.name` | the app name | The Worker name; the URL is `https://<name>.<your-subdomain>.workers.dev`. `ocre deploy` also names the KV namespaces after it (`<name>-cache`). |
| `worker.compatibilityDate` | `2026-09-01` | The workerd behavior the Worker runs with. Change it only on purpose. |
| `worker.entrypoint` | `build/index.js` | The JavaScript shim `worker-build` writes next to the WebAssembly module. |
| `worker.observability` | `{ enabled: true }` | [Workers Logs](https://developers.cloudflare.com/workers/observability/logs/workers-logs/): each request (method, URL, status, CPU time) and each line the Worker logs, including Ocre's `[ocre]` error lines, is kept and searchable in the dashboard (Workers & Pages > your Worker > Logs). On the free plan: 200,000 log events a day, kept 3 days; above that, events are sampled, never billed (September 2026). Remove it or set `enabled: false` to keep only live logs. Ocre reads nothing from it. See [Deployment](../guides/deployment.md#logs). |
| `worker.assets` | absent | `{ notFoundHandling: "single-page-application" }` for a single-page app. The directory itself is set in `wrangler.config.ts`. |

### env: DB (D1)

`DB: bindings.d1({ name: "docs-app" }),`, written by `ocre new`.

- The key must be `DB`: `ctx.db()` looks the binding up by this name, and every `ocre` database command looks for it.
- `name` is the database name. `ocre deploy` creates the database on the first deploy when `cf d1 list --name` does not find it; remote commands resolve the name to the database id the same way, so no `id` is needed.
- Migrations are the numbered SQL files of `migrations/` (cf's default directory; there is no `migrations_dir` key).

Without a `DB` entry, every `ocre` command that needs the database stops before running anything:

```text
error: cloudflare.config.ts has no D1 database bound to `DB`
hint: add `DB: bindings.d1({ name: "docs-app" }),` inside `worker.env`
```

If the Worker runs without the binding anyway (a hand-written `cf deploy` of another config), `ctx.db()?` fails with a 500 and logs ``[ocre] D1 binding `DB` is missing (...)`` followed by the fix.

### env: plain-text variables

`KEY: bindings.text("value"),` entries are plain-text Worker variables, deployed with the code on every `ocre deploy`. `ocre new` sets [`MAIL_FROM`](#mail_from) and leaves [`MAIL_ADAPTER`](#mail_adapter) commented out. Add [`ALLOWED_ORIGINS`](#allowed_origins) when another site calls the app, [`ALLOWED_HOSTS`](#allowed_hosts) to answer only on your own host names, and your own variables (read them with `ctx.env().var("NAME")`, see [Reading your own variables](#reading-your-own-variables)). Never put secrets here: the file is committed.

### env: EMAIL (send_email)

```ts
EMAIL: bindings.sendEmail(),
```

The Cloudflare Email Service binding used when `MAIL_ADAPTER` is `"cloudflare"`. `ocre new` writes it commented out; uncomment it to use it. The key must be `EMAIL`. Without it, sending fails with a 500 whose log says ``cannot send email: the send_email binding `EMAIL` is missing (...)`` followed by the fix. See [Email](../guides/email.md).

### env: CHANNELS and exports: OcreChannel (Durable Objects)

Added by the first `ocre g scaffold <Model> ... --realtime`:

```ts
// in worker.env, after // ocre:env
// The realtime channels' Durable Objects (`ocre::realtime`).
CHANNELS: bindings.durableObject({ worker: "docs-app", exportName: "OcreChannel" }),

// in worker.exports, after // ocre:exports
// Realtime channels (`ocre::realtime`): one Durable Object per channel holds the
// browsers' WebSockets, hibernated between broadcasts so idle connections cost
// no duration. The free plan only accepts SQLite-backed classes.
OcreChannel: exports.durableObject({ storage: "sqlite" }),
```

The binding key must be `CHANNELS` and the export `OcreChannel` (the class Ocre's `realtime` feature exports); `worker` is the app's own Worker name. `ocre deploy` needs no extra step: `cf deploy` creates the namespace from the export. Without the binding, `ocre::realtime::broadcast` and `WebSocketUpgrade::connect` fail with a 500 whose log names the entries to add. See [Realtime](../guides/realtime.md).

### env: STORAGE (R2)

Added by the first generator with an `attachment` field:

```ts
// Files (`ocre::storage`, `attachment` fields): an R2 bucket. `ocre dev` keeps a
// local copy under .wrangler/state; `ocre deploy` creates the bucket if needed.
// Free plan: 10 GB stored, 1M writes and 10M reads a month; deletes are free.
STORAGE: bindings.r2({ name: "docs-app-storage" }),
```

The key must be `STORAGE`. `ocre deploy` checks each R2 `name` with `cf r2 buckets get` and creates the missing ones with `cf r2 buckets create`. R2 must be enabled once in the dashboard (it asks for a payment method even for the free tier); when it is not, the deploy stops with a hint saying so (Cloudflare API code 10042). See [File storage](../guides/files.md).

### env: JOBS and triggers.queue (Queues)

Added by the first `ocre g job`:

```ts
// in worker.env
// Background jobs (`ocre g job`): the Worker sends jobs to this queue and runs
// them (src/jobs/). `ocre deploy` creates it and its dead-letter queue. Free
// plan: 10,000 Queues operations a day (a job costs 3: write, read, delete; a
// retry one more read), messages kept 24 hours, 128 KB each.
JOBS: bindings.queue({ name: "docs-app-jobs" }),

// in worker.triggers
// Runs the jobs of docs-app-jobs: up to 10 messages per run, waiting at most 5 s
// to fill a batch. Each run is one Worker request with 10 ms of CPU on the free
// plan: lower maxBatchSize for CPU-heavy jobs. A failing job is retried with a
// growing delay (30 s, 1 min, 3 min, 9 min, 27 min), then moved to
// docs-app-jobs-failed, where it stays 24 hours (dashboard: Queues > docs-app-jobs-failed).
triggers.queue({ name: "docs-app-jobs", deadLetterQueue: "docs-app-jobs-failed", maxBatchSize: 10, maxBatchTimeout: 5, maxRetries: 5 }),
```

| Key | Value | Meaning |
|---|---|---|
| binding key | `JOBS` | Required name: `ocre::jobs::enqueue` looks it up (`JOBS_<NAME>` for a named queue) |
| `name` | `<app>-jobs` | The same queue for the binding (producer) and the trigger (consumer): the app's Worker sends and runs its own jobs |
| `maxBatchSize` | `10` | Messages per consumer run (Cloudflare allows up to 100) |
| `maxBatchTimeout` | `5` | Seconds to wait to fill a batch (up to 60) |
| `maxRetries` | `5` | Deliveries after the first before the message goes to the dead-letter queue |
| `deadLetterQueue` | `<app>-jobs-failed` | Where failed messages are kept (24 hours on the free plan) |

`ocre deploy` lists the account's queues (`cf queues list`) and creates every queue named here (bindings, triggers and dead-letter queues) that is missing, before deploying. Without the binding, `enqueue` fails with a 500 whose log says to run `ocre g job <Name>`. See [Background jobs and schedules](../guides/jobs.md).

### triggers.scheduled (Cron Triggers)

Added by `ocre g schedule`, one entry per expression, run by `src/schedules/`:

```ts
triggers.scheduled({ schedule: "0 3 * * *" }),
```

Cron expressions are in UTC. The free plan allows 5 Cron Triggers per account (all Workers together); `ocre g schedule` counts the `triggers.scheduled` entries of the file and warns when the app goes past 5.

### env: CACHE (Workers KV)

Added by `ocre g cache`:

```ts
// Cached values for `ocre::cache` (Workers KV), added by `ocre g cache`. The
// first `ocre deploy` creates the namespace and writes its id here; `ocre dev`
// uses a local one. Free plan: 100,000 reads and 1,000 writes a day, 1 GB.
CACHE: bindings.kv(),
```

The key must be `CACHE`. For each KV binding without an `id`, `ocre deploy` looks for a namespace titled `<name>-<binding>` (lowercased, `_` becomes `-`: `docs-app-cache`), creates it when missing, and rewrites the entry to `CACHE: bindings.kv({ id: "<id>" }),`. Commit that change so every machine deploys to the same namespace. Without the binding, `ocre::cache` functions fail with a 500 whose log says to run `ocre g cache`. See [Caching](../guides/caching.md).

### env: AUTH_RATE_LIMITER (rate limiting)

Added by `ocre g auth`:

```ts
// `ocre g auth`: login, sign-up, token and emailed-link routes allow 10 attempts
// a minute per IP address and Cloudflare location (Workers Rate Limiting,
// free plan, no storage used). `period` is 10 or 60 seconds.
AUTH_RATE_LIMITER: bindings.rateLimit({ namespace: "2964407", simple: { limit: 10, period: 60 } }),
```

| Key | Value | Meaning |
|---|---|---|
| binding key | `AUTH_RATE_LIMITER` | The binding the generated `throttle` (in `src/auth_api.rs`) passes to `ocre::security::rate_limit` |
| `namespace` | an integer as a string, derived from the app name | Bindings with the same namespace share counters across the account's Workers: keep it unique per app |
| `simple.limit` | `10` | Requests allowed per key and period; the next one gets `429 Too Many Requests` |
| `simple.period` | `60` | The window in seconds: `10` or `60` only (the type rejects anything else) |

Add more `bindings.rateLimit(...)` entries with other keys for your own routes and call `ocre::security::rate_limit(&ctx, "NAME", &key).await?` (see [Sessions, flash and security](../guides/security.md#rate-limiting)). The binding is on the free plan and uses no D1, KV or Durable Object operation; counters are per Cloudflare location and approximate; `ocre dev` simulates it. Without the binding, `rate_limit` fails with a 500 whose log names the entry to add.

### Binding names Ocre requires

| Binding | cloudflare.config.ts entry | Used by | Added by |
|---|---|---|---|
| `DB` | `bindings.d1` | `ctx.db()`, models, `ocre migrate`, `ocre sql`, `ocre db ...` | `ocre new` |
| `STORAGE` | `bindings.r2` | `ocre::storage`, `attachment` fields | first generator with an `attachment` field |
| `JOBS` | `bindings.queue` (plus `triggers.queue`) | `ocre::jobs::enqueue`, `enqueue_in`, `ocre::mail::deliver_later` | first `ocre g job` |
| `CACHE` | `bindings.kv` | `ocre::cache` | `ocre g cache` |
| `CHANNELS` | `bindings.durableObject` (export `OcreChannel`) | `ocre::realtime` | first `--realtime` scaffold |
| `EMAIL` | `bindings.sendEmail` | `ocre::mail` with `MAIL_ADAPTER` `"cloudflare"` | commented out by `ocre new` |

The names are constants of the crate ([`ocre::storage::STORAGE_BINDING`](/api/ocre/storage/constant.STORAGE_BINDING.html), [`ocre::jobs::QUEUE_BINDING`](/api/ocre/jobs/constant.QUEUE_BINDING.html), [`ocre::cache::CACHE_BINDING`](/api/ocre/cache/constant.CACHE_BINDING.html), [`ocre::realtime::CHANNELS_BINDING`](/api/ocre/realtime/constant.CHANNELS_BINDING.html), [`ocre::mail::EMAIL_BINDING`](/api/ocre/mail/constant.EMAIL_BINDING.html)); they cannot be renamed. Every missing-binding error is an `Error::Internal`: the visitor gets a 500 page (or `{"error": {"status": 500, "message": ...}}` from JSON handlers), and the Worker log gets the full message prefixed with `[ocre]`, including the fix.

Rate limiting bindings are the exception: `ocre::security::rate_limit` takes the binding name as an argument, so `AUTH_RATE_LIMITER` is only the name the `ocre g auth` code uses (`RATE_LIMITER` in `src/auth_api.rs`).

## wrangler.config.ts

`cf dev`, `cf build` and the build step of `cf deploy` hand the build to the app's own wrangler (the `wrangler` devDependency), which reads this file:

```ts
// How the Rust code is built and where the static files are, read by cf
// (`cf dev`, `cf deploy`) through the app's wrangler. Bindings and triggers
// live in cloudflare.config.ts.
import { defineWranglerConfig } from "wrangler/experimental-config";

export default defineWranglerConfig({
	// `ocre dev` sets OCRE_BUILD=--dev (fast, unoptimized); deploys build --release.
	build: { command: 'cargo install -q "worker-build@^0.8" && worker-build ${OCRE_BUILD:---release}' },
	assetsDirectory: "public",
	types: { generate: false },
});
```

`build.command` builds the Rust crate to WebAssembly with `worker-build` (installed with `cargo install` on first use). `${OCRE_BUILD:---release}` makes the mode depend on the `OCRE_BUILD` environment variable:

| Command | `OCRE_BUILD` | Build |
|---|---|---|
| `ocre dev` | `--dev` | Unoptimized, fast to compile |
| `ocre deploy` and every other `ocre` command that builds | `--release` | `lto = true`, `opt-level = "z"` (from `Cargo.toml`) |
| `npx cf deploy` run by hand | unset | `--release` (the default after `:-`) |

Chain a CSS or JavaScript bundler before `worker-build` in `build.command` (see [Assets](../guides/assets.md)).

`assetsDirectory: "public"`: files in `public/` are served by Workers Static Assets before the Worker runs. Requests that match a file cost no Worker request and no CPU ([billing](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/)). Limits on the free plan: 20,000 files per Worker version, 25 MiB per file ([limits](https://developers.cloudflare.com/workers/platform/limits/#static-assets), September 2026).

## package.json

```json
{
  "name": "docs-app",
  "private": true,
  "type": "module",
  "devDependencies": {
    "cf": "1.0.0-beta.5",
    "typescript": "5.9.3",
    "wrangler": "4.144.0"
  }
}
```

- The versions are pinned exactly, and `ocre new`'s `npm install` writes `package-lock.json`: commit both, so every machine and CI run the same `cf`. `ocre doctor` warns when the installed `cf` differs from the version Ocre expects, or `wrangler` is older than 4.136.
- `cf` runs dev, deploy and every Cloudflare API call; `wrangler` is what cf delegates the build to, and what Ocre uses for local D1 commands (see [Why wrangler still appears](../guides/deployment.md#why-wrangler-still-appears)); `typescript` checks the config files.
- `"type": "module"` lets cf load `cloudflare.config.ts` without a module-type warning on every call.
- Node.js 22 or newer is required (`cf`'s `engines`).

## tsconfig.json

```json
{
  "compilerOptions": {
    "module": "nodenext",
    "moduleResolution": "nodenext",
    "target": "es2022",
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true
  },
  "include": ["cloudflare.config.ts", "wrangler.config.ts"]
}
```

It only covers the two config files: without it, editors and `tsc` cannot resolve `cf/config`. Run `npx tsc -p .` after editing `cloudflare.config.ts` by hand; nothing is emitted.

## .dev.vars

`ocre new` writes a git-ignored `.dev.vars` with a random local secret and the `log` mail adapter:

```text
SECRET_KEY_BASE=2db19cad9ab790ae4ef78858a74ecbc0300efd7e700b6513da53bcbd89e2b69e4eb28523c9cd31ea53617afc4495996fac76e80bc29233a1ae1dd464bfe3dd80
MAIL_ADAPTER=log
```

- Format: one `NAME=value` per line (dotenv).
- Only `ocre dev` (`cf dev`) reads it; it never reaches Cloudflare. A name in both `.dev.vars` and a `bindings.text(...)` entry takes the `.dev.vars` value locally, which is how `MAIL_ADAPTER=log` keeps development from sending real mail.
- `.gitignore` lists `.dev.vars`, `.dev.vars.*`, `.prod.vars`, `.env` and `.env.*`. Each clone needs its own: copy the lines above with a value from `ocre secret`.
- `ocre dev` prints what the Worker receives; secrets are hidden (output of an app named `ftapp` with an attachment field):

```text
Using secrets defined in .dev.vars
Your Worker has access to the following bindings:
Binding                                                  Resource                  Mode
env.DB (ftapp)                                           D1 Database               local
env.STORAGE (ftapp-storage)                              R2 Bucket                 local
env.MAIL_FROM ("ftapp <noreply@example.com>")            Environment Variable      local
env.SECRET_KEY_BASE ("(hidden)")                         Environment Variable      local
env.MAIL_ADAPTER ("(hidden)")                            Environment Variable      local
```

## Variables and secrets

Cloudflare gives a Worker two kinds of text values, read the same way in code:

| | Variables (`bindings.text`) | Secrets |
|---|---|---|
| Where | `cloudflare.config.ts`, committed | Encrypted on Cloudflare; never in the repository |
| Set in production | edit `cloudflare.config.ts`, then `ocre deploy` | `ocre secrets push NAME --file .prod.vars` (values read from that git-ignored file, never from the command line) |
| Set for `ocre dev` | `bindings.text(...)`, or `.dev.vars` to override | `.dev.vars` |
| Use for | Sender address, adapter names, allowed origins and hosts | `SECRET_KEY_BASE`, API keys, OAuth client secrets |

```sh
# .prod.vars: NAME=value lines, git-ignored, never committed
ocre secrets push RESEND_API_KEY --file .prod.vars          # uploads the value from the file
ocre secrets list                                           # names only, never values
ocre secret                                                 # a new SECRET_KEY_BASE value for .prod.vars
```

The free plan allows 64 variables and secrets per Worker, 5 KB each ([limits](https://developers.cloudflare.com/workers/platform/limits/#environment-variables), September 2026).

The `ocre` crate reads the following names.

### SECRET_KEY_BASE

- Kind: secret. Required as soon as a request touches the session, the flash or a JWT.
- Meaning: the root key of the app. The session cookie's AES-256-GCM key is derived from it, and `ocre::jwt` derives its HS256 signing key from it with a different label, so one secret covers cookies and tokens.
- Format: at least 64 characters. `ocre secret` prints a new random value of 128 hex characters (`--json`: `{"command":"secret","ok":true,"secret":"..."}`).
- Development: `ocre new` writes one to `.dev.vars`.
- Production: `ocre deploy` checks `cf workers secrets list`; when the Worker has no `SECRET_KEY_BASE` (first deploy), it generates one and uploads it with the deploy (`cf deploy --secrets-file`, from a temporary `.wrangler/ocre-secrets.env` readable only by you and deleted afterwards) and prints `Created the SECRET_KEY_BASE secret on Cloudflare`. An existing secret is never replaced. If the secrets list fails for another reason than a missing Worker, the deploy stops rather than risk overwriting it.
- Rotation: uploading a new value (`ocre secret`, put it in `.prod.vars` as `SECRET_KEY_BASE=...`, then `ocre secrets push SECRET_KEY_BASE --file .prod.vars`) alone makes every session cookie and JWT signed with the old value invalid: everyone is signed out. To rotate without signing anyone out, keep the old value in [`SECRET_KEY_BASE_PREVIOUS`](#secret_key_base_previous) first.
- Errors: when it is missing or shorter than 64 characters, requests that read an existing session cookie or change the session answer 500 and log (captured from `ocre dev` with the line removed from `.dev.vars`):

```text
✘ [ERROR] [ocre] the SECRET_KEY_BASE secret is not set. Fix: run `ocre secret`, put the value in .dev.vars as SECRET_KEY_BASE=... for `ocre dev` (`ocre new` does this), and deploy with `ocre deploy`, which uploads it
```

The short-value message is `SECRET_KEY_BASE is shorter than 64 characters.` followed by the same fix. Requests that do not use the session (no cookie, no flash) still work, which is why a missing secret can go unnoticed until the first form submission. See [Sessions, flash and security](../guides/security.md).

### SECRET_KEY_BASE_PREVIOUS

- Kind: secret. Optional; set only while rotating `SECRET_KEY_BASE`.
- Meaning: previous `SECRET_KEY_BASE` values (Rails' `cookies_rotations`). A session cookie encrypted with one of them is still read, then re-encrypted with the current key in the same response; a JWT signed with one of them still verifies until it expires.
- Format: comma-separated, newest first; spaces around values are ignored; each value at least 64 characters.
- Production: Worker secrets cannot be read back, so upload the value you are about to replace as `SECRET_KEY_BASE_PREVIOUS` together with the new one. In `.prod.vars` (git-ignored):

```text
SECRET_KEY_BASE_PREVIOUS=<the current SECRET_KEY_BASE>
SECRET_KEY_BASE=<a new value from `ocre secret`>
```

```sh
ocre secrets push SECRET_KEY_BASE_PREVIOUS SECRET_KEY_BASE --file .prod.vars   # one upload
```

- Removal: delete `SECRET_KEY_BASE_PREVIOUS` in the dashboard (Workers & Pages > your Worker > Settings > Variables and Secrets) once the longest session lifetime (two weeks for `ocre g auth`) and the JWT lifetime (one hour) have passed; visitors who did not come back by then start with an empty session.
- Errors: a value shorter than 64 characters makes every request that uses the session or a JWT answer 500 and log `SECRET_KEY_BASE_PREVIOUS has a value shorter than 64 characters. Fix: list old SECRET_KEY_BASE values, comma-separated, newest first`.

### ALLOWED_ORIGINS

- Kind: variable (`bindings.text`). Optional.
- Meaning: other origins (a separate frontend, an admin app) allowed to call this app from a browser. Listed origins get CORS headers (methods GET, POST, PUT, PATCH, DELETE; headers `Content-Type`, `Authorization`, `Accept`; credentials allowed) and pass the CSRF check that otherwise refuses cross-site unsafe requests with 403.
- Format: comma-separated origins, scheme and host (and port if any). Spaces and a trailing `/` are ignored; invalid entries are skipped.

```ts
// in worker.env of cloudflare.config.ts
ALLOWED_ORIGINS: bindings.text("https://app.example.com, https://admin.example.com"),
```

- Default: unset or empty, no CORS layer at all: only same-origin browser requests may change data.
- Development: add it to `worker.env` (or `.dev.vars`) with the frontend's local origin, such as `http://localhost:5173`.
- Errors: none; a malformed entry is dropped silently, so check the spelling when a request still gets 403. See [Sessions, flash and security](../guides/security.md).

### ALLOWED_HOSTS

- Kind: variable (`bindings.text`). Optional.
- Meaning: the host names the app answers to (Rails' `config.hosts`). A request for any other `Host` gets a plain-text `403 Forbidden: blocked host. Add it to ALLOWED_HOSTS to allow it.` before the session, CSRF check or any handler runs.
- Format: comma-separated host names, case-insensitive, without scheme or port. An entry starting with `.` also allows every subdomain: `.example.com` allows `example.com` and `www.example.com`.

```ts
ALLOWED_HOSTS: bindings.text("example.com, .example.com"),
```

- Default: unset or empty, every host is allowed.
- Always allowed: `localhost`, `127.0.0.1` and `[::1]`, with any port, so `ocre dev` keeps working.
- Why on Workers: Cloudflare only routes your own host names to the Worker, but the same Worker also answers on `<name>.<subdomain>.workers.dev` and on preview URLs. Listing only your custom domain keeps visitors and search engines on it (list the `workers.dev` host too while you still use it).
- Errors: none at startup; a typo blocks your own domain with the 403 above.

### OAuth client secrets

- Names: `GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET`, `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET` (`client_id_secret` and `client_secret_secret` of `ocre::oauth::GITHUB` and `GOOGLE`).
- Kind: secrets. Required for each provider given to `ocre g auth --oauth`.
- Values: the client id and secret of the OAuth app you register with the provider, with the callback URL `https://<your host>/auth/<provider>/callback` (and `http://localhost:8787/auth/<provider>/callback` for `ocre dev`, in a second OAuth app for GitHub).
- Development: `ocre g auth --oauth` appends commented lines to `.dev.vars`; uncomment them and fill in the values.
- Production: put both in `.prod.vars` and run `ocre secrets push GITHUB_CLIENT_ID GITHUB_CLIENT_SECRET --file .prod.vars`; `ocre secrets list` shows which are set.
- Errors (500, logged) when one is missing: pressing "Continue with GitHub" logs ``the GITHUB_CLIENT_ID secret is not set. Fix: add it to .dev.vars, and `ocre secrets push GITHUB_CLIENT_ID --file .prod.vars` ``; the callback logs ``the GITHUB_CLIENT_SECRET secret is not set. Fix: put it in .dev.vars for `ocre dev` and run `ocre secrets push GITHUB_CLIENT_SECRET --file .prod.vars` for production``. See [Authentication](../guides/authentication.md#continue-with-github-or-google).

### MAIL_ADAPTER

- Kind: variable. Required to send mail (`ocre::mail::send`, `deliver_later`, the auth emails).
- Values: `log` (print the whole email to the Worker log between `[ocre mail]` lines, send nothing), `resend` (Resend's HTTP API, needs [`RESEND_API_KEY`](#resend_api_key)), `cloudflare` (Email Service through the [`EMAIL`](#env-email-send_email) binding). Surrounding spaces are ignored.
- Default: none. Nothing is guessed from which keys exist, so a development machine holding a real key never sends by accident.
- Development: `ocre new` writes `MAIL_ADAPTER=log` to `.dev.vars`.
- Production: uncomment `MAIL_ADAPTER: bindings.text("resend"),` (or `"cloudflare"`) in `worker.env`, then `ocre deploy`.
- Errors (500, logged):
  - unset: ``cannot send email: MAIL_ADAPTER is not set. Fix: set MAIL_ADAPTER to "resend" (with the RESEND_API_KEY secret) or "cloudflare" (with the EMAIL: bindings.sendEmail() binding) in worker.env of cloudflare.config.ts, as MAIL_ADAPTER: bindings.text("resend"); `ocre new` puts MAIL_ADAPTER=log in .dev.vars so `ocre dev` only logs mail``
  - another value, for example `smtp`: `cannot send email: unknown MAIL_ADAPTER "smtp" (expected log, resend or cloudflare). Fix: ...` (same fix).

See [Email](../guides/email.md).

### MAIL_FROM

- Kind: variable. Required to send mail, with every adapter.
- Format: `noreply@yourdomain.com` or `Name <noreply@yourdomain.com>` (the name may be quoted). With Resend or Cloudflare, the domain must be verified with that provider.
- Default: `ocre new` writes `MAIL_FROM: bindings.text("<app> <noreply@example.com>")`; replace `example.com` with your domain before sending real mail.
- Errors (500, logged): unset: `cannot send email: MAIL_FROM is not set. Fix: add MAIL_FROM: bindings.text("App <noreply@yourdomain.com>") to worker.env in cloudflare.config.ts`; unparsable: `cannot send email: MAIL_FROM "<value>" is not an address. Fix: use "noreply@yourdomain.com" or "App <noreply@yourdomain.com>"`.

### RESEND_API_KEY

- Kind: secret. Required when `MAIL_ADAPTER = "resend"`.
- Format: the API key from [resend.com/api-keys](https://resend.com/api-keys), sent as `Authorization: Bearer <key>` to `https://api.resend.com/emails`.
- Production: `ocre secrets push RESEND_API_KEY --file .prod.vars`.
- Development: not needed with `MAIL_ADAPTER=log`. To send real mail from `ocre dev`, put `RESEND_API_KEY=...` and `MAIL_ADAPTER=resend` in `.dev.vars`.
- Errors (500, logged) when missing or blank: ``cannot send email: the RESEND_API_KEY secret is not set. Fix: create a key at https://resend.com/api-keys and run `ocre secrets push RESEND_API_KEY --file .prod.vars` (and put it in .dev.vars to send from `ocre dev`)``.
- Free plan of Resend: 100 emails a day, 3,000 a month ([quotas](https://resend.com/docs/knowledge-base/account-quotas-and-limits), September 2026).

### Reading your own variables

`ctx.env()` gives the raw Workers environment for any other variable or secret:

```rust,check
// src/support.rs
use axum::{Router, extract::State, routing::get};
use ocre::{Ctx, Result};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/support", get(support))
}

/// `SUPPORT_EMAIL: bindings.text(...)` in cloudflare.config.ts; a default when unset.
async fn support(State(ctx): State<Ctx>) -> Result<String> {
    let email = ctx
        .env()
        .var("SUPPORT_EMAIL")
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "support@example.com".to_owned());
    Ok(format!("Write to {email}"))
}
```

Register the module in `src/lib.rs` (`mod support;` under `// ocre:modules`, `.merge(support::routes())` under `// ocre:routes`). For a secret, use `ctx.env().secret("NAME")` the same way.

## Environment variables of the ocre CLI

These are read by the `ocre` command (and cf) on your machine, not by the Worker:

| Name | Read by | Meaning |
|---|---|---|
| `OCRE_BUILD` | `build.command` of `wrangler.config.ts` | `--dev` or `--release`; set by `ocre` for every build (see [wrangler.config.ts](#wranglerconfigts)) |
| `CLOUDFLARE_API_TOKEN` | cf | Authenticates without `ocre login` (CI); the CLI's hints mention it when a Cloudflare call fails |
| `CLOUDFLARE_ACCOUNT_ID` | cf | Picks the account when the token or login sees several, instead of `accountId` in `cloudflare.config.ts` |
| `CF_SEND_TELEMETRY` | cf | `false` turns off cf's anonymous usage telemetry (on by default; `cf cli telemetry disable` does the same for good). Ocre does not change it |

## Cargo.toml

`ocre new docs-app` (full-stack, with `--ocre-path`) writes:

```toml
[package]
name = "docs-app"
version = "0.1.0"
edition = "2024"
publish = false

# Standalone: the app builds even when created inside another Cargo workspace.
[workspace]

[lib]
crate-type = ["cdylib"]

[dependencies]
ocre = { path = "/path/to/ocre/crates/ocre" }
worker = { version = "0.8.7", features = ["http", "axum", "d1"] }
axum = { version = "0.8.9", default-features = false, features = ["form", "json", "query"] }
askama = "0.16.1"
serde = { version = "1.0", features = ["derive"] }

# `strip` breaks wasm-bindgen ("externref table required"); keep symbols.
[profile.release]
lto = true
codegen-units = 1
opt-level = "z"
```

Without `--ocre-path`, the dependency is `ocre = { git = "https://github.com/tgeselle/ocre.rs" }`.

### Ocre features

| Feature | Default | Enables | Turned on by |
|---|---|---|---|
| `html` | yes | askama templates, `render`, HTML error pages, the `Htmx` extractor | on in full-stack apps; off in API-only apps (`default-features = false`) |
| `graphql` | no | `ocre::graphql` (async-graphql, GraphiQL) | `ocre g api <Model> ... --graphql`, which also adds the `async-graphql` dependency |
| `realtime` | no | `ocre::realtime` and the `OcreChannel` Durable Object class | the first `ocre g scaffold <Model> ... --realtime` |

After both generators, the dependency lines read:

```toml
ocre = { path = "/path/to/ocre/crates/ocre", features = ["realtime", "graphql"] }
async-graphql = { version = "7.2.1", default-features = false, features = ["graphiql", "custom-error-conversion"] }
```

GraphQL adds about 1.1 MB of WebAssembly and 20 to 60 ms of CPU each time a Worker instance starts (see [Cost model](../explanations/cost-model.md)); only turn it on when a client needs it.

### API-only mode

`ocre new docs-app --api` writes Ocre without its default `html` feature, no `askama`, and this table at the end:

```toml
[dependencies]
ocre = { git = "https://github.com/tgeselle/ocre.rs", default-features = false }
worker = { version = "0.8.7", features = ["http", "axum", "d1"] }
axum = { version = "0.8.9", default-features = false, features = ["form", "json", "query"] }
serde = { version = "1.0", features = ["derive"] }
...

[package.metadata.ocre]
# JSON only: `ocre g scaffold` generates APIs, Ocre's `html` feature is off.
mode = "api"
```

The generators read `mode = "api"`: `ocre g scaffold` then generates a JSON API (like `ocre g api`), and `ocre g mailer` builds the email text with `format!` instead of askama templates.

## rust-toolchain.toml

```toml
[toolchain]
channel = "stable"
targets = ["wasm32-unknown-unknown"]
```

rustup reads it in the app directory and installs the stable toolchain with the WebAssembly target on first use. A Rust installed without rustup (for example Homebrew's `rust`) ignores it and has no wasm target; `ocre dev`, `ocre deploy` and `ocre new --deploy` then stop with ``the wasm32-unknown-unknown target is not installed for rustc at <sysroot>`` and the hint ``use a rustup toolchain (Homebrew's `rust` has no wasm target) and run `rustup target add wasm32-unknown-unknown` ``. See [Installation](../getting-started/installation.md).

### LOG_LEVEL

- Kind: variable. Optional.
- Value: `debug`, `info`, `warn`, `error` or `off` (`warning` and `fatal` are accepted): the lowest level `ctx.log()` and Ocre write. Default: `debug` in `ocre dev` (debug build), `info` after `ocre deploy` (release build). An unknown value is the default.
- Read on every request, job batch and cron run. See [Errors, logging and debugging](../guides/debugging.md#levels-and-formats).

### LOG_FORMAT

- Kind: variable. Optional.
- Value: `json` (one JavaScript object per line, whose fields Workers Logs indexes) or `text` (`INFO message key=value ...`). Default: `text` in `ocre dev`, `json` after `ocre deploy`.

### SENTRY_DSN

- Kind: secret. Optional; read only when the app registers `ocre::errors::Sentry` (see [Reporting errors](../guides/debugging.md#sending-reports-to-sentry)).
- Value: the project's DSN, `https://<key>@<host>/<project id>`, from Sentry or a Sentry-compatible service.
- When missing, reports are only logged. A value that is not a DSN is logged as ``[ocre] SENTRY_DSN is not a Sentry DSN (...)`` and nothing is sent. The optional `SENTRY_RELEASE` variable names the release in each event.

## See also

- [CLI commands](cli.md): [`ocre dev`](cli.md#ocre-dev), [`ocre deploy`](cli.md#ocre-deploy), [`ocre secret`](cli.md#ocre-secret).
- [Generators](generators.md): which generator adds which `cloudflare.config.ts` entry.
- [Upgrading from wrangler.toml](../guides/upgrading.md): converting an older app.
- [Deployment](../guides/deployment.md): the first deploy, secrets and remote migrations.
- [Free-plan limits](limits.md) and [Cost model](../explanations/cost-model.md).
- `npx cf schema <command>` and `npx cf cli search "..."` for cf's own commands and options; the `cf/config` types for every key of `cloudflare.config.ts`.
