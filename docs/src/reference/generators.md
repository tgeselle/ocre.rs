# Generators

This page documents every `ocre g` generator: its arguments and flags, the naming rules, the files it creates and updates, what the generated code contains, and its errors. Generators write plain Rust, SQL and templates into the app, which the app then owns and edits.

## Before you start

- An Ocre app created with [`ocre new`](cli.md#ocre-new). Run generators from its root or any directory below it (the CLI looks for the nearest `wrangler.toml`).
- Generators only write files: they need no network, no wrangler and no Cloudflare account. Run [`ocre migrate`](cli.md#ocre-migrate) after the ones that add migrations, and `cargo check --target wasm32-unknown-unknown` to type-check the result.
- Keep the `// ocre:...` marker comments of generated files (`// ocre:modules` and `// ocre:routes` in `src/lib.rs`, `// ocre:models` in `src/models/mod.rs`...): generators insert lines right after them.

The examples below were run with `ocre 0.1.0` on an app created by `ocre new blog --starter blog`, in the order of this page.

## How generators behave

- **All or nothing.** A generator computes every change first and writes nothing until the whole generation succeeded, so a failure never leaves half a resource.
- **Never overwrite.** A generator creates new files and only edits existing ones by inserting lines at markers (or appending blocks to `wrangler.toml`). When a file it would create exists, it stops with `<path> already exists` and the hint ``generators create new files only; edit the existing file, or add a migration with `ocre g migration` ``. Running a generator twice is therefore an error, not a no-op. To change a table after its migration was applied, add a migration with [`ocre g migration`](#ocre-g-migration).
- **Report.** The human output lists `  create  <path>` and `  update  <path>` lines, then `Next:` steps. With `--json` it is one object with `command` (`generate <generator>`), `created`, `updated` and `next` (see [the --json contract](cli.md#global-flag-json)).
- **Full-stack or API-only.** An app created with `ocre new --api` has `[package.metadata.ocre] mode = "api"` in `Cargo.toml`. There, `ocre g scaffold` generates a JSON API (like `ocre g api`), and `auth` and `mailer` generate no HTML.
- **Migrations** are numbered after the highest existing number: `migrations/0002_create_comments.sql`, then `0003_...`.
- **Marker errors.** When a marker a generator needs is missing, it fails with ``<file> is missing the `<marker>` marker`` and a hint saying where to put it back.

| Generator | Creates |
|---|---|
| [`ocre g model`](#ocre-g-model) | Migration and `src/models/<model>.rs` |
| [`ocre g scaffold`](#ocre-g-scaffold) | Model (unless it exists) plus HTML CRUD pages; `--realtime` for live updates |
| [`ocre g api`](#ocre-g-api) | Model (unless it exists) plus a JSON REST resource; `--graphql` for GraphQL |
| [`ocre g auth`](#ocre-g-auth) | Users, sessions, password reset, magic links, JWT and API keys (once per app) |
| [`ocre g migration`](#ocre-g-migration) | One numbered SQL migration |
| [`ocre g mailer`](#ocre-g-mailer) | Functions building emails, with templates |
| [`ocre g mailbox`](#ocre-g-mailbox) | The handler of incoming email (once per app) |
| [`ocre g job`](#ocre-g-job) | A background job on Cloudflare Queues |
| [`ocre g schedule`](#ocre-g-schedule) | A task run by a Cron Trigger |
| [`ocre g cache`](#ocre-g-cache) | The `CACHE` Workers KV binding |
| [`ocre g locale`](#ocre-g-locale) | Translation files |

## Fields

`model`, `scaffold`, `api`, `migration` and `job` take fields as `name:type`, with optional suffixes: `?` makes the field optional (the column accepts `NULL`, the Rust type is an `Option`), `^` makes it unique (a unique index plus a "has already been taken" check). Both can be combined (`slug:string?^`). The types, detailed in [Field types](field-types.md):

| Type | SQL column | Rust type | Notes |
|---|---|---|---|
| `string` | `TEXT` | `String` | One-line text; required unless `?` |
| `text` | `TEXT` | `String` | Multi-line text (a textarea in forms); required unless `?` |
| `integer` | `INTEGER` | `i64` | Validated within ±(2^53 - 1), the integers D1 returns exactly |
| `float` | `REAL` | `f64` | |
| `boolean` | `INTEGER NOT NULL DEFAULT 0` | `bool` | A checkbox; cannot be `?` |
| `date` | `TEXT` | `String` | Validated as `YYYY-MM-DD` |
| `datetime` | `TEXT` | `String` | Validated as a date and time |
| `references` | `<name>_id INTEGER REFERENCES <plural>(id) ON DELETE CASCADE`, indexed | `i64` | `author:references` adds `author_id`; `src/models/author.rs` must exist; validated as "must exist" |
| `attachment` | four columns: `<name>_key`, `<name>_filename`, `<name>_content_type` (`TEXT`), `<name>_size` (`INTEGER`) | `ocre::storage::Upload` when received, `Attachment` when stored | A file in R2; cannot be `^`; cannot be named `edit`, `delete` or `new`; must be `?` in JSON APIs; adds the `STORAGE` R2 bucket to `wrangler.toml` |
| `json` | `TEXT CHECK (json_valid(<name>))` | `ocre::serde_json::Value` | Any JSON value; cannot be `^` |

Field names are snake_case, start with a lowercase letter, and must not be reserved: `id`, `created_at` and `updated_at` (every table gets them), Rust keywords (`type`, `match`, `mod`, `ref`, `self`, `use`, `where`, `yield`...), and these SQL keywords: `and`, `asc`, `by`, `case`, `check`, `default`, `desc`, `from`, `group`, `index`, `join`, `key`, `limit`, `not`, `null`, `offset`, `or`, `order`, `primary`, `references`, `select`, `table`, `unique`, `values`. Two fields cannot produce the same column (an attachment `avatar` takes `avatar_key`, `avatar_filename`, `avatar_content_type` and `avatar_size`).

Field errors (shared by every generator that takes fields):

| Error | Hint |
|---|---|
| ``field `title` has no type`` | ``write fields as `name:type`, e.g. `title:string` `` |
| ``invalid field name `<name>` `` | ``use snake_case starting with a letter, e.g. `published_at` `` |
| ``field name `type` is reserved`` | ``` `id`, `created_at` and `updated_at` are generated; Rust and SQL keywords are not allowed. Pick another name, e.g. `kind` for `type` ``` |
| ``unknown field type `strng` for `title` `` | ``types: string, text, integer, float, boolean, date, datetime, references, attachment, json; add `?` for optional, `^` for unique`` |
| ``boolean field `done` cannot be optional`` | ``booleans are true or false (a checkbox); drop the `?` `` |
| ``attachment `a` cannot be unique`` | ``every stored file gets its own random key already; drop the `^` `` |
| ``json field `v` cannot be unique`` | ``a unique index compares JSON text, where key order and spacing differ; drop the `^` `` |
| ``attachment name `edit` clashes with a scaffold route`` | ``` `/<plural>/{id}/edit` is taken; pick another name, e.g. `edit_file` ``` |
| ``field `a_key` is listed twice`` | ``names must differ, and `<name>:attachment` also takes `<name>_key`, `<name>_filename`, `<name>_content_type` and `<name>_size` `` |
| `src/models/owner.rs does not exist` (for `owner:references`) | ``generate the referenced model first, e.g. `ocre g model Owner name:string` `` |

## Model names

`model`, `scaffold` and `api` take a singular model name, in PascalCase, snake_case, kebab-case or with spaces (`BlogPost`, `blog_post`, `blog-post`). It is split into words, which give every other name:

| Input | Struct | Module and file | Table, plural module, URL segment | Human |
|---|---|---|---|---|
| `Post` | `Post` | `post`, `src/models/post.rs` | `posts` | `Post`, `Posts` |
| `BlogPost` | `BlogPost` | `blog_post` | `blog_posts` | `Blog post`, `Blog posts` |
| `category` | `Category` | `category` | `categories` | `Category`, `Categories` |
| `Box` | `Box` | `box` | `boxes` | `Box`, `Boxes` |
| `Person` | `Person` | `person` | `people` | `Person`, `People` |

Only the last word is pluralized: a consonant followed by `y` becomes `ies`; `s`, `x`, `z`, `ch` and `sh` take `es`; the irregular `person`, `child`, `man`, `woman`, `mouse`, `goose`, `tooth` and `foot` become `people`, `children`, `men`, `women`, `mice`, `geese`, `teeth` and `feet`; anything else takes `s`. Other irregular plurals (`Status` gives `statuses`, but `Criterion` gives `criterions`) are not known: rename the table in the migration and the model if needed. The name must start with a letter (``invalid model name `1thing` `` with the hint ``use a singular name starting with a letter, e.g. `Post` or `BlogPost` ``).

Model names are not checked against Rust keywords: `ocre g model Type ...` or `ocre g model Box ...` succeed but write `pub mod type;` or `pub mod box;` into `src/models/mod.rs`, which does not compile. Pick another name (`Kind`, `Crate`...).

## ocre g model

```text
ocre g model <NAME> <FIELDS>...
```

| Argument | Required | Meaning |
|---|---|---|
| `NAME` | yes | Singular model name (see [Model names](#model-names)) |
| `FIELDS` | yes, at least one | `name:type` fields (see [Fields](#fields)) |

Creates the table's migration and the model: every query and rule about the table, which controllers, JSON APIs, GraphQL resolvers and jobs call instead of writing SQL.

```sh
ocre g model Author name:string^ bio:text?
```

```text
  create  migrations/0006_create_authors.sql
  create  src/models/author.rs
  update  src/models/mod.rs

Next:
  ocre migrate
  cargo check --target wasm32-unknown-unknown
```

```json
{"command":"generate model","created":["migrations/0006_create_authors.sql","src/models/author.rs"],"next":["ocre migrate","cargo check --target wasm32-unknown-unknown"],"ok":true,"updated":["src/models/mod.rs"]}
```

Files:

- `migrations/NNNN_create_<plural>.sql`: `CREATE TABLE` with `id INTEGER PRIMARY KEY AUTOINCREMENT`, the field columns, `created_at` and `updated_at` (`TEXT NOT NULL DEFAULT (datetime('now'))`), then a `CREATE UNIQUE INDEX` per `^` field and a `CREATE INDEX` per reference. Skipped when a `*_create_<plural>.sql` migration already exists.
- `src/models/<model>.rs`: the row struct (`Author`, deriving `Deserialize` and `Serialize`), `NewAuthor` (values for a new row), `AuthorChanges` (every field an `Option`; for optional fields `Some(None)` clears the value), `validate()` on both (presence of required text, dates, safe integers, files), and the functions `all(ctx, page)` (newest first), `count`, `find`, `find_many` (100 ids per query), `create`, `update` and `delete`. `create` and `update` add the checks that need the database: uniqueness and existing references.
- `src/models/mod.rs`: `pub mod <model>;` after `// ocre:models`. The first model creates the file and adds `mod models;` to `src/lib.rs`.

A `references` field also updates the referenced model: `ocre g scaffold Comment ... post:references` adds `pub async fn comments(&self, ctx, page)` (has many, newest first) to `src/models/post.rs` after its `// ocre:associations` marker, and gives `Comment` a `post(&self, ctx)` method (belongs to). An `attachment` field adds a `Rules` constant per file (`pub const IMAGE: Rules`, 10 MB and common image, PDF and text types until you edit it), a method returning its `Attachment`, `create`/`update` that store files in R2 and delete replaced ones, and the `STORAGE` bucket to `wrangler.toml` (`bucket_name = "<app>-storage"`) unless it is there.

This is the migration of the fixture's `Setting` model with a JSON field:

```sh
ocre g model Setting key_name:string^ value:json
```

```sql
CREATE TABLE settings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key_name TEXT NOT NULL,
    value TEXT NOT NULL CHECK (json_valid(value)),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX index_settings_on_key_name ON settings (key_name);
```

Errors: the [field](#fields) and [name](#model-names) errors, and `src/models/<model>.rs already exists` when the model exists. See [Models and migrations](../guides/models.md) and [Validations](../guides/validations.md).

## ocre g scaffold

```text
ocre g scaffold <NAME> <FIELDS>... [--realtime]
```

| Argument or flag | Default | Meaning |
|---|---|---|
| `NAME` | required | Singular model name |
| `FIELDS` | required, at least one | `name:type` fields |
| `--realtime` | off | Live index page: creates, edits and deletes appear in every open browser over a WebSocket (htmx `ws` extension, a Durable Object per channel). Full-stack apps only |

Creates the model (like [`ocre g model`](#ocre-g-model), unless `src/models/<model>.rs` exists, in which case the fields only shape the pages), then HTML pages for the full create, read, update, delete cycle.

```sh
ocre g scaffold Comment author:string body:text post:references
```

```text
  create  migrations/0002_create_comments.sql
  create  src/models/comment.rs
  create  src/comments.rs
  create  templates/comments/index.html
  create  templates/comments/show.html
  create  templates/comments/new.html
  create  templates/comments/edit.html
  create  templates/comments/_form.html
  update  src/models/post.rs
  update  src/models/mod.rs
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/comments
```

```json
{"command":"generate scaffold","created":["migrations/0002_create_comments.sql","src/models/comment.rs","src/comments.rs","templates/comments/index.html","templates/comments/show.html","templates/comments/new.html","templates/comments/edit.html","templates/comments/_form.html"],"next":["ocre migrate","ocre dev","open http://localhost:8787/comments"],"ok":true,"updated":["src/models/post.rs","src/models/mod.rs","src/lib.rs"]}
```

`src/<plural>.rs` holds the routes and handlers; `src/lib.rs` gets `mod <plural>;` and `.merge(<plural>::routes())`. HTML forms can only send `GET` and `POST`, so updates and deletes are `POST`s:

| Route | Handler | Effect |
|---|---|---|
| `GET /comments` | `index` | List, newest first, paginated |
| `GET /comments/new` | `new` | New form |
| `POST /comments` | `create` | Create; redirects with a flash notice, or shows the form with errors (422) |
| `GET /comments/{id}` | `show` | One record |
| `GET /comments/{id}/edit` | `edit` | Edit form |
| `POST /comments/{id}` | `update` | Update |
| `POST /comments/{id}/delete` | `delete` | Delete, then redirect |
| `GET /comments/{id}/<attachment>` | `<attachment>_file` | For each `attachment` field: serves the stored file |

The handlers parse the form into a `CommentForm` whose fields are all text (numbers are validated, so a typo shows a field error instead of a failed request), then call the model. The templates extend `layout.html`; `_form.html` holds the fields shared by `new.html` and `edit.html`.

With `--realtime`, the scaffold also creates `templates/<plural>/_row.html` (one row, rendered for the page and for broadcasts), makes the handlers broadcast each change on the `<plural>` channel, and on first use creates `src/realtime.rs` (the `GET /realtime/{channel}` route and the list of channels anyone may listen to), turns on Ocre's `realtime` feature in `Cargo.toml` and declares the `CHANNELS` Durable Object (class `OcreChannel`, SQLite-backed, as the free plan requires) in `wrangler.toml`. Later `--realtime` scaffolds add their channel after `// ocre:channels` in `src/realtime.rs`:

```sh
ocre g scaffold Message body:text --realtime --json
```

```json
{"command":"generate scaffold","created":["migrations/0008_create_messages.sql","src/models/message.rs","src/messages.rs","templates/messages/index.html","templates/messages/show.html","templates/messages/new.html","templates/messages/edit.html","templates/messages/_form.html","templates/messages/_row.html","src/realtime.rs"],"next":["ocre migrate","ocre dev","open http://localhost:8787/messages","open http://localhost:8787/messages in a second window, then create a message"],"ok":true,"updated":["src/models/mod.rs","src/lib.rs","Cargo.toml","wrangler.toml"]}
```

With attachments, the form becomes a file upload and `wrangler.toml` gets the `STORAGE` bucket:

```sh
ocre g scaffold Photo title:string image:attachment notes:attachment?
```

```text
  create  migrations/0009_create_photos.sql
  create  src/models/photo.rs
  create  src/photos.rs
  create  templates/photos/index.html
  create  templates/photos/show.html
  create  templates/photos/new.html
  create  templates/photos/edit.html
  create  templates/photos/_form.html
  update  src/models/mod.rs
  update  wrangler.toml
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/photos
```

In an API-only app, `ocre g scaffold` runs `ocre g api` (without GraphQL), and the report's `command` is `generate api`:

```text
  create  src/models/mod.rs
  create  migrations/0001_create_posts.sql
  create  src/models/post.rs
  create  src/posts_api.rs
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  curl http://localhost:8787/api/posts
```

Errors: the field and name errors; `src/<plural>.rs already exists` (or a template) when the pages exist; in an API-only app, `--realtime updates HTML pages; this app is API-only` with the hint ``run `ocre g scaffold` without --realtime; to push JSON to clients, see Realtime in the Ocre README``. See [Controllers, routing, views and htmx](../guides/controllers.md), [File storage](../guides/files.md) and [Realtime](../guides/realtime.md).

## ocre g api

```text
ocre g api <NAME> <FIELDS>... [--graphql]
```

| Argument or flag | Default | Meaning |
|---|---|---|
| `NAME` | required | Singular model name |
| `FIELDS` | required, at least one | `name:type` fields; attachments must be optional (`file:attachment?`) |
| `--graphql` | off | Also expose the resource on `/graphql`. Per the CLI's help, it adds about 1.1 MB of WebAssembly and 20-60 ms of CPU when a Worker instance starts |

Creates the model unless it exists (so `ocre g api Post ...` after `ocre g scaffold Post ...` adds a JSON API to the same model), then `src/<plural>_api.rs`, registered in `src/lib.rs`. Works in both kinds of apps.

```sh
ocre g api Product name:string^ price:float stock:integer? --graphql
```

```text
  create  migrations/0007_create_products.sql
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

```json
{"command":"generate api","created":["migrations/0007_create_products.sql","src/models/product.rs","src/products_api.rs","src/graphql.rs"],"next":["ocre migrate","ocre dev","curl http://localhost:8787/api/products","open http://localhost:8787/graphql"],"ok":true,"updated":["src/models/mod.rs","src/lib.rs","Cargo.toml"]}
```

Routes of `src/products_api.rs`:

| Route | Effect |
|---|---|
| `GET /api/products?limit=&offset=` | List, newest first |
| `GET /api/products/{id}` | One record |
| `POST /api/products` | Create (every required field); `201` |
| `PATCH /api/products/{id}` | Update only the fields sent; `null` clears an optional field |
| `DELETE /api/products/{id}` | Delete; `204` |
| `GET`, `PUT`, `DELETE /api/<plural>/{id}/<attachment>` | For each attachment: download, upload (multipart), remove the file |

Failed validations answer `422` with `{"error": {"fields": {"title": ["can't be blank"]}}}`. With `--graphql`, the first API creates `src/graphql.rs` (the schema, served by `ocre::graphql::routes` on `GET /graphql` for GraphiQL and `POST /graphql`), turns on Ocre's `graphql` feature and adds the `async-graphql` dependency in `Cargo.toml`; later `--graphql` APIs add their queries and mutations after the `// ocre:graphql-queries` and `// ocre:graphql-mutations` markers.

Errors: the field and name errors; ``attachment `f` must be optional in a JSON API`` with the hint ``JSON cannot carry a file, so create cannot require one: use `f:attachment?`, then upload with `curl -X PUT -F f=@file http://localhost:8787/api/<plural>/1/f` ``; `src/<plural>_api.rs already exists`. See [JSON APIs and GraphQL](../guides/json-apis.md).

## ocre g auth

```text
ocre g auth
```

No arguments. Generates authentication into the app, like Rails 8's authentication generator, so every rule is visible and editable there. It runs once per app.

In a full-stack app:

```sh
ocre g auth
```

```text
  create  migrations/0003_create_users.sql
  create  migrations/0004_create_auth_tokens.sql
  create  migrations/0005_create_api_keys.sql
  create  src/models/user.rs
  create  src/models/api_key.rs
  create  src/models/auth_token.rs
  create  src/auth_api.rs
  create  src/auth.rs
  create  src/registrations.rs
  create  src/sessions.rs
  create  src/passwords.rs
  create  templates/auth/signup.html
  create  templates/auth/login.html
  create  templates/auth/account.html
  create  templates/auth/magic_link_new.html
  create  templates/auth/magic_link_show.html
  create  templates/auth/password_new.html
  create  templates/auth/password_edit.html
  update  src/models/mod.rs
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/signup
```

```json
{"command":"generate auth","created":["migrations/0003_create_users.sql","migrations/0004_create_auth_tokens.sql","migrations/0005_create_api_keys.sql","src/models/user.rs","src/models/api_key.rs","src/models/auth_token.rs","src/auth_api.rs","src/auth.rs","src/registrations.rs","src/sessions.rs","src/passwords.rs","templates/auth/signup.html","templates/auth/login.html","templates/auth/account.html","templates/auth/magic_link_new.html","templates/auth/magic_link_show.html","templates/auth/password_new.html","templates/auth/password_edit.html"],"next":["ocre migrate","ocre dev","open http://localhost:8787/signup"],"ok":true,"updated":["src/models/mod.rs","src/lib.rs"]}
```

| File | Contents |
|---|---|
| `src/models/user.rs`, `api_key.rs`, `auth_token.rs` | Users (email and password), API keys, and single-use tokens for password reset and magic links (full-stack only) |
| `src/auth.rs` | The `CurrentUser` and `OptionalUser` extractors and sign-in/sign-out helpers (full-stack only) |
| `src/registrations.rs` | `GET`/`POST /signup`, `GET /account` |
| `src/sessions.rs` | `GET`/`POST /login`, `POST /logout`, magic-link login (`/magic_link`, `/magic_link/{token}`) |
| `src/passwords.rs` | Password reset: `/passwords/new`, `POST /passwords`, `/passwords/{token}` |
| `src/auth_api.rs` | In every app: the `BearerUser` extractor, `POST /api/auth/signup`, `POST /api/auth/token` (JWT), `GET /api/auth/me`, and API keys (`GET`/`POST /api/auth/keys`, `DELETE /api/auth/keys/{id}`) |
| `templates/auth/*.html` | The pages (full-stack only) |

In an API-only app, only the JSON part is generated: `create_users` and `create_api_keys` migrations, the `user` and `api_key` models and `src/auth_api.rs`; the last next step is a `curl -X POST http://localhost:8787/api/auth/signup ...` command.

Error: `this app already has a User model or a users table` when `src/models/user.rs` or a `*_create_users.sql` migration exists, with the hint ``` `ocre g auth` creates both and runs once per app; to start over, remove src/models/user.rs and the create_users migration ```. See [Authentication](../guides/authentication.md).

## ocre g migration

```text
ocre g migration <NAME> [FIELDS]...
```

| Argument | Required | Meaning |
|---|---|---|
| `NAME` | yes | snake_case migration name |
| `FIELDS` | no | Columns as `name:type` |

Creates `migrations/NNNN_<name>.sql`, starting with `-- Migration: <name>` and a comment reminding that applied migrations must not be edited. The SQL is inferred from the name, like Rails:

| Name | SQL |
|---|---|
| `create_<table>` | `CREATE TABLE <table>` with the fields, `id`, `created_at`, `updated_at` and indexes, as `ocre g model` writes it |
| `add_<anything>_to_<table>` | One `ALTER TABLE <table> ADD COLUMN` per column, then the indexes. Fields are required |
| `remove_<column>_from_<table>` | `ALTER TABLE <table> DROP COLUMN <column>`; with fields, one `DROP COLUMN` per field column instead |
| anything else, no fields | An empty migration to fill in (data changes, custom SQL) |

Existing rows need a value for a new `NOT NULL` column, so `add_..._to_...` adds `DEFAULT ''` to required text, date and datetime columns, `DEFAULT 0` to required numbers, `DEFAULT '{}'` to required `json` columns (optional `json?` columns stay `NULL`), and booleans already default to 0. References and attachments must be optional there.

```sh
ocre g migration add_slug_to_posts slug:string^
```

```text
  create  migrations/0011_add_slug_to_posts.sql

Next:
  ocre migrate
  update the model in src/models/ to match the new columns
```

```sql
-- Migration: add_slug_to_posts
-- Applied once, in file-name order. Never edit a migration after it has been applied.
ALTER TABLE posts ADD COLUMN slug TEXT NOT NULL DEFAULT '';
CREATE UNIQUE INDEX index_posts_on_slug ON posts (slug);
```

```sh
ocre g migration add_slug_to_posts slug:string^ --json
```

```json
{"command":"generate migration","created":["migrations/0011_add_slug_to_posts.sql"],"next":["ocre migrate","update the model in src/models/ to match the new columns"],"ok":true}
```

`ocre g migration remove_slug_from_posts` writes `ALTER TABLE posts DROP COLUMN slug;`, and `ocre g migration backfill_slugs` an empty migration; without fields, the only next step is `ocre migrate`. A migration does not change the model: add or remove the fields in `src/models/<model>.rs` (the row struct, `New...`, `...Changes`, and the SQL of `create` and `update`) yourself.

Errors:

| Error | Hint |
|---|---|
| ``invalid migration name `AddX` `` | ``use snake_case, e.g. `add_slug_to_posts` `` |
| ``cannot tell which table `fix_stuff` changes`` (fields with an unknown name form) | ``name it `create_<table>`, `add_<columns>_to_<table>` or `remove_<columns>_from_<table>` `` |
| `add_..._to_... needs the columns to add` | ``list them like the model fields, e.g. `ocre g migration add_slug_to_posts slug:string^` `` |
| ``` `owner_id` must be optional when added to an existing table ``` | ``SQLite adds reference columns as NULL for existing rows: use `name:references?` `` |
| ``` `<name>` must be optional when added to an existing table ``` (attachment) | ``existing rows have no file: use `name:attachment?` `` |

## ocre g mailer

```text
ocre g mailer <NAME> <ACTIONS>...
```

| Argument | Required | Meaning |
|---|---|---|
| `NAME` | yes | Mailer name, PascalCase or snake_case; a `Mailer` suffix is dropped (`UserMailer`, `user_mailer` and `User` all give `user`) |
| `ACTIONS` | yes, at least one | Email names in snake_case (`welcome`, `password_reset`), one function each |

Creates `src/mailers/<name>.rs` with one function per action, `pub fn welcome(to: &str) -> Result<Email>`, building an `ocre::mail::Email` whose subject is the humanized action (`Password reset`). In a full-stack app, each function renders two askama templates, `templates/mailers/<name>/<action>.txt` and `.html`; in an API-only app, the text is built with `format!` and there are no templates. The first mailer creates `src/mailers/mod.rs` and adds `mod mailers;` to `src/lib.rs`.

```sh
ocre g mailer User welcome password_reset
```

```text
  create  src/mailers/user.rs
  create  templates/mailers/user/welcome.txt
  create  templates/mailers/user/welcome.html
  create  templates/mailers/user/password_reset.txt
  create  templates/mailers/user/password_reset.html
  create  src/mailers/mod.rs
  update  src/lib.rs

Next:
  send it from a handler: ocre::mail::send(&ctx, mailers::user::welcome(&address)?).await?
  ocre dev (MAIL_ADAPTER=log in .dev.vars prints each email instead of sending it)
```

```json
{"command":"generate mailer","created":["src/mailers/user.rs","templates/mailers/user/welcome.txt","templates/mailers/user/welcome.html","templates/mailers/user/password_reset.txt","templates/mailers/user/password_reset.html","src/mailers/mod.rs"],"next":["send it from a handler: ocre::mail::send(&ctx, mailers::user::welcome(&address)?).await?","ocre dev (MAIL_ADAPTER=log in .dev.vars prints each email instead of sending it)"],"ok":true,"updated":["src/lib.rs"]}
```

Errors: ``invalid mailer name `type` `` (hint: ``use a name starting with a letter that is not a Rust keyword, e.g. `User` or `Billing` ``); ``invalid action name `select` `` (hint: ``use snake_case starting with a letter, not a Rust or SQL keyword, e.g. `welcome` or `password_reset` ``); ``action `receipt` is listed twice`` (hint: `list each action once`); `src/mailers/<name>.rs already exists`. Names are checked against the same reserved words as [fields](#fields). See [Email](../guides/email.md).

## ocre g mailbox

```text
ocre g mailbox
```

No arguments. Creates `src/mailbox.rs`, the handler of email that Cloudflare Email Routing sends to the Worker (`pub async fn receive(ctx, email: InboundEmail) -> Result<()>`), and appends the Worker's `email` event to `src/lib.rs`, which calls `ocre::mail::receive(message, env, mailbox::receive)`. A Worker has one email entry point, so an app has one mailbox, which routes by `email.to()` itself.

```sh
ocre g mailbox
```

```text
  create  src/mailbox.rs
  update  src/lib.rs

Next:
  ocre dev
  curl 'http://localhost:8787/cdn-cgi/local/email?from=ada@example.com&to=support@example.com' --data-binary @message.eml
  route addresses to the Worker: Cloudflare dashboard > Email Routing > Routing rules > Send to a Worker
```

```json
{"command":"generate mailbox","created":["src/mailbox.rs"],"next":["ocre dev","curl 'http://localhost:8787/cdn-cgi/local/email?from=ada@example.com&to=support@example.com' --data-binary @message.eml","route addresses to the Worker: Cloudflare dashboard > Email Routing > Routing rules > Send to a Worker"],"ok":true,"updated":["src/lib.rs"]}
```

Errors: `src/mailbox.rs already exists`; `src/lib.rs already handles the `email` event` with the hint ``a Worker has one email entry point: call `ocre::mail::receive(message, env, mailbox::receive)` from it``. See [Email](../guides/email.md).

## ocre g job

```text
ocre g job <NAME> [FIELDS]...
```

| Argument | Required | Meaning |
|---|---|---|
| `NAME` | yes | Job name, PascalCase or snake_case, a verb phrase; a `Job` suffix is dropped (`ImportCsvJob` gives `import_csv` and `ImportCsv`) |
| `FIELDS` | no | The job's arguments as `name:type`; no `attachment`, no `^` |

Creates `src/jobs/<name>.rs`: a struct holding the arguments (serialized as JSON in the queue message, 128 KB at most) with an `async fn perform(self, ctx: &Ctx) -> Result<()>` to fill in. It adds a variant to the `Job` enum and an arm to the `perform` match in `src/jobs/mod.rs`. The first job creates `src/jobs/mod.rs`, adds `mod jobs;` and the Worker's `queue` event to `src/lib.rs`, and, unless a `JOBS` producer exists, appends to `wrangler.toml` the `JOBS` producer on the queue `<app>-jobs` and its consumer (batches of up to 10 messages, 5 retries, dead-letter queue `<app>-jobs-failed`).

```sh
ocre g job SendWelcome user_id:integer
```

```text
  create  src/jobs/send_welcome.rs
  create  src/jobs/mod.rs
  update  src/lib.rs
  update  wrangler.toml

Next:
  enqueue it from a handler: ocre::jobs::enqueue(&ctx, &jobs::Job::SendWelcome(jobs::SendWelcome { user_id })).await?
  ocre dev (jobs run locally; look for `[ocre jobs]` lines in the output)
  ocre deploy creates the queue blog-jobs and its dead-letter queue
```

```json
{"command":"generate job","created":["src/jobs/send_welcome.rs","src/jobs/mod.rs"],"next":["enqueue it from a handler: ocre::jobs::enqueue(&ctx, &jobs::Job::SendWelcome(jobs::SendWelcome { user_id })).await?","ocre dev (jobs run locally; look for `[ocre jobs]` lines in the output)","ocre deploy creates the queue blog-jobs and its dead-letter queue"],"ok":true,"updated":["src/lib.rs","wrangler.toml"]}
```

Later jobs only create their file and update `src/jobs/mod.rs`; the third next step is only printed by the first job. Free plan (September 2026): 10,000 Queues operations a day, a job costing 3 (write, read, delete), and 10 ms of CPU per batch ([Queues pricing](https://developers.cloudflare.com/queues/platform/pricing/), [Workers limits](https://developers.cloudflare.com/workers/platform/limits/#cpu-time)).

Errors:

| Error | Hint |
|---|---|
| ``invalid job name `Job` `` (also Rust keywords) | ``use a verb phrase starting with a letter, not a Rust keyword, e.g. `SendWelcome` or `ImportCsv` `` |
| ``job field `f` cannot be an attachment`` | ``files do not fit in a queue message (128 KB): store the file first and pass its record id, e.g. `post_id:integer` `` |
| ``job field `f` cannot be unique`` | ``` `^` adds a unique index to a table column; jobs have no table: drop the `^` ``` |
| `src/jobs/<name>.rs already exists` | the generic hint |
| ``src/lib.rs already handles the `queue` event`` (first job only) | ``a Worker has one queue entry point: call `ocre::jobs::consume(batch, env, jobs::perform)` from it and create src/jobs/mod.rs by hand`` |

See [Background jobs and schedules](../guides/jobs.md).

## ocre g schedule

```text
ocre g schedule <NAME> <CRON>
```

| Argument | Required | Meaning |
|---|---|---|
| `NAME` | yes | Task name in snake_case (`HourlyPing` is accepted and becomes `hourly_ping`); not a reserved word |
| `CRON` | yes | Cron expression, five fields in UTC (minute, hour, day of month, month, day of week), quoted for the shell: `"0 3 * * *"` |

Creates `src/schedules/<name>.rs` with `pub async fn run(ctx: &Ctx) -> Result<()>` to fill in, adds the expression to `[triggers] crons` in `wrangler.toml` (creating the table if needed), and adds `"<cron>" => <name>::run(&ctx).await,` to the dispatch `match` of `src/schedules/mod.rs`. The first schedule creates `src/schedules/mod.rs` and adds `mod schedules;` and the Worker's `scheduled` event to `src/lib.rs`. The CLI checks the expression's shape (five fields of letters, digits and `*,-/#`), not its values; Cloudflare validates it at deploy ([syntax](https://developers.cloudflare.com/workers/configuration/cron-triggers/#supported-cron-expressions)).

```sh
ocre g schedule nightly_cleanup "0 3 * * *"
```

```text
  create  src/schedules/nightly_cleanup.rs
  create  src/schedules/mod.rs
  update  wrangler.toml
  update  src/lib.rs

Next:
  ocre dev, then: curl 'http://localhost:8787/cdn-cgi/local/scheduled?cron=0+3+*+*+*'
  ocre deploy (Cron Triggers only fire on the deployed Worker, in UTC)
```

```json
{"command":"generate schedule","created":["src/schedules/nightly_cleanup.rs","src/schedules/mod.rs"],"next":["ocre dev, then: curl 'http://localhost:8787/cdn-cgi/local/scheduled?cron=0+3+*+*+*'","ocre deploy (Cron Triggers only fire on the deployed Worker, in UTC)"],"ok":true,"updated":["wrangler.toml","src/lib.rs"]}
```

Free plan (September 2026): 5 Cron Triggers per account and 10 ms of CPU per run ([Workers limits](https://developers.cloudflare.com/workers/platform/limits/)). When the app's `[triggers] crons` holds more than 5 expressions, the generator adds a next step: `this app now has 6 crons; the free plan allows 5 per account: run several tasks from one cron`.

Errors:

| Error | Hint |
|---|---|
| ``invalid schedule name `<name>` `` | ``use snake_case starting with a letter, not a Rust keyword, e.g. `nightly_cleanup` `` |
| ``invalid cron expression `0 3 * *` `` | `quote five fields, in UTC: minute hour day-of-month month day-of-week, e.g. "0 3 * * *" (03:00 daily), "*/15 * * * *" (every 15 minutes) or "0 9 * * MON"` |
| ``cron `0 3 * * *` is already in [triggers] crons`` | `one task per cron: call the new work from the existing task in src/schedules/, or pick another time (e.g. one minute later)` |
| ``wrangler.toml defines `triggers` in a form Ocre cannot edit`` | ``write it as a table: `[triggers]` on its own line, then `crons = ["0 3 * * *"]` `` |
| ``src/lib.rs already handles the `scheduled` event`` (first schedule only) | ``a Worker has one scheduled entry point: call `ocre::jobs::cron(event, env, schedules::run)` from it and create src/schedules/mod.rs by hand`` |

See [Background jobs and schedules](../guides/jobs.md).

## ocre g cache

```text
ocre g cache
```

No arguments. Appends a `[[kv_namespaces]]` entry with `binding = "CACHE"` and no `id` to `wrangler.toml`, for `ocre::cache::fetch` (a read-through cache of JSON values). `ocre dev` uses a local namespace; the first [`ocre deploy`](cli.md#ocre-deploy) creates the real one (titled `<app>-cache`) and writes its `id` into `wrangler.toml`. The binding is opt-in because Workers KV allows 1,000 writes a day on the free plan (September 2026; 100,000 reads a day, 1 GB stored, [KV limits](https://developers.cloudflare.com/kv/platform/limits/)).

```sh
ocre g cache
```

```text
  update  wrangler.toml

Next:
  use it: ocre::cache::fetch(&ctx, "key:v1", Duration::from_secs(3600), || async { ... }).await?
  ocre dev
  ocre deploy (creates the KV namespace)
```

```json
{"command":"generate cache","next":["use it: ocre::cache::fetch(&ctx, \"key:v1\", Duration::from_secs(3600), || async { ... }).await?","ocre dev","ocre deploy (creates the KV namespace)"],"ok":true,"updated":["wrangler.toml"]}
```

Error: ``wrangler.toml already has the `CACHE` KV binding`` with the hint ``nothing to generate: call `ocre::cache::fetch(&ctx, key, ttl, || async { ... })` in a handler``. See [Caching](../guides/caching.md).

## ocre g locale

```text
ocre g locale <CODES>...
```

| Argument | Required | Meaning |
|---|---|---|
| `CODES` | yes, at least one | Locale codes: a 2- or 3-letter lowercase language, optionally followed by `-` and region or script parts of 2 to 8 letters or digits (`en`, `fr`, `pt-BR`, `zh-Hant`) |

Creates `locales/<code>.yml` for each code and declares them in `ocre::locales!(...)` in `src/lib.rs`. The first run sets up translations: its first code becomes the default locale (its file gets a sample `app.welcome` key and comments on the syntax, the others are empty and point to it), `src/lib.rs` gets `static LOCALES: ocre::i18n::Locales = ocre::locales!("en", "fr");`, and `routes()` gets `.layer(ocre::i18n::layer(&LOCALES))` as the last call of its `Router` chain, which the `I18n` extractor needs. Later runs add codes to the declaration.

```sh
ocre g locale en fr
```

```text
  create  locales/en.yml
  create  locales/fr.yml
  update  src/lib.rs

Next:
  add keys to the locale files; take `i18n: ocre::i18n::I18n` in a handler and call i18n.t("key")
  ocre i18n missing
```

```json
{"command":"generate locale","created":["locales/en.yml","locales/fr.yml"],"next":["add keys to the locale files; take `i18n: ocre::i18n::I18n` in a handler and call i18n.t(\"key\")","ocre i18n missing"],"ok":true,"updated":["src/lib.rs"]}
```

Then `ocre g locale de` adds a third locale. [`ocre i18n missing`](cli.md#ocre-i18n-missing) lists the keys each locale still lacks.

Errors:

| Error | Hint |
|---|---|
| ``invalid locale code `EN` `` | `use a language code, optionally with a region or script: en, fr, pt-BR, zh-Hant` |
| ``locale `fr` is already declared in src/lib.rs`` | ``edit locales/fr.yml; `ocre i18n missing` lists keys to translate`` |
| `ocre::locales!() in src/lib.rs lists no locale` | ``put the default locale in it: ocre::locales!("en")`` |
| ``src/lib.rs is missing the `// ocre:routes` marker`` (first run) | ``put `// ocre:routes` on its own line at the end of the `Router::new()` chain in routes()`` |

See [Translations](../guides/i18n.md).

## See also

- [CLI commands](cli.md): `ocre migrate`, `ocre dev`, `ocre deploy`, `ocre routes` and the `--json` contract.
- [Field types](field-types.md): each field type in SQL, Rust, forms and JSON.
- [Models and migrations](../guides/models.md), [Validations](../guides/validations.md).
- [Why generated code](../explanations/generated-code.md).
