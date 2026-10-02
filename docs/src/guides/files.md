# File storage

Ocre stores uploaded files in Cloudflare R2 and describes each one with four columns of the record that owns it, like Active Storage without its extra tables. This page covers `attachment` fields, the code the generators write for them, the `ocre::storage` API for custom upload and download handlers, direct browser-to-R2 uploads and downloads through presigned URLs, file analysis, image variants, and what all of it costs on the free plan.

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

The request limit of `PUT` is `document::FILE.max_bytes as usize + 64 * 1024` (the file plus room for the multipart framing). GraphQL (`--graphql`) exposes the four columns; files go through these REST routes.

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

## Many files per record

`name:attachments` (plural) gives a record any number of files, Rails' `has_many_attached`. It works with `ocre g model`, `scaffold` and `api`:

```sh
ocre g scaffold Album title:string photos:attachments
```

Each file is a row of a child model, `AlbumPhoto` (table `album_photos`: `album_id` with `ON DELETE CASCADE`, and a required `file` attachment checked against `album_photo::FILE`). The parent gets:

| Method | Does |
|---|---|
| `album.attach_photos(&ctx, uploads)` | stores new files (Rails' `photos.attach`); every file is validated before any is stored |
| `album.replace_photos(&ctx, uploads)` | deletes the current files, then attaches (Rails' `photos =`) |
| `album.purge_photos(&ctx)` | deletes the rows and their objects in R2 |
| `album.album_photos(&ctx, page)` | the rows, newest first; `album_photo::query().eq("album_id", id)` for anything else |

Deleting an album deletes its files first, then the rows go with it. The scaffold's show page lists the files (links open them), deletes one, and adds several at once from `<input type="file" name="photos" multiple>` (`POST /albums/{id}/photos`, `GET /albums/{id}/photos/{file_id}`, `POST .../{file_id}/delete`). `ocre g api` adds the JSON routes:

```sh
curl -F photos=@a.png -F photos=@b.png http://localhost:8787/api/albums/1/photos   # add, answers the rows
curl http://localhost:8787/api/albums/1/photos                                      # list
curl http://localhost:8787/api/albums/1/photos/2                                    # one file
curl -X DELETE http://localhost:8787/api/albums/1/photos/2                          # 204
```

A request adding files may carry up to ten files at their limit; a refused file answers 422 and stores none. In a handler of your own, `MultipartForm::files("photos")` takes every file of a multiple input (also sent as `photos[]`).

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
| `Upload::new(filename, content_type, bytes)` | An upload made by the app (Active Storage's `attach(io:)`), cleaned up like a browser's |
| `storage::head(&ctx, key)`, `storage::exists(&ctx, key)` | A `StoredObject` (size, type, ETag, upload time) or `None`; one class B operation |
| `storage::list(&ctx, prefix, cursor, limit)` | One page (up to 1,000) of `StoredObject`s plus the next cursor; one **class A** operation |
| `storage::read_first(&ctx, key, length)` | The first bytes of an object, for `analyze` |
| `storage::presign_get(&ctx, &attachment, disposition, expires_in)`, `storage::serve_redirect(..)` | A download URL straight from R2 / a 302 to it |
| `storage::direct_upload(..)`, `storage::attach_direct_upload(..)` | Start and finish a direct browser-to-R2 upload |
| `storage::purge_unattached(&ctx, prefix, table, column, max_age, cursor)` | Delete direct uploads no row adopted |
| `storage::analyze(bytes)`, `Validator::file_content(field, &upload)` | Real type and image size from the bytes; refuse files whose bytes do not match their type |
| `Variant::new().width(300).fit(Fit::Cover).path(src)` | A Cloudflare Image Transformations URL |
| `storage::public_url(&ctx, key)` | The permanent URL of a file in a public bucket |
| `S3Endpoint::r2(..)`, `endpoint.presign(..)` | SigV4 presigning for R2's S3 API or another S3-compatible store |

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
const LIMIT: usize = IMPORT.max_bytes as usize + 64 * 1024;

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

A route that only signed-in users may use and that always downloads the file under its original name. Any check you put before `serve` is the only protection of the file: R2 objects are never public unless you turn on [public access](#public-files).

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

## Inspecting the bucket: head, exists, list

`storage::head(&ctx, key)` describes an object without reading it (one class B operation): a [`StoredObject`](/api/ocre/storage/struct.StoredObject.html) with `size`, `content_type`, `etag`, `uploaded_at` (Unix seconds) and `filename` (recorded by `store`, `None` for direct uploads), or `None` when the key does not exist. `storage::exists(&ctx, key)` is the same call as a `bool`.

`storage::list(&ctx, prefix, cursor, limit)` returns one page of the objects under a prefix, in key order, with the cursor of the next page (`None` on the last one). Each call is a **class A** operation, like an upload, and decoding 1,000 entries takes a few ms of the 10 ms CPU budget: list from scheduled tasks and admin pages, never on every request.

```rust,check
// src/storage_report.rs: total size of the photo images, 1,000 keys per class A operation.
use ocre::{Ctx, Result, storage};

pub async fn images_size(ctx: &Ctx) -> Result<u64> {
    let (mut total, mut cursor) = (0, None);
    loop {
        let page = storage::list(ctx, "photos/image/", cursor.as_deref(), 1000).await?;
        total += page.objects.iter().map(|object| object.size).sum::<u64>();
        cursor = page.cursor;
        if cursor.is_none() {
            return Ok(total);
        }
    }
}
```

## Presigned URLs and redirect serving

R2 also speaks the S3 API at `https://<account_id>.r2.cloudflarestorage.com`. A presigned URL is an S3 request signed in advance (AWS Signature Version 4, region `auto`, path style `/<bucket>/<key>`): whoever holds it can perform that one request until it expires, without credentials and without the Worker. Ocre signs locally with HMAC-SHA256 (microseconds of CPU, no R2 operation); the browser's download is then a class B operation and an upload a class A one, as through the Worker.

### Settings

Create an R2 API token once in the dashboard (R2 > Manage API tokens > Create API token, permission "Object Read & Write", limited to the `<app>-storage` bucket). It shows an access key ID and a secret access key; keep them as secrets, and the account ID and bucket name as plain variables:

| Name | Kind | Value |
|---|---|---|
| `R2_ACCESS_KEY_ID` | secret | The token's access key ID |
| `R2_SECRET_ACCESS_KEY` | secret | The token's secret access key (also signs the keys of direct uploads) |
| `R2_ACCOUNT_ID` | variable | The account ID shown on the R2 overview page |
| `R2_BUCKET` | variable | `<app>-storage`, the `name` of the `STORAGE` binding |

```ts
// cloudflare.config.ts, in worker.env
R2_ACCOUNT_ID: bindings.text("0123456789abcdef0123456789abcdef"),
R2_BUCKET: bindings.text("docs-app-storage"),
```

```sh
# .prod.vars (git-ignored): R2_ACCESS_KEY_ID=... and R2_SECRET_ACCESS_KEY=...
ocre secrets push R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY --file .prod.vars
```

For `ocre dev`, put the four values in `.dev.vars`. A missing one makes every presigning call fail with a 500 whose log names the missing settings and these steps. Presigned URLs always point to the real bucket: `ocre dev`'s local R2 simulation has no S3 API, so a file uploaded through a presigned URL is not visible to `storage::head` or `storage::serve` in `ocre dev`. Try direct uploads on a deployed Worker.

### Download URLs and redirects

`storage::presign_get(&ctx, &attachment, disposition, expires_in)` returns a URL valid `expires_in` seconds (1 second to 7 days). It carries `response-content-type` and `response-content-disposition`, so R2 answers with the same safe headers as `storage::serve`: the original file name, `inline` only for safe types, HTML and SVG as `application/octet-stream`. The file is then served from R2's host, not the app's origin.

`storage::serve_redirect(&ctx, &attachment, disposition, expires_in)` answers `302 Found` to such a URL (Active Storage's redirect mode), with `Cache-Control: private, max-age=<expires_in / 2>`. Use it for large or popular files: the Worker only signs a URL, and R2 serves `Range` requests itself.

```rust,check
// src/photo_redirects.rs
use axum::{
    Router,
    extract::{Path, State},
    response::Response,
    routing::get,
};
use ocre::{
    Ctx, OptionExt, Result,
    storage::{self, Disposition},
};

use crate::{auth::CurrentUser, models::photo};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/photos/{id}/image/direct", get(image))
}

/// Signed-in users get a 302 to R2, valid 5 minutes.
async fn image(CurrentUser(_user): CurrentUser, State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Response> {
    let record = photo::find(&ctx, id).await?.or_404()?;
    storage::serve_redirect(&ctx, &record.image(), Disposition::Inline, 300)
}
```

Authorize before signing: anyone with the URL can use it until it expires, even after signing out, so keep lifetimes short.

## Direct uploads

A direct upload sends the file from the browser to R2 without passing through the Worker (Active Storage's direct uploads): no 100 MB request limit, no Worker memory, no CPU spent on the body. It takes three requests:

1. The page asks the app to start an upload, with the file's name, type and size. `storage::direct_upload` checks them against the field's `Rules` (422 otherwise), picks a new key under a prefix of its own, and answers a presigned `PUT` URL, the headers to send with it and a `signed_key`.
2. The browser `PUT`s the file to that URL. The URL signs `Content-Type` and `Content-Length`, so R2 refuses a file of another type or size than declared.
3. The form is submitted with the `signed_key` (and the file name) instead of the file. `storage::attach_direct_upload` checks the signature (only keys this app issued are accepted, so nobody can claim another record's file), runs `head` on the object, checks its size and type against the `Rules` again (deleting a refused object) and returns its `Attachment`.

The server side, for the `Photo` scaffold (`ocre g scaffold Photo title:string image:attachment notes:attachment?`):

```rust,check
// src/photo_uploads.rs
use axum::{Router, extract::State, routing::post};
use ocre::{
    ApiResult, Created, Ctx, Error, Json, Validator, params,
    storage::{self, DirectUpload, DirectUploadRequest},
};
use serde::Deserialize;

use crate::models::photo::{self, Photo};

/// Direct uploads of `image` live under their own prefix, so `purge_unattached` can find abandoned ones.
pub const IMAGE_UPLOADS: &str = "uploads/photos/image";

pub fn routes() -> Router<Ctx> {
    Router::new().route("/api/photos/uploads", post(start)).route("/api/photos/direct", post(create))
}

/// Step 1: `{"filename": "beach.png", "content_type": "image/png", "size": 48213}`.
async fn start(State(ctx): State<Ctx>, Json(request): Json<DirectUploadRequest>) -> ApiResult<Json<DirectUpload>> {
    Ok(Json(storage::direct_upload(&ctx, IMAGE_UPLOADS, "image", &request, &photo::IMAGE)?))
}

#[derive(Deserialize)]
struct NewDirectPhoto {
    title: String,
    image_signed_key: String,
    image_filename: String,
}

/// Step 3: the form, with the signed key in place of the file.
async fn create(State(ctx): State<Ctx>, Json(form): Json<NewDirectPhoto>) -> ApiResult<Created<Photo>> {
    Validator::new().required("title", &form.title).finish()?;
    // One class B operation (`head`); 422 when the key is forged, the file missing, or breaks the rules.
    let image =
        storage::attach_direct_upload(&ctx, "image", &form.image_signed_key, &form.image_filename, &photo::IMAGE)
            .await?;
    let mut values = params![form.title];
    values.extend(storage::columns(Some(&image)));
    let sql = "INSERT INTO photos (title, image_key, image_filename, image_content_type, image_size) \
               VALUES (?1, ?2, ?3, ?4, ?5) RETURNING *";
    let created: Result<Option<Photo>, Error> = ctx.db()?.first(sql, values).await;
    if !matches!(created, Ok(Some(_))) {
        storage::delete(&ctx, &image.key).await?; // no row points to it
    }
    Ok(Created(created?.ok_or_else(|| Error::internal("INSERT ... RETURNING returned no row"))?))
}
```

The browser side, plain JavaScript without a build step. `XMLHttpRequest` stands in for Rails' `direct-upload:*` events: `upload.onprogress` is `direct-upload:progress`, `onload` is `direct-upload:end`, `onerror` is `direct-upload:error`:

```html
<form id="photo-form">
  <input name="title" required>
  <input type="file" name="image" accept="image/png,image/jpeg,image/gif,image/webp" required>
  <progress value="0" max="100" hidden></progress>
  <button>Save</button>
  <p class="error" hidden></p>
</form>
<script type="module">
  const form = document.getElementById("photo-form");
  const progress = form.querySelector("progress");
  const error = form.querySelector(".error");
  const postJson = (url, body) =>
    fetch(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });

  // PUT the file to R2 with the signed headers, reporting progress.
  const put = (upload, file) =>
    new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open("PUT", upload.url);
      for (const [name, value] of Object.entries(upload.headers)) xhr.setRequestHeader(name, value);
      xhr.upload.onprogress = (e) => e.lengthComputable && (progress.value = (100 * e.loaded) / e.total);
      xhr.onload = () => (xhr.status < 300 ? resolve() : reject(new Error(`R2 answered ${xhr.status}`)));
      xhr.onerror = () => reject(new Error("upload failed (network, or the bucket's CORS rule)"));
      xhr.send(file);
    });

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const file = form.image.files[0];
    try {
      // 1. Start: the app checks the declared type and size, and signs a PUT.
      const started = await postJson("/api/photos/uploads", {
        filename: file.name,
        content_type: file.type || "application/octet-stream",
        size: file.size,
      });
      if (!started.ok) throw new Error((await started.json()).error.message);
      const upload = await started.json();
      // 2. Upload straight to R2.
      progress.hidden = false;
      await put(upload, file);
      // 3. Submit the form with the signed key.
      const created = await postJson("/api/photos/direct", {
        title: form.title.value,
        image_signed_key: upload.signed_key,
        image_filename: file.name,
      });
      if (!created.ok) throw new Error((await created.json()).error.message);
      window.location = `/photos/${(await created.json()).id}`;
    } catch (e) {
      error.textContent = e.message;
      error.hidden = false;
    }
  });
</script>
```

A 422 from step 1 or 3 carries the field messages (`{"error":{"fields":{"image":["is too large (maximum is 10 MB)"]},...}}`). The URL of step 1 must be used within 10 minutes; a `PUT` started in time may take longer.

### The bundled script

The page above is written by hand to show the steps. Ocre ships the same logic as a script, Active Storage's `activestorage.js`: merge its route once and mark the input.

```rust,ignore
// src/lib.rs, in routes()
.merge(ocre::storage::direct_upload_script()) // GET /ocre/direct-upload.js
```

```html
<script src="/ocre/direct-upload.js" defer></script>
<form action="/photos/direct" method="post">
  <input type="file" name="image" data-direct-upload-url="/api/photos/uploads">
  <button>Save</button>
</form>
```

When the form is submitted, each chosen file is signed (a JSON `POST` to the input's URL, step 1) and `PUT` to R2 (step 2); the form is then submitted without the file, with `image_key` (the signed key) and `image_filename`, which the handler passes to `storage::attach_direct_upload` (step 3). With `multiple`, the two fields repeat. The script dispatches Active Storage's events, which bubble: `direct-uploads:start` / `direct-uploads:end` on the form, and per file `direct-upload:start`, `direct-upload:progress` (`event.detail.progress`, 0 to 100), `direct-upload:error` (call `preventDefault()` to replace the default alert) and `direct-upload:end`.

```js
document.addEventListener("direct-upload:progress", (event) => {
  document.querySelector("progress").value = event.detail.progress;
});
```

### Bucket CORS rule

The browser `PUT`s to R2's host, a different origin, so the bucket needs a CORS rule allowing it. In the dashboard: R2 > `<app>-storage` > Settings > CORS policy > Add, with:

```json
[
  {
    "AllowedOrigins": ["https://docs-app.example.com"],
    "AllowedMethods": ["PUT"],
    "AllowedHeaders": ["content-type"],
    "MaxAgeSeconds": 3600
  }
]
```

Ocre does not set it for you and this guide's authors have not run this step against a live bucket: check the dashboard's current wording, and that the preflight (`OPTIONS`) answers before debugging anything else when `onerror` fires. [Multipart uploads](#large-files-multipart-uploads-that-resume) also need `"ExposeHeaders": ["ETag"]`, so the script can read each part's ETag.

## Large files: multipart uploads that resume

One `PUT` is fine for photos. For videos of several gigabytes, a dropped connection would restart the whole file, and R2 takes at most 5 GB in one `PUT`. `storage::multipart_uploads` sends a file in parts (S3 multipart uploads), four at a time, each retried; the parts already sent are remembered in the browser, so choosing the same file again after a lost connection or a closed tab sends only the missing ones.

```rust,ignore
use ocre::storage::{self, Rules};

// `u64`: files over 4 GB.
pub static VIDEO: Rules = Rules { max_bytes: 20 * 1024 * 1024 * 1024, content_types: &["video/mp4", "video/quicktime"] };

// in routes():
.merge(storage::multipart_uploads("/videos/uploads", "uploads/videos", "video", &VIDEO))
.merge(storage::direct_upload_script())
```

```html
<script src="/ocre/direct-upload.js" defer></script>
<form action="/videos" method="post">
  <input type="file" name="video" data-multipart-upload-url="/videos/uploads">
  <button>Upload</button>
</form>
```

The form is then submitted with `video_key` and `video_filename`, exactly as after a direct upload: the handler calls `storage::attach_direct_upload(&ctx, "video", &form.video_key, &form.video_filename, &VIDEO)`, which checks the assembled object's size and type and returns the `Attachment` to save. The script dispatches the same events (`direct-upload:progress` covers the whole file).

What happens, with the routes under `/videos/uploads`:

1. `POST /videos/uploads` with the file's name, type and size: checked against `VIDEO`, then the upload is created in R2 (one class A operation). The answer is the signed key, R2's `upload_id`, the part size (10 MiB, larger for files that would need over 10,000 parts) and the number of parts.
2. `POST /videos/uploads/parts` with the part numbers still to send: where to `PUT` each one.
3. Each part is `PUT` there (one class A operation each: a 5 GB video is 512 parts). The script keeps every finished part's number and ETag in `localStorage`, under the URL, file name, size and modification date.
4. `POST /videos/uploads/complete` with every part: R2 assembles the object (one class A operation).
5. `POST /videos/uploads/abort` drops an upload; R2 also deletes the parts of an upload left unfinished, after a while set by the bucket's lifecycle rules.

Where the parts go:

- **Straight to R2** in a release build with the `R2_*` [settings](#settings): presigned `PUT` URLs (valid 24 hours), so the file never passes through the Worker, whatever its size. The bucket's [CORS rule](#bucket-cors-rule) must allow `PUT` and expose `ETag`.
- **Through the Worker** otherwise: in `ocre dev` (whose local R2 has no S3 API) and without the `R2_*` settings. Each part is one request to `PUT /videos/uploads/parts/<n>`, streamed into R2 (parts of at most 95 MB, under the 100 MB request limit; files up to about 950 GB). It works on the free plan, but every part costs a Worker request and the CPU of copying it through WebAssembly: prefer the direct mode in production.

The upload's key is signed with `R2_SECRET_ACCESS_KEY`, or `SECRET_KEY_BASE` without it. The through-the-Worker mode was checked in `ocre dev` with a browser (a 25 MB file in three parts, the third failing until the form was sent again, then only that part resent); the direct mode has not been run against a live bucket (October 2026).

## Purging unattached uploads

A direct upload whose form is never submitted (a closed tab, a failed validation) leaves an object no row points to. `storage::purge_unattached(&ctx, prefix, table, column, max_age, cursor)` lists one page (up to 1,000 keys) under the prefix, keeps the objects uploaded more than `max_age` seconds ago, looks them up in `table.column` (`SELECT column FROM table WHERE column IN (...)`, 100 keys per query) and deletes the ones no row references. It returns the deleted keys and the cursor of the next page.

Run it from a scheduled task (`ocre g schedule purge_uploads "every day at 4am"`, see [Background jobs and schedules](jobs.md#schedules)):

```rust,check
// src/schedules/purge_uploads.rs
use ocre::{Ctx, Result, storage};

/// Direct uploads of photo images left unattached for a day. At most 5 pages (5 class A operations) per run.
pub async fn run(ctx: &Ctx) -> Result<()> {
    let mut cursor = None;
    for _ in 0..5 {
        let purged =
            storage::purge_unattached(ctx, "uploads/photos/image", "photos", "image_key", 86_400, cursor.as_deref())
                .await?;
        println!("purge_uploads: {} unattached uploads deleted", purged.deleted.len());
        cursor = purged.cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(())
}
```

Costs and limits:

- Each page is one **class A** operation (1M free per month), whether or not anything is deleted, and attached uploads are listed again on every run. A daily run over 5,000 keys is about 150 class A operations a month.
- Each lookup is a D1 query; index the column (`CREATE UNIQUE INDEX photos_image_key ON photos(image_key);` in a migration) so it reads only the matching rows instead of the whole table.
- Deletes are free. Decoding a page of 1,000 keys takes a few ms of the 10 ms of CPU a run gets: keep the page cap low, and give each attachment its own prefix so listings stay short.
- Keep `max_age` well above the 10 minutes an upload URL lasts plus the time a form stays open; a day is safe.

## File analysis

The content type of an upload is whatever the browser claims. `storage::analyze(&bytes)` reads the file's signature instead (Active Storage's analyzers, without reading pixels): PNG, JPEG, GIF, WebP, AVIF, PDF, ZIP (and the Office and OpenDocument formats built on it), MP4, M4A and QuickTime. For PNG, GIF, WebP and JPEG it also reads the width and height from the header. Text formats (plain text, CSV, HTML, SVG) have no signature and give `None`. It reads the first 32 bytes, plus a JPEG's segment headers: microseconds of CPU whatever the file size.

`Validator::file_content(field, &upload)` uses it to refuse "has content that does not match image/png": a declared type that `analyze` recognizes but the bytes do not carry, or bytes of a recognized type sent under another one. Chain it after `v.file(..)`:

```rust,check
// src/avatars.rs
use axum::{Router, extract::State, routing::put};
use ocre::{
    ApiResult, Created, Ctx, Error, Validator,
    storage::{self, Attachment, Multipart, Rules},
};

const AVATAR: Rules = Rules { max_bytes: 2 * 1024 * 1024, content_types: &["image/png", "image/jpeg", "image/webp"] };

pub fn routes() -> Router<Ctx> {
    Router::new().route("/api/avatars", put(create))
}

/// `curl -X PUT -F avatar=@me.png http://localhost:8787/api/avatars`
async fn create(State(ctx): State<Ctx>, Multipart(mut form): Multipart<{ 3 * 1024 * 1024 }>) -> ApiResult<Created<Attachment>> {
    let avatar = form.file("avatar").ok_or_else(|| Error::bad_request("Send the file as `avatar`"))?;
    let analysis = storage::analyze(&avatar.bytes);
    let mut v = Validator::new();
    v.file("avatar", &avatar, &AVATAR).file_content("avatar", &avatar);
    v.check("avatar", analysis.width.is_some_and(|width| width < 64), "must be at least 64 pixels wide");
    v.finish()?;
    Ok(Created(storage::store(&ctx, "avatars", avatar).await?))
}
```

For a direct upload the bytes are in R2, not in the request: `storage::read_first(&ctx, key, 64 * 1024)` reads the start of the object (one class B operation; use 256 KB for JPEGs with large EXIF blocks) for `analyze`.

## Image variants

Resized versions of images come from [Cloudflare Image Transformations](https://developers.cloudflare.com/images/transform-images/), not from the Worker: a URL `/cdn-cgi/image/<options>/<source>` on the app's own domain makes Cloudflare's edge fetch the source image, resize it and cache the result. `ocre::storage::Variant` builds that URL:

```rust,check
// src/photo_variants.rs
use ocre::storage::{Fit, Variant};

/// Square thumbnails for the photo index; `format=auto` sends AVIF or WebP to browsers that accept them.
pub const THUMB: Variant = Variant::new().width(300).height(300).fit(Fit::Cover).quality(80);

/// `/cdn-cgi/image/width=300,height=300,fit=cover,quality=80,format=auto/photos/1/image`
pub fn thumb_path(photo_id: i64) -> String {
    THUMB.path(&format!("/photos/{photo_id}/image"))
}
```

In a template: `<img src="{{ crate::photo_variants::thumb_path(photo.id) }}" alt="">`. `Fit` maps Active Storage's resize options: `ScaleDown` (`resize_to_limit`), `Contain` (`resize_to_fit`), `Cover` (`resize_to_fill`), `Crop`, `Pad` (`resize_and_pad`).

- **Lazy by design**: a variant is made on its first request and then served from Cloudflare's cache (Rails' lazy variant loading); nothing is precomputed or stored in R2, and the Worker's CPU is not used. The first request of each variant fetches the source, which invokes the Worker once (one class B operation for `storage::serve`).
- **Needs a custom domain**: the app must be served from a zone on Cloudflare with Transformations enabled for it (dashboard: Images > Transformations > enable for the zone). `*.workers.dev` hosts cannot use `/cdn-cgi/image/`; there the URL answers an error.
- **Free quota**: the Images Free plan includes 5,000 unique transformations a month (unverified assumption at the time of writing; check [Images pricing](https://developers.cloudflare.com/images/pricing/)). Each distinct source and option set counts once a month; beyond it, new variants fail rather than being billed on the Free plan (also unverified).
- **Source caching**: `storage::serve` sends `Cache-Control: private, no-cache`; for public images, serve the source with a public cache header (see the `CACHE_CONTROL` example in the rustdoc) so Cloudflare can cache it. Keep variants for images anyone may see: the transformation fetches the source on its own.

## Public files

A bucket can be made public (dashboard: R2 > `<app>-storage` > Settings > Public access): through an `r2.dev` subdomain (rate-limited, meant for development) or a custom domain connected to the bucket. Set its base URL as a variable, and `storage::public_url(&ctx, key)` returns `<base>/<key>` (each segment percent-encoded):

```ts
// cloudflare.config.ts, in worker.env
STORAGE_PUBLIC_URL: bindings.text("https://files.docs-app.example.com"),
```

```rust,check
// src/photo_links.rs
use ocre::{Ctx, Result, storage};

use crate::models::photo::Photo;

/// The permanent link of a photo's image; no Worker, no signing, cached by Cloudflare.
pub fn image_url(ctx: &Ctx, photo: &Photo) -> Result<String> {
    storage::public_url(ctx, &photo.image().key)
}
```

The trade-off is total: every object of the bucket is then readable forever by whoever has its key, with no authorization and no expiry, and the `Content-Type` is the stored one (the safe-type rules of `serve` do not apply; do not make a bucket public if it holds user-uploaded HTML or SVG). Use a separate public bucket for avatars and product images, never for private documents. A missing `STORAGE_PUBLIC_URL` is a 500 whose log says how to set it.

## Images in rich text

A `rich_text` field's scaffold form lets writers drop images into the Trix editor (Action Text attachments). The editor posts each file to `POST /<plural>/embeds` (a multipart `file`, images up to 10 MB, the `EMBED` rules in the controller), the controller stores it in R2 under `<plural>/embeds/` and answers its URL, `GET /<plural>/embeds/<name>`, which the editor puts in the text as `<figure><img src="..."></figure>`. The model's `sanitize` keeps `figure`, `figcaption` and relative `img` sources. The upload runs in `/ocre/direct-upload.js` (the scaffold merges `ocre::storage::direct_upload_script()` into `routes()` once), for any `<trix-editor data-embeds-url="...">`.

An image removed from the text stays in R2; `storage::purge_unattached` cannot tell (the keys are in HTML), so list `<plural>/embeds/` and delete what no text contains if storage matters.

## Safety choices

- **Keys** are random (128 bits), never derived from file names, so a name cannot overwrite or guess another file.
- **File names** lose their directories (old Windows browsers send `C:\...`) and control characters, and are cut to 200 characters with the extension kept; `file` stands in when nothing is left. `Content-Disposition` carries them as ASCII `filename=` plus UTF-8 `filename*=` when needed (RFC 6266).
- **Inline display** is limited to types that cannot run scripts: raster images (PNG, JPEG, GIF, WebP, AVIF, BMP, TIFF, icons), PDF, plain text, audio and video. HTML, SVG, XML and JavaScript are sent as `application/octet-stream` downloads even with `Disposition::Inline`, so an uploaded file never runs as part of the app (Rails' `content_types_allowed_inline` / `content_types_to_serve_as_binary`).
- **Content types** come from the browser: the `Rules` allowlist limits them, and `v.file_content(..)` checks that the bytes match (see [File analysis](#file-analysis)). `X-Content-Type-Options: nosniff` (added to every response) stops browsers from guessing.
- **Direct uploads** sign both the upload URL (type and size) and the key handed back to the form, and are checked again with `head` before they are attached.
- **Validation before storage**: `v.file(..)` runs before `store`, so a refused file costs no R2 operation.
- **Authorization**: file routes are as protected as the handler around them.

## Free-plan costs and limits

R2 is free every month within these amounts ([R2 pricing](https://developers.cloudflare.com/r2/pricing/), September 2026):

| Resource | Free every month | Ocre use |
|---|---|---|
| Storage | 10 GB-month | Every stored file; replaced and deleted files are removed by the generated models |
| Class A operations | 1,000,000 | Each upload (`store`, `store_bytes`, `store_body`, a direct upload's `PUT`) and each `list` page is one |
| Class B operations | 10,000,000 | Each `serve` (downloads and 304s alike), `read`, `read_first`, `head`/`exists` and presigned download is one |
| Deletes | free | `delete`, `delete_attachments` |
| Egress | free | Downloads cost no bandwidth fee |

Past the free tier, R2 Standard storage costs $0.015 per GB-month and Class A operations $4.50 per million (October 2026): see [Free-plan limits: R2](../reference/limits.md#r2-files-binding-storage) for video-sized examples.

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

- Storage services other than R2: `store`, `serve` and the other runtime functions use the `STORAGE` binding. `S3Endpoint` presigns URLs for any S3-compatible store (AWS S3, MinIO), but there is no S3 client in the Worker and no mirroring.
- Image processing inside the Worker: variants come from Cloudflare Image Transformations.
- Cleanup of files whose rows are removed by `ON DELETE CASCADE`: the database deletes the child rows without calling the child model's `delete`, so their files stay in R2. Delete them in the parent's `delete` if that matters; `photos:attachments` children are deleted this way by the generated code.

## See also

- [Field types](../reference/field-types.md): `attachment` and the other types.
- [Generators](../reference/generators.md#ocre-g-scaffold): `ocre g scaffold` and [`ocre g api`](../reference/generators.md#ocre-g-api).
- [Configuration](../reference/configuration.md#env-storage-r2): the `STORAGE` entry.
- [Validations](validations.md): `Validator` and 422 responses.
- [Authentication](authentication.md): `CurrentUser` and ownership checks.
- [Free-plan limits](../reference/limits.md) and [Cost model](../explanations/cost-model.md).
- [`ocre::storage` rustdoc](/api/ocre/storage/index.html).
