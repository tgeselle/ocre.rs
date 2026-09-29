# Upgrading from wrangler.toml

Earlier versions of Ocre generated apps configured by a `wrangler.toml` and ran `npx wrangler` for everything. Ocre now drives Cloudflare's [`cf` CLI](https://www.npmjs.com/package/cf) and reads `cloudflare.config.ts`. This page converts an existing app, step by step: Cloudflare's own converter (`cf migrate`) does most of it, then a few Ocre-specific fixes finish the job.

Until an app is converted, every `ocre` command run in it stops with:

```text
error: /path/to/blog uses wrangler.toml; Ocre now reads cloudflare.config.ts
hint: convert it: `npm install --save-dev --save-exact cf@1.0.0-beta.5 wrangler@4.144.0`, then `npx cf migrate --no-install`, then apply the Ocre fixes of the upgrading guide (<docs>/guides/upgrading.html)
```

## Before you start

- Node.js 22 or newer (`node --version`): cf needs it.
- The new `ocre` CLI installed (`cargo install --git https://github.com/tgeselle/ocre.rs ocre-cli`).
- The app in git with a clean worktree: `cf migrate` refuses to run otherwise (or pass `--force`). Commit or stash first, so every change below shows up in one diff.
- About 15 minutes. Nothing here touches Cloudflare until the last step.

## 1. Install the pinned packages

In the app directory:

```sh
npm install --save-dev --save-exact cf@1.0.0-beta.5 wrangler@4.144.0 typescript@5.9.3
```

This creates `package.json` (if the app had none), `package-lock.json` and `node_modules/`. Then edit `package.json` so it reads like the one `ocre new` writes, with `"type": "module"` (without it, every cf call warns while loading `cloudflare.config.ts`):

```json
{
  "name": "blog",
  "private": true,
  "type": "module",
  "devDependencies": {
    "cf": "1.0.0-beta.5",
    "typescript": "5.9.3",
    "wrangler": "4.144.0"
  }
}
```

The versions must stay exact (no `^`): Ocre is tested with these, and `ocre doctor` warns when the installed ones differ.

## 2. Run cf migrate

```sh
npx cf migrate --no-install
```

It reads `wrangler.toml` and writes two files next to it, `cloudflare.config.ts` (bindings, triggers, variables) and `wrangler.config.ts` (the build command and the `public/` assets directory), and leaves `wrangler.toml` in place. On an Ocre app it exits with code 1 and says the migration "requires follow-up work": that is expected, the next section is the follow-up.

`wrangler.config.ts` needs no change. It should read:

```ts
import { defineWranglerConfig } from "wrangler/experimental-config";

export default defineWranglerConfig({
	build: { command: 'cargo install -q "worker-build@^0.8" && worker-build ${OCRE_BUILD:---release}' },
	types: { generate: false },
	assetsDirectory: "public",
});
```

## 3. Finish cloudflare.config.ts

cf converts most entries: crons become `triggers.scheduled(...)`, the queue consumer becomes `triggers.queue({ name, deadLetterQueue, maxBatchSize, maxBatchTimeout, maxRetries })`, `[vars]` become `bindings.text(...)`, and the D1, KV, R2, queue, rate-limiting and email bindings get their `bindings.*` form. Then fix these:

1. **Remove the stop sign.** Delete the `throw new Error("Migration incomplete…")` line cf put in the file, and its TODO comments once each is dealt with below.
2. **`migrations_dir`.** cf flags it as "requires manual migration". Drop it: there is no such key, and cf's default directory, `./migrations`, is where Ocre keeps migrations.
3. **Durable Object binding** (realtime apps). cf flags it for review. Keep what it generated, `CHANNELS: bindings.durableObject({ worker: "blog", exportName: "OcreChannel" }),` (with your app name as `worker`; the key is required).
4. **`[[migrations]]`** (realtime apps). cf reports it as unsupported. Replace it with the export of the class, inside `worker`:

   ```ts
   exports: {
   	// ocre:exports
   	OcreChannel: exports.durableObject({ storage: "sqlite" }),
   },
   ```

5. **Markers.** Ocre's generators add entries after three marker comments. Put each on its own line: `// ocre:env` inside `worker.env`, `// ocre:triggers` inside `worker.triggers` (add `triggers: [ ... ],` if the app has none yet) and `// ocre:exports` inside `worker.exports` (add `exports: { ... },` if needed). The import line must bring all four names: `import { bindings, defineConfig, exports, triggers } from "cf/config";`.
6. **Literals.** Ocre reads the file without running it, so the values it needs (the worker `name`, `DB`'s `name`, queue, bucket and cron values, KV ids) must be plain string or number literals, as cf writes them. See [Configuration](../reference/configuration.md#how-ocre-reads-and-edits-it).

A converted blog with a job, a cache and realtime looks like this (comments and your own variables may differ):

```ts
import { bindings, defineConfig, exports, triggers } from "cf/config";

export default defineConfig({
	worker: {
		name: "blog",
		compatibilityDate: "2026-09-01",
		entrypoint: "build/index.js",
		observability: { enabled: true },
		env: {
			DB: bindings.d1({ name: "blog" }),
			MAIL_FROM: bindings.text("blog <noreply@example.com>"),
			// ocre:env
			JOBS: bindings.queue({ name: "blog-jobs" }),
			CACHE: bindings.kv({ id: "0f2a..." }),
			CHANNELS: bindings.durableObject({ worker: "blog", exportName: "OcreChannel" }),
		},
		triggers: [
			// ocre:triggers
			triggers.queue({ name: "blog-jobs", deadLetterQueue: "blog-jobs-failed", maxBatchSize: 10, maxBatchTimeout: 5, maxRetries: 5 }),
			triggers.scheduled({ schedule: "0 3 * * *" }),
		],
		exports: {
			// ocre:exports
			OcreChannel: exports.durableObject({ storage: "sqlite" }),
		},
	},
});
```

Compare with the file a new app gets ([Configuration](../reference/configuration.md#cloudflareconfigts)) if something looks off; `ocre new scratch --no-install` in a temporary directory writes a fresh one to copy from, along with an `AGENTS.md` that no longer mentions wrangler.

## 4. Add tsconfig.json and update .gitignore

Create `tsconfig.json`, so editors and `tsc` resolve `cf/config`:

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

Make sure `.gitignore` lists:

```text
node_modules/
.wrangler/
.cloudflare/
.dev.vars
.dev.vars.*
.prod.vars
.env
.env.*
```

`.cloudflare/` is cf's build output; `.env*` files may hold cf credentials (`CLOUDFLARE_API_TOKEN`). Commit `package.json` and `package-lock.json`.

Then check the config:

```sh
npx tsc -p .
```

It prints nothing when the file is right, and names the line of any wrong value (`period: 30` on a rate limiter: `Type '30' is not assignable to type '10 | 60'`).

## 5. Delete wrangler.toml

```sh
git rm wrangler.toml
```

From now on `cloudflare.config.ts` is the only Cloudflare configuration. `.dev.vars` needs no change: `ocre dev` still reads it. The local database under `.wrangler/state` is the one `ocre dev` and `ocre migrate` keep using; `ocre migrate --status` shows whether anything is pending.

## 6. Log in again and check

cf keeps its own login, separate from wrangler's, so log in once more (CI keeps using `CLOUDFLARE_API_TOKEN`, see [Deployment](deployment.md#ci)):

```sh
ocre login
ocre doctor
```

`ocre doctor` checks Node.js, the installed `cf` and `wrangler` against the pins, the login, `cloudflare.config.ts` with cf's own loader and `tsc`, the bindings your code uses, pending migrations and secrets. Fix what it reports, then run `ocre dev` and try the app locally before deploying.

cf sends anonymous usage telemetry by default; `npx cf cli telemetry disable` (or `CF_SEND_TELEMETRY=false`) turns it off.

## Realtime apps: deploy to a preview first

How `cf deploy` maps `exports: { OcreChannel: exports.durableObject({ storage: "sqlite" }) }` onto a Worker first deployed by wrangler with the `[[migrations]]` tag `ocre-realtime-v1` has **not been verified yet**. Durable Object migrations cannot be undone, and Cloudflare refuses rollbacks across them.

So before deploying a realtime app to production, deploy it to a preview first: a second Worker that was deployed once with your previous Ocre (the same app under another `name`, deployed with the old CLI), converted as above, then deployed with the new `ocre deploy`. Check that the deploy succeeds and that realtime pages still connect and receive broadcasts. If it fails, keep production on the previous Ocre and report it; this page will document the extra step once it is known.

Apps without realtime have no Durable Objects, and deploy with `ocre deploy` as before.

## See also

- [Configuration](../reference/configuration.md): every entry of `cloudflare.config.ts`, the markers, `wrangler.config.ts`, `package.json` and `tsconfig.json`.
- [Deployment](deployment.md): what `ocre deploy` does now, and [why wrangler still appears](deployment.md#why-wrangler-still-appears).
- [CLI commands](../reference/cli.md): every command and its errors.
