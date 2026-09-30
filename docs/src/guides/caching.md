# Caching

Ocre has opt-in caching tools chosen for the Workers free plan: `ocre::cache` keeps slow or costly results (JSON values or rendered HTML fragments) in Workers KV, every request remembers its own `SELECT` results and KV reads, and `CacheControl`, `ETag` and `Conditional` let browsers reuse pages with `304 Not Modified` answers. This page shows them, the KV write budget that limits the first, and why Ocre does not wrap Cloudflare's own page caches.

## Before you start

- An Ocre app created with `ocre new`.
- For values in KV: run `ocre g cache` once; it adds the `CACHE` binding (see [Setting up the KV cache](#setting-up-the-kv-cache)). HTTP caching needs no setup.
- The examples use the `Post` model of the blog starter (`ocre new <name> --starter blog`) and the `I18n` extractor, which needs `ocre g locale` (see [Translations](i18n.md)); drop the locale from the ETag in an app without translations.

## Which tool

| | KV values (`ocre::cache::fetch`) | HTTP (`CacheControl`, `ETag`, `Conditional`) |
|---|---|---|
| Saves | Slow or costly work: D1 aggregates over many rows, third-party API calls | Rendering and bandwidth: `304 Not Modified` without a body |
| Where the copy lives | Workers KV, global, eventually consistent (up to 60 s) | The browser (and Cloudflare with Workers Cache, below) |
| Free plan (September 2026) | 100,000 reads and **1,000 writes** a day, 1 GB stored ([KV limits](https://developers.cloudflare.com/kv/platform/limits/)) | Free: no KV or D1 operation |
| Setup | `ocre g cache` | None |

## Setting up the KV cache

```sh
ocre g cache
```

```text
  update  cloudflare.config.ts

Next:
  use it: ocre::cache::fetch(&ctx, "key:v1", Duration::from_secs(3600), || async { ... }).await?
  ocre dev
  ocre deploy (creates the KV namespace)
```

It adds this to `cloudflare.config.ts`, after the `// ocre:env` marker:

```ts
// Cached values for `ocre::cache` (Workers KV), added by `ocre g cache`. The
// first `ocre deploy` creates the namespace and writes its id here; `ocre dev`
// uses a local one. Free plan: 100,000 reads and 1,000 writes a day, 1 GB.
CACHE: bindings.kv(),
```

Running it again fails without changing anything:

```text
error: cloudflare.config.ts already has the `CACHE` binding
hint: nothing to generate: call `ocre::cache::fetch(&ctx, key, ttl, || async { ... })` in a handler
```

- `ocre dev` uses a local namespace (under `.wrangler/state`); its startup bindings table lists `env.CACHE` as a local `KV Namespace`.
- `ocre deploy` gives each `bindings.kv()` entry without an `id` the namespace titled `<worker>-<binding>` (`blog-cache`): it links an existing namespace with that title, or runs `cf kv namespaces create`, then rewrites the entry to `CACHE: bindings.kv({ id: "..." }),`. Commit that change so every later deploy uses the same namespace.
- The binding is not in `ocre new` apps because KV writes are the scarcest free resource (see [The write budget](#the-write-budget)).

Without the binding, every `ocre::cache` function fails with a 500 whose log line names the fix:

```text
KV binding `CACHE` is missing (...). Fix: run `ocre g cache`, which adds `CACHE: bindings.kv(),` to worker.env in cloudflare.config.ts
```

## Caching a value with fetch

`ocre::cache::fetch(&ctx, key, ttl, compute)` is Rails' `Rails.cache.fetch`: it returns the value stored under `key`, or runs `compute`, stores its result for `ttl` and returns it. Values are the JSON of any `Serialize + Deserialize` type.

This endpoint returns post counts, computed at most once an hour per location, and has a second route that forgets them:

```rust,check
// src/stats.rs
use std::time::Duration;

use axum::{Json, Router, extract::State, http::StatusCode, routing::{get, post}};
use ocre::{Ctx, Result, params};
use serde::{Deserialize, Serialize};

/// Cache key of the stats. Bump `v1` whenever `Stats` changes shape.
const KEY: &str = "stats:v1";

/// What `GET /stats` returns (and what KV stores, as JSON).
#[derive(Serialize, Deserialize)]
pub struct Stats {
    pub posts: i64,
    pub published: i64,
    /// Unix time of the computation: equal across responses while the value is cached.
    pub computed_at: i64,
}

pub fn routes() -> Router<Ctx> {
    Router::new().route("/stats", get(show)).route("/stats/refresh", post(refresh))
}

/// One KV read per request; the query runs only on a miss (then one KV write).
async fn show(State(ctx): State<Ctx>) -> Result<Json<Stats>> {
    let stats = ocre::cache::fetch(&ctx, KEY, Duration::from_secs(3600), || compute(&ctx)).await?;
    Ok(Json(stats))
}

/// Forgets the cached value, so the next `GET /stats` recomputes it (one KV write).
async fn refresh(State(ctx): State<Ctx>) -> Result<StatusCode> {
    ocre::cache::delete(&ctx, KEY).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compute(ctx: &Ctx) -> Result<Stats> {
    #[derive(Deserialize)]
    struct Counts {
        posts: i64,
        published: i64,
    }
    let sql = "SELECT COUNT(*) AS posts, COALESCE(SUM(published), 0) AS published FROM posts";
    let counts: Option<Counts> = ctx.db()?.first(sql, params![]).await?;
    let (posts, published) = counts.map_or((0, 0), |c| (c.posts, c.published));
    Ok(Stats { posts, published, computed_at: ocre::now() })
}
```

Register it in `src/lib.rs` (`mod stats;` under `// ocre:modules`, `.merge(stats::routes())` under `// ocre:routes`). With `ocre dev` running and two posts, one published (real run):

```sh
curl -s http://localhost:8787/stats
curl -s http://localhost:8787/stats
curl -s -o /dev/null -w "%{http_code}\n" -X POST http://localhost:8787/stats/refresh
curl -s http://localhost:8787/stats
```

```text
{"posts":2,"published":1,"computed_at":1790656352}
{"posts":2,"published":1,"computed_at":1790656352}
204
{"posts":2,"published":1,"computed_at":1790656360}
```

The second call was a hit (same `computed_at`); after the delete, the value was computed again.

### Rules for keys and values

- **Version the key.** Stored values are the JSON of the type. Put a version in the key (`stats:v1`) and change it when the type changes. A stored value that no longer decodes is logged (`` [ocre cache] decode `stats:v1` failed: ...; recomputing it (change the key to avoid this) ``) and treated as a miss.
- **Never share per-user data.** A key is global to the app: put the user id in keys of per-user values (`dashboard:v1:user:42`).
- **TTL of 60 seconds or more.** KV refuses shorter TTLs; `fetch` and `write` return a 500 naming the fix (`cache TTL ... is below KV's minimum of 60 seconds`). Durations are truncated to whole seconds. Prefer an hour or more (see the budget below).
- **Keys** are 1 to 512 bytes.
- **Eventual consistency.** Other locations may read the old value for up to 60 seconds after a `write` or `delete`. Do not cache values that must be exact right after a change (a balance, a stock count used to accept an order).
- **Invalidate after the write**: call `ocre::cache::delete(&ctx, key)` after the database change behind the value succeeded, for example in the handler that updates a post.

### Failure behavior

`fetch` never fails because of KV itself. When a KV read or write fails (past a daily limit, for example), it logs `` [ocre cache] read `stats:v1` failed: ... `` (or `write`) and computes the value, so the page keeps working, only slower. It fails only for a missing binding, an invalid key or TTL, a value that does not serialize to JSON, or an error returned by `compute` (returned unchanged; nothing is stored).

### read, write and delete

The explicit forms, for when a closure does not fit (a value refreshed by a scheduled task, for example):

| Function | Returns | KV cost | On KV failure |
|---|---|---|---|
| `ocre::cache::fetch(&ctx, key, ttl, compute)` | `Result<T>` | 1 read; a miss adds 1 write | Logged; computes the value |
| `ocre::cache::read::<T>(&ctx, key)` | `Result<Option<T>>`: `None` when absent, expired or undecodable | 1 read | Logged; `None` |
| `ocre::cache::write(&ctx, key, &value, ttl)` | `Result<()>` | 1 write | Error (500) |
| `ocre::cache::delete(&ctx, key)` | `Result<()>`; deleting an absent key succeeds | 1 write | Error (500) |
| `ocre::cache::clear(&ctx, prefix, limit)` | `Result<Cleared>`: how many were deleted, and `more` when keys with the prefix remain | 1 list + 1 write per key | Error (500) |

```rust
// in a scheduled task: refresh the value before visitors ask for it
let rates = fetch_rates(ctx).await?;
ocre::cache::write(ctx, "rates:v1", &rates, Duration::from_secs(6 * 3600)).await?;

// in a handler: use it if present
let rates: Option<Rates> = ocre::cache::read(&ctx, "rates:v1").await?;
```

`clear` is Rails' `Rails.cache.clear`, bounded: it deletes at most `limit` values whose key starts with `prefix` (`""` for all, `"views/"` for fragments), since each delete spends one of the free plan's 1,000 daily KV writes. Call it again, for example from a scheduled task, while `more` is true:

```rust,ignore
let cleared = ocre::cache::clear(&ctx, "views/", 200).await?;
ctx.log().info(format_args!("cleared {} fragments, more: {}", cleared.deleted, cleared.more));
```

Bumping the version in the keys (`v1` to `v2`) needs no delete at all: old values expire with their TTL.

## Caching HTML fragments

askama templates are compiled Rust and cannot wait for KV, so Rails' `<% cache post do %>` becomes a call in the handler: `ocre::cache::fragment(&ctx, &key, ttl, || template)` returns the fragment stored under `key`, or renders the template, stores the HTML and returns it. The result is an `ocre::cache::Fragment`, which the page template writes with `{{ card }}`: it was escaped when it was rendered, so it needs no `|safe`. `ocre::cache::fragments` does the same for a list, with one KV bulk read for all its items (Rails' `render collection:, cached: true`); only the missing items are rendered and written.

```rust,check
// src/cached_posts.rs
use std::time::Duration;

use askama::Template;
use axum::{Router, extract::State, response::Html, routing::get};
use ocre::{Ctx, Page, Result, cache::{self, Fragment}, render};

use crate::models::post::{self, Post};

/// One row; its HTML is cached per post and version.
#[derive(Template)]
#[template(source = r#"<li id="post_{{ post.id }}"><strong>{{ post.title }}</strong> {{ post.body }}</li>"#, ext = "html")]
struct Row<'a> {
    post: &'a Post,
}

#[derive(Template)]
#[template(source = "<ul>{% for row in rows %}{{ row }}{% endfor %}</ul>", ext = "html")]
struct Index {
    rows: Vec<Fragment>,
}

pub fn routes() -> Router<Ctx> {
    Router::new().route("/cached-posts", get(index))
}

/// One KV bulk read for the page; each post changed since its last view costs one render and one KV write.
async fn index(State(ctx): State<Ctx>, page: Page) -> Result<Html<String>> {
    let posts = post::all(&ctx, page).await?;
    let rows = cache::fragments(
        &ctx,
        &posts,
        Duration::from_secs(7 * 86_400),
        // Bump `row-v1` when the Row template changes.
        |post| cache::key(&[&"posts", &post.id, &post.updated_at, &"row-v1"]),
        |post| Row { post },
    )
    .await?;
    render(&Index { rows })
}
```

### Keys

`ocre::cache::key(&[&"posts", &post.id, &post.updated_at, &"row-v1"])` joins its parts with `/` (`posts/1/2026-09-29 14:05:00/row-v1`), like Rails' `cache_key_with_version`; keys longer than 256 bytes become `sha256/<hex>`. Fragments are stored under `views/<key>`.

- **Records**: the id and `updated_at` of every record the fragment shows. Editing a record changes its key, so the old fragment is never read again and expires with its TTL: no `delete`, no extra write.
- **Template version**: Rails adds a digest of the template and its partials to the key. Ocre keeps a version you write (`row-v1`) and bump when the template changes, so a deploy does not invalidate every fragment at once (each rewrite is a KV write out of 1,000 a day).
- **Everything else the HTML depends on**: the locale (`i18n.locale()`) when the fragment is translated, the user's id for per-user HTML. Never cache HTML with a CSRF token or a CSP nonce in it.
- **Nested fragments (Russian doll caching)**: the outer key includes the newest `updated_at` of the records inside, e.g. `cache::key(&[&"posts", &post.id, &post.updated_at, &newest_comment_at, &"card-v1"])` with `newest_comment_at` from `SELECT MAX(updated_at) FROM comments WHERE post_id = ?1`. Rails' `touch: true` does the same by updating the parent's `updated_at` when a child changes; do it in the child's save when the parent's key only uses `post.updated_at`.
- **Formats**: a fragment is plain HTML under a free-form key, so the same fragment serves a page, an htmx swap, a realtime broadcast (`fragment.into_string()`) or an email; add the format to the key only when the HTML differs.

### When to cache fragments

Only when rendering costs noticeable CPU (long lists, Markdown, heavy formatting) and the records are read far more often than they change: a miss costs a KV read plus a write, a hit a KV read. To cache only sometimes (Rails' `cache_if`), put an `if` around the call and render the template directly otherwise. Failures behave like `fetch`: a KV error is logged and the template is rendered.

## Per-request caches

Two caches live in the `Ctx` of one request (or one queued job, or one cron run) and cost nothing:

- **Query cache**: a `SELECT` run twice with the same parameters through `ctx.db()` (or `ctx.db_named(..)`) is answered from memory the second time, without a D1 round trip or rows read, like Rails' query cache. Any other statement (`execute`, `batch`, `INSERT ... RETURNING` through `first`) empties it, so a request always reads its own writes. It keeps up to 100 results. `ctx.db()?.uncached()` bypasses it for a query that must see changes made by other requests during this one.
- **Local cache**: `fetch`, `read` and `fragment` read each KV key at most once per request, and remember what `write` and `delete` did (Rails' local cache).

Nothing is shared between requests: Worker instances have no memory you can rely on.

## Turning caching off

The Worker variable `CACHE_STORE` selects the store without code changes (Rails' `config.cache_store`):

| `CACHE_STORE` | Store |
|---|---|
| absent or `kv` | Workers KV, the `CACHE` namespace |
| `null` | None: `fetch` and `fragment` always compute, `read` finds nothing, `write` and `delete` do nothing, no KV operation |

To develop without the cache (Rails' `bin/rails dev:cache`), add `CACHE_STORE=null` to `.dev.vars` and restart `ocre dev`; remove the line to turn it back on. Any other value is a 500 naming the two valid ones. To empty the cache in production, bump the version in your keys: old entries expire with their TTL, which costs no write (deleting keys one by one would cost one write each).

## The write budget

The free plan allows 1,000 KV writes a day (writes and deletes, to different keys; one write per second per key) and 100,000 reads ([KV limits](https://developers.cloudflare.com/kv/platform/limits/), September 2026). Every `fetch` is a read; every miss, `write` and `delete` is a write.

A key refreshed every `ttl` seconds costs up to 86,400 / `ttl` writes a day, per key:

| TTL | Writes a day, per key | Keys that fit in 1,000 writes |
|---|---|---|
| 60 s (minimum) | 1,440 | none: one key alone is over the budget |
| 5 minutes | 288 | 3 |
| 1 hour | 24 | about 40 |
| 1 day | 1 | about 1,000 |

Use TTLs of an hour or more and few keys. Keys that embed an id (`post:42:v1`) multiply writes by the number of ids requested: keep them for values read far more often than they change. Each `delete` after an update is one more write. Past the daily write limit, `fetch` keeps answering by computing every time.

## HTTP caching: 304 without rendering

`Conditional` is an extractor for the request's `If-None-Match`; `conditional.fresh_when(etag, cache_control, render)` answers `304 Not Modified` without calling `render` when the browser already has this version, like Rails' `fresh_when`. Otherwise it renders and adds the `ETag` and `Cache-Control` headers. The database query that builds the ETag still runs; the template does not. No KV or D1 operation is added.

```rust,check
// src/articles.rs
use askama::Template;
use axum::{
    Router,
    extract::{Path, State},
    response::Response,
    routing::get,
};
use ocre::{
    Ctx, OptionExt, Result,
    cache::{CacheControl, Conditional, ETag},
    i18n::I18n,
    render,
};

use crate::models::post::{self, Post};

#[derive(Template)]
#[template(
    source = r#"<!doctype html>
<html lang="{{ i18n.locale() }}">
<title>{{ post.title }}</title>
<h1>{{ post.title }}</h1>
<p>{{ post.body }}</p>
</html>"#,
    ext = "html"
)]
struct ArticleView {
    post: Post,
    i18n: I18n,
}

pub fn routes() -> Router<Ctx> {
    Router::new().route("/articles/{id}", get(show))
}

/// 304 without rendering when the browser already has this version of the page.
async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>, i18n: I18n, conditional: Conditional) -> Result<Response> {
    let post = post::find(&ctx, id).await?.or_404()?;
    // Everything the page shows: the post (its `updated_at` changes on every edit) and the locale.
    let etag = ETag::of(&(&post, i18n.locale()))?;
    conditional.fresh_when(etag, CacheControl::no_cache(), || render(&ArticleView { post, i18n }))
}
```

A real run against `ocre dev`. The first request renders the page:

```sh
curl -si http://localhost:8787/articles/1
```

```text
HTTP/1.1 200 OK
Transfer-Encoding: chunked
Content-Type: text/html; charset=utf-8
Cache-Control: private, no-cache
ETag: W/"fc878d99b9decd186a69ff08a7b0f4d6"
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0

<!doctype html>
<html lang="en">
<title>Hello</title>
<h1>Hello</h1>
<p>First post</p>
</html>
```

Sending the ETag back, as a browser does, gets a 304 without a body:

```sh
curl -si -H 'If-None-Match: W/"fc878d99b9decd186a69ff08a7b0f4d6"' http://localhost:8787/articles/1
```

```text
HTTP/1.1 304 Not Modified
Cache-Control: private, no-cache
ETag: W/"fc878d99b9decd186a69ff08a7b0f4d6"
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0
```

The same `If-None-Match` with `Accept-Language: fr` gets `200 OK` (the locale is part of the tag), and so does the English page after the post was edited (`updated_at` changed; the new tag was `W/"db7674b7d119cab19e03b8c1a8fa5a0d"`).

### Building the ETag

- `ETag::of(&data)?` hashes the JSON of any serializable data: `ETag::of(&(&posts, i18n.locale()))?`. `ETag::new(version)` hashes a version string you build, cheaper for large data: `ETag::new(format!("{}-{}", post.id, post.updated_at))`. Both give a weak tag, `W/"<32 hex characters>"` (the first 128 bits of a SHA-256). `ETag::strong(version)` gives a strong one, `"<32 hex characters>"` (Rails' `strong_etag:`), which promises byte-identical bodies: use it for exact files or exports, not for pages with nonces or CSRF tokens.
- Put everything the page shows in it: the records, the locale, the signed-in user's id, the flash messages (`flash.notice()`, `flash.alert()`). A page that shows something the tag does not cover can be served stale from the browser's copy.
- Only GET and HEAD requests are considered; for other methods the client is never fresh. Comparison is weak (RFC 9110): `W/"x"` matches `"x"`, and `*` matches any tag.

### Cache-Control policies

`CacheControl` is a response part: return it in a tuple, `(CacheControl::public(Duration::from_secs(300)), Html(page))`, or pass it to `fresh_when`.

| Constructor | Header | Use for |
|---|---|---|
| `CacheControl::no_store()` | `no-store` | Secrets, one-time tokens |
| `CacheControl::no_cache()` | `private, no-cache` | Per-visitor pages, with an `ETag`: the browser keeps a copy and asks every time |
| `CacheControl::private(ttl)` | `private, max-age=N` | Per-visitor data that may be stale for N seconds without asking |
| `CacheControl::public(ttl)` | `public, max-age=N` | Responses identical for every visitor |
| `.stale_while_revalidate(window)` | adds `stale-while-revalidate=N` | After `max-age`, serve the old copy while fetching a new one (ignored by `no_store` and `no_cache`) |

Use `public` only for responses that are the same for everyone: no session data, no flash, and the locale in the path rather than taken from a cookie or `Accept-Language`.

## Serving pages without running the Worker

Two Cloudflare caches can answer requests before the Worker runs. Ocre wraps neither; here is why, as of September 2026:

- The [Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/) (`caches.default`) only works on custom domains: on `*.workers.dev`, where Ocre apps deploy by default, `put` does nothing. It is also local to one data center, and the Worker still runs (and counts) for every request.
- [Workers Cache](https://developers.cloudflare.com/workers/cache/) (`cache: { enabled: true }` in `worker` of `cloudflare.config.ts`) works on `workers.dev` too and serves `CacheControl::public(..)` responses from Cloudflare's tiered cache: hits use no CPU. But on the free plan every hit still counts toward the 100,000 requests a day, and turning it on also counts requests for static assets in `public/`, which are otherwise free ([pricing](https://developers.cloudflare.com/workers/cache/#pricing)). Its cache key ignores cookies and `Accept-Language`, so only mark responses `public` when they are the same for every visitor; responses with `Set-Cookie` are never stored.

For most free-plan apps, `no_cache` pages with an `ETag` (cheap 304s) and KV values for the expensive parts are the better trade.

## Coming from Rails

| Rails | Ocre |
|---|---|
| `Rails.cache.fetch/read/write/delete` | `ocre::cache::fetch/read/write/delete` |
| `cache` view helper, `cache_key_with_version` | `ocre::cache::fragment` in the handler, `ocre::cache::key` |
| `render collection:, cached: true` | `ocre::cache::fragments` (KV bulk reads) |
| Template digests | A version in the key, bumped by hand |
| `touch: true` (Russian doll) | Children's newest `updated_at` in the parent's key |
| Solid Cache, Redis, Memcached, file and memory stores | Workers KV, the one store; `CACHE_STORE=null` for none |
| Query cache, local cache | Per-request caches in `Ctx` |
| `fresh_when`, `stale?`, `http_cache_forever` | `Conditional::fresh_when`, `CacheControl::public(..)` |
| `bin/rails dev:cache` | `CACHE_STORE=null` in `.dev.vars` |

## See also

- [Generators](../reference/generators.md#ocre-g-cache): `ocre g cache`.
- [Configuration](../reference/configuration.md#env-cache-workers-kv): the `CACHE` entry.
- [CLI commands](../reference/cli.md#ocre-deploy): what `ocre deploy` provisions.
- [Translations](i18n.md): the locale in ETags.
- [File storage](files.md): files have their own ETag/304 handling in `storage::serve`.
- [Free-plan limits](../reference/limits.md) and [Cost model](../explanations/cost-model.md).
- [`ocre::cache` rustdoc](/api/ocre/cache/index.html).
