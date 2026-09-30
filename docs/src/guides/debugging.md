# Errors, logging and debugging

This page shows how an Ocre app logs (levels, request-scoped fields, JSON lines for Workers Logs), how errors become HTTP responses, how to report them to a service such as Sentry, what the development error page and the `Server-Timing` header show, how to read a deployed Worker's logs with `ocre logs`, and how to debug a Worker, which has no interactive debugger.

## Before you start

- An Ocre app created with `ocre new`. The examples are plain handlers; add them to `routes()` in `src/lib.rs`.
- `ocre dev` runs a debug build: the development error page, `Server-Timing` and `debug` log lines only exist there. `ocre deploy` builds in release mode.
- Reading production logs with `ocre logs` needs a deployed Worker and a wrangler login (see [Live logs](#live-logs-ocre-logs)).
- Free-plan numbers are dated September 2026; see [Free-plan limits](../reference/limits.md).

## Logging

Every [`Ctx`](https://docs.rs/ocre/latest/ocre/struct.Ctx.html) carries a logger, `ctx.log()`. In a request, each of its lines carries the request's `request_id`, `method` and `path`, so the lines of one request can be found together; `Logger::with` adds fields (Rails' tagged logging):

```rust,check
// src/payments.rs
use axum::extract::{Path, State};
use ocre::{Ctx, Result};

pub async fn pay(State(ctx): State<Ctx>, Path(order_id): Path<i64>) -> Result<&'static str> {
    let log = ctx.log().with("order_id", order_id);
    log.info("payment started");
    // Formatted only when `debug` lines are on: no cost in production.
    log.debug(format_args!("cart: {:?}", [3, 5]));
    if order_id > 1_000_000 {
        log.warn("unusually large order id");
    }
    Ok("OK")
}
```

In `ocre dev` the terminal shows:

```text
INFO payment started method=POST order_id=7 path=/orders/7/pay request_id=950f7d1fe51ecfdb
DEBUG cart: [3, 5] method=POST order_id=7 path=/orders/7/pay request_id=950f7d1fe51ecfdb
```

After `ocre deploy`, the same call writes a JavaScript object, whose fields Workers Logs indexes (filter on `request_id`, `order_id`... in the dashboard):

```json
{"level": "info", "message": "payment started", "method": "POST", "order_id": 7, "path": "/orders/7/pay", "request_id": "8c2f1a0b9d3e4f5a-CDG"}
```

### Levels and formats

Two Worker variables configure logging. Set them in `.dev.vars` for `ocre dev`, and as `bindings.text(...)` entries of `worker.env` in `cloudflare.config.ts` for production:

| Variable | Values | Default in `ocre dev` | Default after `ocre deploy` |
|---|---|---|---|
| `LOG_LEVEL` | `debug`, `info`, `warn`, `error`, `off` (`warning`, `fatal` accepted) | `debug` | `info` |
| `LOG_FORMAT` | `json`, `text` | `text` | `json` |

```ts
// cloudflare.config.ts, inside worker.env
LOG_LEVEL: bindings.text("warn"),
```

Lines go to `console.debug`, `console.info`, `console.warn` or `console.error`, so the level also shows in the dashboard. Rails' `:fatal` and `:unknown` are `error`. Code without a `Ctx` (a helper function, a unit test) uses `ocre::log::Logger::new()`.

### Sensitive fields

Fields whose name contains a fragment of `ocre::security::FILTERED_PARAMETERS` (`passw`, `email`, `secret`, `token`, `_key`...) are written as `[FILTERED]`, at any depth, like Rails' `filter_parameters`. The message itself is not filtered: put values in fields, not in the message.

### What Ocre logs

| Line | Level | When |
|---|---|---|
| `GET /posts 200 in 40 ms (db: 3 queries, 12 ms)` | `debug` | every request |
| `SQL (1.2 ms) SELECT ...` with `duration_ms` (and `failed` on errors) | `debug` | every D1 statement through `ctx.db()` |
| `[ocre] <message>` with `error_class`, `handled`, `source` | `error` | a handler returned `Error::Internal` (a 500) |
| `panicked at src/posts.rs:12:5: <message>` | `error` | a panic, which ends the request |
| `[ocre jobs] ...`, `[ocre cron] ...`, `[ocre mail] ...` | `info` / `error` | jobs, cron runs, mail |

Durations come from the Workers clock, which only advances while the Worker waits for I/O: they measure time spent in D1 and other bindings, not CPU time. Workers Logs shows each request's CPU time.

### Free plan

Workers Logs (`observability: { enabled: true }` in `cloudflare.config.ts`) keeps 200,000 events a day for 3 days; each request is one event, plus one per line it logs. Above that, events are sampled, never billed. Ocre's own per-request and SQL lines are `debug`, so they cost nothing at the default production level `info`.

## Errors as responses

Handlers return `ocre::Result<T>` (HTML pages) or `ocre::ApiResult<T>` (JSON). Each `ocre::Error` variant knows its status, and only client errors show their message:

| Error | Status | The client sees |
|---|---|---|
| `Error::NotFound` (`option.or_404()?`) | 404 | `Not found` |
| `Error::bad_request("...")` | 400 | its message |
| `Error::Unauthorized` | 401 | `Unauthorized` |
| `Error::Forbidden` | 403 | `Forbidden` |
| `Error::Conflict("...")` | 409 | its message |
| `Error::PayloadTooLarge("...")` | 413 | its message |
| `Error::Invalid(fields)` (validations) | 422 | `Validation failed` and the field errors |
| `Error::TooManyRequests` | 429 | `Too many requests. Try again later.` |
| `Error::internal("...")`, and `?` on a `worker::Error` | 500 | `Internal server error`; the message is logged and reported |

JSON errors are `{"error": {"status": 404, "message": "Not found"}}` (plus `"fields"` for a 422). For any other status, return a plain axum response: `(StatusCode::IM_A_TEAPOT, Json(...))`. Wrap a foreign error with a message that names the fix: `.map_err(|err| Error::internal(format!("price feed unreadable ({err}). Fix: ...")))?`.

## Reporting errors

`ctx.errors()` is Rails' `Rails.error`: a report is logged at once, with the request's fields and its context, then handed to every subscriber the app registered.

```rust,check
// src/rates.rs
use axum::extract::State;
use ocre::{Ctx, Result, errors::{Options, Severity}};

pub async fn refresh(State(ctx): State<Ctx>) -> Result<String> {
    // Added to every report of this request.
    ctx.errors().set_context("feed", "ecb");

    // Rails.error.handle: report the error as handled and go on with a fallback.
    let rates: Vec<f64> = ctx.errors().handle(fetch_rates().await).unwrap_or_default();

    // Rails.error.record: report it as unhandled, then fail the request.
    ctx.errors().record(save(&rates).await)?;

    // Rails.error.report, with options.
    if rates.is_empty() {
        let options = Options::new().severity(Severity::Info).context("day", ocre::now() / 86_400).source("rates");
        ctx.errors().report(&"no exchange rates today", options);
    }
    Ok(format!("{} rates", rates.len()))
}

async fn fetch_rates() -> Result<Vec<f64>> {
    Ok(vec![1.08, 0.86])
}

async fn save(_rates: &[f64]) -> Result<()> {
    Ok(())
}
```

| Call | Rails | Handled | Severity |
|---|---|---|---|
| `ctx.errors().report(&err, Options::new())` | `Rails.error.report` | `true` (option) | `warning` (option) |
| `ctx.errors().handle(result)` returns `Option<T>` | `Rails.error.handle`; `.unwrap_or(x)` is `fallback:` | `true` | `warning` |
| `ctx.errors().record(result)` returns `result` | `Rails.error.record` | `false` | `error` |
| `ctx.errors().unexpected("...")` | `Rails.error.unexpected`: panics in debug builds, reports in release builds | `true` | `error` |
| `ctx.errors().set_context(key, value)` | `Rails.error.set_context`, reset for each request, job batch and cron run | | |
| `Options::new().except("sentry")` | `Rails.error.disable(subscriber)` for one report | | |

`handle` and `record` only see errors of their `Result`'s type; other errors propagate with `?` first, which is Rails' filter by exception class.

Ocre reports on its own, with `handled: false`: a handler's `Error::Internal` (source `ocre.request`, context `request_id`, `method`, `path`), a failed job (`ocre.job`, with `job` and `retried`), a failed cron run (`ocre.cron`) and a failed inbound email handler (`ocre.mailbox`).

### Sending reports to Sentry

`ocre::errors::Sentry` sends each report to Sentry, or to any service that speaks Sentry's envelope protocol (GlitchTip, Bugsink...). Register it once per Worker instance, in the `start` function `ocre new` writes in `src/lib.rs` (Ocre's initializers):

```rust
#[event(start)]
fn start() {
    ocre::errors::subscribe(ocre::errors::Sentry);
}
```

Then give the Worker the project's DSN as a secret:

```sh
echo "SENTRY_DSN=https://<key>@o123.ingest.sentry.io/456" >> .prod.vars
ocre secrets push SENTRY_DSN --file .prod.vars
```

Without `SENTRY_DSN` (in `ocre dev`, for instance) nothing is sent. The optional `SENTRY_RELEASE` variable sets the release. Each event carries the error (type, message, source, `handled`), the severity, the environment (`development` or `production`), `request_id` as a tag and the context, filtered like log fields, as extra data.

Deliveries happen after the handler, before the response goes out: an error response waits for them. Each report is one subrequest (the free plan allows 50 per request); a request without errors sends nothing. A failed delivery is logged as a `warn` line and never retried.

### Other services

A subscriber turns a report into the HTTP `POST` to send; Ocre sends it with `fetch`:

```rust,check
// src/error_webhook.rs
use ocre::errors::{Delivery, Report, Subscriber};

/// Posts each error to a chat webhook (URL in the ERRORS_WEBHOOK_URL secret).
pub struct Webhook;

impl Subscriber for Webhook {
    fn name(&self) -> &'static str {
        "webhook"
    }

    fn deliver(&self, report: &Report, vars: &dyn Fn(&str) -> Option<String>) -> Option<Delivery> {
        let url = vars("ERRORS_WEBHOOK_URL")?;
        let text = format!("[{}] {}: {}", report.severity.as_str(), report.source, report.message);
        let body = ocre::serde_json::json!({ "text": text }).to_string();
        Some(Delivery { url, headers: vec![("content-type".into(), "application/json".into())], body })
    }
}
```

Register it in `start()` with `ocre::errors::subscribe(error_webhook::Webhook);`.

## The development error page

In `ocre dev`, a 500 shows the development error page instead of `templates/error.html`: the internal message, the request (id, method, path, query and headers, with cookies, `Authorization` and sensitive names filtered) and the D1 statements it ran, with their durations. A JSON error gets the message as `error.detail`:

```json
{"error": {"status": 500, "message": "Internal server error", "detail": "D1 query failed: no such table: posts. SQL: SELECT ..."}}
```

Release builds never show internal details. Two headers help too:

- `X-Request-Id` on every response: the id of the request's log lines and error reports (Cloudflare's `CF-Ray` id when present). Handlers get it with the `ocre::RequestId` extractor.
- `Server-Timing` in `ocre dev`: `db;dur=12;desc="3 queries", total;dur=40`, shown by the browser's developer tools (Network, then Timing).

## Live logs: `ocre logs`

`ocre logs` streams the deployed Worker's logs as they happen: each request, its console lines and uncaught exceptions. Ctrl-C stops it.

```sh
ocre logs                                  # everything, readable
ocre logs --status error                   # failed invocations only
ocre logs --search checkout --format json  # one JSON object per event
```

It runs the app's `wrangler tail` (cf 1.0.0-beta.5 has no tail command), which uses wrangler's own login, separate from `ocre login`: run `npx wrangler login` in the app once, or set `CLOUDFLARE_API_TOKEN`. Logs of the last 3 days are in the dashboard: Workers & Pages, your Worker, Logs.

## Debugging a Worker

A Worker has no process to attach to, so Rails' `debug` gem, `binding.break` and web-console have no equivalent. What works:

- **Log.** `ctx.log().debug(format_args!("{value:?}"))` shows any `Debug` value in the `ocre dev` terminal; nothing is written in production at the default level.
- **Panics** are logged with their location (`panicked at src/posts.rs:12:5: ...`) before the request fails.
- **In a template**, `{{ value|json }}` prints a serializable value and `{{ "{:?}"|format(value) }}` its `Debug` form (Rails' `debug` and `inspect` helpers).
- **Unit tests** run natively: `cargo test` (or `ocre test`) can use a debugger such as `rust-lldb` or an IDE on the app's model and helper code.
- **Chrome DevTools.** The local dev server started by `ocre dev` exposes the V8 inspector (wrangler's `--inspector-port`, 9229 by default): open `chrome://inspect` to see console output and record CPU profiles of the WebAssembly module. Breakpoints in Rust source are not practical: the module has no source maps.
- **Memory.** Each Worker isolate has 128 MB; memory is freed when the isolate is recycled. Keep request data bounded (`LIMIT` queries, streamed files) rather than hunting leaks with Valgrind-style tools.

## See also

- [Configuration](../reference/configuration.md): `LOG_LEVEL`, `LOG_FORMAT`, `SENTRY_DSN`, typed settings with `ctx.config()`.
- [Deployment](deployment.md#logs): Workers Logs in the dashboard.
- [Controllers and routing](controllers.md): handlers and their errors.
- API: `ocre::log`, `ocre::errors`, `ocre::Error` in the [API index](../api-index.md).
