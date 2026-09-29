// The app's Cloudflare configuration: bindings, triggers, exported classes.
// Types come from `cf/config`, so your editor shows every option; check the
// file with `npx tsc -p .` (or `ocre doctor`). `ocre g ...` adds entries at the
// `// ocre:` markers: keep them, and keep Ocre's entries as literals.
import { bindings, defineConfig, exports, triggers } from "cf/config";

export default defineConfig({
	worker: {
		name: "__APP_NAME__",
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
			DB: bindings.d1({ name: "__APP_NAME__" }),
			// Plain-text variables. Secrets go in .dev.vars for `ocre dev` and in
			// `ocre secrets push NAME --file <file>` for production; .dev.vars
			// overrides these locally.
			// Sender for `ocre::mail::send`: "noreply@yourdomain.com" or "Name <noreply@yourdomain.com>".
			MAIL_FROM: bindings.text("__APP_NAME__ <noreply@example.com>"),
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
		// Cron Triggers (`ocre g schedule`, UTC, run by src/schedules/; free plan:
		// 5 per account) and queue consumers (`ocre g job`).
		triggers: [
			// ocre:triggers
		],
		exports: {
			// ocre:exports
		},
	},
});
