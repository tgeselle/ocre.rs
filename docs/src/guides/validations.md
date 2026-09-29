# Validations

Validations check input before it reaches the database: `ocre::Validator` collects every failed rule with Rails' messages, and `finish()` turns them into `Error::Invalid`, a 422 that HTML forms show next to the typed values and JSON APIs report per field. This page lists every check, where rules belong, and what clients receive.

## Before you start

- An Ocre app from `ocre new`, with a model from `ocre g model`, `ocre g scaffold` or `ocre g api` (see [Models and migrations](models.md)). The examples use the blog starter (`ocre new blog --starter blog`), `ocre g scaffold Comment author:string body:text post:references` and `ocre g api Product name:string^ price:float stock:integer? --graphql`.
- Validations run in the Worker and cost a little CPU and no binding call, except the database checks (uniqueness, references), which each read about one row with the indexes the generators create.

## How a validation works

```rust,check
// src/models/signup_rules.rs
use ocre::{Result, Validator};

pub fn check_signup(name: &str, email: &str, age: i64) -> Result<()> {
    let mut v = Validator::new();
    v.required("name", name).max_length("name", name, 50);
    v.email("email", email);
    v.range("age", age, 13..=120);
    v.finish()
}
```

Each check records an error when it fails and returns `&mut Validator`, so checks chain. Nothing stops at the first failure: `finish()` returns `Ok(())` when every check passed, or `Err(Error::Invalid(errors))` with all of them. An error is an `ocre::FieldError { field, message }`: the message has no field name (`"can't be blank"`), and `full_message()` adds the humanized name (`"Name can't be blank"`; `post_id` becomes `Post`).

## Every check

Messages are the exact strings Ocre adds (Rails' wording).

| Method | Passes when | Message |
|---|---|---|
| `v.required("title", &title)` | not empty after trimming whitespace | `can't be blank` |
| `v.max_length("title", &title, 100)` | at most 100 characters (Unicode characters, not bytes) | `is too long (maximum is 100 characters)` |
| `v.min_length("code", &code, 6)` | at least 6 characters | `is too short (minimum is 6 characters)` |
| `v.range("guests", guests, 1..=12)` | within the inclusive range; any `PartialOrd + Display` type (`i64`, `f64`...) | `must be greater than or equal to 1` / `must be less than or equal to 12` |
| `v.safe_integer("stock", stock)` | within ±`ocre::MAX_SAFE_INTEGER` (2^53 - 1), what D1 returns exactly | `must be less than or equal to 9007199254740991` (or greater than or equal to the negative bound) |
| `v.inclusion("slot", &slot, &["lunch", "dinner"])` | one of the listed values | `is not included in the list` |
| `v.email("email", &email)` | one `@`, text on both sides, a dot in the domain, no spaces or `<>,` | `is invalid` |
| `v.date("date", &date)` | `YYYY-MM-DD`, a real calendar date (month lengths, leap years) | `is not a valid date` |
| `v.datetime("at", &at)` | `YYYY-MM-DD HH:MM[:SS]` with a space or `T` (what `<input type="datetime-local">` sends) | `is not a valid date and time` |
| `v.check("guests", failed, "message")` | `failed` is false: any rule of your own | your message |
| `v.number::<f64>("price", &text)` | the text parses as the target type; returns `Option<T>` | `is not a number` |
| `v.optional_number::<i64>("stock", &text)` | blank, or parses; blank returns `None` without an error | `is not a number` |
| `v.json("data", &text)` | the text is valid JSON; returns `Option<serde_json::Value>` | `is not valid JSON` |
| `v.optional_json("data", &text)` | blank, or valid JSON; blank returns `None` | `is not valid JSON` |
| `v.file("image", &upload, &IMAGE)` | the upload is within `Rules::max_bytes` and has an allowed content type | `is too large (maximum is 10 MB)` / `has an unsupported type (allowed: ...)` |

The other methods: `Validator::new()`, `v.merge(other)` (adds the errors another validator collected), `v.is_valid()` (no error so far) and `v.finish()`. File rules and `v.file` are covered in [File storage](files.md). The API reference is in the [rustdoc of `Validator`](/api/ocre/struct.Validator.html).

A complete set of rules, with a custom one, behind a JSON endpoint:

```rust,check
// src/reservations.rs
use axum::{Router, routing::post};
use ocre::{ApiResult, Created, Ctx, Json, Validator};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/api/reservations", post(create))
}

const SLOTS: &[&str] = &["lunch", "dinner"];

#[derive(Debug, Deserialize, Serialize)]
pub struct NewReservation {
    pub name: String,
    pub email: String,
    pub guests: i64,
    pub slot: String,
    /// `YYYY-MM-DD`
    pub date: String,
    /// `YYYY-MM-DD HH:MM`, optional
    #[serde(default)]
    pub arrives_at: Option<String>,
    #[serde(default)]
    pub promo_code: Option<String>,
    #[serde(default)]
    pub deposit_cents: Option<i64>,
}

impl NewReservation {
    pub fn validate(&self) -> Validator {
        let mut v = Validator::new();
        v.required("name", &self.name).max_length("name", &self.name, 100);
        v.email("email", &self.email);
        v.range("guests", self.guests, 1..=12);
        v.inclusion("slot", &self.slot, SLOTS);
        v.date("date", &self.date);
        if let Some(arrives_at) = &self.arrives_at {
            v.datetime("arrives_at", arrives_at);
        }
        if let Some(code) = &self.promo_code {
            v.min_length("promo_code", code, 6);
        }
        if let Some(cents) = self.deposit_cents {
            v.safe_integer("deposit_cents", cents);
        }
        // A rule of your own: any condition plus a message.
        v.check("guests", self.slot == "lunch" && self.guests > 8, "must be 8 or fewer at lunch");
        v
    }
}

async fn create(Json(new): Json<NewReservation>) -> ApiResult<Created<NewReservation>> {
    new.validate().finish()?;
    Ok(Created(new))
}
```

Registered in `src/lib.rs` (`mod reservations;` under `// ocre:modules`, `.merge(reservations::routes())` under `// ocre:routes`) and called while `ocre dev` runs:

```sh
curl -s -X POST http://localhost:8787/api/reservations -H 'Content-Type: application/json' \
  -d '{"name": "", "email": "ada@", "guests": 10, "slot": "lunch", "date": "2026-02-30", "arrives_at": "2026-02-01 25:00", "promo_code": "abc", "deposit_cents": 9007199254740992}'
```

```json
{"error":{"fields":{"arrives_at":["is not a valid date and time"],"date":["is not a valid date"],"deposit_cents":["must be less than or equal to 9007199254740991"],"email":["is invalid"],"guests":["must be 8 or fewer at lunch"],"name":["can't be blank"],"promo_code":["is too short (minimum is 6 characters)"]},"message":"Validation failed","status":422}}
```

Status 422, every failed field at once. The other messages:

```sh
curl -s -X POST http://localhost:8787/api/reservations -H 'Content-Type: application/json' \
  -d '{"name": "Ada", "email": "ada@example.com", "guests": 2, "slot": "brunch", "date": "2026-10-01"}'
```

```json
{"error":{"fields":{"slot":["is not included in the list"]},"message":"Validation failed","status":422}}
```

With a 101-character name and `"guests": 0`:

```json
{"error":{"fields":{"guests":["must be greater than or equal to 1"],"name":["is too long (maximum is 100 characters)"]},"message":"Validation failed","status":422}}
```

A valid body is echoed with `201 Created`:

```sh
curl -s -X POST http://localhost:8787/api/reservations -H 'Content-Type: application/json' \
  -d '{"name": "Ada", "email": "ada@example.com", "guests": 2, "slot": "dinner", "date": "2026-10-01", "arrives_at": "2026-10-01T19:30"}'
```

```json
{"name":"Ada","email":"ada@example.com","guests":2,"slot":"dinner","date":"2026-10-01","arrives_at":"2026-10-01T19:30","promo_code":null,"deposit_cents":null}
```

A body that does not deserialize (a missing required field, a string where a number is expected, invalid JSON) never reaches `validate()`: `ocre::Json` answers 400 with serde's explanation (see [JSON APIs](json-apis.md#errors)).

## Where rules live

Data rules belong in the model, `src/models/<model>.rs`, so the HTML pages, the JSON API and GraphQL enforce the same ones:

| Rule | Where | Generated for |
|---|---|---|
| Checks on the values alone (presence, length, format, ranges, custom rules) | `New<Model>::validate()` and `<Model>Changes::validate()` | `required` (non-optional `string`/`text`), `date`, `datetime`, `safe_integer` (`integer`), `v.file` (`attachment`) |
| Uniqueness: `has already been taken` | `create` and `update`, with `db.exists(..)` | fields marked `^` |
| Reference: `must exist` | `create` and `update`, with `db.exists(..)` | `references` fields |

A generated `create`, for `Comment` (`post:references`):

```rust
pub async fn create(ctx: &Ctx, new: NewComment) -> Result<Comment> {
    let db = ctx.db()?;
    let mut v = new.validate();
    {
        let post_id = &new.post_id;
        v.check("post_id", !db.exists("SELECT 1 FROM posts WHERE id = ?1 LIMIT 1", params![*post_id]).await?, "must exist");
    }
    v.finish()?;
    db.first("INSERT INTO comments (author, body, post_id) VALUES (?1, ?2, ?3) RETURNING *", params![new.author, new.body, new.post_id])
        .await?
        .ok_or_else(|| Error::internal("INSERT ... RETURNING returned no row"))
}
```

The database checks add to the `validate()` errors, so a JSON client gets both kinds in one answer:

```sh
curl -s -X POST http://localhost:8787/api/products -H 'Content-Type: application/json' \
  -d '{"name": "Teapot", "price": 3, "stock": 9007199254740992}'
```

```json
{"error":{"fields":{"name":["has already been taken"],"stock":["must be less than or equal to 9007199254740991"]},"message":"Validation failed","status":422}}
```

In `update`, the uniqueness check excludes the record itself (`AND id != ?2`): renaming product 3 to an existing name is refused, saving product 1 with its own name is not. `update` checks only the fields being changed.

Add rules to `validate()`; add database rules to `create`/`update` next to the generated ones, with `v.check(field, db.exists(sql, params).await?, message)`. Keep the `UNIQUE` index and `REFERENCES` constraint in the migration too: two requests at the same moment can both pass the check, and the constraint then fails the second insert with a 500.

## What clients receive

`Error::Invalid` has status 422. How it is shown depends on the handler.

### HTML forms re-render

The scaffold's `create` and `update` handlers (`src/<plural>.rs`) match `Error::Invalid` and render the form again with the errors and the values as typed:

```rust
        Err(Error::Invalid(errors)) => {
            Ok((StatusCode::UNPROCESSABLE_ENTITY, render(&NewView { form, errors })?).into_response())
        }
```

`templates/<plural>/_form.html` lists them with `full_message()`:

```html
{% if !errors.is_empty() %}
  <ul class="errors">
    {% for error in errors %}<li>{{ error.full_message() }}</li>{% endfor %}
  </ul>
{% endif %}
```

```sh
curl -s -X POST http://localhost:8787/comments -d 'author=Ada&body=Nice&post_id=99'
```

```html
...
<form action="/comments" method="post">

  <ul class="errors">
    <li>Post must exist</li>
  </ul>

  <label>Author <input name="author" value="Ada" required></label>
  <label>Body <textarea name="body" rows="5" required>Nice</textarea></label>
  <label>Post <input type="number" step="1" name="post_id" value="99" required></label>
  <button type="submit">Create comment</button>
</form>
...
```

The form struct keeps every field as text (`PostForm`, `CommentForm`), so a typo in a number is a field error rather than a failed request: `to_new()` parses with `v.number(..)`, then merges the model's `validate()`. The form's own checks run first; the database checks run in `create` only when they pass, so a form with a blank body and an unknown post shows "Body can't be blank" first, then "Post must exist" on the next submit.

The same pattern, written by hand for the API's `Product` model (`price:float stock:integer?`) plus a free-form JSON field:

```rust,check
// src/product_form.rs
use ocre::{Result, Validator, serde_json::Value};
use serde::Deserialize;

use crate::models::product::NewProduct;

/// What an HTML form sends: every field is text.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProductForm {
    pub name: String,
    pub price: String,
    pub stock: String,
    pub specs: String,
}

impl ProductForm {
    /// Parses the text, then runs the model's rules, so the form shows every
    /// error at once.
    pub fn to_new(&self) -> Result<(NewProduct, Option<Value>)> {
        let mut v = Validator::new();
        let new = NewProduct {
            name: self.name.clone(),
            price: v.number("price", &self.price).unwrap_or_default(),
            stock: v.optional_number("stock", &self.stock),
        };
        let specs = v.optional_json("specs", &self.specs);
        v.merge(new.validate()).finish()?;
        Ok((new, specs))
    }
}
```

Posted as `name=&price=abc&stock=12x&specs={bad` to a JSON handler that calls `form.to_new()?`, it answers:

```json
{"error":{"fields":{"name":["can't be blank"],"price":["is not a number"],"specs":["is not valid JSON"],"stock":["is not a number"]},"message":"Validation failed","status":422}}
```

and `name=Pan&price=12.5&stock=&specs={"size":"L"}` parses to `price` 12.5, `stock` `None` and `specs` `{"size":"L"}`.

An HTML handler that returns `Error::Invalid` with `?` instead of re-rendering gets Ocre's plain error page, status 422:

```html
<h1>422</h1><p>Validation failed</p><ul><li>Title can&#39;t be blank</li><li>Contact email is invalid</li></ul>
```

### JSON APIs report fields

Handlers returning `ocre::ApiResult` turn `Error::Invalid` into a 422 whose `fields` object maps each field to its messages, as shown above. The same shape comes back from `create` and `update` in every generated JSON API (`ocre g api`); see [JSON APIs](json-apis.md#errors).

### GraphQL puts fields in extensions

GraphQL resolvers convert the error with `?`; the response is a 200 with the error in `errors`, the status and fields in `extensions`:

```sh
curl -s -X POST http://localhost:8787/graphql -H 'Content-Type: application/json' \
  -d '{"query": "mutation { createProduct(input: {name: \"\", price: 30}) { id } }"}'
```

```json
{"data":null,"errors":[{"message":"Validation failed","locations":[{"line":1,"column":12}],"path":["createProduct"],"extensions":{"fields":{"name":["can't be blank"]},"status":422}}]}
```

## Custom rules

`v.check(field, failed, message)` covers any rule: pass the condition that means failure and a message without the field name. Cross-field rules, rules on the current time and rules needing the database all use it:

```rust,check
// src/models/event_rules.rs
use ocre::{Ctx, Result, Validator, params};

/// `starts_on` and `ends_on` are `YYYY-MM-DD`, so text order is date order.
pub fn check_dates(v: &mut Validator, starts_on: &str, ends_on: &str) {
    let mut dates = Validator::new();
    dates.date("starts_on", starts_on).date("ends_on", ends_on);
    let both_valid = dates.is_valid();
    v.merge(dates);
    v.check("ends_on", both_valid && ends_on < starts_on, "must be on or after the start date");
}

/// Database rule: at most 20 comments per post.
pub async fn check_comment_quota(ctx: &Ctx, post_id: i64) -> Result<()> {
    let full = ctx
        .db()?
        .exists("SELECT 1 FROM comments WHERE post_id = ?1 LIMIT 1 OFFSET 19", params![post_id])
        .await?;
    let mut v = Validator::new();
    v.check("post_id", full, "has too many comments");
    v.finish()
}
```

The separate `dates` validator tells whether both dates parsed (`is_valid()`), so a malformed date reports one error, not two; `merge` then adds its errors to `v`. Call such functions from the model's `validate()` (checks without the database) or `create`/`update` (database checks), before `finish()`.

## See also

- [Models and migrations](models.md): the generated `validate()`, `create` and `update`
- [Controllers, routing, views and htmx](controllers.md): the scaffold's form handling
- [JSON APIs and GraphQL](json-apis.md): error JSON, 400 versus 422
- [File storage](files.md): `Rules` and `v.file` for uploads
- [Field types](../reference/field-types.md): which checks each type gets
- [API index](../api-index.md) and the [rustdoc of `Validator`](/api/ocre/struct.Validator.html)
