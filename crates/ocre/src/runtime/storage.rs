use axum::{body::Body, http::HeaderMap, response::Response};
use futures_util::StreamExt as _;
use worker::{
    Bucket, Conditional, Env, FixedLengthStream, HttpMetadata, Include, Object, Range, ResponseBody,
    send::{SendFuture, SendWrapper},
    web_sys,
};

use super::Ctx;
use crate::{
    Error, IntoParam as _, Result,
    storage::{
        Attachment, ByteRange, DirectUpload, DirectUploadRequest, Disposition, Fetch, KEYS_PER_QUERY, Listing,
        MAX_LIST, Purged, Rules, S3Endpoint, STORAGE_BINDING, STORAGE_PUBLIC_URL, StoredObject, Upload,
        attachment_from_head, essence, file_response, join_public_url, missing_binding, not_modified, presign_get_url,
        r2_endpoint, redirect_response, referenced_keys_sql, stale_keys, unsatisfiable, upload_secret, verify_key,
    },
};

pub(super) fn bucket(env: &Env) -> Result<Bucket> {
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
/// binding is missing (the message shows the `STORAGE` entry to add
/// to cloudflare.config.ts) or the R2 write fails.
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
/// binding is missing (the message shows the `STORAGE` entry to add
/// to cloudflare.config.ts) or the R2 write fails.
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
/// binding is missing (the message shows the `STORAGE` entry to add
/// to cloudflare.config.ts), the body is not `size` bytes long, or the R2 write fails.
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
/// binding is missing (the message shows the `STORAGE` entry to add
/// to cloudflare.config.ts) or R2 fails.
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
/// binding is missing (the message shows the `STORAGE` entry to add
/// to cloudflare.config.ts) or R2 fails.
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
/// binding is missing (the message shows the `STORAGE` entry to add
/// to cloudflare.config.ts) or R2 fails.
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

/// The [`StoredObject`] of an R2 object (from `head`, `get` or `list` with metadata).
fn stored(object: &Object) -> StoredObject {
    StoredObject {
        key: object.key(),
        size: object.size(),
        content_type: essence(object.http_metadata().content_type.as_deref().unwrap_or_default()),
        etag: object.etag(),
        uploaded_at: i64::try_from(object.uploaded().as_millis() / 1000).unwrap_or(i64::MAX),
        filename: object.custom_metadata().ok().and_then(|mut metadata| metadata.remove("filename")),
    }
}

/// The S3 endpoint of the `R2_*` variables and secrets.
fn endpoint(env: &Env) -> Result<S3Endpoint> {
    r2_endpoint(&|name| env.var(name).ok().map(|value| value.to_string()))
}

/// How long the URL of a [`direct_upload`] can be used to start the `PUT`: 10 minutes.
const DIRECT_UPLOAD_EXPIRES_IN: u64 = 600;

/// Describes the object at `key` without reading it: size, content type, ETag, upload time; `None` when it does not exist.
///
/// Active Storage's `blob.byte_size`/`service.exist?` in one call. Direct
/// uploads are checked with it ([`attach_direct_upload`]), and purges read
/// the upload time.
///
/// Free plan: one R2 class B operation (10M free per month).
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing (the message shows the `STORAGE` entry to add
/// to cloudflare.config.ts) or R2 fails.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::{Path, State};
/// use ocre::{Ctx, OptionExt, Result, storage};
///
/// // GET /files/{*key}/size
/// async fn size(State(ctx): State<Ctx>, Path(key): Path<String>) -> Result<String> {
///     let object = storage::head(&ctx, &key).await?.or_404()?;
///     Ok(format!("{} ({}), uploaded at {}", storage::human_size(object.size), object.content_type, object.uploaded_at))
/// }
/// ```
pub fn head(ctx: &Ctx, key: &str) -> impl Future<Output = Result<Option<StoredObject>>> + Send + use<> {
    let env = ctx.env().clone();
    let key = key.to_owned();
    SendFuture::new(async move { Ok(bucket(&env)?.head(key).await?.as_ref().map(stored)) })
}

/// Returns whether an object exists at `key` (Active Storage's `exist?`); a [`head`] without the details.
///
/// Free plan: one R2 class B operation (10M free per month).
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing or R2 fails.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::{Path, State};
/// use ocre::{Ctx, Result, storage};
///
/// async fn check(State(ctx): State<Ctx>, Path(key): Path<String>) -> Result<String> {
///     Ok(storage::exists(&ctx, &key).await?.to_string())
/// }
/// ```
pub fn exists(ctx: &Ctx, key: &str) -> impl Future<Output = Result<bool>> + Send + use<> {
    let object = head(ctx, key);
    async move { Ok(object.await?.is_some()) }
}

/// Lists one page of the objects whose key starts with `prefix`, in key order, with their size, type and upload time.
///
/// Pass `None` as `cursor` for the first page, then the returned
/// [`Listing::cursor`] until it is `None`. `limit` is clamped to 1..=1000;
/// a page may hold fewer objects even when more follow (R2 caps the
/// metadata it returns), so rely on the cursor, not the count.
///
/// Free plan: one R2 class A operation per call, like an upload (1M free
/// per month), and decoding 1,000 entries takes a few ms of the 10 ms of
/// CPU: list in scheduled tasks or admin pages, never on every request.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing or R2 fails.
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result, storage};
///
/// // Total size of the exports, 1,000 keys (one class A operation) at a time.
/// async fn exports_size(ctx: &Ctx) -> Result<u64> {
///     let (mut total, mut cursor) = (0, None);
///     loop {
///         let page = storage::list(ctx, "exports/", cursor.as_deref(), 1000).await?;
///         total += page.objects.iter().map(|object| object.size).sum::<u64>();
///         cursor = page.cursor;
///         if cursor.is_none() {
///             return Ok(total);
///         }
///     }
/// }
/// ```
pub fn list(
    ctx: &Ctx,
    prefix: &str,
    cursor: Option<&str>,
    limit: u32,
) -> impl Future<Output = Result<Listing>> + Send + use<> {
    let env = ctx.env().clone();
    let prefix = prefix.to_owned();
    let cursor = cursor.map(str::to_owned);
    let limit = limit.clamp(1, MAX_LIST);
    SendFuture::new(async move {
        let bucket = bucket(&env)?;
        let mut request =
            bucket.list().prefix(prefix).limit(limit).include(vec![Include::HttpMetadata, Include::CustomMetadata]);
        if let Some(cursor) = cursor {
            request = request.cursor(cursor);
        }
        let page = request.execute().await?;
        let objects = page.objects().iter().map(stored).collect();
        Ok(Listing { objects, cursor: if page.truncated() { page.cursor() } else { None } })
    })
}

/// Reads the first `length` bytes of an object (all of it when shorter), or `None` when `key` does not exist.
///
/// For [`analyze`](crate::storage::analyze) on a file uploaded directly:
/// the signature and image size are in the first few KB (use 256 KB for
/// JPEGs with large EXIF blocks), so the Worker never holds the whole file.
///
/// Free plan: one R2 class B operation (10M free per month).
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing or R2 fails.
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result, storage};
///
/// async fn dimensions(ctx: &Ctx, key: &str) -> Result<Option<(u32, u32)>> {
///     let Some(start) = storage::read_first(ctx, key, 64 * 1024).await? else { return Ok(None) };
///     let analysis = storage::analyze(&start);
///     Ok(analysis.width.zip(analysis.height))
/// }
/// ```
pub fn read_first(ctx: &Ctx, key: &str, length: u64) -> impl Future<Output = Result<Option<Vec<u8>>>> + Send + use<> {
    let env = ctx.env().clone();
    let key = key.to_owned();
    let length = length.max(1);
    SendFuture::new(async move {
        let bucket = bucket(&env)?;
        let Some(object) = bucket.get(&key).range(Range::Prefix { length }).execute().await? else { return Ok(None) };
        match object.body() {
            Some(body) => Ok(Some(body.bytes().await?)),
            None => Ok(Some(Vec::new())),
        }
    })
}

/// Presigns a `GET` of an attachment on R2's S3 API, valid `expires_in` seconds (at most 7 days).
///
/// The browser downloads straight from R2 (`https://<account>.r2.cloudflarestorage.com/...`),
/// not through the Worker. R2 answers with the same safe `Content-Type`
/// and `Content-Disposition` as [`serve`] (the URL carries
/// `response-content-type` and `response-content-disposition`). Anyone with
/// the URL can download the file until it expires: authorize before
/// presigning and keep lifetimes short. [`serve_redirect`] answers a
/// redirect to it.
///
/// Reads the [`R2_ACCOUNT_ID`](crate::storage::R2_ACCOUNT_ID) and
/// [`R2_BUCKET`](crate::storage::R2_BUCKET) variables and the
/// [`R2_ACCESS_KEY_ID`](crate::storage::R2_ACCESS_KEY_ID) and
/// [`R2_SECRET_ACCESS_KEY`](crate::storage::R2_SECRET_ACCESS_KEY) secrets.
/// Signing is local: no R2 operation, microseconds of CPU; the download
/// is one class B operation. In `ocre dev` the URL points to the real
/// bucket, not the local simulation.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when one of the four
/// settings is missing (the message names them and how to set them), or
/// `expires_in` is 0 or over 7 days.
///
/// # Examples
///
/// ```no_run
/// use axum::{Json, extract::{Path, State}};
/// use ocre::storage::{self, Attachment, Disposition};
/// use ocre::{Ctx, OptionExt, Result, params};
///
/// // GET /api/documents/{id}/download_url: a link valid 5 minutes.
/// async fn download_url(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Json<String>> {
///     let sql = "SELECT file_key AS key, file_filename AS filename, file_content_type AS content_type, \
///                file_size AS size FROM documents WHERE id = ?1 AND file_key IS NOT NULL";
///     let file: Attachment = ctx.db()?.first(sql, params![id]).await?.or_404()?;
///     Ok(Json(storage::presign_get(&ctx, &file, Disposition::Download, 300)?))
/// }
/// ```
pub fn presign_get(ctx: &Ctx, attachment: &Attachment, disposition: Disposition, expires_in: u64) -> Result<String> {
    presign_get_url(&endpoint(ctx.env())?, attachment, disposition, crate::now(), expires_in)
}

/// Presigns a `PUT` of exactly `size` bytes of type `content_type` at `key`, valid `expires_in` seconds.
///
/// The URL signs `Content-Type` (normalized) and `Content-Length`: R2
/// refuses a `PUT` with another type or size. For browser uploads prefer
/// [`direct_upload`], which also picks a new key, checks [`Rules`] and
/// signs the key for [`attach_direct_upload`]. Settings, costs and errors
/// as [`presign_get`]; the upload is one class A operation.
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result, storage};
///
/// // A URL a backup script can `curl -T` to, valid one hour.
/// fn backup_url(ctx: &Ctx, size: u64) -> Result<String> {
///     storage::presign_put(ctx, "backups/latest.tar", "application/x-tar", size, 3600)
/// }
/// ```
pub fn presign_put(ctx: &Ctx, key: &str, content_type: &str, size: u64, expires_in: u64) -> Result<String> {
    let content_type = essence(content_type);
    let size = size.to_string();
    let headers = [("content-type", content_type.as_str()), ("content-length", size.as_str())];
    endpoint(ctx.env())?.presign("PUT", key, &headers, &[], crate::now(), expires_in)
}

/// Starts a direct upload: checks what the browser declares against `rules` and presigns a `PUT` of a new key under `prefix`.
///
/// Rails' `DirectUploadsController#create`. The browser `PUT`s the file to
/// [`DirectUpload::url`] with [`DirectUpload::headers`] (within 10
/// minutes), then submits [`DirectUpload::signed_key`] with its form;
/// [`attach_direct_upload`] checks the stored object and returns its
/// [`Attachment`]. The file never passes through the Worker: no 100 MB
/// request limit, no Worker memory or CPU. The key is
/// `<prefix>/<22 random characters>`; use a prefix of its own (`uploads/photos`)
/// so [`purge_unattached`] can find abandoned uploads.
///
/// Settings as [`presign_get`]; the bucket also needs a CORS rule allowing
/// `PUT` from the app's origin (see the file storage guide). Signing costs
/// no R2 operation; the upload is one class A operation.
///
/// # Errors
///
/// - [`Error::Invalid`](crate::Error::Invalid) (422) on `field` when the
///   declared size or type breaks `rules`.
/// - [`Error::Internal`](crate::Error::Internal) (500) when an `R2_*`
///   setting is missing.
///
/// # Examples
///
/// ```no_run
/// use axum::{Json, extract::State};
/// use ocre::storage::{self, DirectUpload, DirectUploadRequest, Rules};
/// use ocre::{ApiResult, Ctx};
///
/// const VIDEO: Rules = Rules { max_bytes: 500 * 1024 * 1024, content_types: &["video/mp4"] };
///
/// // POST /videos/uploads with {"filename": "a.mp4", "content_type": "video/mp4", "size": 1234}
/// async fn start(State(ctx): State<Ctx>, Json(request): Json<DirectUploadRequest>) -> ApiResult<Json<DirectUpload>> {
///     Ok(Json(storage::direct_upload(&ctx, "uploads/videos", "video", &request, &VIDEO)?))
/// }
/// ```
pub fn direct_upload(
    ctx: &Ctx,
    prefix: &str,
    field: &str,
    request: &DirectUploadRequest,
    rules: &Rules,
) -> Result<DirectUpload> {
    DirectUpload::sign(&endpoint(ctx.env())?, prefix, field, request, rules, crate::now(), DIRECT_UPLOAD_EXPIRES_IN)
}

/// Finishes a direct upload (or a [`multipart_uploads`](crate::storage::multipart_uploads) one): checks the object behind `signed_key` against `rules` and returns its [`Attachment`].
///
/// Rails' `attach(signed_blob_id)`. `signed_key` must come from
/// [`direct_upload`] (a key this app signed, so a client cannot claim
/// another record's file); the object must exist, and its size and
/// content type (as R2 recorded them) must pass `rules`. A refused object
/// is deleted. `filename` is the name the form sends (cleaned up). Save
/// the attachment with [`columns`](crate::storage::columns); if that write
/// fails, [`delete`] the key.
///
/// Free plan: one R2 class B operation (`head`), plus a free delete when
/// the object is refused.
///
/// # Errors
///
/// - [`Error::Invalid`](crate::Error::Invalid) (422) on `field`: "is not a
///   valid upload" (bad signature), "was not uploaded" (no object), or the
///   messages of [`Validator::file`](crate::Validator::file).
/// - [`Error::Internal`](crate::Error::Internal) (500) when neither
///   `R2_SECRET_ACCESS_KEY` nor `SECRET_KEY_BASE` is set (the secret that
///   signs upload keys), the `STORAGE` binding is missing, or R2 fails.
///
/// # Examples
///
/// ```no_run
/// use axum::{Form, extract::{Path, State}};
/// use ocre::storage::{self, Rules};
/// use ocre::{Ctx, IntoParam, Result};
/// use serde::Deserialize;
///
/// const VIDEO: Rules = Rules { max_bytes: 500 * 1024 * 1024, content_types: &["video/mp4"] };
///
/// #[derive(Deserialize)]
/// struct VideoForm {
///     video_key: String,
///     video_filename: String,
/// }
///
/// async fn update(State(ctx): State<Ctx>, Path(id): Path<i64>, Form(form): Form<VideoForm>) -> Result<String> {
///     let video = storage::attach_direct_upload(&ctx, "video", &form.video_key, &form.video_filename, &VIDEO).await?;
///     let mut values = Vec::from(storage::columns(Some(&video)));
///     values.push(id.into_param());
///     let sql = "UPDATE lessons SET video_key = ?1, video_filename = ?2, video_content_type = ?3, video_size = ?4 \
///                WHERE id = ?5";
///     if let Err(err) = ctx.db()?.execute(sql, values).await {
///         storage::delete(&ctx, &video.key).await?;
///         return Err(err);
///     }
///     Ok(video.key)
/// }
/// ```
pub fn attach_direct_upload(
    ctx: &Ctx,
    field: &str,
    signed_key: &str,
    filename: &str,
    rules: &Rules,
) -> impl Future<Output = Result<Attachment>> + Send + use<> {
    let env = ctx.env().clone();
    let (field, signed_key, filename, rules) = (field.to_owned(), signed_key.to_owned(), filename.to_owned(), *rules);
    SendFuture::new(async move {
        let key = verify_key(&field, &upload_secret(&|name| env.var(name).ok().map(|v| v.to_string()))?, &signed_key)?;
        let bucket = bucket(&env)?;
        let object = bucket.head(key).await?.as_ref().map(stored);
        let attached = attachment_from_head(&field, object.as_ref(), &filename, &rules);
        if let (Err(_), Some(refused)) = (&attached, object) {
            bucket.delete(refused.key).await?;
        }
        attached
    })
}

/// Answers `302 Found` to a presigned `GET` of the attachment (Active Storage's redirect mode).
///
/// For large or popular files: the browser downloads from R2 directly,
/// so the Worker only signs a URL. The redirect may be cached privately
/// for half of `expires_in`. Authorize before calling it, as with
/// [`serve`]: whoever gets the URL can use it until it expires. Settings,
/// costs and errors as [`presign_get`].
///
/// # Examples
///
/// ```no_run
/// use axum::{extract::{Path, State}, response::Response};
/// use ocre::storage::{self, Attachment, Disposition};
/// use ocre::{Ctx, OptionExt, Result, params};
///
/// // GET /videos/{id}/file: a redirect valid one hour.
/// async fn file(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Response> {
///     let sql = "SELECT file_key AS key, file_filename AS filename, file_content_type AS content_type, \
///                file_size AS size FROM videos WHERE id = ?1";
///     let video: Attachment = ctx.db()?.first(sql, params![id]).await?.or_404()?;
///     storage::serve_redirect(&ctx, &video, Disposition::Inline, 3600)
/// }
/// ```
pub fn serve_redirect(
    ctx: &Ctx,
    attachment: &Attachment,
    disposition: Disposition,
    expires_in: u64,
) -> Result<Response> {
    Ok(redirect_response(&presign_get(ctx, attachment, disposition, expires_in)?, expires_in))
}

/// The permanent public URL of `key` in a public bucket: `<STORAGE_PUBLIC_URL>/<key>` (Active Storage's `public: true`).
///
/// Requires public access on the bucket (an `r2.dev` URL, rate-limited and
/// meant for development, or a custom domain) and the
/// [`STORAGE_PUBLIC_URL`](crate::storage::STORAGE_PUBLIC_URL) variable.
/// Every object of the bucket is then public forever to whoever has its
/// key, with no authorization and no expiry: use it for avatars and
/// product images, never for private documents. Pure string building: no
/// R2 operation; downloads are class B operations on R2's side.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when
/// `STORAGE_PUBLIC_URL` is not set (the message says how).
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result, storage::{self, Attachment}};
///
/// fn avatar_src(ctx: &Ctx, avatar: &Attachment) -> Result<String> {
///     storage::public_url(ctx, &avatar.key) // https://files.example.com/users/avatar/2u1Vd0zJ8sQqS6rJq0rVmA
/// }
/// ```
pub fn public_url(ctx: &Ctx, key: &str) -> Result<String> {
    join_public_url(ctx.env().var(STORAGE_PUBLIC_URL).ok().map(|value| value.to_string()), key)
}

/// Deletes the objects under `prefix` older than `max_age` seconds that no `table.column` row references, one listing page per call.
///
/// Direct uploads that were never attached (a closed tab, a failed form)
/// would stay in R2 forever: Rails' `ActiveStorage::Blob.unattached`
/// purge. Each call lists one page of up to 1,000 keys from `cursor`
/// (`None` to start), keeps those uploaded more than `max_age` seconds
/// ago, looks them up with `SELECT <column> FROM <table> WHERE <column> IN
/// (...)` (100 keys per query) and deletes the others in one call. Pass
/// the returned [`Purged::cursor`] to the next call until it is `None`.
/// `table` and `column` are identifiers from your code (letters, digits,
/// `_`); give the column an index (`CREATE UNIQUE INDEX ... ON
/// lessons(video_key)`) so each lookup reads only matching rows. Keep
/// `max_age` well above 10 minutes (the lifetime of an upload URL) plus
/// the time a form stays open: a day is safe.
///
/// Free plan: one R2 class A operation per call (the listing), one D1
/// query per 100 old keys, deletes free; a few ms of CPU per 1,000 keys.
/// Run it from a scheduled task, a few pages per run.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when `table` or
/// `column` is not a plain identifier, the `STORAGE` or `DB` binding is
/// missing, or R2 or D1 fails (nothing is deleted for the page then).
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result, storage};
///
/// // src/schedules/purge_uploads.rs, run every day: at most 5 pages (5 class A operations).
/// pub async fn run(ctx: &Ctx) -> Result<()> {
///     let mut cursor = None;
///     for _ in 0..5 {
///         let purged = storage::purge_unattached(ctx, "uploads/videos", "lessons", "video_key", 86_400, cursor.as_deref()).await?;
///         println!("purge_uploads: {} unattached uploads deleted", purged.deleted.len());
///         cursor = purged.cursor;
///         if cursor.is_none() {
///             break;
///         }
///     }
///     Ok(())
/// }
/// ```
pub fn purge_unattached(
    ctx: &Ctx,
    prefix: &str,
    table: &str,
    column: &str,
    max_age: i64,
    cursor: Option<&str>,
) -> impl Future<Output = Result<Purged>> + Send + use<> {
    #[derive(serde::Deserialize)]
    struct Row {
        key: String,
    }
    let ctx = ctx.clone();
    let (table, column) = (table.to_owned(), column.to_owned());
    let page = list(&ctx, prefix, cursor, MAX_LIST);
    SendFuture::new(async move {
        referenced_keys_sql(&table, &column, 0)?;
        let page = page.await?;
        let mut stale = stale_keys(&page.objects, crate::now() - max_age);
        if !stale.is_empty() {
            let db = ctx.db()?;
            let mut referenced = std::collections::HashSet::new();
            for keys in stale.chunks(KEYS_PER_QUERY) {
                let sql = referenced_keys_sql(&table, &column, keys.len())?;
                let params = keys.iter().map(|key| key.as_str().into_param()).collect();
                let rows: Vec<Row> = db.all(&sql, params).await?;
                referenced.extend(rows.into_iter().map(|row| row.key));
            }
            stale.retain(|key| !referenced.contains(key));
            if !stale.is_empty() {
                bucket(ctx.env())?.delete_multiple(stale.iter().map(String::as_str).collect()).await?;
            }
        }
        Ok(Purged { deleted: stale, cursor: page.cursor })
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
///   binding is missing (the message shows the `STORAGE` entry to add
///   to cloudflare.config.ts) or R2 fails.
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
