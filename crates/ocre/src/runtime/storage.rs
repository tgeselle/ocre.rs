use axum::{body::Body, http::HeaderMap, response::Response};
use futures_util::StreamExt as _;
use worker::{
    Bucket, Conditional, Env, FixedLengthStream, HttpMetadata, Range, ResponseBody,
    send::{SendFuture, SendWrapper},
    web_sys,
};

use super::Ctx;
use crate::{
    Error, Result,
    storage::{
        Attachment, ByteRange, Disposition, Fetch, STORAGE_BINDING, Upload, file_response, missing_binding,
        not_modified, unsatisfiable,
    },
};

fn bucket(env: &Env) -> Result<Bucket> {
    env.bucket(STORAGE_BINDING).map_err(|err| missing_binding(&err.to_string()))
}

async fn put(env: &Env, attachment: &Attachment, data: worker::Data) -> Result<()> {
    let metadata = HttpMetadata { content_type: Some(attachment.content_type.clone()), ..Default::default() };
    bucket(env)?
        .put(&attachment.key, data)
        .http_metadata(metadata)
        .custom_metadata([("filename".to_owned(), attachment.filename.clone())])
        .execute()
        .await
        .map_err(|err| Error::internal(format!("R2 put of `{}` failed: {err}", attachment.key)))?;
    Ok(())
}

/// Stores an [`Upload`] in R2 under a new random key starting with `prefix`, and returns the [`Attachment`] to save.
///
/// The key is `<prefix>/<22 random characters>` (`posts/avatar/...`),
/// never derived from the file name; the file name and content type are
/// cleaned up like [`store_bytes`](crate::storage::store_bytes)'s. Check
/// the upload with [`Validator::file`](crate::Validator::file) first, and
/// save the attachment with [`columns`](crate::storage::columns); if saving
/// the row fails, [`delete`](crate::storage::delete) the new key.
///
/// Free plan: one R2 class A operation (1M free per month); copying 10 MB
/// to R2 takes about 0.15 ms of CPU. Stored files count towards the 10
/// GB-month of free storage.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing (the message shows the `[[r2_buckets]]` entry to add
/// to wrangler.toml) or the R2 write fails.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::State;
/// use ocre::storage::{self, Multipart, Rules};
/// use ocre::{Ctx, Error, Result, Validator};
///
/// const DOCUMENT: Rules = Rules { max_bytes: 10 * 1024 * 1024, content_types: &["application/pdf"] };
///
/// // PUT /documents with `curl -X PUT -F file=@spec.pdf`.
/// async fn upload(State(ctx): State<Ctx>, Multipart(mut form): Multipart<{ 11 * 1024 * 1024 }>) -> Result<String> {
///     let file = form.file("file").ok_or_else(|| Error::bad_request("Send the file as `file`"))?;
///     Validator::new().file("file", &file, &DOCUMENT).finish()?;
///     let document = storage::store(&ctx, "documents/file", file).await?;
///     Ok(document.key)
/// }
/// ```
pub fn store(ctx: &Ctx, prefix: &str, upload: Upload) -> impl Future<Output = Result<Attachment>> + Send + use<> {
    store_bytes(ctx, prefix, &upload.filename, &upload.content_type, Vec::from(upload.bytes))
}

/// Stores bytes built by the app (a generated report, an export) like [`store`](crate::storage::store).
///
/// The object gets a new random key under `prefix`; `filename` is cleaned
/// up (no directories or control characters, at most 200 characters) and
/// `content_type` normalized (lowercase, no parameters; empty becomes
/// `application/octet-stream`). The returned [`Attachment`] records all
/// four, ready to save with [`columns`](crate::storage::columns).
///
/// Free plan: one R2 class A operation (1M free per month); copying 10 MB
/// to R2 takes about 0.15 ms of CPU.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing (the message shows the `[[r2_buckets]]` entry to add
/// to wrangler.toml) or the R2 write fails.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::State;
/// use ocre::{Ctx, Result, storage};
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Post {
///     id: i64,
///     title: String,
/// }
///
/// // POST /exports: writes every post to a CSV kept in R2.
/// async fn export(State(ctx): State<Ctx>) -> Result<String> {
///     let posts: Vec<Post> = ctx.db()?.all("SELECT id, title FROM posts ORDER BY id", vec![]).await?;
///     let mut csv = String::from("id,title\n");
///     for post in posts {
///         csv.push_str(&format!("{},\"{}\"\n", post.id, post.title.replace('"', "\"\"")));
///     }
///     let file = storage::store_bytes(&ctx, "exports", "posts.csv", "text/csv", csv.into_bytes()).await?;
///     Ok(file.key)
/// }
/// ```
pub fn store_bytes(
    ctx: &Ctx,
    prefix: &str,
    filename: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> impl Future<Output = Result<Attachment>> + Send + use<> {
    let env = ctx.env().clone();
    let attachment = Attachment::prepare(prefix, filename, content_type, bytes.len() as u64);
    SendFuture::new(async move {
        put(&env, &attachment, bytes.into()).await?;
        Ok(attachment)
    })
}

/// Streams `body` (exactly `size` bytes) into R2 without holding it in memory, and returns its [`Attachment`].
///
/// For a raw request body with a `Content-Length` (`curl -T big.zip`),
/// where [`Multipart`](crate::storage::Multipart) would hold the whole body
/// in memory. R2 needs the length up front: a body of another length makes
/// the write fail and nothing is stored. The file name and content type are
/// cleaned up like [`store_bytes`]'s; no [`Rules`](crate::storage::Rules)
/// are checked, so check `size` and `content_type` first.
///
/// Free plan: one R2 class A operation (1M free per month). Chunks still
/// pass through WebAssembly, but memory stays flat; Cloudflare refuses
/// request bodies over 100 MB on the Free plan.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing (the message shows the `[[r2_buckets]]` entry to add
/// to wrangler.toml), the body is not `size` bytes long, or the R2 write fails.
///
/// # Examples
///
/// ```no_run
/// use axum::{body::Body, extract::State, http::{HeaderMap, header}};
/// use ocre::{Ctx, Error, Result, storage};
///
/// const MAX: u64 = 50 * 1024 * 1024;
///
/// // PUT /backups with `curl -T backup.zip`.
/// async fn upload(State(ctx): State<Ctx>, headers: HeaderMap, body: Body) -> Result<String> {
///     let size: u64 = headers
///         .get(header::CONTENT_LENGTH)
///         .and_then(|value| value.to_str().ok()?.parse().ok())
///         .ok_or_else(|| Error::bad_request("Content-Length required"))?;
///     if size > MAX {
///         return Err(Error::PayloadTooLarge("The backup is too large (maximum is 50 MB)".into()));
///     }
///     let backup = storage::store_body(&ctx, "backups", "backup.zip", "application/zip", size, body).await?;
///     Ok(backup.key)
/// }
/// ```
pub fn store_body(
    ctx: &Ctx,
    prefix: &str,
    filename: &str,
    content_type: &str,
    size: u64,
    body: Body,
) -> impl Future<Output = Result<Attachment>> + Send + use<> {
    let env = ctx.env().clone();
    let attachment = Attachment::prepare(prefix, filename, content_type, size);
    SendFuture::new(async move {
        let chunks = body
            .into_data_stream()
            .map(|chunk| chunk.map(|bytes| bytes.to_vec()).map_err(|err| worker::Error::RustError(err.to_string())));
        put(&env, &attachment, FixedLengthStream::wrap(chunks, size).into()).await?;
        Ok(attachment)
    })
}

/// Reads a whole object into memory, or returns `None` when `key` does not exist.
///
/// Meant for files the Worker itself processes (parsing an uploaded CSV,
/// attaching a file to an email). To send a file to a browser use
/// [`serve`](crate::storage::serve), which streams it without copying it
/// into WebAssembly and handles 304 and `Range`.
///
/// Free plan: one R2 class B operation (10M free per month); the bytes are
/// copied into WebAssembly memory (a Worker has 128 MB).
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing (the message shows the `[[r2_buckets]]` entry to add
/// to wrangler.toml) or R2 fails.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::{Path, State};
/// use ocre::{Ctx, OptionExt, Result, storage};
///
/// // Counts the lines of an uploaded CSV.
/// async fn line_count(State(ctx): State<Ctx>, Path(key): Path<String>) -> Result<String> {
///     let bytes = storage::read(&ctx, &key).await?.or_404()?;
///     Ok(bytes.split(|&b| b == b'\n').filter(|line| !line.is_empty()).count().to_string())
/// }
/// ```
pub fn read(ctx: &Ctx, key: &str) -> impl Future<Output = Result<Option<Vec<u8>>>> + Send + use<> {
    let env = ctx.env().clone();
    let key = key.to_owned();
    SendFuture::new(async move {
        let Some(object) = bucket(&env)?.get(&key).execute().await? else { return Ok(None) };
        match object.body() {
            Some(body) => Ok(Some(body.bytes().await?)),
            None => Ok(Some(Vec::new())),
        }
    })
}

/// Deletes the object at `key`.
///
/// Deleting a missing key is not an error, so a retried cleanup is safe.
/// Delete the object only once no row points to it any more (after the
/// database write succeeded). R2 deletes are free.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing (the message shows the `[[r2_buckets]]` entry to add
/// to wrangler.toml) or R2 fails.
///
/// # Examples
///
/// ```no_run
/// use axum::{extract::{Path, State}, http::StatusCode};
/// use ocre::{Ctx, Result, params, storage};
///
/// // DELETE /exports/{key}: forget a generated export.
/// async fn destroy(State(ctx): State<Ctx>, Path(key): Path<String>) -> Result<StatusCode> {
///     ctx.db()?.execute("DELETE FROM exports WHERE file_key = ?1", params![key.as_str()]).await?;
///     storage::delete(&ctx, &key).await?;
///     Ok(StatusCode::NO_CONTENT)
/// }
/// ```
pub fn delete(ctx: &Ctx, key: &str) -> impl Future<Output = Result<()>> + Send + use<> {
    let env = ctx.env().clone();
    let key = key.to_owned();
    SendFuture::new(async move { Ok(bucket(&env)?.delete(key).await?) })
}

/// Deletes the objects of every `Some` attachment in one R2 call.
///
/// `None` entries (optional attachments left empty) are skipped, and
/// nothing is sent when all are `None`. Generated models call it after a
/// record is deleted, or after an update replaced or removed its files, so
/// a failed database write never loses a file that a row still points to.
/// Missing keys are not an error. R2 deletes are free.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing (the message shows the `[[r2_buckets]]` entry to add
/// to wrangler.toml) or R2 fails.
///
/// # Examples
///
/// ```no_run
/// use axum::{extract::{Path, State}, http::StatusCode};
/// use ocre::storage::{self, Attachment};
/// use ocre::{Ctx, Result, params};
///
/// async fn destroy(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<StatusCode> {
///     let db = ctx.db()?;
///     let image: Option<Attachment> = db
///         .first(
///             "SELECT image_key AS key, image_filename AS filename, image_content_type AS content_type, \
///              image_size AS size FROM photos WHERE id = ?1 AND image_key IS NOT NULL",
///             params![id],
///         )
///         .await?;
///     db.execute("DELETE FROM photos WHERE id = ?1", params![id]).await?;
///     storage::delete_attachments(&ctx, &[image]).await?;
///     Ok(StatusCode::NO_CONTENT)
/// }
/// ```
pub fn delete_attachments(
    ctx: &Ctx,
    attachments: &[Option<Attachment>],
) -> impl Future<Output = Result<()>> + Send + use<> {
    let env = ctx.env().clone();
    let keys: Vec<String> = attachments.iter().flatten().map(|attachment| attachment.key.clone()).collect();
    SendFuture::new(async move {
        if !keys.is_empty() {
            bucket(&env)?.delete_multiple(keys).await?;
        }
        Ok(())
    })
}

/// Streams a stored file to the client, with the headers a browser needs for caching, seeking and saving it.
///
/// The response carries `Content-Type`, `Content-Length`,
/// `Content-Disposition` (the original file name; `inline` only for types
/// safe to display, see [`Disposition`](crate::storage::Disposition)),
/// `ETag` and `Cache-Control` ([`CACHE_CONTROL`](crate::storage::CACHE_CONTROL)).
/// A matching `If-None-Match` answers `304 Not Modified` without a body; a
/// single `Range` (video seeking, resumed downloads) answers 206 with
/// `Content-Range`, or 416 when it lies outside the file (without calling
/// R2). Several ranges, other units and malformed values send the whole
/// file, as RFC 9110 allows. The headers come from `attachment`, so it must
/// be the row saved for that key.
///
/// Check that the user may see the record before calling it: the route is
/// the only protection.
///
/// Free plan: one R2 class B operation per call, 304s included (10M free
/// per month). The bytes never pass through WebAssembly: the R2 stream is
/// attached to the response and [`crate::serve`] answers with it directly,
/// so a download costs almost no CPU whatever its size.
///
/// # Errors
///
/// - [`Error::NotFound`](crate::Error::NotFound) (404) when no object has
///   `attachment.key`.
/// - [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
///   binding is missing (the message shows the `[[r2_buckets]]` entry to add
///   to wrangler.toml) or R2 fails.
///
/// # Examples
///
/// ```no_run
/// use axum::{extract::{Path, Query, State}, http::HeaderMap, response::Response};
/// use ocre::storage::{self, Attachment, Disposition};
/// use ocre::{Ctx, OptionExt, Result, params};
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Download {
///     #[serde(default)]
///     download: bool,
/// }
///
/// // GET /photos/{id}/image, or /photos/{id}/image?download=true for "Save as".
/// async fn image(
///     State(ctx): State<Ctx>,
///     Path(id): Path<i64>,
///     Query(query): Query<Download>,
///     headers: HeaderMap,
/// ) -> Result<Response> {
///     let sql = "SELECT image_key AS key, image_filename AS filename, image_content_type AS content_type, \
///                image_size AS size FROM photos WHERE id = ?1";
///     let image: Attachment = ctx.db()?.first(sql, params![id]).await?.or_404()?;
///     let disposition = if query.download { Disposition::Download } else { Disposition::Inline };
///     storage::serve(&ctx, &image, &headers, disposition).await
/// }
/// ```
pub fn serve(
    ctx: &Ctx,
    attachment: &Attachment,
    headers: &HeaderMap,
    disposition: Disposition,
) -> impl Future<Output = Result<Response>> + Send + use<> {
    let env = ctx.env().clone();
    let attachment = attachment.clone();
    let fetch = Fetch::from_headers(headers, attachment.size);
    SendFuture::new(async move {
        if fetch.range == ByteRange::Unsatisfiable {
            return Ok(unsatisfiable(attachment.size));
        }
        let bucket = bucket(&env)?;
        let mut get = bucket.get(&attachment.key);
        if let Some(etag) = fetch.if_none_match {
            get = get.only_if(Conditional { etag_does_not_match: Some(etag), ..Default::default() });
        }
        if let ByteRange::Partial { offset, length } = fetch.range {
            get = get.range(Range::OffsetWithLength { offset, length });
        }
        let object = get.execute().await?.ok_or(Error::NotFound)?;
        let etag = object.http_etag();
        let Some(body) = object.body() else { return Ok(not_modified(&etag)) };
        let mut response = file_response(&attachment, disposition, &etag, fetch.range, Body::empty());
        match body.response_body()? {
            // Handed to `ocre::serve`, which answers with this stream itself.
            ResponseBody::Stream(stream) => {
                response.extensions_mut().insert(R2Stream(SendWrapper::new(stream)));
            }
            ResponseBody::Body(bytes) => *response.body_mut() = Body::from(bytes),
            ResponseBody::Empty => {}
        }
        Ok(response)
    })
}

/// An R2 object body attached to a response by [`serve`]: `ocre::serve`
/// sends it as the JavaScript response body, so the bytes never pass
/// through WebAssembly and workerd keeps `Content-Length`.
#[derive(Clone)]
pub(crate) struct R2Stream(SendWrapper<web_sys::ReadableStream>);

/// The JavaScript response for `response`'s status and headers, with `stream` as its body.
pub(crate) fn into_js_response(response: Response, stream: R2Stream) -> worker::Result<web_sys::Response> {
    let headers = web_sys::Headers::new()?;
    for (name, value) in response.headers() {
        headers.append(name.as_str(), value.to_str().unwrap_or_default())?;
    }
    let init = web_sys::ResponseInit::new();
    init.set_status(response.status().as_u16());
    init.set_headers(&headers);
    Ok(web_sys::Response::new_with_opt_readable_stream_and_init(Some(&stream.0), &init)?)
}
