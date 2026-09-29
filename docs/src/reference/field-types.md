# Field types

This page lists every field type the `ocre g model`, `ocre g scaffold`, `ocre g api` and `ocre g migration` generators accept, with the SQL column, Rust types, serde attributes, form input, JSON and GraphQL representation and validations each one produces, plus the `?` (optional) and `^` (unique) modifiers and the names that are refused.

## Before you start

- An Ocre app created with `ocre new` (see [CLI commands](cli.md#ocre-new)).
- Fields are written after the model name of a generator: `ocre g model Post title:string^ body:text published:boolean` (see [Generators](generators.md#ocre-g-model)).
- `references` fields need the referenced model to exist first (`ocre g model Author name:string` before `author:references`).
- Everything below was checked by running the generators of the current `ocre` CLI in a new app and quoting the files they wrote.

## Syntax

A field is `name:type`, optionally followed by modifiers:

| Written | Meaning |
|---|---|
| `title:string` | Required: `NOT NULL` column, a value is needed to create a record |
| `summary:string?` | Optional: the column accepts `NULL`, Rust uses `Option<T>` |
| `slug:string^` | Unique: a `CREATE UNIQUE INDEX` plus a "has already been taken" check in `create` and `update` |
| `code:string?^` or `code:string^?` | Optional and unique, in either order: `NULL` is allowed any number of times, other values once |

Field names are snake_case identifiers starting with a letter (`published_at`). Every table also gets `id INTEGER PRIMARY KEY AUTOINCREMENT`, `created_at` and `updated_at` (`TEXT NOT NULL DEFAULT (datetime('now'))`, UTC, written like `2026-09-29 04:20:26`); you do not declare them.

## Summary

| Type | SQL column | Rust type | HTML form input | JSON | GraphQL | Generated validation |
|---|---|---|---|---|---|---|
| `string` | `TEXT` | `String` | `<input>` | string | `String` | "can't be blank" when required |
| `text` | `TEXT` | `String` | `<textarea rows="5">` | string | `String` | "can't be blank" when required |
| `integer` | `INTEGER` | `i64` | `<input type="number" step="1">` | number | `Int` | within ±`ocre::MAX_SAFE_INTEGER` |
| `float` | `REAL` | `f64` | `<input type="number" step="any">` | number | `Float` | none (forms: "is not a number") |
| `boolean` | `INTEGER NOT NULL DEFAULT 0` | `bool` | `<input type="checkbox" value="true">` | `true` / `false` | `Boolean` | none; cannot be optional |
| `date` | `TEXT` | `String` | `<input type="date">` | string `YYYY-MM-DD` | `String` | "is not a valid date" |
| `datetime` | `TEXT` | `String` | `<input type="datetime-local">` | string `YYYY-MM-DD HH:MM[:SS]` (space or `T`) | `String` | "is not a valid date and time" |
| `references` | `INTEGER REFERENCES <plural>(id) ON DELETE CASCADE`, named `<name>_id`, indexed | `i64` | `<input type="number" step="1">` | number | `Int` | "must exist" in `create`/`update` |
| `attachment` | four columns: `<name>_key`, `_filename`, `_content_type` (`TEXT`), `_size` (`INTEGER`) | `ocre::storage::Upload` in inputs; four columns plus an `Attachment` accessor in the record | `<input type="file" accept="...">` | the four columns | the four columns (read only) | size and content type (`v.file`); "can't be blank" when required |
| `json` | `TEXT CHECK (json_valid(<name>))` | `ocre::serde_json::Value` | `<textarea rows="5" spellcheck="false" placeholder="{}">` | the JSON value itself | `JSON` scalar | forms: "is not valid JSON"; cannot be unique |

Without `?`, every column is `NOT NULL`. The sections below give the exact generated code for each type.

## Generated code per role

A model file (`src/models/<model>.rs`) has three structs, and HTML scaffolds add a form struct in `src/<plural>.rs`. The Rust type of a field depends on the struct:

| Struct | Role | Required field | Optional field (`?`) |
|---|---|---|---|
| `Post` | A row read from D1, serialized as the JSON response | `T` | `Option<T>` |
| `NewPost` | Input of `create` (JSON body, GraphQL input, parsed form) | `T` | `Option<T>` with `#[serde(default, deserialize_with = "ocre::optional")]` |
| `PostChanges` | Input of `update`: absent fields keep their value | `Option<T>` | `Option<Option<T>>` with `#[serde(default, deserialize_with = "ocre::patch")]` |
| `PostForm` (HTML scaffolds only) | The form as typed, so errors re-render the typed text | `String` (`bool` for checkboxes) | `String`, empty means `None` |

The serde helpers come from the `ocre` crate:

| Helper | Accepts | Result |
|---|---|---|
| `ocre::optional` | missing key, `null`, a blank string, a typed value, or a string parsed with `FromStr` (`"7"` for an `i64`) | `None` for the first three, `Some(value)` otherwise; a string that does not parse is a 400 |
| `ocre::patch` | same as `optional` | missing key = `None` (keep), `null` or blank string = `Some(None)` (clear), value = `Some(Some(value))` |
| `ocre::patch_json` | any JSON value | missing = `None`, `null` = `Some(None)`, anything else (a JSON string stays a string) = `Some(Some(value))` |
| `ocre::bool_from_sql` | `true`/`false` or the integers 0/1 that D1 returns | `bool` |
| `ocre::json_from_sql` | the JSON text D1 returns (or an already parsed value) | `serde_json::Value` |
| `ocre::optional_json_from_sql` | same, or `NULL` | `Option<serde_json::Value>` |

Required fields in `NewPost` have no attribute: a missing key or a value of the wrong type (`"qty": "3"` for an `integer`) is rejected by the JSON extractor with a 400 before validation runs. Only `boolean` fields have a default in `NewPost` (`#[serde(default)]`: missing means `false`).

## string

Short text on one line.

```sql
name TEXT NOT NULL,
summary TEXT,
```

```rust
// NewItem
pub name: String,
#[serde(default, deserialize_with = "ocre::optional")]
pub summary: Option<String>,
```

- Form: `<input name="name" value="{{ form.name }}" required>`; the `required` attribute only on required fields.
- Validation: `v.required("name", &self.name)` ("can't be blank"; whitespace only is blank). Optional strings have no check; an empty or whitespace-only form value is stored as `NULL`.
- No length limit is generated. Add `v.max_length(..)` in `validate()` if you need one (D1 accepts strings up to 2,000,000 bytes, see [Free-plan limits](limits.md)).

## text

Same as `string` (column `TEXT`, Rust `String`, same validation), rendered as `<textarea name="body" rows="5">` in forms. The difference is only the input.

## integer

```sql
qty INTEGER NOT NULL,
stock INTEGER,
```

- Rust: `i64` / `Option<i64>`.
- Validation: `v.safe_integer("qty", self.qty)`, the range ±9,007,199,254,740,991 (`ocre::MAX_SAFE_INTEGER`, 2^53 - 1). D1 returns numbers as JavaScript numbers, so larger values could not be read back exactly. Messages: "must be greater than or equal to -9007199254740991" / "must be less than or equal to 9007199254740991".
- Forms parse the text with `v.number("qty", &self.qty)` ("is not a number") or `v.optional_number(..)` for optional fields.
- JSON: a required integer must be a JSON number; an optional one also accepts a numeric string (`"stock": "7"` is stored as 7) through `ocre::optional`.

## float

```sql
price REAL NOT NULL,
discount REAL,
```

- Rust: `f64` / `Option<f64>`. Form: `<input type="number" step="any">`, parsed with `v.number` ("is not a number").
- No generated validation: add `v.range("price", self.price, 0.0..=1_000_000.0)` or `v.check(..)` in `validate()` for business rules.

## boolean

```sql
active INTEGER NOT NULL DEFAULT 0,
```

- SQLite has no boolean type: the column holds 0 or 1. The record reads it with `#[serde(deserialize_with = "ocre::bool_from_sql")] pub active: bool`.
- `NewItem` has `#[serde(default)] pub active: bool` (missing = `false`), `ItemChanges` `pub active: Option<bool>`.
- Form: `<input type="checkbox" name="active" value="true">`. An unchecked box is not submitted, so the form struct defaults it to `false`.
- A boolean cannot be optional: `flag:boolean?` fails with ``error: boolean field `flag` cannot be optional`` and ``hint: booleans are true or false (a checkbox); drop the `?` ``. GraphQL inputs default it to `false` (`#[graphql(default)]`).

## date

```sql
born_on TEXT NOT NULL,
due_on TEXT,
```

- Rust `String`, stored as typed. Validation: `v.date("born_on", &self.born_on)` accepts only a real calendar date written `YYYY-MM-DD` (month lengths and leap years checked: `2024-02-29` passes, `2026-02-30` fails with "is not a valid date").
- Form: `<input type="date">`, which browsers submit as `YYYY-MM-DD`.

## datetime

```sql
starts_at TEXT NOT NULL,
ends_at TEXT,
```

- Rust `String`, stored exactly as sent. Validation: `v.datetime(..)` accepts `YYYY-MM-DD HH:MM` or `YYYY-MM-DD HH:MM:SS`, with a space or `T` between date and time (`2026-09-29T18:30`, `2026-09-29 19:00:00`). No time zone is accepted; store UTC. A date alone fails with "is not a valid date and time".
- Form: `<input type="datetime-local">`, which browsers submit as `2026-09-29T18:30`.
- Values are not normalized: a record created with `2026-09-29T18:30` returns `"starts_at":"2026-09-29T18:30"`, while `created_at` uses SQLite's `2026-09-29 04:20:26`. Compare them as text only when they use the same format.

## references

`author:references` creates the column `author_id` pointing to the `authors` table; the referenced model (`src/models/author.rs`) must already exist, otherwise the generator stops with `error: src/models/author.rs does not exist` and ``hint: generate the referenced model first, e.g. `ocre g model Author name:string` ``.

```sql
author_id INTEGER NOT NULL REFERENCES authors(id) ON DELETE CASCADE,
...
CREATE INDEX index_items_on_author_id ON items (author_id);
```

- Rust `i64` (`Option<i64>` with `?`, which omits `NOT NULL`). Deleting an author deletes its items (`ON DELETE CASCADE`); files of the deleted items stay in R2.
- `create` and `update` check the row exists: `v.check("author_id", !db.exists("SELECT 1 FROM authors WHERE id = ?1 LIMIT 1", ..), "must exist")`.
- Associations: `item.author(&ctx)` in the model, `author.items(&ctx, page)` added to `src/models/author.rs`. See [Models and migrations](../guides/models.md#associations).
- Form: `<input type="number" step="1" name="author_id">` labelled "Author". JSON and GraphQL: `author_id` / `authorId`, a number.

## attachment

A file stored in R2 (binding `STORAGE`), described by four columns. `image:attachment doc:attachment?` generates:

```sql
image_key TEXT NOT NULL,
image_filename TEXT NOT NULL,
image_content_type TEXT NOT NULL,
image_size INTEGER NOT NULL,
doc_key TEXT,
doc_filename TEXT,
doc_content_type TEXT,
doc_size INTEGER,
```

```rust
/// Files `image` accepts; `validate()` checks each upload before anything is
/// stored. The whole request is held in the Worker's memory (128 MB): keep
/// `max_bytes` modest, and forms add it to their request limit.
pub const IMAGE: Rules = Rules {
    max_bytes: 10 * 1024 * 1024,
    content_types: &["image/png", "image/jpeg", "image/gif", "image/webp", "application/pdf", "text/plain"],
};

// NewItem: set only by forms and the REST upload route, never by JSON
/// The file to store in R2 (checked against `IMAGE`). JSON cannot carry it.
#[serde(skip)]
pub image: Option<Upload>,

// ItemChanges, optional attachment
/// `Some(Some(file))` replaces the stored file, `Some(None)` removes it (the old one is deleted from R2).
#[serde(skip)]
pub doc: Option<Option<Upload>>,
```

- The record has the four columns plus `item.image()` returning an `ocre::storage::Attachment` (`item.doc()` returns `Option<Attachment>`).
- Validation: `v.file("image", image, &IMAGE)`, messages "is too large (maximum is 10 MB)" and "has an unsupported type (allowed: image/png, ...)". A required attachment also gets `v.check("image", self.image.is_none(), "can't be blank")` in `NewItem::validate`.
- Forms: `<input type="file" name="image" accept="image/png,image/jpeg,...">`, `enctype="multipart/form-data"`, a "Remove" checkbox for optional files on the edit page, and `GET /<plural>/{id}/<name>` to download.
- JSON APIs: attachments must be optional (`ocre g api Document file:attachment` fails with ``error: attachment `file` must be optional in a JSON API``); files are sent with `PUT /api/<plural>/{id}/<name>` as multipart. GraphQL exposes the four columns (`fileKey`, ...) read only.
- Restrictions: cannot be unique (``error: attachment `photo` cannot be unique``), cannot be named `edit`, `delete` or `new` (they clash with scaffold routes), and `<name>_key`, `_filename`, `_content_type`, `_size` cannot be other fields of the same model.

See [File storage](../guides/files.md) for the upload and serving flow.

## json

Any JSON value (object, array, string, number, boolean), stored as its text.

```sql
settings TEXT NOT NULL CHECK (json_valid(settings)),
meta TEXT CHECK (json_valid(meta)),
```

```rust
// Item (the record)
#[serde(deserialize_with = "ocre::json_from_sql")]
pub settings: ocre::serde_json::Value,
#[serde(deserialize_with = "ocre::optional_json_from_sql")]
pub meta: Option<ocre::serde_json::Value>,

// NewItem
pub settings: ocre::serde_json::Value,
#[serde(default)]
pub meta: Option<ocre::serde_json::Value>,

// ItemChanges
pub settings: Option<ocre::serde_json::Value>,
#[serde(default, deserialize_with = "ocre::patch_json")]
pub meta: Option<Option<ocre::serde_json::Value>>,
```

- A `serde_json::Value` binds as its compact JSON text with `params![value]`; D1 returns the text, which `json_from_sql` parses back.
- JSON APIs take and return the value itself: `"settings": {"color": "red", "sizes": [1, 2]}` is answered as the same object, not as a string. For an optional field, a JSON string stays a string: `PATCH` with `"meta": "{}"` stores the string `"{}"`, not an object; `null` clears it.
- Forms: a `<textarea rows="5" spellcheck="false" placeholder="{}">` parsed with `v.json(..)`; text that does not parse (including an empty required field) shows "is not valid JSON". Optional fields use `v.optional_json(..)`: empty means `NULL`.
- GraphQL: the `JSON` scalar (async-graphql's scalar for `serde_json::Value`).
- Cannot be unique: `data:json^` fails with ``hint: a unique index compares JSON text, where key order and spacing differ; drop the `^` ``.
- Build values in Rust with `ocre::serde_json::json!({"color": "red"})`.

## Modifiers

| Modifier | SQL | Rust | Validation | Not allowed for |
|---|---|---|---|---|
| none | `NOT NULL` | `T` | the type's checks, "can't be blank" for `string`/`text`/`attachment` | |
| `?` | nullable column | `Option<T>`, `Option<Option<T>>` in `Changes` | the type's checks when a value is present | `boolean` |
| `^` | `CREATE UNIQUE INDEX index_<table>_on_<column> ON <table> (<column>)` | same | "has already been taken" in `create` and `update` (one `SELECT 1 ... LIMIT 1` per unique field) | `attachment`, `json` |
| `?^` / `^?` | nullable column with a unique index | `Option<T>` | uniqueness checked only when a value is given | `boolean`, `attachment`, `json` |

The index protects against races between two concurrent requests: the check in `create` gives the friendly message, the index guarantees the rule.

## Adding a field to an existing table

`ocre g migration add_<columns>_to_<table> field:type...` uses the same field syntax, with defaults for existing rows (run on a table `items`):

```sh
ocre g migration add_extras_to_items slug:string^ level:integer rating:float? done:boolean due:date data:json extra:json? photo:attachment? owner:references?
```

```sql
-- Migration: add_extras_to_items
-- Applied once, in file-name order. Never edit a migration after it has been applied.
ALTER TABLE items ADD COLUMN slug TEXT NOT NULL DEFAULT '';
ALTER TABLE items ADD COLUMN level INTEGER NOT NULL DEFAULT 0;
ALTER TABLE items ADD COLUMN rating REAL;
ALTER TABLE items ADD COLUMN done INTEGER NOT NULL DEFAULT 0;
ALTER TABLE items ADD COLUMN due TEXT NOT NULL DEFAULT '';
ALTER TABLE items ADD COLUMN data TEXT NOT NULL CHECK (json_valid(data)) DEFAULT '{}';
ALTER TABLE items ADD COLUMN extra TEXT CHECK (json_valid(extra));
ALTER TABLE items ADD COLUMN photo_key TEXT;
ALTER TABLE items ADD COLUMN photo_filename TEXT;
ALTER TABLE items ADD COLUMN photo_content_type TEXT;
ALTER TABLE items ADD COLUMN photo_size INTEGER;
ALTER TABLE items ADD COLUMN owner_id INTEGER REFERENCES owners(id) ON DELETE CASCADE;
CREATE UNIQUE INDEX index_items_on_slug ON items (slug);
CREATE INDEX index_items_on_owner_id ON items (owner_id);
```

- Required text types get `DEFAULT ''`, numbers `DEFAULT 0`, `json` `DEFAULT '{}'`; optional columns get no default. A required unique column (`slug:string^`) gives every existing row the same `''`, so creating the unique index fails (`UNIQUE constraint failed: items.slug`) as soon as the table has two rows: add it as `slug:string?^` and fill it afterwards.
- `references` and `attachment` must be optional here: ``error: `owner_id` must be optional when added to an existing table`` (``hint: SQLite adds reference columns as NULL for existing rows: use `name:references?` ``), and ``error: `pic` must be optional when added to an existing table`` (``hint: existing rows have no file: use `name:attachment?` ``).
- The migration generator does not check that the referenced model exists, and does not change the model: update the struct, `New<Model>`, `<Model>Changes`, `validate()` and the SQL of `create`/`update` yourself (the command says so in its `Next:` lines).

## Names that are refused

Each refused field stops the generator before it writes anything. The inputs and the exact messages (`ocre g model Thing <field>`):

```text
title
error: field `title` has no type
hint: write fields as `name:type`, e.g. `title:string`

Title:string
error: invalid field name `Title`
hint: use snake_case starting with a letter, e.g. `published_at`

title:varchar
error: unknown field type `varchar` for `title`
hint: types: string, text, integer, float, boolean, date, datetime, references, attachment, json; add `?` for optional, `^` for unique

type:string
error: field name `type` is reserved
hint: `id`, `created_at` and `updated_at` are generated; Rust and SQL keywords are not allowed. Pick another name, e.g. `kind` for `type`

a:string b:string a:text
error: field `a` is listed twice
hint: names must differ, and `<name>:attachment` also takes `<name>_key`, `<name>_filename`, `<name>_content_type` and `<name>_size`

avatar:attachment avatar_key:string
error: field `avatar_key` is listed twice
hint: names must differ, and `<name>:attachment` also takes `<name>_key`, `<name>_filename`, `<name>_content_type` and `<name>_size`

edit:attachment
error: attachment name `edit` clashes with a scaffold route
hint: `/<plural>/{id}/edit` is taken; pick another name, e.g. `edit_file`

flag:boolean?
error: boolean field `flag` cannot be optional
hint: booleans are true or false (a checkbox); drop the `?`

photo:attachment^
error: attachment `photo` cannot be unique
hint: every stored file gets its own random key already; drop the `^`

data:json^
error: json field `data` cannot be unique
hint: a unique index compares JSON text, where key order and spacing differ; drop the `^`
```

Reserved names (from `crates/ocre-cli/src/generate/fields.rs`):

- Generated columns: `id`, `created_at`, `updated_at`.
- Rust keywords: `as`, `async`, `await`, `box`, `break`, `const`, `continue`, `crate`, `dyn`, `else`, `enum`, `extern`, `false`, `fn`, `for`, `gen`, `if`, `impl`, `in`, `let`, `loop`, `match`, `mod`, `move`, `mut`, `pub`, `ref`, `return`, `self`, `static`, `struct`, `super`, `trait`, `true`, `try`, `type`, `unsafe`, `use`, `where`, `while`, `yield`.
- SQL keywords: `and`, `asc`, `by`, `case`, `check`, `default`, `desc`, `from`, `group`, `index`, `join`, `key`, `limit`, `not`, `null`, `offset`, `or`, `order`, `primary`, `references`, `select`, `table`, `unique`, `values`.

With `--json`, the same failure is one object on stdout, for example `ocre g model Thing x:strin --json`:

```json
{"error":"unknown field type `strin` for `x`","hint":"types: string, text, integer, float, boolean, date, datetime, references, attachment, json; add `?` for optional, `^` for unique","ok":false}
```

## What a JSON API returns

`ocre g api Gadget name:string^ label:string? qty:integer stock:integer? price:float active:boolean born_on:date starts_at:datetime? author:references? file:attachment? settings:json meta:json? --graphql`, then (with an author 1 in the database):

```sh
curl -s -X POST localhost:8787/api/gadgets -H 'content-type: application/json' \
  -d '{"name":"Lamp","qty":3,"price":19.5,"active":true,"born_on":"2026-09-29","starts_at":"2026-09-29T18:30","author_id":1,"settings":{"color":"red","sizes":[1,2]}}'
```

```json
{"id":1,"name":"Lamp","label":null,"qty":3,"stock":null,"price":19.5,"active":true,"born_on":"2026-09-29","starts_at":"2026-09-29T18:30","author_id":1,"file_key":null,"file_filename":null,"file_content_type":null,"file_size":null,"settings":{"color":"red","sizes":[1,2]},"meta":null,"created_at":"2026-09-29 04:20:26","updated_at":"2026-09-29 04:20:26"}
```

Invalid values are all reported at once with status 422:

```sh
curl -s -X POST localhost:8787/api/gadgets -H 'content-type: application/json' \
  -d '{"name":"","qty":9007199254740992,"price":1,"born_on":"2026-02-30","starts_at":"2026-09-29","author_id":42,"settings":1}'
```

```json
{"error":{"fields":{"author_id":["must exist"],"born_on":["is not a valid date"],"name":["can't be blank"],"qty":["must be less than or equal to 9007199254740991"],"starts_at":["is not a valid date and time"]},"message":"Validation failed","status":422}}
```

Wrong JSON types and missing required keys fail earlier, with a 400 from the JSON extractor:

```json
{"error":{"message":"Failed to deserialize the JSON body into the target type: qty: invalid type: string \"3\", expected i64 at line 1 column 21","status":400}}
{"error":{"message":"Failed to deserialize the JSON body into the target type: missing field `settings` at line 1 column 53","status":400}}
```

On GraphQL, field names are camelCase and optional fields are nullable (`label: String`, `stock: Int`), required ones non-null (`qty: Int!`, `price: Float!`, `active: Boolean!`, `bornOn: String!`). Patch inputs use `MaybeUndefined` for optional fields, so an omitted field keeps its value and `null` clears it:

```json
{"data":{"gadget":{"name":"Lamp","qty":3,"price":19.5,"active":true,"bornOn":"2026-09-29","startsAt":"2026-09-29T18:30","authorId":1,"fileKey":null,"settings":{"color":"red","sizes":[1,2]},"meta":"{}"}}}
```

## Writing the attributes by hand

When you add a column with a migration, copy the attributes of the generated code. A complete module with a boolean, a JSON column, an optional JSON column and patch fields:

```rust,check
// src/models/preference.rs
use ocre::serde_json::Value;
use serde::{Deserialize, Serialize};

/// A row of a `preferences` table: `enabled INTEGER NOT NULL DEFAULT 0`,
/// `data TEXT NOT NULL CHECK (json_valid(data))`, `extra TEXT CHECK (json_valid(extra))`,
/// `note TEXT`, `level INTEGER`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Preference {
    pub id: i64,
    #[serde(deserialize_with = "ocre::bool_from_sql")]
    pub enabled: bool,
    #[serde(deserialize_with = "ocre::json_from_sql")]
    pub data: Value,
    #[serde(deserialize_with = "ocre::optional_json_from_sql")]
    pub extra: Option<Value>,
    pub note: Option<String>,
    pub level: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Values for a new preference.
#[derive(Debug, Clone, Deserialize)]
pub struct NewPreference {
    #[serde(default)]
    pub enabled: bool,
    pub data: Value,
    #[serde(default)]
    pub extra: Option<Value>,
    #[serde(default, deserialize_with = "ocre::optional")]
    pub note: Option<String>,
    #[serde(default, deserialize_with = "ocre::optional")]
    pub level: Option<i64>,
}

/// Changes: a missing key keeps the value, `null` clears an optional one.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PreferenceChanges {
    pub enabled: Option<bool>,
    pub data: Option<Value>,
    #[serde(default, deserialize_with = "ocre::patch_json")]
    pub extra: Option<Option<Value>>,
    #[serde(default, deserialize_with = "ocre::patch")]
    pub note: Option<Option<String>>,
    #[serde(default, deserialize_with = "ocre::patch")]
    pub level: Option<Option<i64>>,
}

impl NewPreference {
    /// The checks the generators write for `level:integer?`.
    pub fn validate(&self) -> ocre::Validator {
        let mut v = ocre::Validator::new();
        if let Some(level) = self.level {
            v.safe_integer("level", level);
        }
        v
    }
}
```

## See also

- [Models and migrations](../guides/models.md): the generated model, queries and associations.
- [Validations](../guides/validations.md): every `Validator` rule and message.
- [Generators](generators.md#ocre-g-model): `ocre g model`, [`ocre g scaffold`](generators.md#ocre-g-scaffold), [`ocre g api`](generators.md#ocre-g-api), [`ocre g migration`](generators.md#ocre-g-migration).
- [File storage](../guides/files.md): attachments.
- [JSON APIs and GraphQL](../guides/json-apis.md): request and response formats.
- [`ocre::Validator`](/api/ocre/struct.Validator.html) and [`ocre::MAX_SAFE_INTEGER`](/api/ocre/constant.MAX_SAFE_INTEGER.html) in the rustdoc.
