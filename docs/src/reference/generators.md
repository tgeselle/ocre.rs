# Generators

This page documents every `ocre g` generator: its arguments and flags, the naming rules, the files it creates and updates, what the generated code contains, and its errors. Generators write plain Rust, SQL and templates into the app, which the app then owns and edits.

## Before you start

- An Ocre app created with [`ocre new`](cli.md#ocre-new). Run generators from its root or any directory below it (the CLI looks for the nearest `cloudflare.config.ts`).
- Generators only write files: they need no network, no cf and no Cloudflare account. Run [`ocre migrate`](cli.md#ocre-migrate) after the ones that add migrations, and `cargo check --target wasm32-unknown-unknown` (or [`ocre test`](cli.md#ocre-test)) to type-check the result.
- Keep the `// ocre:...` marker comments of generated files (`// ocre:modules` and `// ocre:routes` in `src/lib.rs`, `// ocre:models` in `src/models/mod.rs`...): generators insert lines right after them.

The examples below were run with `ocre 0.1.0` on an app created by `ocre new blog --starter blog`, in the order of this page.

## How generators behave

- **All or nothing.** A generator computes every change first and writes nothing until the whole generation succeeded, so a failure never leaves half a resource.
- **Never overwrite by default.** A generator creates new files and only edits existing ones by inserting lines at markers (in `cloudflare.config.ts`, after `// ocre:env`, `// ocre:triggers` and `// ocre:exports`; see [Configuration](configuration.md#canonical-entries)). When a file it would create exists, it stops with `<path> already exists` and the hint ``generators create new files only: pass --skip to keep the existing file, --force to overwrite it, or edit it (`ocre g migration` for tables)``. Lines already present after a marker are not inserted twice. To change a table after its migration was applied, add a migration with [`ocre g migration`](#ocre-g-migration).
- **Report.** The human output lists `  create  <path>`, `  update  <path>` and `  skip    <path>` lines, then `Next:` steps. With `--json` it is one object with `command` (`generate <generator>`), `created`, `updated`, `skipped`, `pretend` and `next` (see [the --json contract](cli.md#global-flag-json)).
- **Recorded.** Every run that writes something saves what it changed in `.ocre/generated/NNNN_<generator>_<name>.json` (see [Generation records and ocre destroy](#generation-records-and-ocre-destroy)). Commit the directory with the code.
- **Full-stack or API-only.** An app created with `ocre new --api` has `[package.metadata.ocre] mode = "api"` in `Cargo.toml`. There, `ocre g scaffold` generates a JSON API (like `ocre g api`), and `auth` and `mailer` generate no HTML.
- **Migrations** are numbered after the highest existing number: `migrations/0002_create_comments.sql`, then `0003_...`.
- **Marker errors.** When a marker a generator needs is missing, it fails with ``<file> is missing the `<marker>` marker`` and a hint saying where to put it back.

| Generator | Creates |
|---|---|
| [`ocre g model`](#ocre-g-model) | Migration and `src/models/<model>.rs` |
| [`ocre g scaffold`](#ocre-g-scaffold) | Model (unless it exists) plus HTML CRUD pages; `--realtime` for live updates |
| [`ocre g api`](#ocre-g-api) | Model (unless it exists) plus a JSON REST resource; `--graphql` for GraphQL |
| [`ocre g resource`](#ocre-g-resource) | Model (unless it exists) plus `index` and `show` actions to fill in |
| [`ocre g controller`](#ocre-g-controller) | A module of GET actions, with a page each (or JSON) |
| [`ocre g auth`](#ocre-g-auth) | Users, sessions, password reset, magic links, email confirmation, JWT and API keys; `--db-sessions`, `--oauth` (once per app) |
| [`ocre g migration`](#ocre-g-migration) | One numbered SQL migration, its SQL inferred from the name |
| [`ocre g mailer`](#ocre-g-mailer) | Functions building emails, with templates |
| [`ocre g mailbox`](#ocre-g-mailbox) | The handler of incoming email (once per app) |
| [`ocre g job`](#ocre-g-job) | A background job on Cloudflare Queues |
| [`ocre g schedule`](#ocre-g-schedule) | A task run by a Cron Trigger |
| [`ocre g cache`](#ocre-g-cache) | The `CACHE` Workers KV binding |
| [`ocre g ci`](#ocre-g-ci) | The GitHub Actions workflow: `ocre ci`'s checks, then `ocre deploy` on main |
| [`ocre g pwa`](#ocre-g-pwa) | Web app manifest, service worker and icon, linked from the layout |
| [`ocre g locale`](#ocre-g-locale) | Translation files |
| [`ocre g override`](#ocre-g-override) | Copies of generator templates in `.ocre/templates/`, which then replace the built-in ones |
| [`ocre g generator`](#ocre-g-generator) | An app generator in `.ocre/generators/<name>/` |
| [`ocre g <name>`](#app-generators-ocre-g-name) | Runs the app generator `.ocre/generators/<name>/` |

## Generator flags

Every generator accepts these flags, before or after its arguments:

| Flag | Effect |
|---|---|
| `--pretend` | Computes and reports the changes (`create`, `update` and `skip` lines, then `(--pretend: nothing was written)`; `"pretend": true` in JSON), writes nothing and records nothing |
| `--force` | Overwrites files the generator creates when they already exist (they are reported as `update`) |
| `--skip` | Keeps files that already exist (reported as `skip`) and generates the rest |

`--force` and `--skip` cannot be combined. Neither changes how existing files such as `src/lib.rs` are edited: lines are inserted at markers once.

```sh
ocre g scaffold Draft title:string --pretend
```

```text
  create  migrations/0004_create_drafts.sql
  create  src/models/draft.rs
  create  src/drafts.rs
  create  templates/drafts/index.html
  create  templates/drafts/show.html
  create  templates/drafts/new.html
  create  templates/drafts/edit.html
  create  templates/drafts/_form.html
  update  src/models/mod.rs
  update  src/lib.rs
(--pretend: nothing was written)

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/drafts
```

Running a controller generator again, keeping what exists:

```sh
ocre g controller Pages about --skip
```

```text
  skip    src/pages.rs
  skip    templates/pages/about.html

Next:
  ocre dev
  open http://localhost:8787/pages/about
```

## Generation records and ocre destroy

Each generator run that changes files writes a JSON record in `.ocre/generated/`, numbered like migrations (`0003_controller_pages.json`): the command as typed, the generator, its first argument, each file created with a SHA-256 of its contents, and for each file updated the lines added and removed around a context line. [`ocre destroy <generator> [NAME]`](cli.md#ocre-destroy) reads the latest matching record and undoes the run: it deletes the files created and takes out the lines added (Cargo.toml, cloudflare.config.ts and package.json changes stay). It refuses when a created file changed since, unless `--force`.

```sh
ocre g scaffold Temp name:string
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

Apps created before records existed have none for their earlier runs: `ocre destroy` then says ``no recorded `ocre g ...` run to destroy``.

## Fields

`model`, `scaffold`, `api`, `resource`, `migration` and `job` take fields as `name:type`, with optional suffixes: `?` makes the field optional (the column accepts `NULL`, the Rust type is an `Option`), `^` makes it unique (a unique index plus a "has already been taken" check). Both can be combined (`slug:string?^`). The types, detailed in [Field types](field-types.md):

| Type | SQL column | Rust type | Notes |
|---|---|---|---|
| `string` | `TEXT` | `String` | One-line text; required unless `?` |
| `text` | `TEXT` | `String` | Multi-line text (a textarea in forms); required unless `?` |
| `integer` (`int`, `small_int`, `big_int`) | `INTEGER` | `i64` | Validated within ±(2^53 - 1), the integers D1 returns exactly |
| `float` (`double`) | `REAL` | `f64` | |
| `decimal` | `TEXT` | `String` | Exact number such as `19.99` (money); validated as a decimal |
| `boolean` (`bool`) | `INTEGER NOT NULL DEFAULT 0` | `bool` | A checkbox; cannot be `?` |
| `date` | `TEXT` | `String` | Validated as `YYYY-MM-DD` |
| `time` | `TEXT` | `String` | Validated as `HH:MM[:SS]` |
| `datetime` (`date_time`) | `TEXT` | `String` | Validated as a date and time |
| `uuid` | `TEXT` | `String` | Validated as a hyphenated UUID |
| `references` | `<name>_id INTEGER REFERENCES <plural>(id) ON DELETE CASCADE` (`SET NULL` when `?`), indexed | `i64` | `author:references` adds `author_id`, `author:references:writer_id` names the column; `src/models/author.rs` must exist; validated as "must exist" |
| `attachment` | four columns: `<name>_key`, `<name>_filename`, `<name>_content_type` (`TEXT`), `<name>_size` (`INTEGER`) | `ocre::storage::Upload` when received, `Attachment` when stored | A file in R2; cannot be `^`; cannot be named `edit`, `delete` or `new`; must be `?` in JSON APIs; adds the `STORAGE` R2 binding to `cloudflare.config.ts` |
| `json` (`jsonb`) | `TEXT CHECK (json_valid(<name>))` | `ocre::serde_json::Value` | Any JSON value; cannot be `^` |
| `enum:<a>,<b>...` | `TEXT CHECK (<name> IN ('a', 'b'))` | a Rust enum generated in the model (`status` gives `Status`) | A `<select>` in forms; cannot be `^`; not with `--graphql` |
| `rich_text` | `TEXT` | `String` | Formatted text (the Trix editor in forms), sanitized when saved; cannot be `^` |
| `polymorphic:<model>,<model>...` | `<name>_type` (an enum of the models) and `<name>_id` (`INTEGER`), indexed together | `<Name>Type` and `i64`; `record.<name>(ctx)` returns an enum of the records | `commentable:polymorphic:post,photo`; the models must exist; checked as "must exist"; cannot be `^` |
| `attachments` (`model`, `scaffold`, `api`, `resource` only) | a child table `<model>_<singular>` with a `file` attachment | the child model, and `attach_<name>` / `replace_<name>` / `purge_<name>` on the parent | `photos:attachments`; plural name; no `?` or `^` (see [Files](../guides/files.md#many-files-per-record)) |

Field names are snake_case, start with a lowercase letter, and must not be reserved: `id`, `created_at` and `updated_at` (every table gets them), Rust keywords (`type`, `match`, `mod`, `ref`, `self`, `use`, `where`, `yield`...), and these SQL keywords: `and`, `asc`, `by`, `case`, `check`, `default`, `desc`, `from`, `group`, `index`, `join`, `key`, `limit`, `not`, `null`, `offset`, `or`, `order`, `primary`, `references`, `select`, `table`, `unique`, `values`. Two fields cannot produce the same column (an attachment `avatar` takes `avatar_key`, `avatar_filename`, `avatar_content_type` and `avatar_size`).

Field errors (shared by every generator that takes fields):

| Error | Hint |
|---|---|
| ``field `title` has no type`` | ``write fields as `name:type`, e.g. `title:string` `` |
| ``invalid field name `<name>` `` | ``use snake_case starting with a letter, e.g. `published_at` `` |
| ``field name `type` is reserved`` | ``` `id`, `created_at` and `updated_at` are generated; Rust and SQL keywords are not allowed. Pick another name, e.g. `kind` for `type` ``` |
| ``unknown field type `strng` for `title` `` | ``types: string, text, rich_text, integer (int, small_int, big_int), float (double), decimal, boolean (bool), date, time, datetime (date_time), uuid, references, attachment, json (jsonb), enum:<value>,<value>..., polymorphic:<model>,<model>..., attachments (many files, `photos:attachments`); `lock_version:integer` turns on optimistic locking; add `?` for optional, `^` for unique`` |
| ``boolean field `done` cannot be optional`` | ``booleans are true or false (a checkbox); drop the `?` `` |
| ``attachment `a` cannot be unique`` | ``every stored file gets its own random key already; drop the `^` `` |
| ``json field `v` cannot be unique`` | ``a unique index compares JSON text, where key order and spacing differ; drop the `^` `` |
| ``enum `s` has no values`` | ``list them after the type, e.g. `s:enum:draft,published` `` |
| ``invalid values `A,b` for enum `s` `` | ``list distinct snake_case values after the type, e.g. `status:enum:draft,published` `` |
| ``enum `s` cannot be unique`` | ``a few values cannot be unique across many rows; drop the `^` `` |
| ``invalid foreign key column `writer` for `author` `` | ``name the column in snake_case ending in `_id`, e.g. `author:references:writer_id` `` |
| ``type `string` of `title` takes no `:long` `` | ``only `references` (the foreign key column, e.g. `author:references:writer_id`) and `enum` (its values, e.g. `status:enum:draft,published`) take an argument`` |
| ``attachment name `edit` clashes with a scaffold route`` | ``` `/<plural>/{id}/edit` is taken; pick another name, e.g. `edit_file` ``` |
| ``field `a_key` is listed twice`` | ``names must differ, and `<name>:attachment` also takes `<name>_key`, `<name>_filename`, `<name>_content_type` and `<name>_size` `` |
| `src/models/owner.rs does not exist` (for `owner:references`) | ``generate the referenced model first, e.g. `ocre g model Owner name:string` `` |

## Model names

`model`, `scaffold`, `api` and `resource` take a singular model name, in PascalCase, snake_case, kebab-case or with spaces (`BlogPost`, `blog_post`, `blog-post`). It is split into words, which give every other name:

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

A `references` field also updates the referenced model: `ocre g scaffold Comment ... post:references` adds `pub async fn comments(&self, ctx, page)` (has many, newest first) to `src/models/post.rs` after its `// ocre:associations` marker, and gives `Comment` a `post(&self, ctx)` method (belongs to). An `attachment` field adds a `Rules` constant per file (`pub const IMAGE: Rules`, 10 MB and common image, PDF and text types until you edit it), a method returning its `Attachment`, `create`/`update` that store files in R2 and delete replaced ones, and the `STORAGE` binding to `cloudflare.config.ts` (`STORAGE: bindings.r2({ name: "<app>-storage" }),`) unless it is there.

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

With `--realtime`, the scaffold also creates `templates/<plural>/_row.html` (one row, rendered for the page and for broadcasts), makes the handlers broadcast each change on the `<plural>` channel, and on first use creates `src/realtime.rs` (the `GET /realtime/{channel}` route and the list of channels anyone may listen to), turns on Ocre's `realtime` feature in `Cargo.toml` and declares the `CHANNELS` Durable Object binding and the `OcreChannel` export (`exports.durableObject({ storage: "sqlite" })`, SQLite-backed, as the free plan requires) in `cloudflare.config.ts`. Later `--realtime` scaffolds add their channel after `// ocre:channels` in `src/realtime.rs`:

```sh
ocre g scaffold Message body:text --realtime --json
```

```json
{"command":"generate scaffold","created":["migrations/0008_create_messages.sql","src/models/message.rs","src/messages.rs","templates/messages/index.html","templates/messages/show.html","templates/messages/new.html","templates/messages/edit.html","templates/messages/_form.html","templates/messages/_row.html","src/realtime.rs"],"next":["ocre migrate","ocre dev","open http://localhost:8787/messages","open http://localhost:8787/messages in a second window, then create a message"],"ok":true,"updated":["src/models/mod.rs","src/lib.rs","Cargo.toml","cloudflare.config.ts"]}
```

With attachments, the form becomes a file upload and `cloudflare.config.ts` gets the `STORAGE` bucket:

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
  update  cloudflare.config.ts
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

Errors: the field and name errors; `src/<plural>.rs already exists` (or a template) when the pages exist; in an API-only app, `--realtime updates HTML pages; this app is API-only` with the hint ``run `ocre g scaffold` without --realtime; to push JSON to clients, see Realtime in the Ocre README``. See [Controllers and routing](../guides/controllers.md), [Views, helpers and forms](../guides/views.md), [File storage](../guides/files.md) and [Realtime](../guides/realtime.md).

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

Errors: the field and name errors; ``attachment `f` must be optional in a JSON API`` with the hint ``JSON cannot carry a file, so create cannot require one: use `f:attachment?`, then upload with `curl -X PUT -F f=@file http://localhost:8787/api/<plural>/1/f` ``; with `--graphql`, ``enum `state` is not supported with --graphql yet`` with the hint ``use `state:string` checked with `v.inclusion(...)` in the model, or generate the JSON API without --graphql``; `src/<plural>_api.rs already exists`. See [JSON APIs and GraphQL](../guides/json-apis.md).

## ocre g resource

```text
ocre g resource <NAME> <FIELDS>... [--api]
```

| Argument or flag | Default | Meaning |
|---|---|---|
| `NAME` | required | Singular model name |
| `FIELDS` | required, at least one | `name:type` fields |
| `--api` | off | JSON actions under `/api/<plural>` in a full-stack app (always JSON in an API-only app) |

Lighter than a scaffold: the model (unless `src/models/<model>.rs` exists) and a controller with `index` (paginated list, newest first) and `show` over it, to fill in with the actions you need. In a full-stack app it writes `src/<plural>.rs` with a `paths` module and the pages `templates/<plural>/index.html` and `show.html`; in JSON mode `src/<plural>_api.rs` answers `GET /api/<plural>` and `GET /api/<plural>/{id}`. The module is registered in `src/lib.rs`.

```sh
ocre g resource Tag name:string^ color:enum:red,green,blue
```

```text
  create  migrations/0003_create_tags.sql
  create  src/models/tag.rs
  create  src/tags.rs
  create  templates/tags/index.html
  create  templates/tags/show.html
  update  src/models/mod.rs
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/tags
```

```json
{"command":"generate resource","created":["migrations/0003_create_tags.sql","src/models/tag.rs","src/tags.rs","templates/tags/index.html","templates/tags/show.html"],"next":["ocre migrate","ocre dev","open http://localhost:8787/tags"],"ok":true,"updated":["src/models/mod.rs","src/lib.rs"]}
```

With `--api`, the last next step is `curl http://localhost:8787/api/<plural>`. The files come from the `resource/` templates, which [`ocre g override`](#ocre-g-override) copies for editing. Errors: the field and name errors, and `<path> already exists`.

## ocre g controller

```text
ocre g controller <NAME> [ACTIONS]... [--api] [--auth]
```

| Argument or flag | Default | Meaning |
|---|---|---|
| `NAME` | required | Controller name, PascalCase or snake_case; a `Controller` suffix is dropped (`PagesController` gives `pages`). Not a Rust keyword, not ending in `api` |
| `ACTIONS` | `index` | Action names in snake_case, one GET route each; `index` answers at the controller's root path |
| `--api` | off | JSON actions under `/api/<name>` in a full-stack app (always JSON in an API-only app) |
| `--auth` | off | Signed-in users only: handlers take `CurrentUser` (HTML) or `BearerUser` (JSON). Needs `src/auth.rs` (or `src/auth_api.rs`) from [`ocre g auth`](#ocre-g-auth) |

Creates a module of GET actions, like `rails generate controller`: in a full-stack app `src/<name>.rs` with one askama view struct per action, a `paths` module (`paths::about()`), and a page per action, `templates/<name>/<action>.html`, extending `layout.html`; in JSON mode `src/<name>_api.rs` with one `ApiResult<Json<...>>` handler per action. The module is registered in `src/lib.rs`.

```sh
ocre g controller Pages about contact
```

```text
  create  src/pages.rs
  create  templates/pages/about.html
  create  templates/pages/contact.html
  update  src/lib.rs

Next:
  ocre dev
  open http://localhost:8787/pages/about
```

```json
{"command":"generate controller","created":["src/pages.rs","templates/pages/about.html","templates/pages/contact.html"],"next":["ocre dev","open http://localhost:8787/pages/about"],"ok":true,"updated":["src/lib.rs"]}
```

`ocre g controller Metrics summary --api` creates `src/metrics_api.rs` with `GET /api/metrics/summary`, answering `{"message": "Edit summary in src/metrics_api.rs"}` until you change it; its next step is `curl http://localhost:8787/api/metrics/summary`. The files come from the `controller/` templates.

Errors:

| Error | Hint |
|---|---|
| ``invalid controller name `type` `` | ``use a name starting with a letter that is not a Rust keyword and does not end in `api`, e.g. `Pages` or `Dashboard` `` |
| ``invalid action name `type` `` | ``use snake_case starting with a letter, not a Rust keyword nor `routes`/`paths`, e.g. `about` or `contact_us` `` |
| ``action `about` is listed twice`` | `list each action once` |
| ``--auth needs src/auth.rs, which `ocre g auth` creates`` (`src/auth_api.rs` in JSON mode) | ``run `ocre g auth` and `ocre migrate` first, or generate the controller without --auth`` |
| `src/<name>.rs already exists` | the generic hint (`--skip`, `--force`) |

See [Controllers, routing, views and htmx](../guides/controllers.md).

## ocre g auth

```text
ocre g auth [--db-sessions] [--oauth <provider,...>]
```

No positional arguments. Generates authentication into the app, like Rails 8's authentication generator, so every rule is visible and editable there. It runs once per app.

| Option | Effect |
|---|---|
| `--db-sessions` | Tracks each sign-in in D1 (`user_sessions`: IP address, browser, last activity, expiry); `/account/sessions` lists the signed-in devices and signs them out. Costs one D1 read per signed-in request |
| `--oauth github,google` | "Continue with GitHub / Google" buttons on the login page (OAuth 2.0 code flow with PKCE, `ocre::oauth`). Accepts `github` and `google`, comma-separated or repeated |

In a new full-stack app:

```sh
ocre g auth
```

```text
  create  migrations/0001_create_users.sql
  create  migrations/0002_create_auth_tokens.sql
  create  migrations/0003_create_api_keys.sql
  create  src/models/mod.rs
  create  src/models/user.rs
  create  src/models/api_key.rs
  create  src/models/auth_token.rs
  create  src/auth_api.rs
  create  src/auth.rs
  create  src/registrations.rs
  create  src/sessions.rs
  create  src/passwords.rs
  create  src/confirmations.rs
  create  templates/auth/signup.html
  create  templates/auth/login.html
  create  templates/auth/account.html
  create  templates/auth/magic_link_new.html
  create  templates/auth/magic_link_show.html
  create  templates/auth/password_new.html
  create  templates/auth/password_edit.html
  create  templates/auth/confirmation_show.html
  update  src/lib.rs
  update  cloudflare.config.ts

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/signup
```

```json
{"command":"generate auth","created":["migrations/0001_create_users.sql","migrations/0002_create_auth_tokens.sql","migrations/0003_create_api_keys.sql","src/models/mod.rs","src/models/user.rs","src/models/api_key.rs","src/models/auth_token.rs","src/auth_api.rs","src/auth.rs","src/registrations.rs","src/sessions.rs","src/passwords.rs","src/confirmations.rs","templates/auth/signup.html","templates/auth/login.html","templates/auth/account.html","templates/auth/magic_link_new.html","templates/auth/magic_link_show.html","templates/auth/password_new.html","templates/auth/password_edit.html","templates/auth/confirmation_show.html"],"next":["ocre migrate","ocre dev","open http://localhost:8787/signup"],"ok":true,"updated":["src/lib.rs","cloudflare.config.ts"]}
```

In an app that already has models, `src/models/mod.rs` is updated instead of created, and the migrations take the next numbers.

| File | Contents |
|---|---|
| `src/models/user.rs`, `api_key.rs`, `auth_token.rs` | Users (email, password digest, `confirmed_at`), API keys, and single-use emailed tokens for password reset, magic links and email confirmation (full-stack only) |
| `src/auth.rs` | The `CurrentUser`, `ConfirmedUser` and `OptionalUser` extractors, `sign_in`/`sign_out`, `SESSION_SECONDS` (two weeks) and `OAUTH_PROVIDERS` (full-stack only) |
| `src/registrations.rs` | `GET`/`POST /signup`, `GET /account`, `POST /account/delete` |
| `src/sessions.rs` | `GET`/`POST /login` ("remember me"), `POST /logout`, magic-link login (`/magic_link`, `/magic_link/{token}`) |
| `src/passwords.rs` | Password reset: `/passwords/new`, `POST /passwords`, `/passwords/{token}` |
| `src/confirmations.rs` | Email confirmation: `POST /confirmations` (send a new link), `/confirmations/{token}` |
| `src/auth_api.rs` | In every app: the `BearerUser` extractor, `throttle`, `POST /api/auth/signup`, `POST /api/auth/token` (JWT), `GET`/`DELETE /api/auth/me`, and API keys (`GET`/`POST /api/auth/keys`, `DELETE /api/auth/keys/{id}`) |
| `templates/auth/*.html` | The pages (full-stack only) |
| `cloudflare.config.ts` | The `AUTH_RATE_LIMITER: bindings.rateLimit(...)` binding (10 requests a minute, `namespace` derived from the app name), used by `throttle` on every route that checks a password or sends an email; not added again when present |

`ocre g auth --db-sessions --oauth github,google` also creates `migrations/*_create_user_sessions.sql`, `migrations/*_create_identities.sql`, `src/models/user_session.rs`, `src/models/identity.rs`, `src/user_sessions.rs` (`GET /account/sessions`, `POST /account/sessions/{id}/delete`, `POST /account/sessions/others/delete`), `src/oauth.rs` (`POST /auth/{provider}`, `GET /auth/{provider}/callback`) and `templates/auth/user_sessions.html`, links the sessions page from `account.html`, and appends commented `GITHUB_CLIENT_ID`/`GITHUB_CLIENT_SECRET` (and `GOOGLE_...`) lines to `.dev.vars`. One extra next step per provider:

```text
  register an OAuth app with github (callback https://<your host>/auth/github/callback), put GITHUB_CLIENT_ID and GITHUB_CLIENT_SECRET in .dev.vars and their production values in .prod.vars, then `ocre secrets push GITHUB_CLIENT_ID GITHUB_CLIENT_SECRET --file .prod.vars`
```

In an API-only app, only the JSON part is generated: `create_users` and `create_api_keys` migrations, the `user` and `api_key` models, `src/auth_api.rs` and the rate limiter in `cloudflare.config.ts`; the last next step is a `curl -X POST http://localhost:8787/api/auth/signup ...` command.

Errors:

- `this app already has a User model or a users table` when `src/models/user.rs` or a `*_create_users.sql` migration exists, with the hint ``` `ocre g auth` creates both and runs once per app; to start over, remove src/models/user.rs and the create_users migration ```.
- ``unknown OAuth provider `twitter` `` with the hint `--oauth accepts github, google (comma-separated)`.
- `--db-sessions and --oauth need HTML pages, and this app is API-only`, with the hint ``run `ocre g auth` without them: JSON clients use JWTs and API keys, which `DELETE /api/auth/keys/{id}` revokes``.

See [Authentication](../guides/authentication.md).

## ocre g migration

```text
ocre g migration <NAME> [FIELDS]...
```

| Argument | Required | Meaning |
|---|---|---|
| `NAME` | yes | snake_case migration name |
| `FIELDS` | no | Columns as `name:type`, or column names for the index names below |

Creates `migrations/NNNN_<name>.sql`, starting with `-- Migration: <name>` and a comment reminding that applied migrations must not be edited. The SQL is inferred from the name, like Rails:

| Name | SQL |
|---|---|
| `create_<table>` | `CREATE TABLE <table>` with the fields, `id`, `created_at`, `updated_at` and indexes, as `ocre g model` writes it |
| `add_<anything>_to_<table>` | One `ALTER TABLE <table> ADD COLUMN` per column, then the indexes. Fields are required |
| `remove_<column>_from_<table>` | `DROP INDEX IF EXISTS index_<table>_on_<column>` (SQLite refuses to drop an indexed column), then `ALTER TABLE <table> DROP COLUMN <column>`; with fields, one `DROP COLUMN` per field column instead, after dropping the indexes of the unique and reference fields |
| `add_index_to_<table> <column>...` | `CREATE INDEX index_<table>_on_<a>_and_<b> ON <table> (<a>, <b>)`, columns in the order given (names only, no types) |
| `add_unique_index_to_<table> <column>...` | The same with `CREATE UNIQUE INDEX` |
| `remove_index_from_<table> <column>...` | `DROP INDEX IF EXISTS index_<table>_on_<a>_and_<b>` |
| `rename_<column>_to_<new>_in_<table>` | `ALTER TABLE <table> RENAME COLUMN <column> TO <new>` |
| `rename_<table>_to_<new>` | `ALTER TABLE <table> RENAME TO <new>` |
| `drop_<table>` | `DROP TABLE <table>` |
| `rebuild_<table>` | SQLite's table rebuild, copied from the table's definition in `db/schema.sql` (written by [`ocre db schema`](cli.md#ocre-db-schema)): create `<table>_new`, copy the rows, drop the old table, rename, recreate its indexes, between `PRAGMA defer_foreign_keys` lines. Edit its `CREATE TABLE` to change what `ALTER TABLE` cannot (a column's type, `NOT NULL`, `DEFAULT`, `CHECK`, `REFERENCES`, an enum's values) |
| anything else, no fields | An empty migration to fill in (data changes, custom SQL) |

`rename_...`, `drop_...` and `rebuild_...` take no fields. After them, and after the field forms, the next steps remind you to update the model; the index forms only print `ocre migrate`.

Existing rows need a value for a new `NOT NULL` column, so `add_..._to_...` adds `DEFAULT ''` to required text, date, time, datetime, decimal and uuid columns, `DEFAULT 0` to required numbers, `DEFAULT '{}'` to required `json` columns, the first value to required `enum` columns (optional columns stay `NULL`), and booleans already default to 0. References and attachments must be optional there.

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

`ocre g migration remove_slug_from_posts` writes `DROP INDEX IF EXISTS index_posts_on_slug;` then `ALTER TABLE posts DROP COLUMN slug;`, and `ocre g migration backfill_slugs` an empty migration; without fields, the only next step is `ocre migrate`. A migration does not change the model: add or remove the fields in `src/models/<model>.rs` (the row struct, `New...`, `...Changes`, and the SQL of `create` and `update`) yourself.

Index and rename migrations:

```sh
ocre g migration add_index_to_books pages released_on
ocre g migration rename_summary_to_blurb_in_books
```

```sql
-- Migration: add_index_to_books
-- Applied once, in file-name order. Never edit a migration after it has been applied.
CREATE INDEX index_books_on_pages_and_released_on ON books (pages, released_on);
```

```sql
-- Migration: rename_summary_to_blurb_in_books
-- Applied once, in file-name order. Never edit a migration after it has been applied.
ALTER TABLE books RENAME COLUMN summary TO blurb;
```

A table rebuild, after `ocre migrate` and `ocre db schema`:

```sh
ocre g migration rebuild_posts
```

```sql
-- Migration: rebuild_posts
-- Applied once, in file-name order. Never edit a migration after it has been applied.
-- Rebuilds `posts` to change what ALTER TABLE cannot (a column's type, NOT NULL,
-- DEFAULT, CHECK or REFERENCES): edit the CREATE TABLE below, and keep both
-- column lists of the INSERT in step with it.
PRAGMA defer_foreign_keys = true;
CREATE TABLE posts_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    published INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO posts_new (id, title, body, published, created_at, updated_at) SELECT id, title, body, published, created_at, updated_at FROM posts;
DROP TABLE posts;
ALTER TABLE posts_new RENAME TO posts;
PRAGMA defer_foreign_keys = false;
```

Errors:

| Error | Hint |
|---|---|
| ``invalid migration name `AddX` `` | ``use snake_case, e.g. `add_slug_to_posts` `` |
| ``cannot tell which table `fix_stuff` changes`` (fields with an unknown name form) | ``name it `create_<table>`, `add_<columns>_to_<table>` or `remove_<columns>_from_<table>` `` |
| `add_..._to_... needs the columns to add` | ``list them like the model fields, e.g. `ocre g migration add_slug_to_posts slug:string^` `` |
| ``` `owner_id` must be optional when added to an existing table ``` | ``SQLite adds reference columns as NULL for existing rows: use `name:references?` `` |
| ``` `<name>` must be optional when added to an existing table ``` (attachment) | ``existing rows have no file: use `name:attachment?` `` |
| ``` `add_index_to_posts` needs the indexed column names ``` | ``list them in index order, without types, e.g. `ocre g migration add_index_to_posts author_id created_at` `` |
| ``` `drop_tags` takes no fields ``` | ``the name says it all, e.g. `rename_title_to_headline_in_posts`, `drop_tags`, `rebuild_posts` `` |
| ``cannot tell what `rename_title` renames`` | ``name it `rename_<column>_to_<new>_in_<table>` or `rename_<table>_to_<new>` `` |
| `db/schema.sql not found` (`rebuild_...`) | ``run `ocre migrate` then `ocre db schema`: the rebuild copies the table's current definition`` |
| ``table `posts` is not in db/schema.sql`` | ``run `ocre migrate` then `ocre db schema` to refresh it, and check the table name`` |
| ``` `comments` references `posts`: rebuilding it would delete or clear their rows ``` | ``D1 enforces foreign keys, so dropping `posts` runs ON DELETE on `comments`; add a new column and backfill it instead`` |

## ocre g mailer

```text
ocre g mailer <NAME> <ACTIONS>...
```

| Argument | Required | Meaning |
|---|---|---|
| `NAME` | yes | Mailer name, PascalCase or snake_case; a `Mailer` suffix is dropped (`UserMailer`, `user_mailer` and `User` all give `user`) |
| `ACTIONS` | yes, at least one | Email names in snake_case (`welcome`, `password_reset`), one function each |

Creates `src/mailers/<name>.rs` with one function per action, `pub fn welcome(to: &str) -> Result<Email>`, building an `ocre::mail::Email` whose subject is the humanized action (`Password reset`) and passing it through `defaults` of `src/mailers/mod.rs`. In a full-stack app, each function renders two askama templates, `templates/mailers/<name>/<action>.txt` and `.html`, which extend `templates/mailers/layout.txt` and `layout.html` (created once, unless they exist); in an API-only app, the text is built with `format!` and there are no templates. Each action gets a preview in `PREVIEWS` of `src/mailers/mod.rs`, shown at `/ocre/dev/mailers` in `ocre dev`. The first mailer creates `src/mailers/mod.rs` (with `defaults` and `PREVIEWS`; keep the `// ocre:mailers` and `// ocre:mailer-previews` markers) and adds `mod mailers;` and `.merge(ocre::mail::dev_routes(mailers::PREVIEWS))` to `src/lib.rs` (replacing the `dev_routes(&[])` that `ocre g mailbox` adds).

```sh
ocre g mailer User welcome password_reset
```

```text
  create  src/mailers/user.rs
  create  templates/mailers/user/welcome.txt
  create  templates/mailers/user/welcome.html
  create  templates/mailers/user/password_reset.txt
  create  templates/mailers/user/password_reset.html
  create  templates/mailers/layout.html
  create  templates/mailers/layout.txt
  create  src/mailers/mod.rs
  update  src/lib.rs

Next:
  send it from a handler: ocre::mail::send(&ctx, mailers::user::welcome(&address)?).await?
  ocre dev, then open http://localhost:8787/ocre/dev/mailers to preview it (MAIL_ADAPTER=log in .dev.vars prints each email sent instead of sending it)
```

```json
{"command":"generate mailer","created":["src/mailers/user.rs","templates/mailers/user/welcome.txt","templates/mailers/user/welcome.html","templates/mailers/user/password_reset.txt","templates/mailers/user/password_reset.html","templates/mailers/layout.html","templates/mailers/layout.txt","src/mailers/mod.rs"],"next":["send it from a handler: ocre::mail::send(&ctx, mailers::user::welcome(&address)?).await?","ocre dev, then open http://localhost:8787/ocre/dev/mailers to preview it (MAIL_ADAPTER=log in .dev.vars prints each email sent instead of sending it)"],"ok":true,"updated":["src/lib.rs"]}
```

A `src/mailers/mod.rs` written by an older Ocre (without `defaults` or `PREVIEWS`) keeps working: the new mailer does not call `defaults`, and no preview is added.

Errors: ``invalid mailer name `type` `` (hint: ``use a name starting with a letter that is not a Rust keyword, e.g. `User` or `Billing` ``); ``invalid action name `select` `` (hint: ``use snake_case starting with a letter, not a Rust or SQL keyword, e.g. `welcome` or `password_reset` ``); ``action `receipt` is listed twice`` (hint: `list each action once`); `src/mailers/<name>.rs already exists`; ``src/lib.rs is missing the `// ocre:routes` marker`` (first mailer with previews). Names are checked against the same reserved words as [fields](#fields). See [Email](../guides/email.md).

## ocre g mailbox

```text
ocre g mailbox
```

No arguments. Creates `src/mailbox.rs`, the handler of email that Cloudflare Email Routing sends to the Worker (`pub async fn receive(ctx, email: InboundEmail) -> Result<()>`), and appends the Worker's `email` event to `src/lib.rs`, which calls `ocre::mail::receive(message, env, mailbox::receive)`. Unless a mailer added them, it also merges the development pages (`.merge(ocre::mail::dev_routes(&[]))`), whose `/ocre/dev/mailbox` form delivers test email in `ocre dev`. A Worker has one email entry point, so an app has one mailbox, which routes by `email.to()` itself.

```sh
ocre g mailbox
```

```text
  create  src/mailbox.rs
  update  src/lib.rs

Next:
  ocre dev
  open http://localhost:8787/ocre/dev/mailbox to deliver a test email
  or: curl 'http://localhost:8787/cdn-cgi/local/email?from=ada@example.com&to=support@example.com' --data-binary @message.eml
  route addresses to the Worker: Cloudflare dashboard > Email Routing > Routing rules > Send to a Worker
```

```json
{"command":"generate mailbox","created":["src/mailbox.rs"],"next":["ocre dev","open http://localhost:8787/ocre/dev/mailbox to deliver a test email","or: curl 'http://localhost:8787/cdn-cgi/local/email?from=ada@example.com&to=support@example.com' --data-binary @message.eml","route addresses to the Worker: Cloudflare dashboard > Email Routing > Routing rules > Send to a Worker"],"ok":true,"updated":["src/lib.rs"]}
```

Errors: `src/mailbox.rs already exists`; `src/lib.rs already handles the `email` event` with the hint ``a Worker has one email entry point: call `ocre::mail::receive(message, env, mailbox::receive)` from it``. See [Email](../guides/email.md).

## ocre g job

```text
ocre g job <NAME> [FIELDS]... [--queue <QUEUE>]
```

| Argument | Required | Meaning |
|---|---|---|
| `NAME` | yes | Job name, PascalCase or snake_case, a verb phrase; a `Job` suffix is dropped (`ImportCsvJob` gives `import_csv` and `ImportCsv`) |
| `FIELDS` | no | The job's arguments as `name:type`; no `attachment`, no `^` |
| `--queue <QUEUE>` | no | The queue the job is sent to, lowercase letters, digits and `-` (default `default`): its own Cloudflare queue and consumer, for jobs that must not wait behind others |

Creates `src/jobs/<name>.rs`: a struct holding the arguments (serialized as JSON in the queue message, 128 KB at most) with `fn perform_later(self, ctx)`, which sends it to its queue, and an `async fn perform(self, ctx: &Ctx) -> Result<()>` to fill in. It adds a variant to the `Job` enum and an arm to the `perform` match in `src/jobs/mod.rs`. The first job creates `src/jobs/mod.rs`, adds `mod jobs;` and the Worker's `queue` event to `src/lib.rs`, and, unless a `JOBS` producer exists, adds to `cloudflare.config.ts` the `JOBS: bindings.queue({ name: "<app>-jobs" })` producer (after `// ocre:env`) and its `triggers.queue(...)` consumer (after `// ocre:triggers`) (batches of up to 10 messages, 5 retries, dead-letter queue `<app>-jobs-failed`). `--queue urgent` also adds, unless a `JOBS_URGENT` binding exists, `JOBS_URGENT: bindings.queue({ name: "<app>-jobs-urgent" })` and its consumer (`maxBatchTimeout: 1`, dead-letter queue `<app>-jobs-urgent-failed`).

```sh
ocre g job SendWelcome user_id:integer
```

```text
  create  src/jobs/send_welcome.rs
  create  src/jobs/mod.rs
  update  src/lib.rs
  update  cloudflare.config.ts

Next:
  enqueue it from a handler: jobs::SendWelcome { user_id }.perform_later(&ctx).await?
  ocre dev (jobs run locally; look for `[ocre jobs]` lines in the output)
  ocre deploy creates the queue blog-jobs and its dead-letter queue
```

```json
{"command":"generate job","created":["src/jobs/send_welcome.rs","src/jobs/mod.rs"],"next":["enqueue it from a handler: jobs::SendWelcome { user_id }.perform_later(&ctx).await?","ocre dev (jobs run locally; look for `[ocre jobs]` lines in the output)","ocre deploy creates the queue blog-jobs and its dead-letter queue"],"ok":true,"updated":["src/lib.rs","cloudflare.config.ts"]}
```

Later jobs only create their file and update `src/jobs/mod.rs`; a `deploy creates the queue` step is printed for each queue a run adds to `cloudflare.config.ts`. Free plan (September 2026): 10,000 Queues operations a day, a job costing 3 (write, read, delete), and 10 ms of CPU per batch ([Queues pricing](https://developers.cloudflare.com/queues/platform/pricing/), [Workers limits](https://developers.cloudflare.com/workers/platform/limits/#cpu-time)).

Errors:

| Error | Hint |
|---|---|
| ``invalid job name `Job` `` (also Rust keywords) | ``use a verb phrase starting with a letter, not a Rust keyword, e.g. `SendWelcome` or `ImportCsv` `` |
| ``job field `f` cannot be an attachment`` | ``files do not fit in a queue message (128 KB): store the file first and pass its record id, e.g. `post_id:integer` `` |
| ``job field `f` cannot be unique`` | ``` `^` adds a unique index to a table column; jobs have no table: drop the `^` ``` |
| `src/jobs/<name>.rs already exists` | the generic hint |
| ``invalid queue name `Urgent` `` | ``use lowercase letters, digits and `-`, e.g. `--queue urgent` `` |
| ``src/lib.rs already handles the `queue` event`` (first job only) | ``a Worker has one queue entry point: call `ocre::jobs::consume(batch, env, jobs::perform)` from it and create src/jobs/mod.rs by hand`` |

See [Background jobs and schedules](../guides/jobs.md).

## ocre g schedule

```text
ocre g schedule <NAME> <WHEN>
```

| Argument | Required | Meaning |
|---|---|---|
| `NAME` | yes | Task name in snake_case (`HourlyPing` is accepted and becomes `hourly_ping`); not a reserved word |
| `WHEN` | yes | When, in UTC, quoted for the shell: plain English (`"every 15 minutes"`, `"every day at 3am"`, `"every monday at 9:30"`, `"midnight on tuesdays"`, `"every weekday at 18:00"`, `"monthly"`) or a cron expression of five fields (minute, hour, day of month, month, day of week): `"0 3 * * *"` |

Creates `src/schedules/<name>.rs` with `pub async fn run(ctx: &Ctx) -> Result<()>` to fill in (its comment keeps the English phrase), adds `triggers.scheduled({ schedule: "<cron>" }),` after the `// ocre:triggers` marker of `cloudflare.config.ts`, and adds `"<cron>" => <name>::run(&ctx).await,` to the dispatch `match` of `src/schedules/mod.rs`. The first schedule creates `src/schedules/mod.rs` and adds `mod schedules;` and the Worker's `scheduled` event to `src/lib.rs`. English phrases are converted to cron (the full list is in [When: English or cron](../guides/jobs.md#when-english-or-cron)); for a cron expression the CLI checks its shape (five fields of letters, digits and `*,-/#`), not its values; Cloudflare validates it at deploy ([syntax](https://developers.cloudflare.com/workers/configuration/cron-triggers/#supported-cron-expressions)).

```sh
ocre g schedule nightly_cleanup "every day at 3am"
```

```text
  create  src/schedules/nightly_cleanup.rs
  create  src/schedules/mod.rs
  update  cloudflare.config.ts
  update  src/lib.rs

Next:
  ocre dev, then: ocre schedules run nightly_cleanup
  ocre deploy (Cron Triggers only fire on the deployed Worker; this one runs at `0 3 * * *`, UTC)
```

```json
{"command":"generate schedule","created":["src/schedules/nightly_cleanup.rs","src/schedules/mod.rs"],"next":["ocre dev, then: ocre schedules run nightly_cleanup","ocre deploy (Cron Triggers only fire on the deployed Worker; this one runs at `0 3 * * *`, UTC)"],"ok":true,"updated":["cloudflare.config.ts","src/lib.rs"]}
```

Free plan (September 2026): 5 Cron Triggers per account and 10 ms of CPU per run ([Workers limits](https://developers.cloudflare.com/workers/platform/limits/)). When the app's `[triggers] crons` holds more than 5 expressions, the generator adds a next step: `this app now has 6 crons; the free plan allows 5 per account: run several tasks from one cron`.

Errors:

| Error | Hint |
|---|---|
| ``invalid schedule name `<name>` `` | ``use snake_case starting with a letter, not a Rust keyword, e.g. `nightly_cleanup` `` |
| ``invalid schedule `every 15 seconds` `` | The accepted English phrases and the cron form, and that Cron Triggers run at most once a minute |
| ``cron `0 3 * * *` is already in [triggers] crons`` | `one task per cron: call the new work from the existing task in src/schedules/, or pick another time (e.g. one minute later)` |
| ``cron `0 3 * * *` is already scheduled in cloudflare.config.ts`` | `one task per cron: call the new work from the existing task in src/schedules/, or pick another time (e.g. one minute later)` |
| ``cloudflare.config.ts is missing the `// ocre:triggers` marker`` | ``put `// ocre:triggers` on its own line inside `worker.triggers`: Ocre adds its entries after it`` |
| ``src/lib.rs already handles the `scheduled` event`` (first schedule only) | ``a Worker has one scheduled entry point: call `ocre::jobs::cron(event, env, schedules::run)` from it and create src/schedules/mod.rs by hand`` |

See [Background jobs and schedules](../guides/jobs.md).

## ocre g cache

```text
ocre g cache
```

No arguments. Adds `CACHE: bindings.kv(),` (no `id`) after the `// ocre:env` marker of `cloudflare.config.ts`, for `ocre::cache::fetch` (a read-through cache of JSON values). `ocre dev` uses a local namespace; the first [`ocre deploy`](cli.md#ocre-deploy) creates the real one (titled `<app>-cache`) and writes its `id` into the entry (`CACHE: bindings.kv({ id: "<id>" }),`). The binding is opt-in because Workers KV allows 1,000 writes a day on the free plan (September 2026; 100,000 reads a day, 1 GB stored, [KV limits](https://developers.cloudflare.com/kv/platform/limits/)).

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

```json
{"command":"generate cache","next":["use it: ocre::cache::fetch(&ctx, \"key:v1\", Duration::from_secs(3600), || async { ... }).await?","ocre dev","ocre deploy (creates the KV namespace)"],"ok":true,"updated":["cloudflare.config.ts"]}
```

Error: ``cloudflare.config.ts already has the `CACHE` binding`` with the hint ``nothing to generate: call `ocre::cache::fetch(&ctx, key, ttl, || async { ... })` in a handler``. See [Caching](../guides/caching.md).

## ocre g data

```text
ocre g data <NAME>
```

Read-only data shipped with the Worker (Loco's data loaders): writes `data/<name>.json` (a sample `[{ "name": "Example" }]`) and `src/data/<name>.rs`, which compiles the file in with `include_str!` and parses it once per Worker instance into `Vec<Entry>` (declare the fields in `Entry`). The first one also writes `src/data/mod.rs` and `mod data;` in `src/lib.rs`. Read it with `crate::data::<name>::all()`. The module has a test checking that the file matches `Entry`. A Worker has no disk: the data changes with a deploy.

```sh
ocre g data countries
```

```json
{"command":"generate data","created":["src/data/mod.rs","data/countries.json","src/data/countries.rs"],"next":["put the entries in data/countries.json and their fields in `Entry` (src/data/countries.rs)","read them with `crate::data::countries::all()`"],"ok":true,"updated":["src/lib.rs"]}
```

Errors: ``invalid data name `Bad-Name` `` (hint: snake_case), `data/<name>.json already exists`, and a missing `// ocre:modules` or `// ocre:data` marker.

## ocre g system_test

```text
ocre g system_test <NAME>
ocre g system-test <NAME>
```

A browser test (Rails' system tests) in `tests/system/<name>.spec.ts`, run by [Playwright](https://playwright.dev) against the test server of [`ocre test --e2e`](cli.md#ocre-test). The first one also writes `playwright.config.ts` (tests in `tests/system/`, `baseURL` from `BASE_URL`, one worker, a desktop and a phone screen, a screenshot and trace per failure), adds `"@playwright/test": "1.63.0"` to the `devDependencies` of `package.json`, and `test-results/` and `playwright-report/` to `.gitignore`.

```sh
ocre g system_test signing_up
```

```json
{"command":"generate system_test","created":["playwright.config.ts","tests/system/signing_up.spec.ts"],"next":["npm install","npx playwright install chromium (once per machine)","edit tests/system/signing_up.spec.ts, then ocre test --e2e"],"ok":true,"updated":["package.json",".gitignore"]}
```

Errors: ``invalid system test name `Bad` ``, `tests/system/<name>.spec.ts already exists`, and ``package.json has no `"devDependencies": {` block``. See [Testing](../guides/testing.md#browser-tests-ocre-g-system_test).

## ocre g ci

```text
ocre g ci
```

No arguments. Writes `.github/workflows/ci.yml`, the app's GitHub Actions workflow (Rails' generated CI config):

- a `check` job on every push and pull request: checkout, `rustup component add rustfmt clippy` (`rust-toolchain.toml` adds the wasm32 target), a Rust cache, then the steps of [`ocre ci`](cli.md#ocre-ci) in the same order (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo check --target wasm32-unknown-unknown`), and `ocre i18n missing` when `locales/*.yml` exist (it installs the CLI first);
- a `deploy` job after it, on pushes to `main` only, one at a time: Node.js 22, `npm ci` (the pinned cf and wrangler), `cargo install --git https://github.com/tgeselle/ocre.rs ocre-cli`, then `ocre deploy --json` with `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` from the repository secrets.

The comment at the top of the file lists the two secrets to add (Settings > Secrets and variables > Actions) and the token's permissions: Account "Workers Scripts: Edit" and "D1: Edit", plus "Queues: Edit", "Workers KV Storage: Edit" and "Workers R2 Storage: Edit" when the app uses them, and "Workers Routes: Edit" on the zone of a [custom domain](cli.md#ocre-domains). Commit `package-lock.json` and the KV ids the first `ocre deploy` writes into `cloudflare.config.ts` (see [Deployment](../guides/deployment.md#ci)). Free plan: GitHub Actions minutes are free for public repositories; the deploy uses no paid Cloudflare feature.

```text
  create  .github/workflows/ci.yml

Next:
  add the repository secrets CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID (see the comment at the top of the workflow)
  ocre ci (the same checks, locally)
  git add .github package-lock.json && git commit, then push to main
```

Error: `.github/workflows/ci.yml already exists` (the shared hint; `--force` rewrites it).

## ocre g pwa

```text
ocre g pwa
```

No arguments; full-stack apps. Makes the app a Progressive Web App, like the manifest and service worker of new Rails 8 apps. Static files in the assets directory (`public/`), which Cloudflare serves before the Worker runs (free, not Worker requests):

| File | Contents |
|---|---|
| `public/manifest.webmanifest` | Name (the Worker's), `start_url`, `display: standalone`, colors and the icon, so browsers offer to install the app |
| `public/service-worker.js` | Caches the home page at install and answers with it when a page cannot load offline; `push` and `notificationclick` handlers for web push, to fill in |
| `public/pwa.js` | Registers the service worker (a file, so the default `script-src 'self'` Content-Security-Policy allows it) |
| `public/icon.svg` | A placeholder icon with the app's initial |

It also inserts the `<link rel="manifest">`, `theme-color`, icon and `pwa.js` tags before `</head>` in `templates/layout.html`; `ocre destroy pwa` takes them out.

Errors: `a PWA needs HTML pages; this app is API-only`, `templates/layout.html not found`, `templates/layout.html has no </head>`, and ``templates/layout.html already links a web app manifest`` (hint: ``nothing to generate: edit manifest.webmanifest in the assets directory``).

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

## ocre g override

```text
ocre g override [PATHS]...
```

Copies built-in generator templates into `.ocre/templates/`, where they replace the built-in ones for every later run until you delete them (Loco's `generate override`, Rails' `lib/templates`). A path names one template (`controller/view.html`) or every template of a generator (`controller`). Without paths, it lists the templates; overridden ones are marked `(overridden in .ocre/templates/)`.

```sh
ocre g override
```

```text
  controller/api.rs
  controller/html.rs
  controller/view.html
  resource/api.rs
  resource/html.rs
  resource/index.html
  resource/show.html
  scaffold/_form.html
  scaffold/_row.html
  scaffold/edit.html
  scaffold/index.html
  scaffold/new.html
  scaffold/show.html

Next:
  ocre g override <path> (e.g. controller/html.rs, or controller for all its files)
```

With `--json`, the list is `"templates": [{"path": "controller/api.rs", "overridden": false}, ...]`.

```sh
ocre g override controller
```

```text
  create  .ocre/templates/controller/api.rs
  create  .ocre/templates/controller/html.rs
  create  .ocre/templates/controller/view.html

Next:
  edit the files in .ocre/templates/ (<%= value %>, <% for x in xs %>...<% endfor %>); generators use them until deleted
```

Templates are [minijinja](https://docs.rs/minijinja) with ERB-style delimiters, so the askama and Rust braces of the generated code stay literal: `<%= value %>` prints a value, `<% for x in xs %>...<% endfor %>` and `<% if api %>...<% endif %>` are blocks, `<%# ... %>` is a comment, and a newline right after a block tag is dropped. An unknown variable is an error. The variables each template sees:

| Templates | Variables |
|---|---|
| `controller/html.rs`, `controller/api.rs` | `command`, `module` (`pages`), `file` (`pages` or `pages_api`), `human` (`Pages`), `auth`, `actions` (each with `name`, `pascal`, `human`, `path`) |
| `controller/view.html` | `module`, `action` (`name`, `pascal`, `human`, `path`) |
| `resource/*` | `command`, `model`, `singular`, `plural`, `human_singular`, `human_plural`, `fields` (each with `label` and `display`, the askama expression showing the value) |
| `scaffold/*.html` | `model`, `singular`, `plural`, `human_singular`, `human_plural`, `lower`, `realtime`, `multipart`, `fields` (each with `name`, `label`, `attachment`, `optional`, `display`, `show`, `input`) |

Errors: ``no generator template `nope` `` with the list of templates as hint; `.ocre/templates/<path> already exists` (pass `--force` to copy the built-in template again); when an override does not render, ``.ocre/templates/<path> failed to render: ...`` with the hint ``fix .ocre/templates/<path>, or delete it to use the built-in template again``.

## ocre g generator

```text
ocre g generator <NAME>
```

Creates an app generator to edit, in `.ocre/generators/<name>/` (Rails' `generate generator`): a `generator.toml` describing it and one example template.

```sh
ocre g generator service
```

```text
  create  .ocre/generators/service/generator.toml
  create  .ocre/generators/service/src/services/<%= singular %>.rs

Next:
  edit the templates in .ocre/generators/service/
  ocre g service Example name:string --pretend
```

## App generators (`ocre g <name>`)

```text
ocre g <name> <Name> [ARGS]... [--key=value]... [--flag]...
```

Any generator name that is not built in runs `.ocre/generators/<name>/`. Every file of that directory except `generator.toml` is a template (same syntax as [overrides](#ocre-g-override)), and so is its path: `src/services/<%= singular %>.rs` becomes `src/services/billing.rs` for `ocre g service Billing`. Templates see:

| Variable | Value for `ocre g service BlogPost amount:integer total:decimal? --api --queue=urgent` |
|---|---|
| `model`, `singular`, `plural` | `BlogPost`, `blog_post`, `blog_posts` |
| `human_singular`, `human_plural` | `Blog post`, `Blog posts` |
| `args` | the arguments after the name: `["amount:integer", "total:decimal?"]` |
| `fields` | when every argument is `name:type` (see [Fields](#fields)): each with `name`, `label`, `rust_type` (`i64`, `Option<String>`), `optional`, `unique` |
| `options` | `--key=value` and `--flag` arguments: `{"api": "true", "queue": "urgent"}` (dashes in keys become `_`) |

`generator.toml` has an optional `description` and `[[insert]]` tables adding a line after a marker line of an existing file (the three values are templates too):

```toml
description = "Service object in src/services/"

[[insert]]
file = "src/services/mod.rs"
after = "// ocre:services"
line = "pub mod <%= singular %>;"
```

```sh
ocre g service Billing amount:integer total:decimal? --pretend
```

```text
  create  src/services/billing.rs
(--pretend: nothing was written)
```

The `--pretend`, `--force`, `--skip` and `--json` flags work as for built-in generators; the JSON `command` is `generate custom`, and the run is recorded, so `ocre destroy service Billing` undoes it.

Errors:

| Error | Hint |
|---|---|
| ``unknown generator `nope` `` | ``run `ocre g --help` for the built-in generators; app generators live in .ocre/generators/<name>/ (create one with `ocre g generator nope`)`` |
| ``` `ocre g service` needs a name ``` | ``run `ocre g service <Name> [args...]`, e.g. `ocre g service Invoice` `` |
| `.ocre/generators/service/generator.toml is invalid: ...` | ``keys: `description`, and [[insert]] tables with `file`, `after` and `line` `` |
| `<file> does not exist` (an `[[insert]]` target) | ``create it with the `<marker>` marker line, or change the [[insert]] of .ocre/generators/<name>/generator.toml`` |
| ``<file> is missing the `<marker>` marker`` | ``put `<marker>` on its own line where the generated lines go`` |
| ``.ocre/generators/<name>/<file> failed to render: ...`` | ``fix .ocre/generators/<name>/<file>`` |

## See also

- [CLI commands](cli.md): `ocre migrate`, `ocre dev`, `ocre deploy`, `ocre routes` and the `--json` contract.
- [Field types](field-types.md): each field type in SQL, Rust, forms and JSON.
- [Models and migrations](../guides/models.md), [Validations](../guides/validations.md).
- [Why generated code](../explanations/generated-code.md).
