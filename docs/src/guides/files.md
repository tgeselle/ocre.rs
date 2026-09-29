# File storage

Ocre stores uploaded files in Cloudflare R2 and describes each one with four columns of the record that owns it, like Active Storage without its extra tables. This page covers `attachment` fields, the code the generators write for them, the `ocre::storage` API for custom upload and download handlers, and what uploads cost on the free plan.

## Before you start

- An Ocre app created with `ocre new` (see [Installation](../getting-started/installation.md)).
- R2 enabled once on the Cloudflare account before the first deploy (see [Deploying: enable R2 once](#deploying-enable-r2-once)). Local development needs nothing: `ocre dev` simulates R2.
- The generator that adds the first `attachment` field also adds the `STORAGE` R2 binding to `cloudflare.config.ts`; nothing else to configure.
- The protected download example uses `crate::auth::CurrentUser`, created by `ocre g auth` (see [Authentication](authentication.md)).

## How a file is stored

A file lives in the R2 bucket bound as `STORAGE`, under a random key such as `photos/image/2u1Vd0zJ8sQqS6rJq0rVmA`. The record that owns it keeps four columns: an attachment named `image` is stored as `image_key`, `image_filename`, `image_content_type` and `image_size`. The framework reads them back as an [`ocre::storage::Attachment`](/api/ocre/storage/struct.Attachment.html):

```rust
pub struct Attachment {
    pub key: String,          // `<prefix>/<22 random characters>` in the bucket
    pub filename: String,     // the uploader's file name, cleaned up
    pub content_type: String, // lowercase, without parameters: `image/png`
    pub size: i64,            // bytes
}
```

Keys are never reused: replacing a file stores a new object under a new key and deletes the old one once the row points to the new one.

## Adding an attachment field

`attachment` is a field type of every model generator. Suffix it with `?` to make it optional; `^` (unique) is refused because every file already gets its own key.

```sh
ocre g scaffold Photo title:string image:attachment notes:attachment?
```

```text
  create  migrations/0002_create_photos.sql
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

What it generates, piece by piece:

| Piece | What it does |
|---|---|
| Migration | `image_key`, `image_filename`, `image_content_type` `TEXT NOT NULL` and `image_size INTEGER NOT NULL`; the same four columns without `NOT NULL` for `notes` |
| `pub const IMAGE: Rules` / `NOTES` in the model | Largest file (10 MB) and the allowed content types: PNG, JPEG, GIF, WebP, PDF, plain text |
| `photo.image()` / `photo.notes()` | The `Attachment` (`Option<Attachment>` for the optional one) |
| `NewPhoto` / `PhotoChanges` | `image: Option<Upload>` and `notes: Option<Upload>` (`Option<Option<Upload>>` in `PhotoChanges`: `Some(None)` removes the file); `#[serde(skip)]`, since JSON cannot carry a file |
| `validate()` | `image` can't be blank on create; every upload is checked against its `Rules` with `v.file(..)` before anything is stored |
| `create` / `update` / `delete` | Store new files under `photos/image/<random>` and `photos/notes/<random>`, then write the row. Files are deleted again when the write fails; replaced and removed files are deleted after it succeeds; a deleted record's files are deleted with it |
| HTML forms | `enctype="multipart/form-data"`, a file input per attachment with an `accept` list, and a "Remove notes" checkbox on the edit page for the optional file |
| `FORM_LIMIT` in `src/photos.rs` | Largest request the forms accept: the sum of the files' limits plus 1 MB for the text fields. A larger request gets a 413 page |
| `GET /photos/{id}/image`, `GET /photos/{id}/notes` | Streams the file with `storage::serve` (404 when the optional file is absent) |
| `cloudflare.config.ts` | `STORAGE: bindings.r2({ name: "<app>-storage" })`, added by the first generator that needs it |

The `cloudflare.config.ts` entry, after the `// ocre:env` marker:

```ts
// Files (`ocre::storage`, `attachment` fields): an R2 bucket. `ocre dev` keeps a
// local copy under .wrangler/state; `ocre deploy` creates the bucket if needed.
// Free plan: 10 GB stored, 1M writes and 10M reads a month; deletes are free.
STORAGE: bindings.r2({ name: "platapp-storage" }),
```

The rules are plain constants in `src/models/photo.rs`; change them there:

```rust
/// Files `image` accepts; `validate()` checks each upload before anything is
/// stored. The whole request is held in the Worker's memory (128 MB): keep
/// `max_bytes` modest, and forms add it to their request limit.
pub const IMAGE: Rules = Rules {
    max_bytes: 10 * 1024 * 1024,
    content_types: &["image/png", "image/jpeg", "image/gif", "image/webp", "application/pdf", "text/plain"],
};
```

Content types are exact and lowercase. There is no wildcard: `image/*` would admit SVG, which can carry scripts.

`create` in the generated model shows the order of operations (validate, store, insert, clean up on failure):

```rust
pub async fn create(ctx: &Ctx, new: NewPhoto) -> Result<Photo> {
    let db = ctx.db()?;
    new.validate().finish()?;
    // Files go to R2 once the values are valid; they are deleted again if the INSERT fails.
    let image = match new.image {
        Some(upload) => Some(storage::store(ctx, "photos/image", upload).await?),
        None => None,
    };
    let notes = match new.notes {
        Some(upload) => Some(storage::store(ctx, "photos/notes", upload).await?),
        None => None,
    };
    let mut params = params![new.title];
    params.extend(storage::columns(image.as_ref()));
    params.extend(storage::columns(notes.as_ref()));
    let created = db.first("INSERT INTO photos (title, image_key, image_filename, image_content_type, image_size, notes_key, notes_filename, notes_content_type, notes_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) RETURNING *", params).await;
    if !matches!(created, Ok(Some(_))) {
        storage::delete_attachments(ctx, &[image, notes]).await?;
    }
    created?.ok_or_else(|| Error::internal("INSERT ... RETURNING returned no row"))
}
```

Set files only through the model (`NewPhoto { image: Some(upload), .. }`, `PhotoChanges { notes: Some(None), .. }`); never write the `*_key` columns by hand, or R2 objects leak or rows point to deleted objects.

### Trying the HTML scaffold

With `ocre dev` running, open `http://localhost:8787/photos/new`, or post the form with curl:

```sh
curl -s -o /dev/null -w "%{http_code} %{redirect_url}\n" \
  -F title=Beach -F image=@beach.png http://localhost:8787/photos
```

```text
303 http://localhost:8787/photos/1
```

Without a file, the form comes back with a 422 and the error list (`Image can&#39;t be blank`). File inputs cannot be refilled by the server, so after an error the visitor chooses the file again.

## Attachments in a JSON API

`ocre g api` accepts attachment fields, but they must be optional: a JSON body cannot carry a file, so `create` cannot require one.

```sh
ocre g api Report file:attachment
```

```text
error: attachment `file` must be optional in a JSON API
hint: JSON cannot carry a file, so create cannot require one: use `file:attachment?`, then upload with `curl -X PUT -F file=@file http://localhost:8787/api/<plural>/1/file`
```

```sh
ocre g api Document name:string file:attachment?
```

Besides the five REST routes (see [JSON APIs and GraphQL](json-apis.md)), each attachment gets three routes in `src/documents_api.rs`:

| Route | Effect |
|---|---|
| `GET /api/documents/{id}/file` | The file, streamed like the HTML scaffold's (404 when there is none) |
| `PUT /api/documents/{id}/file` | Multipart body with the file as `file`; stores it and deletes the one it replaces; 422 when the file is missing or not allowed |
| `DELETE /api/documents/{id}/file` | Removes the file; answers the updated record |

The request limit of `PUT` is `document::FILE.max_bytes + 64 * 1024` (the file plus room for the multipart framing). GraphQL (`--graphql`) exposes the four columns; files go through these REST routes.

A session with `ocre dev` running (output captured from a real run):

```sh
curl -s -X POST http://localhost:8787/api/documents -H 'Content-Type: application/json' -d '{"name":"Spec"}'
```

```json
{"id":1,"name":"Spec","file_key":null,"file_filename":null,"file_content_type":null,"file_size":null,"created_at":"2026-09-29 04:34:44","updated_at":"2026-09-29 04:34:44"}
```

```sh
curl -s -X PUT -F file=@spec.pdf http://localhost:8787/api/documents/1/file
```

```json
{"id":1,"name":"Spec","file_key":"documents/file/JZJAQ032E8H_AiY5YnpcYA","file_filename":"spec.pdf","file_content_type":"application/pdf","file_size":19,"created_at":"2026-09-29 04:34:44","updated_at":"2026-09-29 04:34:44"}
```

A type outside the rules, and a request without the file:

```sh
curl -s -X PUT -F file=@logo.svg http://localhost:8787/api/documents/1/file
curl -s -X PUT -F name=x http://localhost:8787/api/documents/1/file
```

```json
{"error":{"fields":{"file":["has an unsupported type (allowed: image/png, image/jpeg, image/gif, image/webp, application/pdf, text/plain)"]},"message":"Validation failed","status":422}}
{"error":{"fields":{"file":["can't be blank"]},"message":"Validation failed","status":422}}
```

Removing it:

```sh
curl -s -X DELETE http://localhost:8787/api/documents/1/file
```

```json
{"id":1,"name":"Spec","file_key":null,"file_filename":null,"file_content_type":null,"file_size":null,"created_at":"2026-09-29 04:34:44","updated_at":"2026-09-29 04:34:44"}
```

The content type is the one the client sends. curl guesses it from a few extensions (`.pdf`, `.png`, `.svg`...) and sends `application/octet-stream` for others such as `.csv`; name the type explicitly then: `-F 'file=@contacts.csv;type=text/csv'`.

## Downloads: ETag, 304 and Range

Every generated file route calls `storage::serve`, which answers with the headers a browser needs to cache, seek and save the file:

```sh
curl -si http://localhost:8787/api/documents/1/file
```

```text
HTTP/1.1 200 OK
Content-Length: 19
Content-Type: application/pdf
Accept-Ranges: bytes
Cache-Control: private, no-cache
Content-Disposition: inline; filename="spec.pdf"
ETag: "b6ab5f279cbcf9a1b96b3ab5b207cf94"
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0

%PDF-1.4 fake spec
```

Sending the `ETag` back answers `304 Not Modified` without a body:

```sh
curl -si -H 'If-None-Match: "b6ab5f279cbcf9a1b96b3ab5b207cf94"' http://localhost:8787/api/documents/1/file
```

```text
HTTP/1.1 304 Not Modified
Cache-Control: private, no-cache
ETag: "b6ab5f279cbcf9a1b96b3ab5b207cf94"
...
```

A single `Range` (video seeking, resumed downloads) answers 206; a range past the end answers 416 without calling R2:

```sh
curl -si -H 'Range: bytes=0-7' http://localhost:8787/api/documents/1/file
curl -si -H 'Range: bytes=100-' http://localhost:8787/api/documents/1/file
```

```text
HTTP/1.1 206 Partial Content
Content-Length: 8
Content-Type: application/pdf
Content-Range: bytes 0-7/19
Accept-Ranges: bytes
Cache-Control: private, no-cache
Content-Disposition: inline; filename="spec.pdf"
ETag: "b6ab5f279cbcf9a1b96b3ab5b207cf94"
...

%PDF-1.4

HTTP/1.1 416 Range Not Satisfiable
Content-Length: 0
Content-Range: bytes */19
...
```

Several ranges, other units and malformed values send the whole file, as RFC 9110 allows. `Cache-Control: private, no-cache` (`storage::CACHE_CONTROL`) lets browsers keep the file but revalidate it each time; override the header on the returned response for files that may be cached longer.

## The ocre::storage API

Everything the generated code uses is public, for handlers the generators do not write. Full signatures are in the [rustdoc of `ocre::storage`](/api/ocre/storage/index.html).

| Item | Use |
|---|---|
| `Multipart(mut form): Multipart<LIMIT>` | Extractor for a `multipart/form-data` body of at most `LIMIT` bytes |
| `form.form::<T>()` | The text fields, deserialized like axum's `Form` (file fields ignored); 400 `Invalid form: ...` on a missing or unparsable field |
| `form.text("name")` | One text field, `Option<&str>` |
| `form.file("name")` | Takes the first file sent as `name`: `Option<Upload>`, `None` when no file was chosen |
| `Upload { filename, content_type, bytes }`, `upload.size()` | A received file, before it is stored |
| `Rules { max_bytes, content_types }`, `rules.allows(ct)` | What a file field accepts |
| `Validator::file(field, &upload, &rules)` | Adds "is too large (maximum is 10 MB)" and/or "has an unsupported type (allowed: ...)" |
| `storage::store(&ctx, prefix, upload)` | Stores an upload under `<prefix>/<random>`; returns the `Attachment` |
| `storage::store_bytes(&ctx, prefix, filename, content_type, bytes)` | Stores bytes the app made (an export, a report) |
| `storage::store_body(&ctx, prefix, filename, content_type, size, body)` | Streams a raw request body of known length into R2 without holding it in memory |
| `storage::read(&ctx, key)` | The whole object as `Option<Vec<u8>>`, for files the Worker processes itself |
| `storage::delete(&ctx, key)` | Deletes one object; a missing key is not an error |
| `storage::delete_attachments(&ctx, &[Option<Attachment>])` | Deletes the objects of every `Some` attachment in one R2 call |
| `storage::serve(&ctx, &attachment, &headers, Disposition::Inline)` | Streams a file with ETag/304, Range and a safe `Content-Disposition`; 404 when the object is missing |
| `Disposition::Inline` / `Disposition::Download` | Show safe types in the browser / always download |
| `storage::columns(Some(&attachment))`, `storage::column_changes(..)` | Query parameters for the four columns in an `INSERT` / `UPDATE` |
| `storage::human_size(bytes)`, `attachment.human_size()` | `512 bytes`, `2 KB`, `1.5 MB` |

The `Multipart` extractor rejects bad requests before the handler runs: 400 when the body is not `multipart/form-data` with a boundary or is malformed, 413 "The request is too large (maximum is ...)" when `Content-Length` announces more than `LIMIT` (before anything is read) or as soon as the body passes it. Browsers (`Accept: text/html`) get an HTML error page, other clients JSON.

Every storage function fails with a 500 whose log names the missing `STORAGE: bindings.r2(...)` entry when the `STORAGE` binding is absent.

### A custom upload handler

This endpoint accepts a CSV or text file plus a `source` text field, validates both, stores the file and answers its `Attachment`. Add the module to `src/lib.rs` (`mod imports;` under `// ocre:modules`, `.merge(imports::routes())` under `// ocre:routes`).

```rust,check
// src/imports.rs
use axum::{Router, extract::State, routing::put};
use ocre::{
    ApiResult, Created, Ctx, Validator,
    storage::{self, Attachment, Multipart, Rules},
};

/// What an import accepts: CSV or plain text, 2 MB at most (no wildcards: list exact types).
const IMPORT: Rules = Rules { max_bytes: 2 * 1024 * 1024, content_types: &["text/csv", "text/plain"] };
/// Largest request: the file at its limit plus room for the text fields and the multipart framing.
const LIMIT: usize = IMPORT.max_bytes + 64 * 1024;

pub fn routes() -> Router<Ctx> {
    Router::new().route("/api/imports", put(create))
}

/// `curl -X PUT -F source=crm -F 'file=@contacts.csv;type=text/csv' http://localhost:8787/api/imports`
async fn create(State(ctx): State<Ctx>, Multipart(mut form): Multipart<LIMIT>) -> ApiResult<Created<Attachment>> {
    let source = form.text("source").unwrap_or_default().to_owned();
    let file = form.file("file"); // None when no file was sent
    let mut v = Validator::new();
    v.required("source", &source);
    v.check("file", file.is_none(), "can't be blank");
    if let Some(file) = &file {
        v.file("file", file, &IMPORT);
    }
    v.finish()?; // 422 with every message; nothing is stored
    let file = file.expect("the validator refused a missing file");
    // One R2 class A operation; the key is `imports/<source>/<22 random characters>`.
    let attachment = storage::store(&ctx, &format!("imports/{source}"), file).await?;
    Ok(Created(attachment))
}
```

Real runs against `ocre dev`:

```sh
curl -s -w "\n%{http_code}\n" -X PUT -F source=crm -F 'file=@contacts.csv;type=text/csv' http://localhost:8787/api/imports
```

```text
{"key":"imports/crm/O3B4U81uD8w9O8AJO5Torw","filename":"contacts.csv","content_type":"text/csv","size":55}
201
```

Every problem is reported at once:

```sh
curl -s -X PUT -F file=@logo.svg http://localhost:8787/api/imports
```

```json
{"error":{"fields":{"file":["has an unsupported type (allowed: text/csv, text/plain)"],"source":["can't be blank"]},"message":"Validation failed","status":422}}
```

A 3 MB file is refused by the extractor from its `Content-Length`, before the body is read:

```sh
curl -s -H 'Expect:' -X PUT -F source=crm -F 'file=@big.csv;type=text/csv' http://localhost:8787/api/imports
```

```json
{"error":{"message":"The request is too large (maximum is 2.1 MB)","status":413}}
```

curl adds `Expect: 100-continue` to bodies over 1 MB; in our run against `ocre dev`, curl then printed the 413 but kept waiting until its timeout. `-H 'Expect:'` turns that off.

To keep the file, save the returned `Attachment` in a row with `storage::columns(Some(&attachment))`, and `storage::delete(&ctx, &attachment.key)` if that write fails, as generated `create` functions do. For an existing model, prefer adding an `attachment` field with a migration and going through the model.

### Serving a file

A route that only signed-in users may use and that always downloads the file under its original name. Any check you put before `serve` is the only protection of the file: R2 objects are never public.

```rust,check
// src/downloads.rs
use axum::{
    Router,
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::get,
};
use ocre::{
    Ctx, OptionExt, Result,
    storage::{self, Disposition},
};

use crate::{auth::CurrentUser, models::photo};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/photos/{id}/image/download", get(download))
}

/// Signed-in users only (visitors are sent to /login), and always as a download with the original name.
async fn download(
    CurrentUser(_user): CurrentUser,
    State(ctx): State<Ctx>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Response> {
    let record = photo::find(&ctx, id).await?.or_404()?;
    storage::serve(&ctx, &record.image(), &headers, Disposition::Download).await
}
```

A visitor is redirected; a signed-in user (session cookie in `jar`) gets the file with `attachment`:

```sh
curl -si http://localhost:8787/photos/1/image/download
curl -si -b jar http://localhost:8787/photos/1/image/download
```

```text
HTTP/1.1 303 See Other
Location: /login
...

HTTP/1.1 200 OK
Content-Length: 15
Content-Type: image/png
Accept-Ranges: bytes
Cache-Control: private, no-cache
Content-Disposition: attachment; filename="beach.png"
ETag: "87942cd740b20e26ee606ffa3d9cb033"
...
```

The generated `GET /photos/{id}/image` answers the same file with `Content-Disposition: inline; filename="beach.png"`.

To restrict files to their owner, load the record filtered by `user.id` (`WHERE id = ?1 AND user_id = ?2`) and answer 404 otherwise, as for any other record (see [Authentication](authentication.md)).

## Safety choices

- **Keys** are random (128 bits), never derived from file names, so a name cannot overwrite or guess another file.
- **File names** lose their directories (old Windows browsers send `C:\...`) and control characters, and are cut to 200 characters with the extension kept; `file` stands in when nothing is left. `Content-Disposition` carries them as ASCII `filename=` plus UTF-8 `filename*=` when needed (RFC 6266).
- **Inline display** is limited to types that cannot run scripts: raster images (PNG, JPEG, GIF, WebP, AVIF, BMP, TIFF, icons), PDF, plain text, audio and video. HTML, SVG, XML and JavaScript are sent as `application/octet-stream` downloads even with `Disposition::Inline`, so an uploaded file never runs as part of the app (Rails' `content_types_allowed_inline` / `content_types_to_serve_as_binary`).
- **Content types** come from the browser: the `Rules` allowlist limits them, nothing sniffs file contents. `X-Content-Type-Options: nosniff` (added to every response) stops browsers from guessing.
- **Validation before storage**: `v.file(..)` runs before `store`, so a refused file costs no R2 operation.
- **Authorization**: file routes are as protected as the handler around them.

## Free-plan costs and limits

R2 is free every month within these amounts ([R2 pricing](https://developers.cloudflare.com/r2/pricing/), September 2026):

| Resource | Free every month | Ocre use |
|---|---|---|
| Storage | 10 GB-month | Every stored file; replaced and deleted files are removed by the generated models |
| Class A operations | 1,000,000 | Each upload (`store`, `store_bytes`, `store_body`) is one |
| Class B operations | 10,000,000 | Each `serve` (downloads and 304s alike) and each `read` is one |
| Deletes | free | `delete`, `delete_attachments` |
| Egress | free | Downloads cost no bandwidth fee |

Worker limits that shape uploads ([Workers limits](https://developers.cloudflare.com/workers/platform/limits/), September 2026):

- **Memory**: a Worker has 128 MB, and the `Multipart` extractor holds the whole request in memory while R2 gets a copy. Keep `max_bytes` in the tens of MB.
- **Request size**: Cloudflare refuses request bodies over 100 MB on the Free plan before they reach the Worker.
- **CPU**: uploads are split with a substring search, about 1.2 ms per 10 MB in WebAssembly (memchr, measured in V8), plus about 0.15 ms per 10 MB to copy them to R2. Downloads never pass through WebAssembly: `storage::serve` attaches R2's stream to the response and `ocre::serve` answers with it directly (this is why the Worker's `fetch` returns `worker::web_sys::Response`), so a download costs almost no CPU whatever its size.
- For a single large file, `store_body` streams a raw body (`curl -T big.zip`) of known `Content-Length` into R2 with flat memory; it checks no `Rules`, so check the size and type yourself first.

## Local development

`ocre dev` runs the local R2 simulation of `cf dev`: objects are kept under `.wrangler/state` (git-ignored) and survive restarts. `ocre db reset` only deletes the local database, not the stored objects. The bindings table printed at startup shows the bucket:

```text
env.STORAGE (platapp-storage)                              R2 Bucket                 local
```

## Deploying: enable R2 once

`ocre deploy` runs `cf r2 buckets get <bucket>` for every `bindings.r2(...)` entry and `cf r2 buckets create` for the missing ones, before deploying. R2 has to be enabled once per account in the Cloudflare dashboard (Storage & databases > R2), which asks for a payment method even for the free tier. When it is not, the check fails with Cloudflare API code 10042 and the CLI prints this hint (from `crates/ocre-cli/src/cloudflare.rs`):

```text
hint: enable R2 once in the Cloudflare dashboard (Storage & databases > R2; the free plan asks for a payment method but charges nothing within 10 GB, 1M writes and 10M reads a month), then run `ocre deploy` again
```

## Not included

- Presigned URLs and direct browser-to-R2 uploads (they need R2 S3 API credentials and SigV4 signing).
- Public buckets and custom domains for R2 (served by Cloudflare without the Worker; set them up in the dashboard).
- Image resizing and variants.
- Cleanup of files whose rows are removed by `ON DELETE CASCADE`: the database deletes the child rows without calling the child model's `delete`, so their files stay in R2. Delete them in the parent's `delete` if that matters.

## See also

- [Field types](../reference/field-types.md): `attachment` and the other types.
- [Generators](../reference/generators.md#ocre-g-scaffold): `ocre g scaffold` and [`ocre g api`](../reference/generators.md#ocre-g-api).
- [Configuration](../reference/configuration.md#env-storage-r2): the `STORAGE` entry.
- [Validations](validations.md): `Validator` and 422 responses.
- [Authentication](authentication.md): `CurrentUser` and ownership checks.
- [Free-plan limits](../reference/limits.md) and [Cost model](../explanations/cost-model.md).
- [`ocre::storage` rustdoc](/api/ocre/storage/index.html).
