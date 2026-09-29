# JSON APIs and GraphQL

The `ocre g api` generator writes a JSON REST resource under `/api/<plural>` whose handlers call the model, and `--graphql` exposes the same resource on `/graphql`; errors are JSON with the HTTP status, field errors included. This page covers the generated API, hand-written JSON endpoints, GraphQL, API-only apps and cross-origin clients.

## Before you start

- An Ocre app from `ocre new` (full-stack or `--api`). The examples use `ocre g api Product name:string^ price:float stock:integer? --graphql` in the blog starter app, and `ocre dev` serving `http://localhost:8787`.
- Nothing else: no binding beyond the `DB` database every app has.
- Free plan (September 2026): 100,000 requests a day and 10 ms of CPU per request ([Workers limits](https://developers.cloudflare.com/workers/platform/limits/)); D1 counts every row a query scans ([D1 pricing](https://developers.cloudflare.com/d1/platform/pricing/)), which is why lists are capped at 100 rows. GraphQL costs more, see [GraphQL](#graphql).

## Generate an API

```sh
ocre g api Product name:string^ price:float stock:integer? --graphql
```

```text
  create  migrations/0005_create_products.sql
  create  src/models/product.rs
  create  src/products_api.rs
  create  src/graphql.rs
  update  src/models/mod.rs
  update  src/lib.rs
  update  Cargo.toml

Next:
  ocre migrate
  ocre dev
  curl http://localhost:8787/api/products
  open http://localhost:8787/graphql
```

Without `--graphql`, only the migration, the model and `src/products_api.rs` are written and registered. When the model already exists (after `ocre g model` or `ocre g scaffold`), it is reused and only the API is added; the fields are still required on the command line, so pass the model's. In an API-only app, `ocre g scaffold` does the same as `ocre g api`. See [Generators](../reference/generators.md#ocre-g-api).

### Routes

| Method | Path | Handler | Success |
|---|---|---|---|
| GET | `/api/products?limit=&offset=` | `index` | 200, a JSON array, newest first; `limit` 1-100 (default 50), `offset` from 0; a `Link` header to the other pages |
| GET | `/api/products/{id}` | `show` | 200, the record |
| POST | `/api/products` | `create` | 201, the created record; every required field must be sent |
| PATCH | `/api/products/{id}` | `update` | 200, the updated record; only the fields sent change |
| DELETE | `/api/products/{id}` | `delete` | 204, no body |

The whole controller:

```rust
pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/api/products", get(index).post(create))
        .route("/api/products/{id}", get(show).patch(update).delete(delete))
}

/// One page (`?limit=&offset=`), with a `Link` header to the next and previous pages.
async fn index(State(ctx): State<Ctx>, page: Page) -> ApiResult<(PageLinks, Json<Vec<Product>>)> {
    let products = product::all(&ctx, page).await?;
    Ok((page.links("/api/products", products.len()), Json(products)))
}

async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>) -> ApiResult<Json<Product>> {
    Ok(Json(product::find(&ctx, id).await?.or_404()?))
}

async fn create(State(ctx): State<Ctx>, Json(new): Json<NewProduct>) -> ApiResult<Created<Product>> {
    Ok(Created(product::create(&ctx, new).await?))
}

async fn update(
    State(ctx): State<Ctx>,
    Path(id): Path<i64>,
    Json(changes): Json<ProductChanges>,
) -> ApiResult<Json<Product>> {
    Ok(Json(product::update(&ctx, id, changes).await?.or_404()?))
}

async fn delete(State(ctx): State<Ctx>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    if product::delete(&ctx, id).await? { Ok(StatusCode::NO_CONTENT) } else { Err(Error::NotFound.into()) }
}
```

Every rule is in the model (`src/models/product.rs`, see [Models](models.md)), so the API, HTML pages and GraphQL share it. The request and response bodies are the model's structs: `NewProduct` for `POST`, `ProductChanges` for `PATCH`, `Product` for responses.

## Try it with curl

Create (201):

```sh
curl -s -X POST http://localhost:8787/api/products \
  -H 'Content-Type: application/json' \
  -d '{"name": "Teapot", "price": 24.5, "stock": 12}'
```

```json
{"id":1,"name":"Teapot","price":24.5,"stock":12,"created_at":"2026-09-29 04:49:11","updated_at":"2026-09-29 04:49:11"}
```

An optional field can be left out (`-i` shows the status line and headers):

```sh
curl -si -X POST http://localhost:8787/api/products \
  -H 'Content-Type: application/json' \
  -d '{"name": "Mug", "price": 8}'
```

```text
HTTP/1.1 201 Created
Transfer-Encoding: chunked
Content-Type: application/json
...
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
...

{"id":2,"name":"Mug","price":8.0,"stock":null,"created_at":"2026-09-29 04:49:11","updated_at":"2026-09-29 04:49:11"}
```

List and show:

```sh
curl -s 'http://localhost:8787/api/products?limit=10'
```

```json
[{"id":2,"name":"Mug","price":8.0,"stock":null,"created_at":"2026-09-29 04:49:11","updated_at":"2026-09-29 04:49:11"},{"id":1,"name":"Teapot","price":24.5,"stock":12,"created_at":"2026-09-29 04:49:11","updated_at":"2026-09-29 04:49:11"}]
```

```sh
curl -s http://localhost:8787/api/products/1
```

```json
{"id":1,"name":"Teapot","price":24.5,"stock":12,"created_at":"2026-09-29 04:49:11","updated_at":"2026-09-29 04:49:11"}
```

Update the price and clear the stock; `name` is not sent, so it stays:

```sh
curl -s -X PATCH http://localhost:8787/api/products/1 \
  -H 'Content-Type: application/json' \
  -d '{"price": 19.9, "stock": null}'
```

```json
{"id":1,"name":"Teapot","price":19.9,"stock":null,"created_at":"2026-09-29 04:49:11","updated_at":"2026-09-29 04:49:11"}
```

Delete (204, empty body), then the record is gone:

```sh
curl -si -X DELETE http://localhost:8787/api/products/2
```

```text
HTTP/1.1 204 No Content
...
```

```sh
curl -s -w '\n%{http_code}\n' -X DELETE http://localhost:8787/api/products/2
```

```text
{"error":{"message":"Not found","status":404}}
404
```

## Errors

Every error is a JSON object `{"error": {"status", "message"}}` with the same HTTP status; a failed validation adds `fields`. Internal errors answer `Internal server error` and log the details (D1 message, SQL) to the Worker log, never to the client.

| Status | When | Body |
|---|---|---|
| 400 | Body is not JSON, lacks `Content-Type: application/json`, misses a required field or has a wrong type; bad `limit`/`offset` | `{"error":{"message":"<explanation>","status":400}}` |
| 401 | `Error::Unauthorized` (e.g. missing Bearer token after `ocre g auth`); adds `WWW-Authenticate: Bearer` | `{"error":{"message":"Unauthorized","status":401}}` |
| 403 | `Error::Forbidden` | `{"error":{"message":"Forbidden","status":403}}` |
| 404 | Unknown id | `{"error":{"message":"Not found","status":404}}` |
| 413 | `Error::PayloadTooLarge` (uploads over the limit) | the message |
| 422 | Failed validation | `{"error":{"fields":{...},"message":"Validation failed","status":422}}` |
| 500 | Anything unexpected | `{"error":{"message":"Internal server error","status":500}}` |

Validation errors list every field at once, including the database checks (unique names):

```sh
curl -s -X POST http://localhost:8787/api/products \
  -H 'Content-Type: application/json' \
  -d '{"name": "Teapot", "price": 3, "stock": 9007199254740992}'
```

```json
{"error":{"fields":{"name":["has already been taken"],"stock":["must be less than or equal to 9007199254740991"]},"message":"Validation failed","status":422}}
```

```sh
curl -s -X POST http://localhost:8787/api/products \
  -H 'Content-Type: application/json' \
  -d '{"name": " ", "price": 3}'
```

```json
{"error":{"fields":{"name":["can't be blank"]},"message":"Validation failed","status":422}}
```

A body that does not fit the struct is a 400 with serde's explanation, before any validation:

```sh
curl -s -X POST http://localhost:8787/api/products -H 'Content-Type: application/json' -d '{"name": "Cup"}'
```

```json
{"error":{"message":"Failed to deserialize the JSON body into the target type: missing field `price` at line 1 column 15","status":400}}
```

```sh
curl -s -X POST http://localhost:8787/api/products -H 'Content-Type: application/json' -d '{"name": "Cup", "price": "cheap"}'
```

```json
{"error":{"message":"Failed to deserialize the JSON body into the target type: price: invalid type: string \"cheap\", expected f64 at line 1 column 32","status":400}}
```

```sh
curl -s -X POST http://localhost:8787/api/products -H 'Content-Type: application/json' -d '{"name": "Cup",'
```

```json
{"error":{"message":"Failed to parse the request body as JSON: EOF while parsing a value at line 1 column 15","status":400}}
```

```sh
curl -s -X POST http://localhost:8787/api/products -d 'name=Cup&price=2'
```

```json
{"error":{"message":"Expected request with `Content-Type: application/json`","status":400}}
```

Pagination errors:

```sh
curl -s 'http://localhost:8787/api/products?limit=500'
curl -s 'http://localhost:8787/api/products?offset=-1'
curl -s 'http://localhost:8787/api/products?limit=abc'
```

```json
{"error":{"message":"limit must be between 1 and 100","status":400}}
{"error":{"message":"offset must be 0 or more","status":400}}
{"error":{"message":"Failed to deserialize query string: limit: invalid digit found in string","status":400}}
```

A `PATCH` to an unknown id is a 404 (`{"error":{"message":"Not found","status":404}}`), as is `GET`.

## Pagination, versions and formats

List endpoints read `?limit=&offset=` with `ocre::Page` and answer a `Link` header (RFC 8288, GitHub's convention) pointing to the neighbouring pages:

```sh
curl -si 'http://localhost:8787/api/products?limit=2&offset=2'
```

```text
HTTP/1.1 200 OK
Content-Type: application/json
link: </api/products?limit=2&offset=4>; rel="next", </api/products?limit=2&offset=0>; rel="prev", </api/products?limit=2&offset=0>; rel="first"
...
```

`next` is there when the page is full (`Page::next`): Ocre does not run a `COUNT(*)`, which would read every row of the table from D1's daily 5 million. A client follows `next` until it is absent; the last page may be empty when the total is a multiple of `limit`. An endpoint that must report a total runs its own count (see [Models](models.md)).

Versions are path prefixes: keep the current routes in a module and nest it, then nest the next version's module beside it when the contract changes (the routes then read `/products` inside the module):

```rust
fn routes() -> Router<Ctx> {
    Router::new()
        .nest("/api/v1", products_v1::routes())
        .nest("/api/v2", products_v2::routes())
        // ocre:routes
}
```

`ocre routes` lists nested routes with their prefix. One action that serves HTML to browsers and JSON to API clients reads the `Accept` header with `ocre::Format` (see [Controllers](controllers.md#formats-respond_to)). Every error has the same shape, `{"error": {"status": ..., "message": ..., "fields": ...}}` (see [Errors](#errors)), including unmatched paths in API-only apps.

## Optional fields and partial updates

The model's input structs decide what a JSON body may contain:

```rust
pub struct NewProduct {
    pub name: String,
    pub price: f64,
    #[serde(default, deserialize_with = "ocre::optional")]
    pub stock: Option<i64>,
}

pub struct ProductChanges {
    pub name: Option<String>,
    pub price: Option<f64>,
    #[serde(default, deserialize_with = "ocre::patch")]
    pub stock: Option<Option<i64>>,
}
```

| Body of `PATCH` | `stock` in `ProductChanges` | Effect |
|---|---|---|
| `{}` (field missing) | `None` | keep |
| `{"stock": null}` or `{"stock": ""}` | `Some(None)` | clear (`NULL`) |
| `{"stock": 7}` or `{"stock": "7"}` | `Some(Some(7))` | set |
| `{"stock": "lots"}` | | 400 (does not parse) |

`ocre::optional` (in `NewProduct`) reads the same inputs into `Option<i64>`: missing, `null` and `""` are `None`. Accepting numbers as strings and empty strings as "no value" lets the same structs read HTML forms. For required fields in `ProductChanges` (`name`, `price`), a missing key or `null` both keep the value; `{"name": ""}` is sent to validation and refused with `can't be blank`. Optional `json` fields are the exception: they use `#[serde(default)]` in `New<Model>` and `#[serde(default, deserialize_with = "ocre::patch_json")]` in `<Model>Changes`, so a JSON string stays a string instead of being parsed (see [Field types](../reference/field-types.md)).

## Hand-written JSON endpoints

Any handler can be a JSON endpoint: return `ApiResult<T>` (an alias of `Result<T, ApiError>`), take bodies with `ocre::Json<T>`, and use `?` on anything returning `ocre::Result`, since `ApiError` converts from `ocre::Error` and `worker::Error`.

| Type | Use |
|---|---|
| `ocre::ApiResult<T>` | return type; errors become the JSON error body with their status |
| `ocre::ApiError(Error)` | the error type; `Err(Error::NotFound.into())` builds one |
| `ocre::Json<T>` | extractor: requires `Content-Type: application/json`, rejects with a JSON 400 (unlike `axum::Json`, whose rejections are plain text); response: 200 with `T` serialized |
| `ocre::Created<T>` | response: 201 with `T` serialized |
| `ocre::Page` | extractor: `?limit=&offset=`, JSON 400 when out of range |
| `axum::http::StatusCode` | response without body, e.g. `StatusCode::NO_CONTENT` |

An endpoint with counts across tables, and one that changes a single field through the model:

```rust,check
// src/stats_api.rs
use axum::{
    Router,
    extract::{Path, State},
    routing::{get, put},
};
use ocre::{ApiResult, Ctx, Error, Json, OptionExt, params};
use serde::{Deserialize, Serialize};

use crate::models::post::{self, Post, PostChanges};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/api/stats", get(stats))
        .route("/api/posts/{id}/published", put(set_published))
}

#[derive(Deserialize, Serialize)]
struct Stats {
    posts: i64,
    published: i64,
    comments: i64,
}

/// GET /api/stats: three counts in one query.
async fn stats(State(ctx): State<Ctx>) -> ApiResult<Json<Stats>> {
    let sql = "SELECT (SELECT COUNT(*) FROM posts) AS posts,
                      (SELECT COUNT(*) FROM posts WHERE published = 1) AS published,
                      (SELECT COUNT(*) FROM comments) AS comments";
    let stats = ctx.db()?.first::<Stats>(sql, params![]).await?;
    Ok(Json(stats.ok_or_else(|| Error::internal("SELECT of counts returned no row"))?))
}

#[derive(Deserialize)]
struct Publish {
    published: bool,
}

/// PUT /api/posts/{id}/published with {"published": true}: goes through the
/// model, so its validations apply.
async fn set_published(
    State(ctx): State<Ctx>,
    Path(id): Path<i64>,
    Json(body): Json<Publish>,
) -> ApiResult<Json<Post>> {
    let changes = PostChanges { published: Some(body.published), ..Default::default() };
    Ok(Json(post::update(&ctx, id, changes).await?.or_404()?))
}
```

Register it in `src/lib.rs` (`mod stats_api;` under `// ocre:modules`, `.merge(stats_api::routes())` under `// ocre:routes`). In the blog app, with the `Comment` scaffold:

```sh
curl -s http://localhost:8787/api/stats
```

```json
{"posts":2,"published":1,"comments":0}
```

```sh
curl -s -X PUT http://localhost:8787/api/posts/2/published -H 'Content-Type: application/json' -d '{"published": true}'
```

```json
{"id":2,"title":"Draft","body":"Not yet","published":true,"created_at":"2026-09-29 04:48:44","updated_at":"2026-09-29 04:49:13"}
```

```sh
curl -s -X PUT http://localhost:8787/api/posts/2/published -H 'Content-Type: application/json' -d '{"published": "yes"}'
curl -s -X PUT http://localhost:8787/api/posts/42/published -H 'Content-Type: application/json' -d '{"published": true}'
```

```json
{"error":{"message":"Failed to deserialize the JSON body into the target type: published: invalid type: string \"yes\", expected a boolean at line 1 column 19","status":400}}
{"error":{"message":"Not found","status":404}}
```

Handlers of HTML pages return `ocre::Result` and render errors as HTML; JSON handlers return `ApiResult`. Use one or the other per handler.

## GraphQL

`--graphql` adds, for each resource, these operations on `POST /graphql`, and the GraphiQL editor on `GET /graphql` (open `http://localhost:8787/graphql` in a browser):

| Operation | Arguments | Returns |
|---|---|---|
| `products` | `limit` (default 50), `offset` (default 0) | `[Product!]!`, newest first |
| `product` | `id` | `Product` or `null` |
| `createProduct` | `input: NewProductInput!` | `Product!` |
| `updateProduct` | `id`, `patch: ProductPatch!` | `Product!`, error with status 404 for an unknown id |
| `deleteProduct` | `id` | `true`, error with status 404 for an unknown id |

The resolvers are in `src/products_api.rs`, below the REST handlers, and call the same model functions. `src/graphql.rs` builds the schema once per Worker instance and merges each resource's query and mutation objects after the `// ocre:graphql-queries` and `// ocre:graphql-mutations` markers. In `ProductPatch`, optional fields are `MaybeUndefined`: omitted keeps, `null` clears.

```sh
curl -s -X POST http://localhost:8787/graphql -H 'Content-Type: application/json' \
  -d '{"query": "{ products(limit: 5) { id name price stock } }"}'
```

```json
{"data":{"products":[{"id":1,"name":"Teapot","price":19.9,"stock":null}]}}
```

```sh
curl -s -X POST http://localhost:8787/graphql -H 'Content-Type: application/json' \
  -d '{"query": "mutation { createProduct(input: {name: \"Kettle\", price: 30}) { id name stock } }"}'
```

```json
{"data":{"createProduct":{"id":3,"name":"Kettle","stock":null}}}
```

```sh
curl -s -X POST http://localhost:8787/graphql -H 'Content-Type: application/json' \
  -d '{"query": "mutation { updateProduct(id: 3, patch: {stock: null, price: 28}) { id price stock } }"}'
```

```json
{"data":{"updateProduct":{"id":3,"price":28.0,"stock":null}}}
```

Errors follow GraphQL conventions: the HTTP status is 200 and the error is in `errors`, with Ocre's status (and validation `fields`) in `extensions`:

```sh
curl -s -X POST http://localhost:8787/graphql -H 'Content-Type: application/json' \
  -d '{"query": "mutation { createProduct(input: {name: \"\", price: 30}) { id } }"}'
```

```json
{"data":null,"errors":[{"message":"Validation failed","locations":[{"line":1,"column":12}],"path":["createProduct"],"extensions":{"fields":{"name":["can't be blank"]},"status":422}}]}
```

```sh
curl -s -X POST http://localhost:8787/graphql -H 'Content-Type: application/json' \
  -d '{"query": "mutation { deleteProduct(id: 99) }"}'
```

```json
{"data":null,"errors":[{"message":"Not found","locations":[{"line":1,"column":12}],"path":["deleteProduct"],"extensions":{"status":404}}]}
```

`product(id: 99)` returns `{"data":{"product":null}}`. A body that is not a GraphQL request is a 400: `{"errors":[{"message":"invalid GraphQL request: expected ident at line 1 column 2"}]}`.

Cost on the free plan: GraphQL is opt-in (the `graphql` feature of `ocre` in `Cargo.toml`, which the first `--graphql` enables, plus the `async-graphql` dependency). It adds about 1.1 MB to the WebAssembly binary and 20-60 ms of CPU each time a new Worker instance starts (measured with `wrangler tail`), against 10 ms per request on the free plan, which Cloudflare tolerates when overruns are infrequent. Add it only when a client needs GraphQL; each resolver's queries cost the same D1 rows as the REST handler.

## API-only apps

`ocre new <name> --api` creates an app without templates or askama: `ocre` is built with `default-features = false` (no `html` feature), `Cargo.toml` records `[package.metadata.ocre] mode = "api"`, and `ocre g scaffold` generates JSON APIs. Its `src/lib.rs` answers `GET /` with a JSON status:

```rust
#[derive(Serialize)]
struct Status {
    app: &'static str,
    status: &'static str,
}

async fn status() -> Json<Status> {
    Json(Status { app: "shop", status: "ok" })
}
```

```sh
ocre new shop --api --yes
cd shop
ocre g scaffold Item name:string
```

```text
  create  src/models/mod.rs
  create  migrations/0001_create_items.sql
  create  src/models/item.rs
  create  src/items_api.rs
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  curl http://localhost:8787/api/items
```

Without the `html` feature, errors that would otherwise be HTML pages (such as a missing session secret) are JSON too.

## Calling the API from another origin (CORS)

A browser frontend served from another origin (a separate SPA, a mobile web view) needs that origin listed in the `ALLOWED_ORIGINS` Worker variable, comma-separated, in `cloudflare.config.ts`:

```ts
// in worker.env
ALLOWED_ORIGINS: bindings.text("https://app.example.com"),
```

Listed origins get CORS headers (methods `GET, POST, PUT, PATCH, DELETE`; headers `Content-Type, Authorization, Accept`; credentials allowed) and pass the CSRF check; unlisted ones get neither. A preflight from the listed origin:

```sh
curl -si -X OPTIONS http://localhost:8787/api/products \
  -H 'Origin: https://app.example.com' \
  -H 'Access-Control-Request-Method: POST' \
  -H 'Access-Control-Request-Headers: content-type'
```

```text
HTTP/1.1 200 OK
Content-Length: 0
Access-Control-Allow-Origin: https://app.example.com
Allow: GET,HEAD,POST
Vary: origin
access-control-allow-credentials: true
access-control-allow-headers: content-type,authorization,accept
access-control-allow-methods: GET,POST,PUT,PATCH,DELETE
...
```

A browser request from an unlisted site that changes data is refused with a plain-text 403:

```sh
curl -s -X DELETE http://localhost:8787/api/products/3 -H 'Origin: https://evil.example' -H 'Sec-Fetch-Site: cross-site'
```

```text
Forbidden: cross-site request. Add the origin to ALLOWED_ORIGINS to allow it.
```

Clients that are not browsers (curl, servers, mobile apps) send no `Origin` or `Sec-Fetch-Site` header and are not affected. See [Sessions, flash and security](security.md) and [Configuration](../reference/configuration.md#allowed_origins).

## Authentication

APIs are public until you protect them. After `ocre g auth`, take `BearerUser(user): BearerUser` (from `crate::auth_api`) in a handler, before `Json(..)`: it accepts `Authorization: Bearer <JWT or API key>` and answers a JSON 401 with `WWW-Authenticate: Bearer` otherwise. Tokens come from `POST /api/auth/token`; API keys from `POST /api/auth/keys`. Filter queries by `user.id` for records a user owns. See [Authentication](authentication.md).

## See also

- [Models and migrations](models.md): the model behind every API
- [Validations](validations.md): rules and the 422 `fields` object
- [Controllers and routing](controllers.md): routing and extractors in general
- [Authentication](authentication.md): `BearerUser`, JWTs and API keys
- [File storage](files.md): attachments in JSON APIs (`PUT /api/<plural>/{id}/<file>`)
- [Generators](../reference/generators.md#ocre-g-api), [Configuration](../reference/configuration.md#allowed_origins), [Free-plan limits](../reference/limits.md)
- [API index](../api-index.md) and the [rustdoc](/api/ocre/index.html)
