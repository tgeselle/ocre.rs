# Models and migrations

A model is a generated Rust file, `src/models/<model>.rs`, that holds every query and rule about one D1 table, and migrations are numbered SQL files that create and change those tables. This page explains the generated model section by section (queries, callbacks, associations, enums), the `ocre::Query` builder, transactions, how to change the schema and undo a change, encrypted columns, several databases, and what is not supported.

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

Field types are `string`, `text`, `rich_text`, `integer`, `float`, `decimal`, `boolean`, `date`, `time`, `datetime`, `uuid`, `references`, `attachment`, `json` and `enum:<value>,<value>...`; the suffix `?` makes a field optional (`NULL` allowed) and `^` unique, and a `lock_version:integer` field turns on [optimistic locking](#optimistic-locking). [Field types](../reference/field-types.md) lists the SQL and Rust type of each. `ocre g scaffold` and `ocre g api` create the model the same way when it does not exist yet (see [Generators](../reference/generators.md#ocre-g-model)). The suffixes change what a `references` field generates (see [Associations](#associations)): `author:references?` is an optional parent (`ON DELETE SET NULL`), `user:references^` a one-to-one link (has one).

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

`AuthorChanges::changed()` lists the fields a change sets (`["name"]`), Rails' `changed` and `*_changed?`: a `before_update` callback can act only when a field changes (`if changes.changed().contains(&"email") { ... }`). The previous values are one `find(ctx, id)` away when a callback needs them (`*_was`). There is no other dirty tracking: records are plain values, and nothing saves them behind your back.

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

`validate()` returns an `ocre::Validator` holding every failed check, without touching the database. Generated rules: `required` for non-optional `string`/`text`, format checks for `date`, `time`, `datetime`, `decimal` and `uuid`, `safe_integer` for integers, file rules for attachments. `AuthorChanges::validate()` checks only the fields being changed. Add your own rules here; [Validations](validations.md) lists every check.

### Queries

| Function | SQL | Returns |
|---|---|---|
| `query()` | `SELECT * FROM authors`, as an `ocre::Query<Author>` to refine (see [The query builder](#the-query-builder)) | `Query<Author>` |
| `all(ctx, page)` | `SELECT * FROM authors ORDER BY id DESC LIMIT ?1 OFFSET ?2` | `Vec<Author>`, newest first |
| `count(ctx)` | `SELECT COUNT(*) AS count FROM authors` | `i64` |
| `find(ctx, id)` | `SELECT * FROM authors WHERE id = ?1 LIMIT ?2` | `Option<Author>` |
| `find_many(ctx, &ids)` | `SELECT * FROM authors WHERE id IN (?1, ?2, ...)`, 100 ids per query | `Vec<Author>`, in no particular order |
| `create(ctx, new)` | `before_create`, `validate()`, database checks, `INSERT ... RETURNING *`, `after_create` | the new `Author`, or `Error::Invalid` (422) |
| `update(ctx, id, changes)` | `before_update`, `validate()`, database checks, `UPDATE ... RETURNING *`, `after_update` | `Some(Author)`, `None` when the id does not exist, or `Error::Invalid` |
| `delete(ctx, id)` | `before_delete`, `DELETE FROM authors WHERE id = ?1 RETURNING *`, `after_delete` | `true`, or `false` when the id does not exist |

`all`, `count`, `find` and `find_many` are one line each on top of `query()`:

```rust
pub fn query() -> Query<Author> {
    Query::table("authors")
}

pub async fn find(ctx: &Ctx, id: i64) -> Result<Option<Author>> {
    query().eq("id", id).first(&ctx.db()?).await
}
```

`page` is an `ocre::Page` (`limit` 1 to 100, default 50; `offset` from 0); handlers get it from `?limit=&offset=`, other code builds one with `Page::new(limit, offset)?`. Every function returns `ocre::Result`, so handlers use `?`. Missing records are `None`/`false`, not errors: the handler decides, usually with `.or_404()?`.

`create` and `update` add the checks that need the database to the `validate()` errors, then fail with all of them at once:

```rust
pub async fn create(ctx: &Ctx, mut new: NewAuthor) -> Result<Author> {
    before_create(ctx, &mut new).await?;
    let db = ctx.db()?;
    let mut v = new.validate();
    {
        let name = &new.name;
        v.check("name", db.exists("SELECT 1 FROM authors WHERE name = ?1 LIMIT 1", params![name]).await?, "has already been taken");
    }
    v.finish()?;
    let record: Author = db
        .first("INSERT INTO authors (name, bio) VALUES (?1, ?2) RETURNING *", params![new.name, new.bio])
        .await?
        .ok_or_else(|| Error::internal("INSERT ... RETURNING returned no row"))?;
    after_create(ctx, &record).await?;
    Ok(record)
}
```

A unique field (`^`) gets the "has already been taken" check (in `update`, `AND id != ?2` excludes the record itself); a `references` field gets "must exist". The `UNIQUE` index and the `REFERENCES` constraint in the migration stay as the last line of defense.

`update` writes only the fields that are `Some`, with one `CASE WHEN` per column, and refreshes `updated_at`:

```rust
    let updated: Option<Author> = db
        .first(
            "UPDATE authors SET name = CASE WHEN ?1 THEN ?2 ELSE name END, bio = CASE WHEN ?3 THEN ?4 ELSE bio END, updated_at = datetime('now') WHERE id = ?5 RETURNING *",
            params![changes.name.is_some(), changes.name, changes.bio.is_some(), changes.bio.flatten(), id],
        )
        .await?;
```

### Callbacks

The end of every model file holds six private functions that `create`, `update` and `delete` call, like Rails' callbacks. They do nothing until you fill them in:

| Function | Called | Typical use |
|---|---|---|
| `before_create(ctx, &mut new)` | before `validate()` and the `INSERT` | normalize input (trim, lowercase an email), fill in defaults |
| `after_create(ctx, &record)` | after the `INSERT` | send an email, enqueue a job |
| `before_update(ctx, id, &mut changes)` | before `validate()` and the `UPDATE` | normalize the changed fields |
| `after_update(ctx, &record)` | after a successful `UPDATE` (not when the id does not exist) | refresh a cache, broadcast |
| `before_delete(ctx, id)` | before the `DELETE` | refuse to delete (return an error) |
| `after_delete(ctx, &record)` | after the `DELETE`, with the deleted row (`RETURNING *`) | delete files, clean up other tables |

Every controller, JSON API and GraphQL resolver goes through `create`/`update`/`delete`, so a callback runs for all of them. An `Err` returned by a `before_*` callback stops the operation before anything is written; for example, keeping authors that still have tasks (`Task` from `ocre g model Task title:string status:enum:open,done author:references?`):

```rust
/// Before the DELETE of the author `id`: return an error to keep it.
async fn before_delete(ctx: &Ctx, id: i64) -> Result<()> {
    let has_tasks = crate::models::task::query().eq("author_id", id).exists(&ctx.db()?).await?;
    let mut v = Validator::new();
    v.check("tasks", has_tasks, "must be deleted first");
    v.finish()
}
```

`delete` then answers `Error::Invalid` (422, "Tasks must be deleted first") and the author stays. The HTML scaffold shows it on the error page; JSON clients get it in `fields`.

An `Err` from an `after_*` callback is returned to the caller, but the write it follows is already done: D1 keeps no transaction open between two queries. Writes that must succeed or fail together go into one [`db.batch`](#transactions). Bulk operations (`update_all`, `delete_all`, hand-written SQL) and `ON DELETE CASCADE` run no callback.

The rest of Rails' callback API maps to plain code in these functions:

| Rails | In Ocre |
|---|---|
| `before_validation`, `after_validation` | `before_create` / `before_update` run before `validate()`; code after `v.finish()?` in `create` / `update` runs after it |
| `before_save`, `after_save` (create and update) | a private function both `before_create` and `before_update` (or both `after_*`) call; `around_*`: code before and after in the same function |
| `after_commit`, `after_create_commit`, `after_update_commit`, `after_destroy_commit` | the `after_*` functions: each generated write is committed when it returns (D1 auto-commits every statement) |
| `after_rollback` | the `Err` of the write or of `db.batch`: nothing was applied |
| `if:` / `unless:` conditions, `on:` | an `if` in the function; `changes.changed()` tells which fields an update sets |
| `throw :abort` | return an `Err` from a `before_*` function |
| callback objects, shared callbacks | a function in a module of your own, called from several models' callbacks |
| `dependent: :destroy` running the children's callbacks | in `before_delete`, delete the children through their model's `delete` (each runs its callbacks and deletes its files) instead of relying on `ON DELETE CASCADE` |
| association callbacks (`before_add`, `after_remove`) | the join or child model's `before_create` / `after_delete` |
| skipping callbacks (`update_column`, `update_all`, `delete`, `insert_all`) | `query().update_all(..)`, `delete_all`, `db.execute`, `db.batch`: they run no model code and no validation |
| `after_initialize`, `after_find`, `after_touch`, `Model.suppress` | none: rows are plain values deserialized by serde (derive values in a method), and nothing saves implicitly |

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

The suffixes and the shape of the model choose the kind of association:

| Fields | Migration | Generated |
|---|---|---|
| `post:references` | `post_id INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE`, index | belongs to: `comment.post(ctx)`; has many: `post.comments(ctx, page)` |
| `author:references?` | `author_id INTEGER REFERENCES authors(id) ON DELETE SET NULL`, index | the same, with `task.author(ctx)` returning `None` when `author_id` is `NULL`; deleting the author keeps its tasks and clears their `author_id` |
| `user:references^` (in `ocre g model Profile bio:text user:references^`) | `NOT NULL ... ON DELETE CASCADE` plus a unique index | has one: `user.profile(ctx)` returns `Option<Profile>` instead of a list |
| two references or more and no other field, e.g. `ocre g model Tagging post:references tag:references` | the references, plus a unique index on the pair | a join model: has many through, `post.tags(ctx, page)` and `tag.posts(ctx, page)` (a `JOIN` on `taggings`), and a "has already been taken" check on the pair in `create` |
| `author:references:writer_id` (in `ocre g model Book title:string author:references:writer_id`) | `writer_id INTEGER NOT NULL REFERENCES authors(id) ON DELETE CASCADE`, index | belongs to named after the column, `book.writer(ctx)`; has many `author.books(ctx, page)` |
| `employee:references:manager_id?` in `ocre g model Employee name:string employee:references:manager_id?` | `manager_id INTEGER REFERENCES employees(id) ON DELETE SET NULL`, index | a self join: belongs to `employee.manager(ctx)`, has many `employee.employees(ctx, page)` (rename it `reports` in the file if you like) |

With `Tagging` from the command above, the blog starter's `Post` gets:

```rust
    /// Tags of this post, through taggings, most recently linked first.
    pub async fn tags(&self, ctx: &Ctx, page: ocre::Page) -> Result<Vec<crate::models::tag::Tag>> {
        ctx.db()?
            .all(
                "SELECT tags.* FROM tags JOIN taggings ON taggings.tag_id = tags.id WHERE taggings.post_id = ?1 ORDER BY taggings.id DESC LIMIT ?2 OFFSET ?3",
                params![self.id, page.limit, page.offset],
            )
            .await
    }
```

Tag a post with `tagging::create(&ctx, NewTagging { post_id, tag_id }).await?` and untag it with `tagging::query().eq("post_id", post_id).eq("tag_id", tag_id).delete_all(&ctx.db()?).await?`.

Each `references` field also writes two eager-loading functions in the model that holds it, to load the association of a whole list in one query per 100 ids (see [Avoid N+1 queries](#avoid-n1-queries)):

| Function | In | Returns |
|---|---|---|
| `preload_<parents>(ctx, &records)` | `comment::preload_posts(&ctx, &comments)` | `HashMap<i64, Post>`: the parent of every record, by id |
| `for_<parents>(ctx, &parent_ids)` | `comment::for_posts(&ctx, &post_ids)` | `Vec<Comment>`: every child of these parents, newest first (not paginated: keep the id list short) |

#### Association options, the Ocre way

Rails configures associations with options; in Ocre each one is a line of code in the model, where you can read it:

| Rails option | In Ocre |
|---|---|
| `dependent: :destroy / :nullify / :restrict_with_error` | `ON DELETE CASCADE` (required reference), `ON DELETE SET NULL` (optional), or an error from `before_delete` (see [Callbacks](#callbacks)) |
| `class_name:`, `foreign_key:` | `author:references:writer_id` names the column; the target is the model before `:references` |
| `counter_cache: true` | a `comments_count INTEGER NOT NULL DEFAULT 0` column on the parent, updated by the child's `after_create` / `after_delete` (below), or a `COUNT(*)` on the indexed foreign key when the list is short |
| `touch: true` | `crate::models::post::touch(ctx, comment.post_id).await?` in the child's `after_create` / `after_update` / `after_delete`: every model has `touch(ctx, id)`, which sets `updated_at` (and bumps `lock_version`) without validation or callbacks |
| `inverse_of`, association caching, `reload_<name>` | not needed: an association is an `async` function returning plain values; call it again to reload |
| `validate: true` (`validates_associated`) | call the other model's `validate()` and `v.merge(..)` it in `validate()` or `create` |
| `autosave: true`, nested attributes | call the other model's `create`/`update` from the handler or a callback; several writes that must succeed together go into one [`db.batch`](#transactions) |
| scopes on an association (`-> { where(...) }`) | `crate::models::comment::query().eq("post_id", post.id).eq("approved", true)`, or a scope function in the child model |
| `polymorphic: true` | `commentable:polymorphic:post,photo` (see [Polymorphic references](#polymorphic-references)) |
| `has_many_attached` | `photos:attachments` (see [Files](files.md#many-files-per-record)) |

A counter cache and `touch`, in the child model (`src/models/comment.rs`, with `comments_count` added to `posts` by `ocre g migration add_comments_count_to_posts comments_count:integer`):

```rust
/// After the INSERT: count the comment on its post and touch the post.
async fn after_create(ctx: &Ctx, comment: &Comment) -> Result<()> {
    let sql = "UPDATE posts SET comments_count = comments_count + 1, updated_at = datetime('now') WHERE id = ?1";
    ctx.db()?.execute(sql, params![comment.post_id]).await?;
    Ok(())
}

/// After the DELETE, with the deleted row.
async fn after_delete(ctx: &Ctx, comment: &Comment) -> Result<()> {
    let sql = "UPDATE posts SET comments_count = comments_count - 1, updated_at = datetime('now') WHERE id = ?1";
    ctx.db()?.execute(sql, params![comment.post_id]).await?;
    Ok(())
}
```

Each callback costs one row written. Bulk deletes (`delete_all`, `ON DELETE CASCADE`) skip it: recount with `UPDATE posts SET comments_count = (SELECT COUNT(*) FROM comments WHERE comments.post_id = posts.id)` when you use them.

### Polymorphic references

A record that may belong to one of several models (Rails' `belongs_to :commentable, polymorphic: true`) takes a `polymorphic` field listing them:

```sh
ocre g scaffold Comment body:text commentable:polymorphic:post,photo
```

It becomes two fields: `commentable_type`, an [enum](#enum-fields) of the models' singular names (`CommentableType::Post`, stored as `'post'`), and `commentable_id`, an integer, with an index on the pair. SQLite cannot declare a foreign key to two tables, so `create` (and `update`, when a change sets both) checks that the record exists: `Commentable must exist` otherwise. The model gets an enum of the records and an accessor:

```rust,ignore
// src/models/comment.rs
pub enum Commentable {
    Post(crate::models::post::Post),
    Photo(crate::models::photo::Photo),
}

let parent = comment.commentable(&ctx).await?; // Option<Commentable>: None once the record is deleted
match parent {
    Some(Commentable::Post(post)) => { /* ... */ }
    Some(Commentable::Photo(photo)) => { /* ... */ }
    None => {}
}
```

Each listed model gets the has-many side, `post.comments(ctx, page)` (a query on `commentable_type = 'post'`). Add `?` for an optional reference (`commentable:polymorphic:post,photo?`). Deleting a post does not delete its comments (there is no foreign key): delete them in the post's `before_delete`. The models must exist before the field names them; a model may list itself (`subject:polymorphic:note,post?` on `Note`).

### Enum fields

`status:enum:open,done` stores one of a fixed list of values. The migration keeps the text and refuses anything else:

```sql
    status TEXT NOT NULL CHECK (status IN ('open', 'done')),
```

and the model gets a Rust enum named after the field, used in the row, `New` and `Changes` structs:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum Status {
    #[default]
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "done")]
    Done,
}
```

`Status::ALL` lists the values in order (for select boxes), `as_str()` and `Display` give the stored text, `FromStr` parses it, and `IntoParam` binds it, so `task::query().eq("status", Status::Done)` is the scope Rails generates as `Task.done`. The first value is the `Default`. JSON bodies send the text (`"status": "done"`); an unknown value fails deserialization with a 400. HTML scaffold forms parse the text with `v.one_of("status", &form.status)`, which adds "is not included in the list" (see [Validations](validations.md#every-check)). Values must be distinct snake_case words; an enum cannot be unique (`^`), and `--graphql` does not support enum fields yet.

Adding a value later means changing the `CHECK`, which SQLite cannot alter: add the variant, then rebuild the table with `ocre g migration rebuild_tasks` (see [Generate a migration](#generate-a-migration)).

### Rich text fields

`body:rich_text` is Action Text without its extra table: formatted text (bold, links, headings, lists, quotes, code) typed in the [Trix](https://trix-editor.org) editor, the one Rails uses, and kept as HTML in a `TEXT` column of the record.

```sh
ocre g scaffold Article title:string body:rich_text
```

- **Stored safe.** `create` and `update` run the HTML through `ocre::security::sanitize` before writing it: scripts, styles, event handlers and `javascript:` links are gone before the row exists, whatever the client sent (the JSON API included).
- **Validated on its text.** A required rich text field checks `v.required("body", &ocre::security::strip_tags(body))`, so an empty editor (`<div><br></div>`) is "can't be blank".
- **Forms.** The scaffold's `_form.html` loads Trix 2.1.19 from unpkg.com (allowed by the generated Content-Security-Policy: `script-src` and `style-src` list `https://unpkg.com`) and renders `<input type="hidden" id="article_body" name="body"><trix-editor input="article_body"></trix-editor>`; the editor fills the hidden input, which the form submits like any field. The toolbar's file button is hidden: files belong in an `attachment` field.
- **Rendering.** `{{ article.body|rich_text }}` prints the HTML (sanitized again on the way out, a few microseconds of CPU); `{{ article.body|plain_text|truncate(80) }}` prints its text, as the index page does (Rails' `to_plain_text`). Both filters come from `ocre::filters` (`use ocre::filters;` in the controller, which the scaffold writes).
- **Style.** Trix's stylesheet styles the editor; style the rendered HTML with your own CSS (`dd div`, `.content h1`...), the equivalent of Rails' `action_text/contents/_content` partial.

Apps created before `rich_text` existed need `"https://unpkg.com"` added to `style_src` in `content_security_policy()` (`src/lib.rs`), or the editor shows without its styles.

## Migrations

Migrations live in `migrations/`, named `NNNN_<name>.sql` (four digits, one more than the highest existing number). `ocre migrate` applies them in file-name order and records each applied name in the `d1_migrations` table:

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

`ocre g migration <name> [arguments...]` infers the SQL from the name, as Rails does. The first names take fields (`name:type`), the others column names or nothing:

| Name | SQL | Arguments |
|---|---|---|
| `create_<table>` | `CREATE TABLE` with `id`, the fields, `created_at`, `updated_at`, plus indexes | fields |
| `add_<anything>_to_<table>` | `ALTER TABLE <table> ADD COLUMN ...` per column, plus indexes | fields, required |
| `remove_<column>_from_<table>` | `DROP INDEX IF EXISTS index_<table>_on_<column>` (SQLite refuses to drop an indexed column), then `ALTER TABLE <table> DROP COLUMN <column>` | optional fields: when given, one `DROP COLUMN` per field instead |
| `add_index_to_<table>` | `CREATE INDEX index_<table>_on_<a>_and_<b> ON <table> (a, b)` | column names, in index order, required |
| `add_unique_index_to_<table>` | the same with `CREATE UNIQUE INDEX` | column names, required |
| `remove_index_from_<table>` | `DROP INDEX IF EXISTS index_<table>_on_<a>_and_<b>` (the name the two above give) | column names, required |
| `rename_<column>_to_<new>_in_<table>` | `ALTER TABLE <table> RENAME COLUMN <column> TO <new>` | none |
| `rename_<table>_to_<new>` | `ALTER TABLE <table> RENAME TO <new>` | none |
| `drop_<table>` | `DROP TABLE <table>` | none |
| `rebuild_<table>` | SQLite's table rebuild (create a new table, copy the rows, drop, rename, recreate the indexes), from `db/schema.sql` (see [Change a column: rebuild the table](#change-a-column-rebuild-the-table)) | none |
| anything else | empty file to fill in (data changes, custom SQL) | none allowed |

Index, rename and drop migrations need no schema file:

```sh
ocre g migration add_index_to_posts published created_at
ocre g migration rename_body_to_content_in_posts
```

```sql
-- Migration: add_index_to_posts
-- Applied once, in file-name order. Never edit a migration after it has been applied.
CREATE INDEX index_posts_on_published_and_created_at ON posts (published, created_at);
```

```sql
-- Migration: rename_body_to_content_in_posts
-- Applied once, in file-name order. Never edit a migration after it has been applied.
ALTER TABLE posts RENAME COLUMN body TO content;
```

Renames, drops and rebuilds end with the "update the model in src/models/ to match the new columns" step: the struct fields and the SQL of the model still use the old names. A renamed table keeps its index names (`index_tags_on_label` on `labels`), so `remove_index_from_labels label` would not find it; drop it by its real name in a hand-written migration. `add_index_to_posts` without columns fails with ``hint: list them in index order, without types, e.g. `ocre g migration add_index_to_posts author_id created_at` ``.

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

Existing rows need a value for a new column. Optional columns get `NULL`; required ones get a default (`''` for text, dates and times, `0` for numbers, `'{}'` for JSON, `0` for booleans, the first value for an enum). References and attachments added to an existing table must be optional:

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

`ocre migrate` runs the app's wrangler on the local database (`wrangler d1 migrations apply DB --local`, see [Why wrangler still appears](deployment.md#why-wrangler-still-appears)) and shows its output:

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
error: `wrangler d1 migrations apply DB --local` failed (exit status: 1)
hint: read the wrangler output above; the first error line names the cause
```

Fix the file (it was never applied) and run `ocre migrate` again. Add `--remote` to either command for the production database on Cloudflare. `ocre deploy` applies remote migrations itself (see [Deployment](deployment.md)), and `ocre dev` applies local ones before starting.

### The schema file

`ocre db schema` writes the database's current `CREATE` statements to `db/schema.sql` (Rails' `structure.sql`), read from `sqlite_master`; `--remote` dumps the production database instead:

```sh
ocre migrate && ocre db schema
```

```text
  create  db/schema.sql
```

```sql
-- Schema of the local D1 database, written by `ocre db schema` from sqlite_master.
-- A snapshot for reading: migrations/ are the source of truth. Run `ocre db schema` again after `ocre migrate`.

CREATE TABLE authors (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    bio TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX index_authors_on_name ON authors (name);
...
```

It is the quickest way for a person or an agent to see every table at once. Nothing loads it: a new database is always built by replaying `migrations/` (Rails' `schema.rb`/`structure.sql` in one SQL format). Commit it if you want schema changes visible in code review, and regenerate it after each `ocre migrate`.

### Column options, constraints and comments

Migrations are SQL, so Rails' column modifiers and table options are SQLite syntax in the file (edit a generated migration before applying it, or write one with a name the generator does not read):

| Rails | SQLite, in the migration |
|---|---|
| `default:`, `null: false` | `views INTEGER NOT NULL DEFAULT 0`, `status TEXT NOT NULL DEFAULT 'draft'` |
| `limit:`, `precision:`, `scale:` | a `CHECK (length(code) <= 8)`; SQLite ignores declared sizes (`VARCHAR(8)` is `TEXT`); exact numbers are the `decimal` field type |
| `collation:` | `email TEXT NOT NULL COLLATE NOCASE` (case-insensitive comparisons and unique index) |
| `add_check_constraint` | `CHECK (price >= 0)` on the column or the table; to add one to an existing table, rebuild it (below) |
| `comment:` | an SQL comment inside `CREATE TABLE`: `price REAL NOT NULL, -- in euros`; SQLite keeps the statement's text, so `ocre db schema` shows it in `db/schema.sql` |
| `if_not_exists:`, `force:`, `id: false`, `primary_key:` | `CREATE TABLE IF NOT EXISTS`, `DROP TABLE IF EXISTS` first, any primary key you declare (`PRIMARY KEY (a, b)`, `id TEXT PRIMARY KEY` for UUIDs); generated models expect an `INTEGER` `id`, so such tables get a hand-written model module |
| `change_table` (several changes at once) | several `ALTER TABLE` statements in one migration file, applied together |
| `change_column`, `change_column_null`, `change_column_default` | a table rebuild (below) |

### Change a column: rebuild the table

SQLite's `ALTER TABLE` can add, drop and rename columns, but not change a column's type, `NOT NULL`, `DEFAULT`, `CHECK` or `REFERENCES`. For those, SQLite's documented answer is to rebuild the table. `ocre g migration rebuild_<table>` writes that migration from the table's definition in `db/schema.sql` (run `ocre db schema` first):

```sh
ocre g migration rebuild_authors
```

```sql
-- Migration: rebuild_authors
-- Applied once, in file-name order. Never edit a migration after it has been applied.
-- Rebuilds `authors` to change what ALTER TABLE cannot (a column's type, NOT NULL,
-- DEFAULT, CHECK or REFERENCES): edit the CREATE TABLE below, and keep both
-- column lists of the INSERT in step with it.
PRAGMA defer_foreign_keys = true;
CREATE TABLE authors_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    bio TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO authors_new (id, name, bio, created_at, updated_at) SELECT id, name, bio, created_at, updated_at FROM authors;
DROP TABLE authors;
ALTER TABLE authors_new RENAME TO authors;
CREATE UNIQUE INDEX index_authors_on_name ON authors (name);
PRAGMA defer_foreign_keys = false;
```

Edit the `CREATE TABLE authors_new` (for example `bio TEXT NOT NULL DEFAULT ''`, or a new value in an enum's `CHECK`), keep the two column lists of the `INSERT` in step, then `ocre migrate`. D1 applies the file as one unit, so a failed copy (a row that breaks the new `NOT NULL`) leaves the old table untouched. Test it on the local database with real-looking data before `--remote`.

A table that other tables reference cannot be rebuilt this way: dropping it would run their `ON DELETE CASCADE` or `SET NULL`, because D1 enforces foreign keys. The generator refuses:

```text
error: `comments` references `posts`: rebuilding it would delete or clear their rows
hint: D1 enforces foreign keys, so dropping `posts` runs ON DELETE on `comments`; add a new column and backfill it instead
```

Without `db/schema.sql` it answers ``hint: run `ocre migrate` then `ocre db schema`: the rebuild copies the table's current definition``.

### Undo a migration

D1 migrations only go forward: D1 (and cf) have `d1 migrations apply` and `list`, and no command that runs a migration backwards, so Ocre has no `down` migrations, no `rollback` and no `redo`. Undo a change the way it was made, with a new migration:

| Mistake | Fix |
|---|---|
| A column should not exist | `ocre g migration remove_<column>_from_<table>` |
| A column has the wrong name | `ocre g migration rename_<column>_to_<new>_in_<table>` |
| A column has the wrong type or constraint | `ocre g migration rebuild_<table>`, then edit it |
| A table should not exist | `ocre g migration drop_<table>` |
| An index is missing or wrong | `add_index_to_<table>` / `remove_index_from_<table>` |
| A migration changed data wrongly | a migration with `UPDATE` statements that repair it |

Locally, when the migration was never deployed, the shortcut is to fix the file and start over: `ocre db reset` deletes the local database, applies every migration and loads the seeds.

In production, when a migration or a bad `UPDATE`/`DELETE` destroyed data, [D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/) restores the whole database to any minute of the last 7 days on the Workers Free plan (30 days on Workers Paid), at no cost and without any setup:

```sh
npx cf d1 list --name <database_name>                                   # its uuid
npx cf d1 time-travel get-bookmark <uuid> --timestamp 2026-09-29T10:00:00+00:00
npx cf d1 time-travel restore <uuid> --timestamp 2026-09-29T10:00:00+00:00
```

A restore overwrites everything written since that minute, including the `d1_migrations` rows: the migrations applied after it become pending again. Delete or fix those files before the next `ocre deploy`, which would apply them again. Note the current bookmark (`get-bookmark` without `--timestamp`) first, to go back to if the restore itself was a mistake (`restore <uuid> --bookmark <it>`). `<database_name>` is the `name` of the `DB` binding in `cloudflare.config.ts`; these commands act on the remote database only.

### Data migrations

Keep schema changes and data changes apart (Rails' maintenance-task advice):

- A small, one-off fix of existing rows (a backfill with a single `UPDATE ... SET slug = lower(replace(title, ' ', '-'))`) can be its own migration: `ocre g migration backfill_slugs` writes an empty numbered file for the SQL. It runs once, locally and on `ocre deploy`, in order with the schema changes.
- A change that needs Rust (computing values, calling an API) or touches many rows is a job: it processes a batch per run within the CPU limit and enqueues itself with a cursor for the rest (see [Background jobs](jobs.md)), started once with `ocre schedules run` or from an admin route. It can run again safely if it skips rows already done.
- Change the code to accept both shapes before the data moves, and remove the old shape in a later deploy.

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
ocre db reset           # local only: delete .wrangler/state/v3/d1, apply every migration, load db/fixtures and db/seeds.sql
ocre sql "SELECT id, title, published, slug FROM posts"
```

```text
id | title | published | slug
---+-------+-----------+-----
1  | Hello | 1         | NULL
2  | Draft | 0         | NULL
(2 rows)
```

`ocre sql` accepts several statements separated by `;`, runs on the local database unless `--remote`, and with `--json` returns D1's results in `rows`:

```json
{"command":"sql","ok":true,"rows":[{"meta":{"duration":1},"results":[{"id":1,"title":"Hello"}],"success":true}]}
```

Seeds run every time you call `ocre db seed`: write them so a second run does no harm (`INSERT OR IGNORE`, or run them after `ocre db reset`). Queries from the CLI count toward the D1 quotas like any other. See [CLI commands](../reference/cli.md#ocre-migrate) for every flag.

The other database commands: `ocre db create` (`--remote` creates the D1 database on Cloudflare when missing), `ocre db version` (the last applied migration; `--remote` too), and, on the local database only, `ocre db drop`, `ocre db truncate` (empties every app table, keeps the schema and `d1_migrations`), `ocre db seed --replant` (truncate, then seed; Loco's `seed --reset`) and `ocre db prepare` (applies pending migrations, and seeds a database it just created; safe to run any time). See [CLI commands](../reference/cli.md).

#### Fixtures

Fixtures are Rails-style named rows, one YAML (or JSON) file per table in `db/fixtures/` (Loco's `src/fixtures`). `ocre db seed` loads them first, then runs `db/seeds.sql`; either may be missing. `ocre db reset` and a fresh `ocre db prepare` load both too.

```yaml
# db/fixtures/authors.yml
ada:
  name: Ada
  bio: Mathematician

# db/fixtures/posts.yml
DEFAULTS: &defaults       # never inserted: values shared with `<<: *defaults`
  published: true
hello:
  <<: *defaults
  title: Hello from $LABEL # $LABEL is the row's label: "Hello from hello"
  author: ada              # author_id = the id of the row labelled `ada`
draft:
  <<: *defaults
  id: 42                   # explicit id
  title: Draft
  published: false
  tags: [rust, d1]         # sequences and mappings are stored as JSON text
```

Each file's table is emptied first (`DELETE FROM`, foreign keys deferred), then gets one `INSERT` per row, so loading twice gives the same rows. A row without `id` gets Rails' stable id for its label (`crc32(label) % (2^30 - 1)`), so `author: ada` works without knowing ada's id. `author: ada` becomes `author_id` when the word `author_id` appears in `migrations/*.sql`; the label is looked up in the `authors` fixtures first, else in the only file that defines it. `--from <dir>` loads another directory, e.g. `ocre db seed --from test/fixtures`.

Fixtures load into the local database only: they replace table rows, and Ocre never deletes production data. `ocre db seed --remote` refuses when there are fixtures to load; put production data in `db/seeds.sql`.

`ocre db dump` writes the other way: each app table (or `--tables posts,authors`) to `db/fixtures/<table>.yml`, one row per label `<table>_<id>` with its explicit id, readable by `ocre db seed`. It never overwrites a file without `--force`; `--dir <dir>` writes elsewhere, `--remote` reads the production database (one `SELECT *` per table, a D1 row read per row). BLOB columns come back as JSON arrays of bytes, loaded as text.

#### Static data

Read-only data that never changes at runtime (countries, plans, a price list; Loco's `data/` loaders) belongs in the binary, not in D1: `include_str!("../data/countries.json")` compiles the file in, and a `LazyLock` parses it once per Worker instance. Reading it costs no D1 row and no request; the file adds its size to the WebAssembly binary (3 MB compressed on the free plan), and changing it is a deploy.

```rust,check
// src/models/countries.rs
use std::sync::LazyLock;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Country {
    pub code: String,
    pub name: String,
}

/// In an app: `include_str!("../../data/countries.json")`.
const COUNTRIES_JSON: &str = r#"[{"code": "FR", "name": "France"}, {"code": "JP", "name": "Japan"}]"#;

/// Parsed on first use, then shared by every request of this Worker instance.
static COUNTRIES: LazyLock<Vec<Country>> =
    LazyLock::new(|| ocre::serde_json::from_str(COUNTRIES_JSON).expect("data/countries.json is valid"));

pub fn country(code: &str) -> Option<&'static Country> {
    COUNTRIES.iter().find(|country| country.code == code)
}
```

#### Data migrations

Migrations change the schema; changing existing rows (backfilling a new column, splitting a name) is a data migration. A single `UPDATE ... WHERE ...` in the SQL migration is fine for small tables. For large ones, write a [job](jobs.md) that walks the table with `query().batches(..)` a few batches per run and enqueues itself with the last id until `next` returns `None`: each run stays within the 50 queries and 10 ms of CPU of a free-plan invocation, and D1 never runs one statement over millions of rows.

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

## The query builder

`ocre::Query<T>` builds one `SELECT` on one table, step by step, and runs it on D1. Every model's `query()` starts one with the row type set (`post::query()` is `Query::<Post>::table("posts")`), so terminal methods return `Post`s. It is a thin layer over SQL, not an ORM: each method adds one clause, nothing runs until a terminal method, and the SQL it sends is predictable.

Two rules keep it safe from SQL injection:

- **Values** (`eq`, `is_in`, `contains`...) are always bound parameters, never written into the SQL.
- **Column names and SQL fragments** are `&'static str`: they come from your code, not from a request. To sort by a column the visitor picks, `match` their input onto a fixed name, as below.

### Scopes and a search

Scopes are plain functions taking and returning a `Query`; `scope(f)` applies one, and they take arguments like any function. Put them in the model file, below `query()`:

```rust,check
// src/models/post.rs, below query() (shown as a module of its own)
use ocre::{Ctx, Direction, Page, Paginated, Query, Result};

use crate::models::post::{self, Post};

/// Published posts only.
pub fn published(query: Query<Post>) -> Query<Post> {
    query.eq("published", true)
}

/// Titles containing `term`; `%` and `_` in it match literally.
pub fn titled(query: Query<Post>, term: &str) -> Query<Post> {
    query.contains("title", term)
}

/// A page of published posts, optionally filtered, sorted by a column the
/// visitor picks from a fixed list.
pub async fn search(ctx: &Ctx, term: Option<&str>, sort: &str, direction: Direction, page: Page) -> Result<Paginated<Post>> {
    let mut query = post::query().scope(published);
    if let Some(term) = term {
        query = query.scope(|q| titled(q, term));
    }
    // The column comes from this match, never from the request.
    let column = match sort {
        "title" => "title",
        _ => "created_at",
    };
    query.order_by(column, direction).order_desc("id").paginate(&ctx.db()?, page).await
}
```

`search(&ctx, Some("50% off"), "title", Direction::Desc, page)` with `?limit=20` sends two queries, the rows and the total:

```sql
SELECT * FROM posts WHERE published = ?1 AND title LIKE ?2 ESCAPE '\' ORDER BY title DESC, id DESC LIMIT ?3 OFFSET ?4
-- params: 1, '%50\% off%', 20, 0
SELECT COUNT(*) AS count FROM posts WHERE published = ?1 AND title LIKE ?2 ESCAPE '\'
```

`contains` escapes `%`, `_` and `\` with `ocre::escape_like` (Rails' `sanitize_sql_like`); use `escape_like` yourself in hand-written `LIKE` SQL.

`ocre::Paginated<T>` serializes as `{"items": [...], "total": 42, "limit": 20, "offset": 0}` and has `current_page()`, `total_pages()`, `has_next()`, `has_previous()`, `next_page()`, `previous_page()` (each an `Option<Page>`) and `map(f)` to turn rows into view structs. The total costs a second query that reads every matching row: on a large table, prefer `all` with `page.next(rows.len())` links (see [Controllers](controllers.md)).

`ocre::Direction` deserializes from `asc` or `desc` (any case; `asc` by default), so a handler takes it straight from the query string:

```rust,check
// src/post_search.rs
use axum::{
    Router,
    extract::{Query, State},
    routing::get,
};
use ocre::{ApiResult, Ctx, Direction, Json, Page, Paginated};
use serde::Deserialize;

use crate::models::post::{self, Post};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/api/posts/search", get(search))
}

#[derive(Deserialize)]
pub struct Search {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub direction: Direction,
}

/// `GET /api/posts/search?q=rust&sort=title&direction=desc&limit=20`
async fn search(State(ctx): State<Ctx>, page: Page, Query(search): Query<Search>) -> ApiResult<Json<Paginated<Post>>> {
    let mut query = post::query().eq("published", true);
    let term = search.q.trim();
    if !term.is_empty() {
        query = query.any(|q| q.contains("title", term).contains("body", term));
    }
    let column = if search.sort == "title" { "title" } else { "created_at" };
    Ok(Json(query.order_by(column, search.direction).paginate(&ctx.db()?, page).await?))
}
```

(`axum::extract::Query` reads the query string; `ocre::Query` is the builder. Import only one of them under that name.)

### Conditions

Conditions combine with `AND`, in the order they are added. The column argument may be qualified (`comments.author`) after a join.

| Method | SQL |
|---|---|
| `eq(col, v)`, `ne(col, v)` | `col = ?`, `col != ?` (`eq(col, None)` matches nothing: use `is_null`) |
| `gt`, `gte`, `lt`, `lte` | `col > ?`, `>=`, `<`, `<=` |
| `between(col, low, high)` | `col BETWEEN ? AND ?` (inclusive) |
| `is_in(col, values)`, `not_in(col, values)` | `col IN (?, ?...)`, `col NOT IN (...)`; an empty `is_in` matches nothing (D1 binds 100 parameters at most per query) |
| `is_null(col)`, `is_not_null(col)` | `col IS NULL`, `col IS NOT NULL` |
| `like(col, pattern)`, `not_like(col, pattern)` | `col LIKE ?` with your pattern (`%` and `_` are wildcards) |
| `contains(col, text)`, `starts_with`, `ends_with` | `col LIKE ? ESCAPE '\'` with `%text%`, `text%`, `%text`, the text escaped |
| `where_sql(fragment, params![..])` | `(fragment)`, with bare `?` placeholders bound in order |
| `any(\|q\| q.a(..).b(..))` | `(a OR b)` |
| `not(\|q\| q.a(..).b(..))` | `NOT (a AND b)` |
| `none()` | `WHERE 0`: no row, and no row read (Rails' `none`) |
| `where_associated(table, fk)`, `where_missing(table, fk)` | `EXISTS (SELECT 1 FROM table WHERE table.fk = <this table>.id)`, or `NOT EXISTS`: rows with / without a child (Rails' `where.associated` / `where.missing` on a has-many side; for a belongs-to side, `is_not_null` / `is_null` on the column) |
| `date_range(col, from, to)` | `col BETWEEN ? AND ?` with both bounds, `col > ?` or `col < ?` with one, nothing with none (Loco's `DateRangeBuilder`, for `?from=&to=` filters) |
| `unscope_where()` | removes the conditions added so far (a model's default conditions, see below); chain new ones for Rails' `rewhere` |

The builder numbers every placeholder `?1, ?2...` in the final SQL, including those of `where_sql` and `having`, so fragments mix freely with the other methods:

```rust
post::query()
    .any(|q| q.is_null("slug").eq("slug", ""))
    .not(|q| q.eq("published", false))
    .where_sql("created_at >= datetime('now', ?)", params!["-7 days"])
// SELECT * FROM posts WHERE (slug IS NULL OR slug = ?1) AND NOT (published = ?2) AND (created_at >= datetime('now', ?3))
```

### Order, limits, columns, joins, groups

| Method | SQL |
|---|---|
| `order_asc(col)`, `order_desc(col)`, `order_by(col, direction)` | `ORDER BY col ASC/DESC`, appended after any order already set |
| `order_in(col, values)` | `ORDER BY CASE col WHEN ? THEN 0 ... ELSE n END` (Rails' `in_order_of`; unlisted values last) |
| `order_sql(term)` | a raw term, e.g. `lower(title) ASC` |
| `reorder()` | removes the order set so far (a scope's default order) |
| `reverse_order()` | flips every `ASC`/`DESC` (Rails' `reverse_order`); `ORDER BY <table>.id DESC` when no order is set |
| `limit(n)`, `offset(n)`, `page(page)` | `LIMIT ?`, `OFFSET ?`; `page` sets both from `?limit=&offset=` |
| `unscope_limit()` | removes the limit and offset set so far |
| `select::<U>(columns)` | `SELECT columns` instead of `*`; the rows become `U`, a struct whose fields match the column names (`AS` for expressions) |
| `distinct()` | `SELECT DISTINCT` |
| `join(clause)` | the clause as written: `JOIN ...` or `LEFT JOIN ...` |
| `group_by(columns)`, `having(fragment, params![..])` | `GROUP BY columns HAVING (fragment)` |

### Running a query

Terminal methods take `&Db` (`&ctx.db()?`, or `&ctx.db_named("...")?` for another database) and return `ocre::Result`:

| Method | Runs | Returns |
|---|---|---|
| `all(&db)` | the query | `Vec<T>`, every row in memory: bound it with `limit` or `page` |
| `first(&db)` | the query with `LIMIT 1` | `Option<T>` (Rails' `find_by`/`first`; add an order for "first") |
| `paginate(&db, page)` | the rows and `COUNT(*)` | `Paginated<T>` |
| `count(&db)` | `SELECT COUNT(*)`, ignoring order and limits | `i64` |
| `exists(&db)` | `SELECT 1 ... LIMIT 1` | `bool` (Rails' `exists?`) |
| `pluck::<V>(&db, expr)` | `SELECT expr AS value`, keeping order and limits | `Vec<V>` (Rails' `pluck`, `ids`) |
| `aggregate::<V>(&db, expr)` | `SELECT SUM(price) AS value ...`, without order or limits | `Option<V>`: `None` when SQL gives `NULL` (the `SUM` of no row) |
| `update_all(&db, vec![(col, value.into_param())])` | `UPDATE table SET col = ? WHERE ...` | rows changed |
| `delete_all(&db)` | `DELETE FROM table WHERE ...` | rows deleted |
| `first_or_create(&db, \|\| async { .. })` | the query with `LIMIT 1`, then your `create` when nothing matched | `T` (Rails' `find_or_create_by`; two racing requests can both create) |
| `create_or_first(&db, \|\| async { .. })` | your `create`, then the query when it failed with "has already been taken" or `UNIQUE constraint failed` | `T` (Rails' `create_or_find_by`: safe under races, needs a `UNIQUE` index) |
| `batches(size, \|row\| row.id)`, then `next(&db)` | `WHERE id > <last id> ORDER BY id LIMIT size`, one query per call | `Option<Vec<T>>`, `None` when done (Rails' `find_in_batches`; loop over each batch for `find_each`) |
| `explain(&db)` | `EXPLAIN QUERY PLAN SELECT ...` | `Vec<String>`, one line per step (Rails' `explain`) |

`update_all` and `delete_all` ignore joins, order and limits, skip validations and callbacks, do not touch `updated_at` unless you list it, and, without any condition, change every row of the table. `delete_all` leaves the R2 files of attachment columns in place.

For batches and custom calls, `to_statement()`, `count_statement()`, `exists_statement()`, `value_statement(expr)`, `aggregate_statement(expr)`, `update_statement(sets)`, `delete_statement()` and `explain_statement()` return the `ocre::Statement` (SQL plus parameters) without running it: put several in one [transaction](#transactions), or check the SQL in a unit test.

`batches` pages by id (keyset pagination), so the hundredth batch costs the same as the first, unlike `OFFSET`. A Worker invocation may run 50 queries on the free plan: walk a large table from a job or a scheduled task, a few batches per run, and store `batches.after()` (the last id) to resume the next run with `.resume_after(Some(id))`.

A model's default scope (Rails' `default_scope`) is its `query()`: add the condition there (`Query::table("posts").is_null("deleted_at")` for soft-deleted rows) and every generated function, controller and API applies it; `Query::table("posts")` or `unscope_where()` is the unscoped query. Block-level scoping (`Post.where(..).scoping { }`) has no equivalent: pass the query or a scope function to the code that needs it.

A few calculations, eager loads and bulk changes on the blog's tables:

```rust,check
// src/models/stats.rs
use ocre::{Ctx, IntoParam, Result, params};
use serde::{Deserialize, Serialize};

use crate::models::{
    comment,
    post::{self, Post},
    product,
};

/// One row per post: columns of the SELECT, by name.
#[derive(Debug, Deserialize, Serialize)]
pub struct CommentCount {
    pub post_id: i64,
    pub comments: i64,
}

/// The ten posts with the most comments (two or more).
pub async fn busiest_posts(ctx: &Ctx) -> Result<Vec<CommentCount>> {
    comment::query()
        .select::<CommentCount>("post_id, COUNT(*) AS comments")
        .group_by("post_id")
        .having("COUNT(*) >= ?", params![2])
        .order_desc("comments")
        .limit(10)
        .all(&ctx.db()?)
        .await
}

/// Posts `author` commented on, newest first (a JOIN, one query).
pub async fn commented_by(ctx: &Ctx, author: &str) -> Result<Vec<Post>> {
    post::query()
        .select::<Post>("posts.*")
        .distinct()
        .join("JOIN comments ON comments.post_id = posts.id")
        .eq("comments.author", author)
        .order_desc("posts.id")
        .limit(20)
        .all(&ctx.db()?)
        .await
}

/// The average price, `None` when there is no product.
pub async fn average_price(ctx: &Ctx) -> Result<Option<f64>> {
    product::query().aggregate(&ctx.db()?, "AVG(price)").await
}

/// Ids of the latest drafts.
pub async fn draft_ids(ctx: &Ctx) -> Result<Vec<i64>> {
    post::query().eq("published", false).order_desc("id").limit(100).pluck(&ctx.db()?, "id").await
}

/// Whether a post has any comment: stops at the first one.
pub async fn has_comments(ctx: &Ctx, post_id: i64) -> Result<bool> {
    comment::query().eq("post_id", post_id).exists(&ctx.db()?).await
}

/// Unpublishes the given posts in one statement; returns how many changed.
pub async fn unpublish(ctx: &Ctx, ids: &[i64]) -> Result<usize> {
    post::query()
        .is_in("id", ids.iter().copied())
        .update_all(&ctx.db()?, vec![("published", false.into_param())])
        .await
}

/// Deletes the comments of an author (no callbacks run).
pub async fn purge_author(ctx: &Ctx, author: &str) -> Result<usize> {
    comment::query().eq("author", author).delete_all(&ctx.db()?).await
}

/// Posts nobody commented on yet (`NOT EXISTS`, one index lookup per post).
pub async fn uncommented(ctx: &Ctx) -> Result<Vec<Post>> {
    post::query().where_missing("comments", "post_id").order_desc("id").limit(20).all(&ctx.db()?).await
}

/// Walks every post, 100 at a time (a job or scheduled task: one query per batch).
pub async fn count_words(ctx: &Ctx) -> Result<usize> {
    let db = ctx.db()?;
    let mut batches = post::query().batches(100, |post| post.id);
    let mut words = 0;
    while let Some(posts) = batches.next(&db).await? {
        words += posts.iter().map(|post| post.body.split_whitespace().count()).sum::<usize>();
    }
    Ok(words)
}

/// The product with this name, created when missing. `name` has a UNIQUE
/// index: when two requests race, the loser reads the winner's row.
pub async fn product_named(ctx: &Ctx, name: &str) -> Result<product::Product> {
    let db = ctx.db()?;
    product::query()
        .eq("name", name)
        .create_or_first(&db, || async {
            let sql = "INSERT INTO products (name, price) VALUES (?1, 0) RETURNING *";
            db.first(sql, params![name]).await?.ok_or_else(|| ocre::Error::internal("no row returned"))
        })
        .await
}
```

`busiest_posts` sends `SELECT post_id, COUNT(*) AS comments FROM comments GROUP BY post_id HAVING (COUNT(*) >= ?1) ORDER BY comments DESC LIMIT ?2`, and `commented_by` sends `SELECT DISTINCT posts.* FROM posts JOIN comments ON comments.post_id = posts.id WHERE comments.author = ?1 ORDER BY posts.id DESC LIMIT ?2`.

Rows read are what D1 bills: `count`, `aggregate` and `paginate`'s total read every matching row, `exists` and `first` stop early, and a filter or order on a column without an index scans the whole table. `references` and `^` columns are indexed; add others with `ocre g migration add_index_to_<table> <columns>`.

SQLite tells whether a query uses an index (`query.explain(&db).await?` returns the same `detail` lines from code). Paste the SQL (with a sample value in place of each `?N`) into `EXPLAIN QUERY PLAN` on the local database; `SEARCH ... USING INDEX` is good, `SCAN <table>` reads every row:

```sh
ocre sql "EXPLAIN QUERY PLAN SELECT * FROM comments WHERE post_id = 1"
```

```text
id | parent | notused | detail
---+--------+---------+--------------------------------------------------------------
3  | 0      | 61      | SEARCH comments USING INDEX index_comments_on_post_id (post_id=?)
(1 row)
```

## Custom queries

When the builder does not fit (a subquery, `UNION`, `INSERT ... ON CONFLICT`, a window function, a report across tables), write the SQL. Every query goes through `ctx.db()?`, the `DB` binding of `cloudflare.config.ts`, with `?1, ?2...` placeholders and `params![...]`. Never build SQL with `format!` from user input: values bound as parameters cannot change the statement.

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
use ocre::{Ctx, Page, Result, escape_like, params};
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
    let pattern = format!("%{}%", escape_like(term));
    ctx.db()?
        .all(
            "SELECT * FROM posts WHERE published = ?1 AND title LIKE ?2 ESCAPE '\\' ORDER BY id DESC LIMIT 20",
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

`format!` builds the `LIKE` pattern, a value bound to `?2`, not the SQL; `escape_like` makes a `%` or `_` typed by the user match literally (with `ESCAPE '\'` in the SQL). Returned as JSON from a handler (`Ok(Json(reports::posts_with_comment_counts(&ctx, page).await?))`), on the seeded database with three comments added:

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

Loading a list and then one related record per item runs 1 + N queries: slow, N times the rows read, and limited to 50 queries per request on the free plan. Ocre has no lazy loading to detect (an association is an `async` function you call), so the rule is simply: never call an association or `find` in a loop. Load the whole list's associations at once with the generated `preload_<parents>` (belongs to), `for_<parents>` (has many) or `find_many`, which use `WHERE id IN (...)` (100 ids per query):

```rust,check
// src/models/feed.rs
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
/// instead of 1 + one `comment.post(ctx)` per comment.
pub async fn recent_comments(ctx: &Ctx, page: Page) -> Result<Vec<CommentWithPost>> {
    let comments = comment::all(ctx, page).await?;
    let posts = comment::preload_posts(ctx, &comments).await?;
    Ok(comments
        .into_iter()
        .map(|comment| {
            let post = posts.get(&comment.post_id).cloned();
            CommentWithPost { comment, post }
        })
        .collect())
}

#[derive(Serialize)]
pub struct PostWithComments {
    pub post: Post,
    pub comments: Vec<Comment>,
}

/// A page of posts with all their comments: 2 queries, whatever the page size.
pub async fn posts_with_comments(ctx: &Ctx, page: Page) -> Result<Vec<PostWithComments>> {
    let posts = post::all(ctx, page).await?;
    let ids: Vec<i64> = posts.iter().map(|post| post.id).collect();
    let mut comments = comment::for_posts(ctx, &ids).await?;
    Ok(posts
        .into_iter()
        .map(|post| {
            let (mine, rest) = comments.drain(..).partition(|comment| comment.post_id == post.id);
            comments = rest;
            PostWithComments { post, comments: mine }
        })
        .collect())
}
```

Returned as JSON with `?limit=2`:

```json
[{"comment":{"id":3,"author":"Cy","body":"On two","post_id":2,"created_at":"2026-09-29 04:50:08","updated_at":"2026-09-29 04:50:08"},"post":{"id":2,"title":"Draft","body":"Not yet","published":false,"created_at":"2026-09-29 04:48:44","updated_at":"2026-09-29 04:49:13"}},{"comment":{"id":2,"author":"Bob","body":"Second","post_id":1,"created_at":"2026-09-29 04:50:08","updated_at":"2026-09-29 04:50:08"},"post":{"id":1,"title":"Hello","body":"First post","published":true,"created_at":"2026-09-29 04:48:44","updated_at":"2026-09-29 04:48:44"}}]
```

`preload_posts` returns a `HashMap<i64, Post>` keyed by id (rows come in no particular order, and missing ids are skipped). `for_posts` is not paginated: use it for a page of parents, not for a whole table. For aggregates (counts, sums), prefer one `JOIN ... GROUP BY` query as in `busiest_posts` and `posts_with_comment_counts` above.

## Many rows in one query

A free-plan invocation runs 50 D1 queries, and D1 binds at most 100 parameters per statement, so writing rows one by one does not scale. `ocre::bulk` writes many rows in one statement: the rows go as one JSON array parameter, unpacked by SQLite's `json_each`.

| Builder | SQL | Rails |
|---|---|---|
| `bulk::insert(table, &columns, &rows)` | `INSERT INTO t (a, b) SELECT value ->> '$.a', value ->> '$.b' FROM json_each(?1)` | `insert_all` |
| `bulk::upsert(table, key, &columns, &rows)` | the same, `ON CONFLICT (key) DO UPDATE SET` the other columns | `upsert_all` |
| `bulk::update(table, key, &columns, &rows, touch)` | `UPDATE t SET a = row.value ->> '$.a' ... FROM json_each(?1) AS row WHERE t.key = row.value ->> '$.key'`, plus `updated_at` when `touch` | `update_all` with a value per row |

```rust,ignore
#[derive(serde::Serialize)]
struct Play {
    track_id: i64,
    played_at: String,
}

let insert = ocre::bulk::insert("plays", &["track_id", "played_at"], &plays)?;
let inserted = ctx.db()?.execute(&insert.sql, insert.params).await?; // one query, however many rows
```

Rows are structs or maps (anything serializing to a JSON object); each listed column is read by name, a missing one is `NULL`. Booleans are stored as `1`/`0`, nested values as JSON text. The builders return a `Statement`, so several go into one `db.batch` to apply together. They skip the models' validations and callbacks, like Rails' `insert_all`: validate first when the data comes from users. Table and column names must be plain identifiers (never user input); a D1 statement is limited in size, so send a few thousand small rows at a time (`rows.chunks(500)`).

## Transactions

D1 runs every statement in auto-commit mode, and refuses `BEGIN TRANSACTION` and `SAVEPOINT` from a Worker. The unit of atomicity is `db.batch(statements)`: D1 runs the statements in order, in one round trip, and when one fails none of them is applied. There is no transaction left open while Rust code runs, so a batch cannot read a value and decide what to write next.

```rust,check
// src/models/merging.rs
use ocre::{Ctx, IntoParam, Result, Statement, params};

use crate::models::{comment, post};

/// Moves every comment of post `from` to post `into`, then deletes `from`:
/// all three statements are applied, or none. `false` when `from` did not exist.
pub async fn merge_posts(ctx: &Ctx, from: i64, into: i64) -> Result<bool> {
    let move_comments = comment::query().eq("post_id", from).update_statement(vec![("post_id", into.into_param())]);
    let touch = Statement::new("UPDATE posts SET updated_at = datetime('now') WHERE id = ?1", params![into]);
    let delete_post = post::query().eq("id", from).delete_statement();
    // The rows changed by each statement, in order.
    let changed = ctx.db()?.batch(vec![move_comments, touch, delete_post]).await?;
    Ok(changed[2] == 1)
}

/// Takes `quantity` from a product's stock only if that much is left. The
/// check and the write are one statement, so two requests at the same moment
/// cannot both take the last item.
pub async fn take_stock(ctx: &Ctx, product_id: i64, quantity: i64) -> Result<bool> {
    let changed = ctx
        .db()?
        .execute(
            "UPDATE products SET stock = stock - ?1, updated_at = datetime('now') WHERE id = ?2 AND stock >= ?1",
            params![quantity, product_id],
        )
        .await?;
    Ok(changed == 1)
}
```

What this means in practice:

- Each generated `create`, `update` and `delete` writes with one statement, so it is atomic on its own. Their uniqueness and "must exist" checks are separate reads: keep the `UNIQUE` index and `REFERENCES` constraint as the real guarantee.
- Writes to several rows or tables that must succeed together go into one `batch`: build the statements with `Statement::new(sql, params![..])` or the builder's `*_statement()` methods. Callbacks and validations do not run for them.
- A decision based on a value ("only if enough stock is left", "only if still a draft") goes into the `WHERE` of the write, and the number of rows changed tells whether it happened. [Optimistic locking](#optimistic-locking) is the generated form of this rule.
- A batch counts the rows read and written by each statement, as if each ran alone; it saves round trips, not quota.
- Work that must happen after a write but may fail independently (an email, a call to another API) goes in a background job enqueued after the write, not in the batch.

## Optimistic locking

Two people open the same record, both save: without a check, the second save silently overwrites the first. A `lock_version:integer` field (Rails' magic column) makes `update` refuse the stale one:

```sh
ocre g scaffold Article title:string body:rich_text lock_version:integer
# or on an existing table:
ocre g migration add_lock_version_to_articles lock_version:integer
```

The column is `lock_version INTEGER NOT NULL DEFAULT 0`. The record has `pub lock_version: i64`; `NewArticle` has no such field (a new row starts at 0); `ArticleChanges` has `pub lock_version: Option<i64>`, the version the change was made from. `update` bumps and checks it in the same statement:

```sql
UPDATE articles SET ..., lock_version = lock_version + 1, updated_at = datetime('now')
WHERE id = ?4 AND (?5 IS NULL OR lock_version = ?5) RETURNING *
```

When no row comes back although the id exists, `update` returns `Error::Conflict` (409, "This article was changed by someone else since you opened it: reload it and apply your changes again."), Rails' `StaleObjectError`. `lock_version: None` skips the check (code that updates without having read the row).

- **HTML scaffolds** put `<input type="hidden" name="lock_version" value="{{ form.lock_version }}">` in the edit form; a stale submit shows the 409 error page.
- **JSON APIs** return `lock_version` with every record; clients send it back in `PATCH` bodies (`{"title": "New", "lock_version": 3}`) and get a JSON 409 when someone else saved first. GraphQL patches take `lockVersion`.
- **Cost:** nothing extra on success; one `SELECT 1` (one row read) to tell a conflict from a missing id.

Pessimistic locking (`SELECT ... FOR UPDATE`, `with_lock`) has no D1 equivalent: D1 never holds a transaction open while Rust runs. For "check then write" on one row, put the check in the `WHERE` of the write, as in `take_stock` under [Transactions](#transactions).

## Encrypted columns

`ocre::encryption` encrypts a column's value in Rust before it reaches D1 and decrypts it when the row is read, like Rails' `encrypts`. Someone who reads the database (a leaked export, the D1 console) sees only ciphertext. The cipher is AES-256-GCM, with keys derived from `SECRET_KEY_BASE` (HKDF-SHA256, separate from the cookie key); a changed value fails to decrypt instead of giving a wrong result. Two field types go in the row struct:

| Type | Stored text | Can be searched |
|---|---|---|
| `ocre::encryption::Encrypted` | a new random nonce each write: the same value never gives the same text | no: only read and written |
| `ocre::encryption::Deterministic` | the nonce comes from the value (HMAC-SHA256): equal values give equal texts | yes, with `eq`/`is_in` and a `UNIQUE` index; the price is that anyone can see which rows share a value |

Both deserialize by decrypting, bind as a parameter by encrypting (`IntoParam`), serialize to JSON as the plain value, and print `Encrypted(..)` in `Debug` output and logs. Declare the column `TEXT` (a value takes about 4/3 of its length plus 42 characters) and write the model by hand or edit a generated one: the generators have no encrypted field type. For a `contacts` table made with `ocre g migration create_contacts email:string^ phone:string?`:

```rust,check
// src/models/contact.rs
use ocre::{
    Ctx, Error, Query, Result,
    encryption::{self, Deterministic, Encrypted},
    params,
};
use serde::{Deserialize, Serialize};

/// A row of `contacts`: `email` and `phone` are stored encrypted.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Contact {
    pub id: i64,
    pub email: Deterministic,
    pub phone: Option<Encrypted>,
    pub created_at: String,
    pub updated_at: String,
}

pub fn query() -> Query<Contact> {
    Query::table("contacts")
}

/// Emails are normalized before encryption, so lookups ignore case.
fn normalize(email: &str) -> String {
    email.trim().to_lowercase()
}

pub async fn create(ctx: &Ctx, email: &str, phone: Option<String>) -> Result<Contact> {
    ctx.db()?
        .first(
            "INSERT INTO contacts (email, phone) VALUES (?1, ?2) RETURNING *",
            params![Deterministic::from(normalize(email)), phone.map(Encrypted::from)],
        )
        .await?
        .ok_or_else(|| Error::internal("INSERT ... RETURNING returned no row"))
}

/// Deterministic encryption makes `WHERE email = ?1` work.
pub async fn find_by_email(ctx: &Ctx, email: &str) -> Result<Option<Contact>> {
    query().eq("email", Deterministic::from(normalize(email))).first(&ctx.db()?).await
}

/// During a key rotation, rows written with the previous key have another
/// text: look for every candidate.
pub async fn find_by_email_during_rotation(ctx: &Ctx, email: &str) -> Result<Option<Contact>> {
    let candidates = encryption::installed()?.deterministic_candidates(&normalize(email));
    query().is_in("email", candidates).first(&ctx.db()?).await
}

/// Rewrites a batch of rows with the current key (run it from a job until
/// it returns 0).
pub async fn reencrypt(ctx: &Ctx, after_id: i64) -> Result<usize> {
    let db = ctx.db()?;
    let rows = query().gt("id", after_id).order_asc("id").limit(20).all(&db).await?;
    for row in &rows {
        db.execute(
            "UPDATE contacts SET email = ?1, phone = ?2 WHERE id = ?3",
            params![row.email.clone(), row.phone.clone(), row.id],
        )
        .await?;
    }
    Ok(rows.len())
}
```

`contact.email.as_str()` (or `{{ contact.email }}` in a template, through `Display`) is the plain value. Things to know:

- **Keys.** `Ctx` installs them the first time a Worker instance handles a request: one key derivation, then microseconds of CPU per value. Without a valid `SECRET_KEY_BASE` (64 characters or more), reading an encrypted row fails with a 500 whose log names the fix, and binding one writes `NULL`. Losing `SECRET_KEY_BASE` loses the data: keep a copy.
- **Rotation.** Put the old secret in `SECRET_KEY_BASE_PREVIOUS` (comma-separated for several) when you change `SECRET_KEY_BASE` (see [Security](security.md#rotating-secret_key_base-without-signing-everyone-out)): old values still decrypt, new writes use the new key. Deterministic lookups need `deterministic_candidates` until a job such as `reencrypt` has rewritten every row; then drop the previous secret.
- **Existing plain rows.** To encrypt a column that already has data, read it with `encryption::installed()?.decrypt_or_plaintext(&text)` (plain text passes through, like Rails' `support_unencrypted_data`) while a job rewrites the rows.
- **What does not work** on encrypted columns: `LIKE`, ranges, `ORDER BY` and aggregates (the database sees only ciphertext), and a `UNIQUE` index on an `Encrypted` column. Validate the plain value before encrypting it.
- **Tests.** Native code that reads encrypted values calls `encryption::install(Encryptor::new(&secret, &[])?)` first.

## Several databases

An app can use several D1 databases, for example to keep analytics or audit logs out of the main one (each database has its own 500 MB limit on the free plan; the account's 5 GB of storage and daily row quotas are shared). Each is a `bindings.d1(...)` entry in `worker.env` of `cloudflare.config.ts`, with its own binding name, and its own migrations folder:

```ts
DB: bindings.d1({ name: "blog" }),
ANALYTICS: bindings.d1({ name: "blog-analytics" }), // migrations in migrations/analytics/
```

`ctx.db()?` is the `DB` binding; `ctx.db_named("ANALYTICS")?` returns the other one as the same `ocre::Db`, so every query method and the builder's terminal methods work on it:

```rust,check
// src/models/page_view.rs
use ocre::{Ctx, Query, Result, params};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PageView {
    pub id: i64,
    pub path: String,
    pub created_at: String,
}

pub fn query() -> Query<PageView> {
    Query::table("page_views")
}

pub async fn record(ctx: &Ctx, path: &str) -> Result<()> {
    ctx.db_named("ANALYTICS")?.execute("INSERT INTO page_views (path) VALUES (?1)", params![path]).await?;
    Ok(())
}

pub async fn views_of(ctx: &Ctx, path: &str) -> Result<i64> {
    query().eq("path", path).count(&ctx.db_named("ANALYTICS")?).await
}
```

A missing binding is a 500 whose log says which `bindings.d1(...)` entry to add. The Ocre CLI manages only the `DB` database (`ocre migrate`, `ocre sql`, `ocre db ...`, and `ocre deploy`, which creates and migrates it); manage the others with cf in production:

```sh
mkdir -p migrations/analytics
npx cf d1 migrations create create_page_views --dir migrations/analytics   # an empty numbered file to fill in
npx cf d1 create --name blog-analytics                                   # once, before the first deploy
npx cf d1 list --name blog-analytics                                     # its uuid
npx cf d1 migrations apply <uuid> --dir migrations/analytics             # production
```

Locally, cf 1.0.0-beta.5 cannot migrate a local database yet (see [Why wrangler still appears](deployment.md#why-wrangler-still-appears)); do what Ocre does for `DB` and run the app's wrangler with a small config of its own, `db/analytics-d1.json`:

```json
{"name": "blog", "compatibility_date": "2026-09-01", "d1_databases": [{"binding": "ANALYTICS", "database_name": "blog-analytics", "migrations_dir": "../migrations/analytics"}]}
```

```sh
node_modules/.bin/wrangler d1 migrations apply ANALYTICS --local -c db/analytics-d1.json --persist-to .wrangler/state
```

`--persist-to .wrangler/state` is the state `ocre dev` uses, so the local Worker sees the tables.

Queries cannot join tables of two databases and a `batch` runs on one database: load ids from one, then `find_many`-style `is_in` queries on the other.

## Read replicas

D1 can keep read-only copies of a database near the Workers that use it ([read replication](https://developers.cloudflare.com/d1/best-practices/read-replication/), free). Ocre routes queries through D1 sessions so each visitor still reads what they wrote (Rails' automatic role switching):

1. turn replication on for the database (dashboard: D1 > database > Settings);
2. set the variable in `cloudflare.config.ts` (and `D1_REPLICAS=on` in `.dev.vars` to try it locally, where there is no replica):

```ts
D1_REPLICAS: bindings.text("on"),
```

Then every request served by `ocre::serve` uses one session per database: a `GET` or `HEAD` may start on any replica, other methods start on the primary, and later queries of the request see the earlier ones. After a request that wrote, the response sets `ocre_d1_<binding>` (HttpOnly, 5 minutes) with the session's bookmark, and the visitor's next requests resume from it, so the page shown after a form has the new row. Read-only responses set no cookie and stay cacheable. Jobs, crons and email handlers query the primary. Nothing changes in the models: `ctx.db()` returns the session. See [`ocre::replicas`](/api/ocre/replicas/index.html).

## Form objects and plain structs

Rails' Active Model makes a plain Ruby class behave like a model: attributes, validations, callbacks, naming, serialization. In Ocre any struct already does, with serde and `ocre::Validator`, so a form that is not a table (a contact form, a sign-up that writes two tables, a search) is a struct with a `validate()` function:

```rust,check
// src/models/contact_form.rs
use ocre::{Result, Validator};
use serde::{Deserialize, Serialize};

/// The contact form: no table, but typed fields, defaults and validations.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ContactForm {
    pub name: String,
    pub email: String,
    pub message: String,
    pub accept_terms: bool,
    /// Never serialized back to a client (Rails' `except:`).
    #[serde(skip_serializing)]
    pub honeypot: String,
}

impl ContactForm {
    /// Rails' `valid?` / `errors`: every failed check at once.
    pub fn validate(&self) -> Validator {
        let mut v = Validator::new();
        v.required("name", &self.name).email("email", &self.email).min_length("message", &self.message, 10);
        v.acceptance("accept_terms", self.accept_terms).absence("honeypot", &self.honeypot);
        v
    }

    /// Rails' `validate!`: `Error::Invalid` (422) with every message.
    pub fn validate_strict(&self) -> Result<()> {
        self.validate().finish()
    }
}
```

A handler takes it with `Form(form): Form<ContactForm>` (or `Json`), calls `form.validate().finish()?`, then does the work: send the email, or call two models' `create` in turn. How each Active Model module maps:

| Active Model | In Ocre |
|---|---|
| `API`, `Model`, `AttributeAssignment`, `Attributes` | a struct deriving `Deserialize` (mass assignment from a form, JSON or query string), with typed fields and `#[serde(default)]` defaults |
| `Validations`, `validates_with`, `validates_each`, custom validators | `validate() -> Validator`; a reusable rule is a function `fn slug(v: &mut Validator, field: &str, value: &str)`; `v.merge(other.validate())` combines validators (see [Validations](validations.md)) |
| `Callbacks`, `before_validation` | plain code before `validate()` (the generated `before_create` / `before_update` run before validation) |
| `Conversion` (`to_param`, `to_key`), `Naming` (`model_name`, `route_key`) | the generated `paths` module of each controller (`paths::show(post.id)`) and the model file's name; no reflection |
| `Dirty` | `<Model>Changes` and its `changed()` (see [New and Changes](#new-and-changes)) |
| `Serialization`, `as_json(only:, except:, methods:, include:)` | serde: `#[serde(skip_serializing)]`, `rename`, `flatten`, or a view struct built from the record (`Paginated::map` for lists) holding exactly what the response shows |
| `SecurePassword` (`has_secure_password`) | `ocre g auth`: `ocre::password` digests with a confirmation check and password resets (see [Authentication](authentication.md)) |
| `Translation` (`human_attribute_name`) | `FieldError::full_message` humanizes field names; translated names come from locale files (see [I18n](i18n.md)) |
| `Lint::Tests` | not needed: the compiler checks what a view or handler uses |

## What is not supported

Ocre models are generated Rust over SQL, not an ORM, and D1 is SQLite behind an HTTP API. These Rails features have no equivalent; the second column is what to do instead:

| Not supported | Instead |
|---|---|
| `down` migrations, `db:rollback`, `db:migrate:redo`, reversible `change` | D1 migrations are forward-only: a new migration, or D1 Time Travel (see [Undo a migration](#undo-a-migration)) |
| `Model.transaction do ... end`, savepoints, `after_commit` | `db.batch` (see [Transactions](#transactions)); a job enqueued after the write |
| Lazy loading, `strict_loading` | associations are explicit `async` functions; `includes` / `preload` / `eager_load` are `preload_<parents>`, `for_<parents>`, `find_many`, or one `join` + `select` into a row struct |
| Pessimistic locking (`lock`, `SELECT ... FOR UPDATE`) | conditions in the `UPDATE`'s `WHERE`, checking the rows changed; `lock_version` for [optimistic locking](#optimistic-locking) |
| Composite primary keys, single-table inheritance, delegated types | a plain `id` key and a unique index on the pair; a `kind` enum column (one table), or one table per type pointed to by a [polymorphic reference](#polymorphic-references) |
| Tables without `id`/`created_at`/`updated_at` from the generators | an empty migration with a name the generator does not read (`ocre g migration events_table`), your own `CREATE TABLE`, and a hand-written module |
| `ActiveModel` modules on plain structs | serde and `Validator` (see [Form objects and plain structs](#form-objects-and-plain-structs)) |
| Fixtures' `created_at`/`updated_at` filled automatically, ERB in fixture files | write the values in `db/fixtures/*.yml`; YAML anchors and `<<` share them |
| `readonly` records | records are plain values: nothing saves them except the model's `update` |
| Enum fields with `--graphql` | a `string` field with `v.inclusion(..)` |
| Joins across databases | two queries, one per database |

## See also

- [Validations](validations.md): every `Validator` check, and how errors reach forms and JSON
- [Controllers and routing](controllers.md): calling the model from handlers
- [JSON APIs and GraphQL](json-apis.md): the model behind `/api/<plural>` and `/graphql`
- [Generators](../reference/generators.md#ocre-g-model), [Field types](../reference/field-types.md), [CLI commands](../reference/cli.md#ocre-migrate)
- [Free-plan limits](../reference/limits.md)
- [API index](../api-index.md) and the [rustdoc](/api/ocre/index.html)
