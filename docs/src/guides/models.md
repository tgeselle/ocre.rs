# Models and migrations

A model is a generated Rust file, `src/models/<model>.rs`, that holds every query and rule about one D1 table, and migrations are numbered SQL files that create and change those tables. This page explains the generated model section by section, how to change the schema, and how to write your own queries without N+1 problems.

## Before you start

- An Ocre app created with `ocre new` (see [Installation](../getting-started/installation.md)). The examples use the blog starter (`ocre new blog --starter blog`), whose `Post` model has `title:string body:text published:boolean`.
- `ocre dev` or `ocre migrate` applies migrations to the local database; nothing here needs a Cloudflare account until you add `--remote`.
- Free plan (September 2026, [D1 pricing](https://developers.cloudflare.com/d1/platform/pricing/)): 5 million rows read and 100,000 rows written a day, 5 GB of storage in total; [D1 limits](https://developers.cloudflare.com/d1/platform/limits/): 500 MB per database, 50 queries per Worker invocation, 100 bound parameters per query. Rows read counts every row a query scans, not only those it returns.

## Generate a model

`ocre g model <Model> field:type...` writes the migration that creates the table and the model file, and registers the module in `src/models/mod.rs`:

```sh
ocre g model Author name:string^ bio:text?
```

```text
  create  migrations/0002_create_authors.sql
  create  src/models/author.rs
  update  src/models/mod.rs

Next:
  ocre migrate
  cargo check --target wasm32-unknown-unknown
```

Field types are `string`, `text`, `integer`, `float`, `boolean`, `date`, `datetime`, `references`, `attachment` and `json`; the suffix `?` makes a field optional (`NULL` allowed) and `^` unique. [Field types](../reference/field-types.md) lists the SQL and Rust type of each. `ocre g scaffold` and `ocre g api` create the model the same way when it does not exist yet (see [Generators](../reference/generators.md#ocre-g-model)).

The migration:

```sql
CREATE TABLE authors (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    bio TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX index_authors_on_name ON authors (name);
```

Every table gets `id`, `created_at` and `updated_at` (UTC text such as `2026-09-29 04:36:41`). Apply it with `ocre migrate` (or just run `ocre dev`, which migrates first).

## The model file, section by section

`src/models/author.rs` is plain Rust you own: read it, change it, add to it. Controllers (HTML pages, JSON API, GraphQL resolvers) call its functions instead of writing SQL, so a rule written here applies everywhere.

### The record struct

```rust
/// A row of the `authors` table.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Author {
    pub id: i64,
    pub name: String,
    pub bio: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
```

One field per column, deserialized from D1 rows by column name. Optional fields are `Option`. Booleans need a helper, because SQLite stores them as INTEGER 0/1; the blog starter's `Post` has:

```rust
    #[serde(deserialize_with = "ocre::bool_from_sql")]
    pub published: bool,
```

JSON columns use `ocre::json_from_sql` (`ocre::optional_json_from_sql` for optional ones) the same way.

### New and Changes

```rust
/// Values for a new author.
#[derive(Debug, Clone, Deserialize)]
pub struct NewAuthor {
    pub name: String,
    #[serde(default, deserialize_with = "ocre::optional")]
    pub bio: Option<String>,
}

/// Changes to a author: absent fields keep their value; for optional fields
/// `Some(None)` clears it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthorChanges {
    pub name: Option<String>,
    #[serde(default, deserialize_with = "ocre::patch")]
    pub bio: Option<Option<String>>,
}
```

`NewAuthor` is the input of `create`: every required field, optional fields as `Option`. `AuthorChanges` is the input of `update`: every field is `Option`, and `None` keeps the stored value. Optional columns are `Option<Option<T>>`: `None` keeps, `Some(None)` clears, `Some(Some(value))` sets. Both structs deserialize from JSON bodies, so the JSON API takes them directly; `ocre::optional` and `ocre::patch` also treat an empty string as "no value", which is what HTML forms send (see [JSON APIs](json-apis.md#optional-fields-and-partial-updates)).

### validate()

```rust
impl NewAuthor {
    /// Checks that need no database; `create` adds uniqueness and references.
    pub fn validate(&self) -> Validator {
        let mut v = Validator::new();
        v.required("name", &self.name);
        v
    }
}
```

`validate()` returns an `ocre::Validator` holding every failed check, without touching the database. Generated rules: `required` for non-optional `string`/`text`, `date`/`datetime` format checks, `safe_integer` for integers, file rules for attachments. `AuthorChanges::validate()` checks only the fields being changed. Add your own rules here; [Validations](validations.md) lists every check.

### Queries

| Function | SQL | Returns |
|---|---|---|
| `all(ctx, page)` | `SELECT * FROM authors ORDER BY id DESC LIMIT ?1 OFFSET ?2` | `Vec<Author>`, newest first |
| `count(ctx)` | `SELECT COUNT(*) AS count FROM authors` | `i64` |
| `find(ctx, id)` | `SELECT * FROM authors WHERE id = ?1` | `Option<Author>` |
| `find_many(ctx, &ids)` | `SELECT * FROM authors WHERE id IN (?1, ?2, ...)`, 100 ids per query | `Vec<Author>`, in no particular order |
| `create(ctx, new)` | `validate()`, database checks, then `INSERT ... RETURNING *` | the new `Author`, or `Error::Invalid` (422) |
| `update(ctx, id, changes)` | `validate()`, database checks, then `UPDATE ... RETURNING *` | `Some(Author)`, `None` when the id does not exist, or `Error::Invalid` |
| `delete(ctx, id)` | `DELETE FROM authors WHERE id = ?1` | `true`, or `false` when the id does not exist |

`page` is an `ocre::Page` (`limit` 1 to 100, default 50; `offset` from 0); handlers get it from `?limit=&offset=`, other code builds one with `Page::new(limit, offset)?`. Every function returns `ocre::Result`, so handlers use `?`. Missing records are `None`/`false`, not errors: the handler decides, usually with `.or_404()?`.

`create` and `update` add the checks that need the database to the `validate()` errors, then fail with all of them at once:

```rust
pub async fn create(ctx: &Ctx, new: NewAuthor) -> Result<Author> {
    let db = ctx.db()?;
    let mut v = new.validate();
    {
        let name = &new.name;
        v.check("name", db.exists("SELECT 1 FROM authors WHERE name = ?1 LIMIT 1", params![name]).await?, "has already been taken");
    }
    v.finish()?;
    db.first("INSERT INTO authors (name, bio) VALUES (?1, ?2) RETURNING *", params![new.name, new.bio])
        .await?
        .ok_or_else(|| Error::internal("INSERT ... RETURNING returned no row"))
}
```

A unique field (`^`) gets the "has already been taken" check (in `update`, `AND id != ?2` excludes the record itself); a `references` field gets "must exist". The `UNIQUE` index and the `REFERENCES` constraint in the migration stay as the last line of defense.

`update` writes only the fields that are `Some`, with one `CASE WHEN` per column, and refreshes `updated_at`:

```rust
    db.first(
        "UPDATE authors SET name = CASE WHEN ?1 THEN ?2 ELSE name END, bio = CASE WHEN ?3 THEN ?4 ELSE bio END, updated_at = datetime('now') WHERE id = ?5 RETURNING *",
        params![changes.name.is_some(), changes.name, changes.bio.is_some(), changes.bio.flatten(), id],
    )
    .await
```

### Associations

A `references` field connects two models. With the blog starter:

```sh
ocre g scaffold Comment author:string body:text post:references
```

The migration adds `post_id INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE` and an index on `post_id`. Deleting a post deletes its comments in the database (attached files of cascaded rows are not deleted from R2; see [File storage](files.md)). The generator writes both sides:

```rust
// src/models/comment.rs: belongs to
impl Comment {
    /// The post this comment belongs to.
    pub async fn post(&self, ctx: &Ctx) -> Result<Option<crate::models::post::Post>> {
        crate::models::post::find(ctx, self.post_id).await
    }

    // ocre:associations
}
```

```rust
// src/models/post.rs: has many, added under the `// ocre:associations` marker
impl Post {
    // ocre:associations
    /// Comments of this post, newest first.
    pub async fn comments(&self, ctx: &Ctx, page: Page) -> Result<Vec<crate::models::comment::Comment>> {
        ctx.db()?
            .all(
                "SELECT * FROM comments WHERE post_id = ?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3",
                params![self.id, page.limit, page.offset],
            )
            .await
    }
}
```

In a handler: `let comments = post.comments(&ctx, page).await?;` and `let post = comment.post(&ctx).await?;`. Keep the `// ocre:associations` and `// ocre:models` markers: later generators insert code after them.

## Migrations

Migrations live in `migrations/`, named `NNNN_<name>.sql` (four digits, one more than the highest existing number). Wrangler applies them in file-name order and records each applied name in the `d1_migrations` table:

```sh
ocre sql "SELECT id, name FROM d1_migrations"
```

```text
id | name
---+---------------------------
1  | 0001_create_posts.sql
2  | 0002_create_authors.sql
3  | 0003_create_comments.sql
4  | 0004_add_slug_to_posts.sql
5  | 0005_create_products.sql
(5 rows)
```

A migration runs once per database. Editing an applied file changes nothing where it already ran (its name is recorded) but changes what a new database gets, so local and production schemas drift. Never edit an applied migration; write a new one.

### Generate a migration

`ocre g migration <name> [field:type...]` infers the SQL from the name, as Rails does:

| Name | SQL | Fields |
|---|---|---|
| `create_<table>` | `CREATE TABLE` with `id`, the fields, `created_at`, `updated_at`, plus indexes | the columns |
| `add_<anything>_to_<table>` | `ALTER TABLE <table> ADD COLUMN ...` per column, plus indexes | required |
| `remove_<column>_from_<table>` | `ALTER TABLE <table> DROP COLUMN <column>` | optional: when given, one `DROP COLUMN` per field instead |
| anything else | empty file to fill in (data changes, custom SQL) | none allowed |

```sh
ocre g migration add_slug_to_posts slug:string?
```

```text
  create  migrations/0004_add_slug_to_posts.sql

Next:
  ocre migrate
  update the model in src/models/ to match the new columns
```

```sql
-- Migration: add_slug_to_posts
-- Applied once, in file-name order. Never edit a migration after it has been applied.
ALTER TABLE posts ADD COLUMN slug TEXT;
```

Existing rows need a value for a new column. Optional columns get `NULL`; required ones get a default (`''` for text, `0` for numbers, `'{}'` for JSON, `0` for booleans). References and attachments added to an existing table must be optional:

```sh
ocre g migration add_author_to_posts author:references
```

```text
error: `author_id` must be optional when added to an existing table
hint: SQLite adds reference columns as NULL for existing rows: use `name:references?`
```

A required unique column (`slug:string^`) fails on a table with two rows or more: every row gets the default `''`, and the unique index refuses the duplicates (`UNIQUE constraint failed`). Add it as `slug:string?^` (a unique index allows many `NULL`s), fill it, then enforce presence in `validate()`.

Other names give an empty migration, with only the two comment lines; write the SQL yourself:

```sh
ocre g migration backfill_slugs
```

```sql
-- Migration: backfill_slugs
-- Applied once, in file-name order. Never edit a migration after it has been applied.
UPDATE posts SET slug = lower(replace(title, ' ', '-')) WHERE slug IS NULL;
```

A name the generator cannot read with fields is refused:

```text
error: cannot tell which table `fix_things` changes
hint: name it `create_<table>`, `add_<columns>_to_<table>` or `remove_<columns>_from_<table>`
```

### Apply migrations

Check what is pending, then apply it. Here after `ocre g migration add_views_to_posts views:integer`:

```sh
ocre migrate --status
```

```text
 ⛅️ wrangler 4.143.0
────────────────────
Resource location: local
...
Migrations to be applied:
┌─────────────────────────────┐
│ Name                        │
├─────────────────────────────┤
│ 0006_add_views_to_posts.sql │
└─────────────────────────────┘

Next:
  ocre migrate
```

With `--json`, the pending names are in `pending`:

```sh
ocre migrate --status --json
```

```json
{"command":"migrate","next":["ocre migrate"],"ok":true,"pending":["0006_add_views_to_posts.sql"]}
```

`ocre migrate` runs `wrangler d1 migrations apply <database> --local` and shows wrangler's output:

```sh
ocre migrate
```

```text
...
? About to apply 1 migration(s)
Your database may not be available to serve requests during the migration, continue?
🤖 Using fallback value in non-interactive context: yes
🌀 Executing on local database blog (DB) from .wrangler/state/v3/d1:
🌀 To execute on your remote database, add a --remote flag to your wrangler command.
🚣 2 commands executed successfully.
┌─────────────────────────────┬────────┐
│ name                        │ status │
├─────────────────────────────┼────────┤
│ 0006_add_views_to_posts.sql │ ✅     │
└─────────────────────────────┴────────┘
```

Each migration file runs as one unit: when a statement fails, none of the file's statements stay applied and the migration stays pending. For a file whose second statement inserts into a missing table:

```text
✘ [ERROR] no such table: nope_table: SQLITE_ERROR
...
error: `wrangler d1 migrations apply blog --local` failed (exit status: 1)
hint: read the wrangler output above; the first error line names the cause
```

Fix the file (it was never applied) and run `ocre migrate` again. Add `--remote` to either command for the production database on Cloudflare. `ocre deploy` applies remote migrations itself (see [Deployment](deployment.md)), and `ocre dev` applies local ones before starting.

### Seeds, reset and ad-hoc SQL

`ocre db seed` runs `db/seeds.sql` (you create it; `ocre new` does not):

```sql
-- db/seeds.sql
INSERT INTO posts (title, body, published) VALUES ('Hello', 'First post', 1);
INSERT INTO posts (title, body, published) VALUES ('Draft', 'Not yet', 0);
INSERT INTO authors (name, bio) VALUES ('Ada', 'Mathematician');
```

```sh
ocre db seed            # local; --remote for production
ocre db reset           # local only: delete .wrangler/state/v3/d1, apply every migration, run db/seeds.sql
ocre sql "SELECT id, title, published, slug FROM posts"
```

```text
id | title | published | slug
---+-------+-----------+-----
1  | Hello | 1         | NULL
2  | Draft | 0         | NULL
(2 rows)
```

`ocre sql` accepts several statements separated by `;`, runs on the local database unless `--remote`, and with `--json` returns wrangler's results in `rows`:

```json
{"command":"sql","ok":true,"rows":[{"meta":{"duration":1},"results":[{"id":1,"title":"Hello"}],"success":true}]}
```

Seeds run every time you call `ocre db seed`: write them so a second run does no harm (`INSERT OR IGNORE`, or run them after `ocre db reset`). Queries from the CLI count toward the D1 quotas like any other. See [CLI commands](../reference/cli.md#ocre-migrate) for every flag.

## Update the model after a migration

A migration changes the table, not the Rust code. After `add_slug_to_posts slug:string?`, change `src/models/post.rs` in five places (the generator's "Next" line reminds you):

```rust
// 1. The record struct: one field per column.
pub struct Post {
    // ...
    pub slug: Option<String>,
}

// 2. NewPost: optional input.
pub struct NewPost {
    // ...
    #[serde(default, deserialize_with = "ocre::optional")]
    pub slug: Option<String>,
}

// 3. PostChanges: keep / clear / set.
pub struct PostChanges {
    // ...
    #[serde(default, deserialize_with = "ocre::patch")]
    pub slug: Option<Option<String>>,
}
```

```rust
// 4. create: the column and its placeholder.
    db.first(
        "INSERT INTO posts (title, body, published, slug) VALUES (?1, ?2, ?3, ?4) RETURNING *",
        params![new.title, new.body, new.published, new.slug],
    )

// 5. update: one CASE WHEN per column, placeholders renumbered.
    db.first(
        "UPDATE posts SET title = CASE WHEN ?1 THEN ?2 ELSE title END, body = CASE WHEN ?3 THEN ?4 ELSE body END, published = CASE WHEN ?5 THEN ?6 ELSE published END, slug = CASE WHEN ?7 THEN ?8 ELSE slug END, updated_at = datetime('now') WHERE id = ?9 RETURNING *",
        params![changes.title.is_some(), changes.title, changes.body.is_some(), changes.body, changes.published.is_some(), changes.published, changes.slug.is_some(), changes.slug.flatten(), id],
    )
```

Add rules for the new field to both `validate()` functions if it needs any. Then fix what the compiler reports in the controllers: the HTML scaffold's `PostForm` in `src/posts.rs` (a `slug: String` field, `from_record`, `to_new`, `to_changes`) and `templates/posts/_form.html`; a GraphQL node and input in `src/<plural>_api.rs`. Check with:

```sh
cargo check --target wasm32-unknown-unknown
```

A removed column goes the other way: delete the field from the three structs, the SQL and the forms. `SELECT *` keeps working as long as the struct has no field the table lacks (serde ignores extra columns).

## Custom queries

Every query goes through `ctx.db()?`, the `DB` binding of `wrangler.toml`, with `?1, ?2...` placeholders and `params![...]`. Never build SQL with `format!` from user input: values bound as parameters cannot change the statement.

| Method | Use for | Returns |
|---|---|---|
| `db.all::<T>(sql, params)` | `SELECT` returning rows | `Vec<T>` (all rows in memory: always `LIMIT`) |
| `db.first::<T>(sql, params)` | one row, `INSERT/UPDATE ... RETURNING *` | `Option<T>` |
| `db.execute(sql, params)` | `INSERT`, `UPDATE`, `DELETE` without rows | rows changed (`usize`) |
| `db.exists(sql, params)` | `SELECT 1 ... LIMIT 1` | `bool` |
| `db.batch(statements)` | several `ocre::Statement`s in one transaction | rows changed per statement |

`params!` accepts strings, integers, `f64`, `bool` (bound as 1/0), `Option` (`None` binds `NULL`) and `serde_json::Value` (bound as JSON text), mixed freely. A failed query is `Error::Internal`: the client gets a 500 "Internal server error", and the Worker log gets the D1 message and the SQL. The full API is in the [rustdoc of `Db`](/api/ocre/struct.Db.html).

Queries on one table belong in its model file, next to the generated functions. Queries across tables can go in a module of their own under `src/models/` (add `pub mod reports;` under `// ocre:models` in `src/models/mod.rs`):

```rust,check
// src/models/reports.rs
use ocre::{Ctx, Page, Result, params};
use serde::{Deserialize, Serialize};

use crate::models::post::Post;

/// One row of `posts_with_comment_counts`: columns of the SELECT, by name.
#[derive(Debug, Deserialize, Serialize)]
pub struct PostSummary {
    pub id: i64,
    pub title: String,
    #[serde(deserialize_with = "ocre::bool_from_sql")]
    pub published: bool,
    pub comments: i64,
}

/// Posts with their number of comments, in one query (a JOIN, not one
/// COUNT per post).
pub async fn posts_with_comment_counts(ctx: &Ctx, page: Page) -> Result<Vec<PostSummary>> {
    ctx.db()?
        .all(
            "SELECT posts.id, posts.title, posts.published, COUNT(comments.id) AS comments
             FROM posts LEFT JOIN comments ON comments.post_id = posts.id
             GROUP BY posts.id ORDER BY posts.id DESC LIMIT ?1 OFFSET ?2",
            params![page.limit, page.offset],
        )
        .await
}

/// Published posts whose title contains `term`.
pub async fn search_published(ctx: &Ctx, term: &str) -> Result<Vec<Post>> {
    let pattern = format!("%{term}%");
    ctx.db()?
        .all(
            "SELECT * FROM posts WHERE published = ?1 AND title LIKE ?2 ORDER BY id DESC LIMIT 20",
            params![true, pattern],
        )
        .await
}

/// Unpublishes every post older than `days`; returns how many changed.
pub async fn unpublish_older_than(ctx: &Ctx, days: i64) -> Result<usize> {
    ctx.db()?
        .execute(
            "UPDATE posts SET published = 0, updated_at = datetime('now')
             WHERE published = 1 AND created_at < datetime('now', '-' || ?1 || ' days')",
            params![days],
        )
        .await
}
```

`format!` builds the `LIKE` pattern, a value bound to `?2`, not the SQL. Returned as JSON from a handler (`Ok(Json(reports::posts_with_comment_counts(&ctx, page).await?))`), on the seeded database with three comments added:

```json
[{"id":2,"title":"Draft","published":false,"comments":1},{"id":1,"title":"Hello","published":true,"comments":2}]
```

Things to know about D1 values:

- **Booleans** are INTEGER 0/1: bind a `bool` with `params!`, read it with `#[serde(deserialize_with = "ocre::bool_from_sql")]`, compare in SQL with `published = 1`.
- **Integers** come back as JavaScript numbers, exact only within ±`ocre::MAX_SAFE_INTEGER` (2^53 - 1 = 9007199254740991). A larger value cannot be read back into `i64`, so generated models validate integer fields with `v.safe_integer(..)`, and an `i64` beyond it is bound as decimal text.
- **Dates** are TEXT: `created_at` is `YYYY-MM-DD HH:MM:SS` in UTC; compare with SQLite's `datetime('now', '-7 days')`. `ocre::now()` gives the current Unix time in seconds (`std::time::SystemTime::now()` panics in WebAssembly).
- **JSON** columns hold JSON text: bind a `serde_json::Value`, read it with `ocre::json_from_sql`.
- **Indexes**: rows read counts every row scanned. `WHERE` and `ORDER BY` on an unindexed column scan the whole table; `references` and `^` fields are indexed, add others with `CREATE INDEX` in a migration.

## Avoid N+1 queries

Loading a list and then one related record per item runs 1 + N queries: slow, N times the rows read, and limited to 50 queries per request on the free plan. Collect the ids and load them with `find_many`, which uses `WHERE id IN (...)` (100 ids per query):

```rust,check
// src/models/feed.rs
use std::collections::HashMap;

use ocre::{Ctx, Page, Result};
use serde::Serialize;

use crate::models::{
    comment::{self, Comment},
    post::{self, Post},
};

#[derive(Serialize)]
pub struct CommentWithPost {
    pub comment: Comment,
    pub post: Option<Post>,
}

/// The latest comments with their post: 2 queries for the whole page,
/// instead of 1 + one `post::find` per comment.
pub async fn recent_comments(ctx: &Ctx, page: Page) -> Result<Vec<CommentWithPost>> {
    let comments = comment::all(ctx, page).await?;
    let mut ids: Vec<i64> = comments.iter().map(|c| c.post_id).collect();
    ids.sort_unstable();
    ids.dedup();
    let posts: HashMap<i64, Post> = post::find_many(ctx, &ids).await?.into_iter().map(|p| (p.id, p)).collect();
    Ok(comments
        .into_iter()
        .map(|comment| {
            let post = posts.get(&comment.post_id).cloned();
            CommentWithPost { comment, post }
        })
        .collect())
}
```

Returned as JSON with `?limit=2`:

```json
[{"comment":{"id":3,"author":"Cy","body":"On two","post_id":2,"created_at":"2026-09-29 04:50:08","updated_at":"2026-09-29 04:50:08"},"post":{"id":2,"title":"Draft","body":"Not yet","published":false,"created_at":"2026-09-29 04:48:44","updated_at":"2026-09-29 04:49:13"}},{"comment":{"id":2,"author":"Bob","body":"Second","post_id":1,"created_at":"2026-09-29 04:50:08","updated_at":"2026-09-29 04:50:08"},"post":{"id":1,"title":"Hello","body":"First post","published":true,"created_at":"2026-09-29 04:48:44","updated_at":"2026-09-29 04:48:44"}}]
```

`find_many` returns rows in no particular order and skips missing ids, hence the `HashMap`. For aggregates (counts, sums), prefer one `JOIN ... GROUP BY` query as in `posts_with_comment_counts` above.

## See also

- [Validations](validations.md): every `Validator` check, and how errors reach forms and JSON
- [Controllers, routing, views and htmx](controllers.md): calling the model from handlers
- [JSON APIs and GraphQL](json-apis.md): the model behind `/api/<plural>` and `/graphql`
- [Generators](../reference/generators.md#ocre-g-model), [Field types](../reference/field-types.md), [CLI commands](../reference/cli.md#ocre-migrate)
- [Free-plan limits](../reference/limits.md)
- [API index](../api-index.md) and the [rustdoc](/api/ocre/index.html)
