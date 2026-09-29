# Why generated code

Ocre's generators write models, controllers, templates and migrations into your app as plain Rust files that you own, instead of hiding them behind macros, derives or an ORM. This page explains why, how generators change files that already exist, what the approach costs, and how it compares with Rails and Loco.

## Before you start

Nothing to install to read this page. The examples come from an app made with `ocre new shop` (and `ocre new blog --starter blog` for the `Post` model); every command shown was run with the `ocre` CLI.

## What a generator writes

```sh
ocre g scaffold Product name:string price:float
```

```text
  create  src/models/mod.rs
  create  migrations/0001_create_products.sql
  create  src/models/product.rs
  create  src/products.rs
  create  templates/products/index.html
  create  templates/products/show.html
  create  templates/products/new.html
  create  templates/products/edit.html
  create  templates/products/_form.html
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/products
```

That is 438 lines: the SQL migration (7), the model (160), the controller with its routes, path helpers and form parsing (188) and five askama templates (83). The model file holds the struct, the create and update inputs (`NewProduct`, `ProductChanges`), their `validate()`, every query, and empty callbacks (`before_create`, `after_update`...). For example, `create` in the blog starter's `src/models/post.rs`:

```rust
pub async fn create(ctx: &Ctx, mut new: NewPost) -> Result<Post> {
    before_create(ctx, &mut new).await?;
    let db = ctx.db()?;
    new.validate().finish()?;
    let record: Post = db
        .first("INSERT INTO posts (title, body, published) VALUES (?1, ?2, ?3) RETURNING *", params![new.title, new.body, new.published])
        .await?
        .ok_or_else(|| Error::internal("INSERT ... RETURNING returned no row"))?;
    after_create(ctx, &record).await?;
    Ok(record)
}
```

Nothing is generated at compile time or at run time: what you read is what runs.

## Why not macros, derives or an ORM

- **The code that runs is in the repository.** An agent (or a person) that opens `src/models/post.rs` sees every SQL statement, validation rule and association of posts, in one file, without expanding a macro or reading framework internals. Agents write better changes when they can read the existing code for the same thing.
- **Everything is greppable.** `grep -rn "FROM posts" src` finds every query on a table; `ocre routes` lists every route with its handler, read from the source.
- **Every query is visible.** On the free plan, D1 counts rows read and written per day and a request has 10 ms of CPU. A query hidden behind an ORM call or a lazy association is where N+1 queries come from; generated code shows the one query per call, and lists load their associations with `find_many` and `preload_<parents>` (100 ids per query).
- **Local changes stay local.** A rule for one model is an edit in that model's file, not a framework option. Adding a query is adding a function next to the generated ones:

  ```rust,check
  // Added to src/models/post.rs, next to the generated `all`
  // (shown as its own module here, hence the `use` of `Post`).
  use ocre::{Ctx, Page, Result, params};

  use crate::models::post::Post;

  /// Published posts, newest first.
  pub async fn published(ctx: &Ctx, page: Page) -> Result<Vec<Post>> {
      ctx.db()?
          .all(
              "SELECT * FROM posts WHERE published = 1 ORDER BY id DESC LIMIT ?1 OFFSET ?2",
              params![page.limit, page.offset],
          )
          .await
  }
  ```

- **Small binaries.** There is no ORM in the WebAssembly module: no schema read at run time, no lazy loading, no identity map. `ocre::Query` (what `post::query().eq("published", true).order_desc("id")` builds) only assembles one SQL string and its parameters, and `to_statement()` shows exactly what it will send.

Ocre still uses derives where the result is standard and stable: serde (`Deserialize`, `Serialize`) for rows and forms, askama's `Template` for views. The framework's own macros are `params!` (a list of SQL parameters) and `locales!` (translations compiled into the binary). What Ocre does not do is put application behavior (queries, validations, routes, callbacks) behind them.

## How generators change existing files

Generators create new files and never overwrite one. Running a scaffold twice fails and writes nothing:

```sh
ocre g scaffold Product name:string price:float
```

```text
error: src/products.rs already exists
hint: generators create new files only; edit the existing file, or add a migration with `ocre g migration`
```

To change a model after it exists, add a migration (`ocre g migration add_sku_to_products sku:string?`) and edit the model by hand: its struct, `New<Model>`, `<Model>Changes`, `validate()` and the SQL of `create` and `update`. `ocre g scaffold` and `ocre g api` reuse a model that already exists, so an HTML scaffold and a JSON API can share one.

Files that several generators extend (`src/lib.rs`, `src/models/mod.rs`, `src/jobs/mod.rs`...) carry marker comments. A generator inserts its line right after the marker, with the marker's indentation, and leaves the rest of the file alone:

| Marker | File | Receives |
|---|---|---|
| `// ocre:modules` | `src/lib.rs` | `mod <name>;` for each new module |
| `// ocre:routes` | `src/lib.rs`, in `routes()` | `.merge(<module>::routes())` |
| `// ocre:models` | `src/models/mod.rs` | `pub mod <model>;` |
| `// ocre:associations` | `src/models/<model>.rs`, in `impl <Model>` | has-many, has-one and has-many-through functions such as `post.comments(&ctx, page)`, from a `references` field in another model |
| `// ocre:jobs`, `// ocre:job-variants`, `// ocre:job-dispatch` | `src/jobs/mod.rs` | the job's module, its `Job` variant and its `perform` arm |
| `// ocre:schedules`, `// ocre:schedule-dispatch` | `src/schedules/mod.rs` | the task's module and its cron arm |
| `// ocre:mailers` | `src/mailers/mod.rs` | `pub mod <mailer>;` |
| `// ocre:channels` | `src/realtime.rs` | a channel name anyone may open |
| `// ocre:graphql-queries`, `// ocre:graphql-mutations` | `src/graphql.rs` | the resource's query and mutation types |

Keep the markers when you edit these files. Without one, the generator stops, names the marker and where it belongs, and writes nothing: generators collect every change first and only write when all of them succeeded. With `// ocre:routes` removed from `src/lib.rs`:

```sh
ocre g scaffold Order total:float
```

```text
error: src/lib.rs is missing the `// ocre:modules` or `// ocre:routes` marker
hint: put `// ocre:modules` on its own line where `mod` declarations go, and `// ocre:routes` inside the `Router::new()` chain
```

No `orders` file was created. Generators also append to `wrangler.toml` (bindings such as `JOBS`, `STORAGE`, `CACHE`, `CHANNELS`) and turn on Cargo features in `Cargo.toml` (`graphql`, `realtime`) the first time a feature needs them.

## Trade-offs

- **Improvements to generators do not reach existing code.** When a new Ocre version generates a better controller, files you generated earlier stay as they were; there is no `ocre update` or regeneration. To see what changed, generate the same resource in a scratch app (`ocre new scratch --yes`, then the same `ocre g ...` command) and compare it with yours.
- **Schema changes are manual.** A migration that adds or removes a column does not update the model; the struct, the inputs, `validate()` and the SQL are edited by hand (the generated `AGENTS.md` says so to agents).
- **More code in the repository.** A resource is a few hundred lines you review, own and keep consistent. Uniformity is a convention: generated files start alike, and your edits can make them diverge.
- **No undo.** There is no `ocre destroy`; delete the files and the marker lines a generator added (the `created` and `updated` lists of its output, or `--json`, name them).

## What stays in the framework crate

The `ocre` crate keeps what must be right once and is the same in every app. These parts improve when you update the dependency (`cargo update -p ocre` for the default git dependency on `https://github.com/tgeselle/ocre.rs`), with no change to generated files:

- Everything that talks to the Workers JavaScript runtime: D1 (`Db`), Queues (`jobs`), R2 (`storage`), KV (`cache`), Durable Objects (`realtime`), email sending and receiving (`mail`).
- The middleware of `ocre::serve`: encrypted cookie sessions, CSRF protection, CORS, security headers.
- Security primitives: `password` (PBKDF2 through WebCrypto), `token` (random tokens and digests), `jwt` (HS256).
- Shared types and parsers: `Error` and its HTML/JSON rendering, `Validator`, `Json`, `Page`, multipart parsing, the translation file parser, ETag and `Cache-Control` handling.

The line is the same one Rails 8 draws for authentication: `ocre g auth` writes users, sessions, password resets and API keys into the app, and calls `ocre::password`, `ocre::token` and `ocre::jwt` for the cryptography.

## Compared with Rails and Loco

| | Rails | Loco | Ocre |
|---|---|---|---|
| Model | A class inheriting `ApplicationRecord`: columns come from the database schema at run time; queries are built by Active Record | SeaORM entities generated from the database schema into `src/models/_entities/` (regenerated by `cargo loco db entities`), plus a model file for your code | One Rust file: struct, inputs, validations, callbacks and queries, never regenerated |
| Queries | Built at run time by Active Record | Built by SeaORM | `ocre::Query` chains in the model (one table, bound values), or SQL written out |
| After a schema change | Nothing to update in the model | Regenerate the entities | Edit the model |

All three generate controllers, views and migrations; the difference is the model. Rails also has `rails destroy` to remove what a generator wrote; Ocre has no equivalent.

Ocre generates more of the application than either, because on Cloudflare's free plan every query and every kilobyte of WebAssembly counts, and because code an agent can read is code an agent can change safely.

## See also

- [Generators](../reference/generators.md): every generator and what it writes, e.g. [`ocre g scaffold`](../reference/generators.md#ocre-g-scaffold) and [`ocre g migration`](../reference/generators.md#ocre-g-migration)
- [Models and migrations](../guides/models.md): editing a generated model after a migration
- [Architecture](architecture.md): what the `ocre` crate does at run time
- [Rails generators](https://guides.rubyonrails.org/command_line.html#bin-rails-generate) and [Loco models](https://loco.rs/docs/the-app/models/)
