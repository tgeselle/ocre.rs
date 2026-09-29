# Views, helpers and forms

Views in Ocre are [askama](https://askama.readthedocs.io/) templates compiled into the Worker: a template is a Rust struct whose fields are the template's variables, so a typo in a variable name or a missing value is a compile error, not a blank page in production. This page covers layouts and partials, the view helpers (numbers, dates, text), forms with validation errors, and responses in other formats (XML, CSV, plain text). Interactivity with htmx has its own page, [htmx](htmx.md).

## Before you start

- A full-stack app from `ocre new` (API-only apps have no templates). The examples use the blog starter (`ocre new blog --starter blog`), whose `Post` has `title`, `body`, `published`, `created_at` and `updated_at`.
- Rendering is plain string building inside the Worker: a normal page takes a fraction of a millisecond of the free plan's 10 ms of CPU per request, and costs no binding call. Templates are compiled in, so `ocre dev` rebuilds the Worker to show a change (save a `.rs` file under `src/` if only a template changed).

## Templates

A view is a struct deriving `askama::Template`, rendered with `ocre::render`, which returns `Result<Html<String>>`:

```rust
#[derive(Template)]
#[template(path = "posts/show.html")]
struct ShowView {
    flash: Flash,
    post: Post,
}

async fn show(State(ctx): State<Ctx>, flash: Flash, Path(id): Path<i64>) -> Result<Html<String>> {
    render(&ShowView { flash, post: post::find(&ctx, id).await?.or_404()? })
}
```

`path = "posts/show.html"` is a file under `templates/`. Short templates can live in the Rust file with `source = "..."` and `ext = "html"` (the extension decides escaping: `html` escapes, `txt` does not). Rails' conventions map to askama as follows:

| Rails | Ocre (askama) |
|---|---|
| ERB `<%= %>` / `<% %>` | `{{ expression }}` / `{% statement %}`; values are HTML-escaped |
| Implicit rendering of `show.html.erb` | explicit: `render(&ShowView { .. })`; the struct names the file |
| `render "other_action"`, `render template:` | render another view struct |
| `render inline:` | `#[template(source = "...", ext = "html")]` |
| `render plain:` / `html:` / `body:` | return a `&str`/`String`, `Html(..)`, or `([(header::CONTENT_TYPE, "...")], body)` |
| `render json:` | `ocre::Json(value)` (see [JSON APIs](json-apis.md)) |
| `render xml:`, Builder templates, `atom_feed` | a template with `ext = "xml"` (below) |
| `render status: :unprocessable_entity` | `(StatusCode::UNPROCESSABLE_ENTITY, render(&view)?)` |
| `head :no_content` | return `StatusCode::NO_CONTENT` |
| `DoubleRenderError` | impossible: a handler returns one response |
| `local_assigns`, strict locals | struct fields; optional ones are `Option<T>` (`{% if let Some(x) = x %}`) |
| Template compilation and caching | done by `cargo build`: templates are Rust code in the Worker |
| `render file:` | not available (no filesystem on Workers): put the file in `public/` or R2 |

## Layouts

`templates/layout.html` is the application layout. Pages extend it and fill its blocks, the equivalent of `yield` and `content_for`:

```html
{% extends "layout.html" %}

{% block title %}Post {{ post.id }}{% endblock %}

{% block content %}
<h1>{{ post.title }}</h1>
{% endblock %}
```

A block the page does not fill keeps the layout's default (`{% block title %}blog{% endblock %}` in the layout). Add named regions (Rails' `yield :sidebar`) by adding blocks to the layout: `{% block sidebar %}{% endblock %}`. A section layout (Rails' nested layouts) is a template that extends `layout.html` and defines new blocks for its pages:

```html
{# templates/admin/layout.html #}
{% extends "layout.html" %}
{% block content %}
<nav><a href="/admin/posts">Posts</a> · <a href="/admin/users">Users</a></nav>
{% block admin %}{% endblock %}
{% endblock %}
```

Admin pages then `{% extends "admin/layout.html" %}` and fill `admin`. A page that needs no layout (an email, a fragment for htmx) simply does not extend one.

`templates/error.html` is the page for errors (404, 422, 500...); see [Controllers](controllers.md#error-pages).

## Partials and components

`{% include "posts/_form.html" %}` inserts another template, which sees the including template's variables (Rails' partials; the scaffold's `new.html` and `edit.html` share `_form.html` this way). To give a partial a differently named variable (Rails' `locals:` and `as:`), bind it first with `{% let %}`:

```html
{% for comment in post_comments %}
  {% let item = comment %}
  {% include "comments/_comment.html" %}
{% endfor %}
```

Macros are components with arguments (Rails' partials with locals, view components):

```html
{% macro field(name, label, value, errors) %}
<label>{{ label }} <input name="{{ name }}" value="{{ value }}"></label>
{% for error in errors %}{% if error.field == name %}<span class="errors">{{ error.message }}</span>{% endif %}{% endfor %}
{% endmacro %}

{% call field("title", "Title", form.title, errors) %}{% endcall %}
```

`{% call %}` needs its `{% endcall %}`; anything between the two is the caller body, which the macro prints with `{{ caller() }}` (a block component, like a Rails partial rendered with a block). Macros defined in another file are imported with `{% import "forms.html" as forms %}` and called with `{% call forms::field(...) %}{% endcall %}`.

## Collections

| Rails | askama |
|---|---|
| `render @posts` / `collection:` | `{% for post in posts %}...{% endfor %}` |
| Empty-collection fallback | `{% for %}...{% else %}No posts yet.{% endfor %}` |
| `post_counter`, `post_iteration.first?`/`last?` | `loop.index` (from 1), `loop.index0`, `loop.first`, `loop.last` |
| `spacer_template:` | `{% if !loop.last %}<hr>{% endif %}` |
| Heterogeneous collections | an `enum` and `{% match item %}{% when Item::Post with (post) %}...{% endmatch %}` |
| Partial layouts | wrap the loop body in a macro |

```rust,check
use askama::Template;
use axum::{extract::State, response::Html};
use ocre::{Ctx, Page, Result, filters, render};

use crate::models::post::{self, Post};

#[derive(Template)]
#[template(
    source = r#"{% extends "layout.html" %}
{% block content %}
<ol>
{% for post in posts %}
  <li class="{{ ocre::helpers::class_names([("post", true), ("draft", !post.published)]) }}">
    <a href="/posts/{{ post.id }}">{{ post.title|truncate(60) }}</a>
    · {{ post.body|wordcount }} words · {{ post.created_at|time_ago_in_words }} ago
  </li>
  {% if !loop.last %}<hr>{% endif %}
{% else %}
  <li>No posts yet.</li>
{% endfor %}
</ol>
{% endblock %}"#,
    ext = "html"
)]
struct IndexView {
    posts: Vec<Post>,
}

async fn index(State(ctx): State<Ctx>, page: Page) -> Result<Html<String>> {
    render(&IndexView { posts: post::all(&ctx, page).await? })
}
```

## View helpers

`ocre::helpers` has Rails' formatting helpers as plain functions, and `ocre::filters` exposes them to templates as askama filters. askama finds custom filters in a module named `filters` where the template struct is defined, so a controller brings them in with `use ocre::filters;` (as the example above does):

| Filter | Output | Rails |
|---|---|---|
| `{{ views\|number_with_delimiter }}` | `1,234,567` | `number_with_delimiter` |
| `{{ ratio\|number_with_precision(2) }}` | `3.14` | `number_with_precision` |
| `{{ price\|number_to_currency("$") }}` | `$1,234.50` | `number_to_currency` |
| `{{ rate\|number_to_percentage(1) }}` | `12.3%` | `number_to_percentage` |
| `{{ size\|number_to_human_size }}` | `1.5 KB` | `number_to_human_size` |
| `{{ visits\|number_to_human }}` | `1.23 Million` | `number_to_human` |
| `{{ post.created_at\|time_ago_in_words }} ago` | `about 3 hours ago` | `time_ago_in_words` |
| `{{ from\|distance_of_time_in_words(to) }}` | `2 days` | `distance_of_time_in_words` |
| `{{ post.created_at\|strftime("%b %-d, %Y") }}` | `Sep 29, 2026` | `l` / `to_fs` |
| `{{ post.body\|excerpt(q, 40) }}` | `...text around q...` | `excerpt` |
| `{{ post.body\|highlight(q) }}` | the text with each match in `<mark>` | `highlight` |
| `{{ post.body\|word_wrap(72) }}` | lines of at most 72 characters | `word_wrap` |
| `{{ ocre::helpers::class_names([("active", current)]) }}` | `active` when `current` | `class_names` / `token_list` |

askama's own filters cover the rest: `truncate(n)`, `wordcount`, `linebreaks` / `linebreaksbr` / `paragraphbreaks` (`simple_format`), `pluralize` (`{{ n }} post{{ n|pluralize }}`), `filesizeformat`, `urlencode`, `upper`, `lower`, `title`, `capitalize`, `json`, `fmt` (`{{ ratio|fmt("{:.2}") }}`) and `format` (`{{ "{:?}"|format(value) }}`, Rails' `debug` inside a `<pre>`). Time filters read Unix seconds or the `TEXT` timestamps D1 stores (`2026-09-29 14:05:00`, UTC); number filters read any number. Other text passes through unchanged. They are English; translated text comes from [Translations](i18n.md).

Your own helpers (Rails' `app/helpers`) are either methods on the view struct, called as `{{ self.method() }}` or `{{ method() }}`, or filters. To add filters, make the `filters` module yours and re-export Ocre's:

```rust,check
use askama::Template;

mod filters {
    pub use ocre::filters::*;

    /// `{{ post.title|shout }}`: `HELLO!`.
    #[askama::filter_fn]
    pub fn shout(value: impl std::fmt::Display, _: &dyn askama::Values) -> askama::Result<String> {
        Ok(format!("{}!", value.to_string().to_uppercase()))
    }
}

#[derive(Template)]
#[template(source = "<h1>{{ title|shout }}</h1><p>{{ views|number_with_delimiter }} views</p>", ext = "html")]
struct Banner {
    title: String,
    views: i64,
}

impl Banner {
    /// A helper method, called as `{{ reading_time() }}` in the template.
    fn reading_time(&self) -> String {
        format!("{} min", self.views / 200 + 1)
    }
}
```

Links and buttons are HTML: `<a href="{{ paths::show(post.id) }}">` (the scaffold's [path helpers](controllers.md#path-helpers) replace `link_to` and `url_for`), a `<form method="post">` with a button replaces `button_to`, `<a href="mailto:{{ user.email }}">` replaces `mail_to`. There is no tag builder: templates are HTML. `sanitize` and `strip_tags` for user-supplied HTML are in `ocre::security` (see [Security](security.md)).

## Forms

A form posts to a handler that deserializes it into a struct with axum's `Form` extractor. The scaffold's pattern keeps every field as text until it is validated, so a typo in a number becomes a field error, and renders the form again with status 422 and the typed values when validation fails:

```rust,check
use askama::Template;
use axum::{
    Form, Router,
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use ocre::{FieldError, Result, Validator, render};
use serde::Deserialize;

pub fn routes() -> Router<ocre::Ctx> {
    Router::new().route("/newsletter", get(new).post(create))
}

/// What the form submits. `#[serde(default)]`: a missing field is empty and
/// an unchecked checkbox is `false`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SignupForm {
    email: String,
    frequency: String,
    age: String,
    terms: bool,
}

#[derive(Template)]
#[template(
    source = r#"{% extends "layout.html" %}
{% macro errors_for(errors, name) %}{% for error in errors %}{% if error.field == name %}<small class="errors">{{ error.message }}</small>{% endif %}{% endfor %}{% endmacro %}
{% block content %}
<form action="/newsletter" method="post">
  <label>Email <input type="email" name="email" value="{{ form.email }}" required></label>
  {% call errors_for(errors, "email") %}{% endcall %}
  <label>Frequency
    <select name="frequency">
      {% for (value, label) in self.frequencies() %}
      <option value="{{ value }}"{% if self.chosen(value) %} selected{% endif %}>{{ label }}</option>
      {% endfor %}
    </select>
  </label>
  <label>Age <input type="number" name="age" value="{{ form.age }}"></label>
  {% call errors_for(errors, "age") %}{% endcall %}
  <label><input type="checkbox" name="terms" value="true"{% if form.terms %} checked{% endif %}> I accept the terms</label>
  {% call errors_for(errors, "terms") %}{% endcall %}
  <button type="submit">Subscribe</button>
</form>
{% endblock %}"#,
    ext = "html"
)]
struct SignupView {
    form: SignupForm,
    errors: Vec<FieldError>,
}

impl SignupView {
    /// The choices of the select (a helper method of the view).
    fn frequencies(&self) -> [(&'static str, &'static str); 2] {
        [("weekly", "Every week"), ("monthly", "Every month")]
    }

    /// Whether `value` is the submitted frequency.
    fn chosen(&self, value: &str) -> bool {
        self.form.frequency == value
    }
}

async fn new() -> Result<Response> {
    Ok(render(&SignupView { form: SignupForm::default(), errors: vec![] })?.into_response())
}

async fn create(Form(form): Form<SignupForm>) -> Result<Response> {
    let mut v = Validator::new();
    v.email("email", &form.email);
    v.inclusion("frequency", &form.frequency, &["weekly", "monthly"]);
    let age: Option<i64> = v.optional_number("age", &form.age);
    if let Some(age) = age {
        v.greater_than_or_equal_to("age", age, 13);
    }
    v.acceptance("terms", form.terms);
    if !v.is_valid() {
        let errors = v.errors().to_vec();
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, render(&SignupView { form, errors })?).into_response());
    }
    // ...store the subscription...
    Ok(Redirect::to("/").into_response())
}
```

Rails' form helpers map to HTML inputs:

| Rails | HTML in an Ocre template |
|---|---|
| `form_with url:` / `model:` | `<form action="{{ paths::index() }}" method="post">`; the edit form posts to `{{ paths::show(id) }}` (record identification: the scaffold generates both pages) |
| `text_field`, `email_field`, `number_field`, `date_field`, `time_field`, `datetime_local_field`, `color_field`, `range_field`, `search_field`, `telephone_field`, `url_field`, `password_field`, `hidden_field` | `<input type="text|email|number|date|time|datetime-local|color|range|search|tel|url|password|hidden">`, `value="{{ form.x }}"` |
| `text_area` | `<textarea name="body">{{ form.body }}</textarea>` |
| `check_box` (with its hidden `0` input) | `<input type="checkbox" name="x" value="true">` and `#[serde(default)] x: bool`: an unchecked box is simply absent |
| `radio_button` | `<input type="radio" name="kind" value="a"{% if form.kind == "a" %} checked{% endif %}>` |
| `select`, `options_for_select`, `collection_select`, `grouped_options_for_select` | a `<select>` with a `{% for %}` over a slice or the records, `<optgroup>` for groups |
| `date_select`, `time_select` (multi-parameter attributes) | `type="date"` / `type="time"` inputs; the browser's picker; `Validator::date` checks the text |
| `file_field` | `<input type="file">` in a `enctype="multipart/form-data"` form; see [File storage](files.md) |
| `fields_for`, nested attributes | several structs flattened into one form struct, saved in the handler (see [Models](models.md) for transactions with `batch`) |
| `label`, `submit` | `<label>`, `<button type="submit">` |
| Custom `FormBuilder` | askama macros, as `errors_for` above |
| `_method` override for `PATCH`/`DELETE` | not used: HTML routes use `POST /posts/{id}` and `POST /posts/{id}/delete` (see [Controllers](controllers.md#routes)) |
| `authenticity_token` | not needed: Ocre refuses cross-site form posts by origin (see [Security](security.md)) |

axum's `Form` reads one value per name; fields named `tags[]` or `post[title]` are not grouped into arrays or nested structs. Give each field its own name, or post JSON from JavaScript to an `ocre::Json` handler when a form needs a list.

## Other formats

A template with `ext = "xml"` escapes for XML, which covers RSS and Atom feeds (Rails' Builder templates and `atom_feed`):

```rust,check
use askama::Template;
use axum::{extract::State, http::header, response::IntoResponse};
use ocre::{Ctx, Page, Result};

use crate::models::post::{self, Post};

#[derive(Template)]
#[template(
    source = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Blog</title>
  <id>https://blog.example/</id>
  <updated>{% if let Some(post) = posts.first() %}{{ post.updated_at|strftime("%FT%TZ") }}{% endif %}</updated>
  {% for post in posts %}
  <entry>
    <id>https://blog.example/posts/{{ post.id }}</id>
    <title>{{ post.title }}</title>
    <updated>{{ post.updated_at|strftime("%FT%TZ") }}</updated>
    <content type="text">{{ post.body }}</content>
  </entry>
  {% endfor %}
</feed>"#,
    ext = "xml"
)]
struct Feed {
    posts: Vec<Post>,
}

mod filters {
    pub use ocre::filters::*;
}

async fn feed(State(ctx): State<Ctx>) -> Result<impl IntoResponse> {
    let posts = post::all(&ctx, Page::new(20, 0)?).await?;
    let xml = Feed { posts }.render()?;
    Ok(([(header::CONTENT_TYPE, "application/atom+xml; charset=utf-8")], xml))
}
```

Plain text is a `String` (`text/plain`), JavaScript is `([(header::CONTENT_TYPE, "text/javascript")], body)`, and a CSV or any generated file goes out with `ocre::storage::send_data` (see [Controllers](controllers.md#files-and-downloads)). One handler answering HTML or JSON uses `ocre::Format` (Rails' `respond_to`, see [Controllers](controllers.md#formats-respond_to)).

## Localized views

Translated text comes from `locales/*.yml` through the `I18n` extractor, passed to the view as a field (`{{ i18n.t("posts.title") }}`; see [Translations](i18n.md)). For pages whose whole markup differs per language (Rails' `show.fr.html.erb`), define one view struct per language and pick it with `match i18n.locale()`.

## See also

- [htmx](htmx.md): boosted navigation, partial responses, inline editing, live validation
- [Controllers and routing](controllers.md): routes, paths, requests, error pages
- [Validations](validations.md): model rules and error messages
- [Assets](assets.md): CSS, JavaScript and images in `public/`
- The [askama book](https://askama.readthedocs.io/) for the full template syntax
