//! Storage operations for files another service uploads or reads (ranged
//! reads, copies, multipart uploads driven by the app, ListParts and
//! ListMultipartUploads on R2's S3 API) and the development route that
//! stands for presigned URLs.

use axum::{
    Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::StreamExt as _;
use worker::{
    Env, FixedLengthStream, HttpMetadata, Include, Range, ResponseBody, UploadedPart, send::SendFuture, web_sys,
};

use super::{
    Ctx,
    storage::{bucket, dev_base, endpoint, r2_retried},
};
use crate::{
    ApiError, Error, Result, http,
    log::Logger,
    retry::{RETRIES, s3_retryable, with_retries},
    storage::{
        CompletedPart, DEV_MARKERS, DEV_STORAGE_PATH, DevGet, DevPut, Disposition, MAX_PARTS, PendingUpload,
        StoredPart, check_expires_in, dev_parts_prefix, dev_upload_from_marker, dev_upload_marker, dev_url, essence,
        parse_parts, parse_uploads, upload_gone,
    },
};

/// Pages of a ListParts or ListMultipartUploads read at most (1,000 entries each).
const MAX_PAGES: usize = 10;

/// How long the Worker's own signed S3 calls are valid.
const S3_CALL_EXPIRES_IN: u64 = 300;

/// Largest single `PUT` R2 accepts (5 GiB less 5 MiB); [`copy`] copies larger objects in parts.
const MAX_SINGLE_PUT: u64 = 5 * 1024 * 1024 * 1024 - 5 * 1024 * 1024;

/// Parts of a large [`copy`]: 1 GiB, two subrequests each.
const COPY_PART: u64 = 1024 * 1024 * 1024;

fn r2_error(action: &str, key: &str, err: impl std::fmt::Display) -> Error {
    Error::internal(format!("R2 could not {action} `{key}`: {err}"))
}

/// The JavaScript stream of an R2 object's body, to write it elsewhere without copying it into WebAssembly.
fn js_stream(object: &worker::Object) -> Result<Option<web_sys::ReadableStream>> {
    let Some(body) = object.body() else { return Ok(None) };
    match body.response_body()? {
        ResponseBody::Stream(stream) => Ok(Some(stream)),
        _ => Err(Error::internal("R2 answered a body that is not a stream")),
    }
}

/// Reads `length` bytes of an object from `offset` (fewer at its end), or returns `None` when `key` does not exist.
///
/// For files read piece by piece without holding them whole: a video's
/// index at its end, a ZIP's central directory, the header of a large
/// file. [`read_first`](crate::storage::read_first) reads from the start.
///
/// Free plan: one R2 class B operation (10M free per month).
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing, or R2 fails (an `offset` past the end of the object
/// is a failure of R2's).
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result, storage};
///
/// // The last 64 KB of a file (a ZIP's central directory).
/// async fn tail(ctx: &Ctx, key: &str) -> Result<Option<Vec<u8>>> {
///     let Some(object) = storage::head(ctx, key).await? else { return Ok(None) };
///     let length = object.size.min(64 * 1024);
///     storage::read_range(ctx, key, object.size - length, length).await
/// }
/// ```
pub fn read_range(
    ctx: &Ctx,
    key: &str,
    offset: u64,
    length: u64,
) -> impl Future<Output = Result<Option<Vec<u8>>>> + Send + use<> {
    let env = ctx.env().clone();
    let key = key.to_owned();
    let length = length.max(1);
    SendFuture::new(async move {
        let range = Range::OffsetWithLength { offset, length };
        let (bucket, key) = (&bucket(&env)?, key.as_str());
        let get = move || bucket.get(key).range(range.clone()).execute();
        let object = r2_retried("read", key, get).await.map_err(|err| r2_error("read", key, err))?;
        let Some(object) = object else { return Ok(None) };
        match object.body() {
            Some(body) => Ok(Some(body.bytes().await?)),
            None => Ok(Some(Vec::new())),
        }
    })
}

/// Copies the object at `from` to `to` inside the bucket (content type kept) and returns its size, or `None` when `from` does not exist.
///
/// The bytes stream from R2 to R2 through the Worker without being held in
/// memory. Objects up to 5 GiB are one `PUT`; larger ones are copied in
/// parts of 1 GiB (a 10 GB file: 10 parts, about 20 subrequests). An
/// object already at `to` is replaced.
///
/// Free plan: one class B operation per read (one per part) and one class
/// A operation per write (plus the create and complete of a multipart copy).
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing or R2 fails (a multipart copy is aborted then).
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result, storage};
///
/// // Keeps a copy of a reported file before its owner can delete it.
/// async fn preserve(ctx: &Ctx, key: &str, report_id: i64) -> Result<bool> {
///     Ok(storage::copy(ctx, key, &format!("evidence/{report_id}")).await?.is_some())
/// }
/// ```
pub fn copy(ctx: &Ctx, from: &str, to: &str) -> impl Future<Output = Result<Option<u64>>> + Send + use<> {
    let env = ctx.env().clone();
    let (from, to) = (from.to_owned(), to.to_owned());
    SendFuture::new(async move {
        let bucket = &bucket(&env)?;
        let from_ref = from.as_str();
        let head = r2_retried("read", from_ref, move || bucket.head(from_ref)).await;
        let Some(head) = head.map_err(|err| r2_error("read", &from, err))? else {
            return Ok(None);
        };
        let size = head.size();
        let metadata = head.http_metadata();
        if size <= MAX_SINGLE_PUT {
            let get = move || bucket.get(from_ref).execute();
            let object = r2_retried("read", from_ref, get).await.map_err(|err| r2_error("read", &from, err))?;
            let Some(stream) = object.as_ref().map(js_stream).transpose()?.flatten() else { return Ok(None) };
            bucket
                .put(&to, worker::Data::ReadableStream(stream))
                .http_metadata(metadata)
                .execute()
                .await
                .map_err(|err| r2_error("write", &to, err))?;
            return Ok(Some(size));
        }
        let upload = bucket
            .create_multipart_upload(&to)
            .http_metadata(metadata)
            .execute()
            .await
            .map_err(|err| r2_error("start the copy to", &to, err))?;
        let mut parts = Vec::new();
        let (mut offset, mut number) = (0, 1);
        while offset < size {
            let length = COPY_PART.min(size - offset);
            let get = move || bucket.get(from_ref).range(Range::OffsetWithLength { offset, length }).execute();
            let read = r2_retried("read", from_ref, get).await;
            let stream = match read {
                Ok(object) => object.as_ref().map(js_stream).transpose()?.flatten(),
                Err(err) => {
                    upload.abort().await.ok();
                    return Err(r2_error("read", &from, err));
                }
            };
            let Some(stream) = stream else {
                upload.abort().await.ok();
                return Ok(None);
            };
            match upload.upload_part(number, worker::Data::ReadableStream(stream)).await {
                Ok(part) => parts.push(part),
                Err(err) => {
                    upload.abort().await.ok();
                    return Err(r2_error("copy a part to", &to, err));
                }
            }
            offset += length;
            number += 1;
        }
        upload.complete(parts).await.map_err(|err| r2_error("assemble the copy", &to, err))?;
        Ok(Some(size))
    })
}

/// Starts a multipart upload of `key` with `content_type`, and returns its upload id.
///
/// For uploads the app drives itself: a service (a GPU worker, a partner's
/// API) uploads the parts to [`presign_part`] URLs, then the app lists them
/// ([`list_parts`]) and assembles the object ([`complete_multipart`]).
/// Browser uploads of a form's file use
/// [`multipart_uploads`](crate::storage::multipart_uploads) instead. Parts
/// are 5 MiB at least (but the last) and number 1 to 10,000. R2 drops an
/// upload left unfinished after 7 days; [`abort_multipart`] drops it at
/// once.
///
/// Free plan: one R2 class A operation, then one per part and one for the
/// completion.
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
/// // Where a GPU worker uploads a 3-part result: the upload id and one URL per part, valid a day.
/// async fn output_target(ctx: &Ctx, job_id: i64) -> Result<(String, Vec<String>)> {
///     let key = format!("processed/{job_id}.mp4");
///     let upload_id = storage::create_multipart(ctx, &key, "video/mp4").await?;
///     let urls = (1..=3).map(|part| storage::presign_part(ctx, &key, &upload_id, part, 86_400)).collect::<Result<_>>()?;
///     Ok((upload_id, urls))
/// }
/// ```
pub fn create_multipart(
    ctx: &Ctx,
    key: &str,
    content_type: &str,
) -> impl Future<Output = Result<String>> + Send + use<> {
    let env = ctx.env().clone();
    let (key, content_type) = (key.to_owned(), essence(content_type));
    SendFuture::new(async move { create(&env, &key, content_type).await })
}

pub(super) async fn create(env: &Env, key: &str, content_type: String) -> Result<String> {
    let metadata = HttpMetadata { content_type: Some(content_type), ..Default::default() };
    let bucket = bucket(env)?;
    let upload = bucket
        .create_multipart_upload(key)
        .http_metadata(metadata)
        .execute()
        .await
        .map_err(|err| r2_error("start the upload of", key, err))?;
    let upload_id = upload.upload_id().await;
    if dev_base(env).is_some() {
        bucket
            .put(dev_upload_marker(key, &upload_id), Vec::new())
            .execute()
            .await
            .map_err(|err| r2_error("record the upload of", key, err))?;
    }
    Ok(upload_id)
}

/// Presigns the `PUT` of part `part_number` of the multipart upload `upload_id` of `key`, valid `expires_in` seconds.
///
/// The uploader `PUT`s the part's bytes to the URL; R2 answers its `ETag`
/// (also found later with [`list_parts`]). In debug builds the URL is the
/// app's development route on the local bucket. Signing is local: no R2
/// operation.
///
/// # Errors
///
/// - [`Error::BadRequest`](crate::Error::BadRequest) (400) when
///   `part_number` is not 1 to 10,000.
/// - [`Error::Internal`](crate::Error::Internal) (500) when an `R2_*`
///   setting is missing in a release build, or `expires_in` is 0 or over 7 days.
pub fn presign_part(ctx: &Ctx, key: &str, upload_id: &str, part_number: u16, expires_in: u64) -> Result<String> {
    if part_number == 0 || u64::from(part_number) > MAX_PARTS {
        return Err(Error::bad_request(format!("part {part_number} is not between 1 and {MAX_PARTS}")));
    }
    let number = part_number.to_string();
    let query = [("partNumber", number.as_str()), ("uploadId", upload_id)];
    match dev_base(ctx.env()) {
        Some(base) => {
            check_expires_in(expires_in)?;
            Ok(dev_url(&base, key, &query, expires_in))
        }
        None => endpoint(ctx.env())?.presign("PUT", key, &[], &query, crate::now(), expires_in),
    }
}

/// Assembles the multipart upload `upload_id` of `key` from `parts`; `false` when the upload no longer exists.
///
/// `parts` are every part, in order, with the `ETag` R2 answered (quoted
/// or not): the uploader's report, or [`list_parts`] mapped with
/// [`StoredPart::completed`](crate::storage::StoredPart::completed). The
/// object exists once this returns `true`. `false` means the upload was
/// completed (perhaps by a concurrent call: check the object with
/// [`head`](crate::storage::head)), aborted, or expired.
///
/// Free plan: one R2 class A operation.
///
/// # Errors
///
/// [`Error::BadRequest`](crate::Error::BadRequest) (400) when R2 refuses
/// the parts (a missing part, a wrong `ETag`, a part under 5 MiB but the
/// last); [`Error::Internal`](crate::Error::Internal) when the `STORAGE`
/// binding is missing.
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result, storage};
///
/// // The worker said it is done: assemble whatever parts it stored.
/// async fn finish(ctx: &Ctx, key: &str, upload_id: &str) -> Result<bool> {
///     let Some(parts) = storage::list_parts(ctx, key, upload_id).await? else {
///         return storage::exists(ctx, key).await; // completed already
///     };
///     let parts: Vec<_> = parts.iter().map(storage::StoredPart::completed).collect();
///     storage::complete_multipart(ctx, key, upload_id, &parts).await
/// }
/// ```
pub fn complete_multipart(
    ctx: &Ctx,
    key: &str,
    upload_id: &str,
    parts: &[CompletedPart],
) -> impl Future<Output = Result<bool>> + Send + use<> {
    let env = ctx.env().clone();
    let (key, upload_id, parts) = (key.to_owned(), upload_id.to_owned(), parts.to_vec());
    SendFuture::new(async move { complete(&env, &key, &upload_id, &parts).await })
}

pub(super) async fn complete(env: &Env, key: &str, upload_id: &str, parts: &[CompletedPart]) -> Result<bool> {
    if parts.is_empty() {
        return Err(Error::bad_request("complete needs the parts"));
    }
    let bucket = &bucket(env)?;
    // R2 answers a part's ETag in quotes to a presigned PUT, without them to the binding.
    let uploaded =
        move || parts.iter().map(|part| UploadedPart::new(part.part_number, part.etag.trim_matches('"').to_owned()));
    // `complete` consumes the upload, so each attempt resumes it. A repeat
    // after a success answers NoSuchUpload: `false`, as documented.
    let complete = move || async move { bucket.resume_multipart_upload(key, upload_id)?.complete(uploaded()).await };
    let completed = r2_retried("assemble", key, complete).await;
    if let Err(err) = completed {
        let message = err.to_string();
        if upload_gone(&message) {
            return Ok(false);
        }
        return Err(Error::bad_request(format!("R2 could not assemble `{key}`: {message}")));
    }
    forget_dev_upload(env, key, upload_id).await?;
    Ok(true)
}

/// Drops the multipart upload `upload_id` of `key` and its parts; an upload already gone is not an error.
///
/// Free plan: aborting is free.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when the `STORAGE`
/// binding is missing or R2 fails.
pub fn abort_multipart(ctx: &Ctx, key: &str, upload_id: &str) -> impl Future<Output = Result<()>> + Send + use<> {
    let env = ctx.env().clone();
    let (key, upload_id) = (key.to_owned(), upload_id.to_owned());
    SendFuture::new(async move { abort(&env, &key, &upload_id).await })
}

pub(super) async fn abort(env: &Env, key: &str, upload_id: &str) -> Result<()> {
    let upload = bucket(env)?.resume_multipart_upload(key, upload_id)?;
    let upload = &upload;
    if let Err(err) = r2_retried("abort the upload of", key, move || upload.abort()).await {
        let message = err.to_string();
        if !upload_gone(&message) {
            return Err(r2_error("abort the upload of", key, message));
        }
    }
    forget_dev_upload(env, key, upload_id).await
}

/// Development: removes the markers of a finished upload.
async fn forget_dev_upload(env: &Env, key: &str, upload_id: &str) -> Result<()> {
    if dev_base(env).is_none() {
        return Ok(());
    }
    let bucket = bucket(env)?;
    let parts = bucket.list().prefix(dev_parts_prefix(upload_id)).execute().await?;
    let mut markers: Vec<String> = parts.objects().iter().map(worker::Object::key).collect();
    markers.push(dev_upload_marker(key, upload_id));
    bucket.delete_multiple(markers).await?;
    Ok(())
}

/// The parts stored so far in the multipart upload `upload_id` of `key`, by part number; `None` when the upload no longer exists.
///
/// The R2 binding cannot list parts: this calls ListParts on R2's S3 API
/// (one subrequest per 1,000 parts), so it needs the `R2_*` settings of
/// [`presign_get`](crate::storage::presign_get). In debug builds it lists
/// the parts uploaded through the development route instead.
///
/// Free plan: one R2 class A operation per 1,000 parts.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when an `R2_*` setting
/// is missing (release builds), or R2 answers an error.
pub fn list_parts(
    ctx: &Ctx,
    key: &str,
    upload_id: &str,
) -> impl Future<Output = Result<Option<Vec<StoredPart>>>> + Send + use<> {
    let env = ctx.env().clone();
    let (key, upload_id) = (key.to_owned(), upload_id.to_owned());
    SendFuture::new(async move {
        if dev_base(&env).is_some() {
            return dev_list_parts(&env, &key, &upload_id).await;
        }
        let endpoint = endpoint(&env)?;
        let mut parts = Vec::new();
        let mut marker: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = vec![("uploadId", upload_id.as_str()), ("max-parts", "1000")];
            if let Some(marker) = &marker {
                query.push(("part-number-marker", marker));
            }
            let url = endpoint.presign("GET", &key, &[], &query, crate::now(), S3_CALL_EXPIRES_IN)?;
            let response = s3_get("list the parts of", &key, url).await?;
            match response.status {
                404 => return Ok(None),
                200 => {
                    let (page, next) = parse_parts(&response.text());
                    parts.extend(page);
                    marker = next;
                    if marker.is_none() {
                        parts.sort_by_key(|part: &StoredPart| part.part_number);
                        return Ok(Some(parts));
                    }
                }
                status => return Err(r2_error("list the parts of", &key, format!("{status} {}", response.text()))),
            }
        }
        Err(r2_error("list the parts of", &key, "more than 10,000 parts"))
    })
}

/// The multipart uploads in progress whose key starts with `prefix`, oldest first.
///
/// For a cleanup task that aborts uploads left behind, or for completing
/// the upload of an output whose uploader never reported. Calls
/// ListMultipartUploads on R2's S3 API (one subrequest per 1,000 uploads;
/// `R2_*` settings as [`list_parts`]); in debug builds, the uploads started
/// with [`create_multipart`] or
/// [`multipart_uploads`](crate::storage::multipart_uploads) and not
/// finished.
///
/// Free plan: one R2 class A operation per 1,000 uploads.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) (500) when an `R2_*` setting
/// is missing (release builds), or R2 answers an error.
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result, storage};
///
/// // Keeps only the newest upload of each output (a retried task starts a new one).
/// async fn drop_older_uploads(ctx: &Ctx, key: &str) -> Result<()> {
///     let mut uploads = storage::list_multipart_uploads(ctx, key).await?;
///     uploads.retain(|upload| upload.key == key);
///     uploads.pop(); // the newest
///     for upload in uploads {
///         storage::abort_multipart(ctx, key, &upload.upload_id).await?;
///     }
///     Ok(())
/// }
/// ```
pub fn list_multipart_uploads(
    ctx: &Ctx,
    prefix: &str,
) -> impl Future<Output = Result<Vec<PendingUpload>>> + Send + use<> {
    let env = ctx.env().clone();
    let prefix = prefix.to_owned();
    SendFuture::new(async move {
        let mut uploads = if dev_base(&env).is_some() {
            dev_list_uploads(&env, &prefix).await?
        } else {
            s3_list_uploads(&env, &prefix).await?
        };
        uploads.sort_by(|a, b| a.initiated.cmp(&b.initiated));
        Ok(uploads)
    })
}

async fn s3_list_uploads(env: &Env, prefix: &str) -> Result<Vec<PendingUpload>> {
    let endpoint = endpoint(env)?;
    let mut uploads = Vec::new();
    let mut markers: Option<(String, String)> = None;
    for _ in 0..MAX_PAGES {
        let mut query = vec![("uploads", ""), ("prefix", prefix), ("max-uploads", "1000")];
        if let Some((key, id)) = &markers {
            query.extend([("key-marker", key.as_str()), ("upload-id-marker", id.as_str())]);
        }
        let url = endpoint.presign("GET", "", &[], &query, crate::now(), S3_CALL_EXPIRES_IN)?;
        let response = s3_get("list the uploads under", prefix, url).await?;
        if response.status != 200 {
            return Err(r2_error("list the uploads under", prefix, format!("{} {}", response.status, response.text())));
        }
        let (page, next) = parse_uploads(&response.text());
        uploads.extend(page);
        markers = next;
        if markers.is_none() {
            return Ok(uploads);
        }
    }
    Err(r2_error("list the uploads under", prefix, "more than 10,000 uploads"))
}

/// Sends a signed `GET` to R2's S3 API, and again (see [`with_retries`])
/// when R2 answers 500, 502, 503 or 504 or the connection fails
/// ([`s3_retryable`]). Each retry is logged at `warn` and is one more subrequest.
async fn s3_get(action: &str, key: &str, url: String) -> Result<http::Response> {
    let url = &url;
    let warn = |retry, outcome: &Result<http::Response>| {
        let cause = match outcome {
            Ok(response) => format!("HTTP {}", response.status),
            Err(err) => err.to_string(),
        };
        Logger::new().warn(format_args!("R2 retry {retry}/{RETRIES} to {action} `{key}` after: {cause}"));
    };
    with_retries(move || http::Request::get(url).send(), s3_retryable, warn, super::sleep).await
}

async fn dev_list_parts(env: &Env, key: &str, upload_id: &str) -> Result<Option<Vec<StoredPart>>> {
    let bucket = bucket(env)?;
    if bucket.head(dev_upload_marker(key, upload_id)).await?.is_none() {
        return Ok(None);
    }
    let listed =
        bucket.list().prefix(dev_parts_prefix(upload_id)).include(vec![Include::CustomMetadata]).execute().await?;
    let mut parts: Vec<StoredPart> = listed
        .objects()
        .iter()
        .filter_map(|object| {
            let meta = object.custom_metadata().ok()?;
            Some(StoredPart {
                part_number: object.key().rsplit('/').next()?.parse().ok()?,
                etag: meta.get("etag")?.clone(),
                size: meta.get("size")?.parse().ok()?,
            })
        })
        .collect();
    parts.sort_by_key(|part| part.part_number);
    Ok(Some(parts))
}

async fn dev_list_uploads(env: &Env, prefix: &str) -> Result<Vec<PendingUpload>> {
    let listed = bucket(env)?.list().prefix(format!("{DEV_MARKERS}uploads/{prefix}")).execute().await?;
    Ok(listed
        .objects()
        .iter()
        .filter_map(|object| {
            let initiated = format!("{:020}", object.uploaded().as_millis());
            dev_upload_from_marker(&object.key(), initiated)
        })
        .collect())
}

/// `/ocre/dev/storage/{*key}`: presigned URLs in debug builds, on the local bucket (see [`DEV_STORAGE_PATH`]).
pub(crate) fn dev_routes() -> Router<Ctx> {
    Router::new().route(
        &format!("{DEV_STORAGE_PATH}/{{*key}}"),
        get(|State(ctx): State<Ctx>, Path(key): Path<String>, Query(query): Query<DevGet>, headers: HeaderMap| {
            dev_get(ctx, key, query, headers)
        })
        .put(
            |State(ctx): State<Ctx>,
             Path(key): Path<String>,
             Query(query): Query<DevPut>,
             headers: HeaderMap,
             body: Body| {
                SendFuture::new(async move {
                    dev_put(&ctx, &key, &query, &headers, body).await.map_err(ApiError::from).into_response()
                })
            },
        ),
    )
}

/// A `GET` of a presigned URL: the file, with `Range` and the URL's response overrides.
fn dev_get(ctx: Ctx, key: String, query: DevGet, headers: HeaderMap) -> impl Future<Output = Response> + Send {
    SendFuture::new(async move {
        let found = super::storage::head(&ctx, &key).await;
        let object = match found {
            Ok(Some(object)) => object,
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(err) => return ApiError::from(err).into_response(),
        };
        let filename = key.rsplit('/').next().unwrap_or(&key).to_owned();
        let attachment = object.attachment(&filename);
        let mut response = match super::storage::serve(&ctx, &attachment, &headers, Disposition::Inline).await {
            Ok(response) => response,
            Err(err) => return ApiError::from(err).into_response(),
        };
        let overrides =
            [(header::CONTENT_TYPE, query.content_type), (header::CONTENT_DISPOSITION, query.content_disposition)];
        for (name, value) in overrides {
            if let Some(value) = value.and_then(|value| HeaderValue::from_str(&value).ok()) {
                response.headers_mut().insert(name, value);
            }
        }
        response
    })
}

/// Streams `size` bytes of `body` as part `part` of the upload `upload_id` of `key`, and returns its `ETag`.
///
/// In development, also records the part for [`list_parts`].
pub(super) async fn upload_part(
    env: &Env,
    key: &str,
    upload_id: &str,
    part: u16,
    size: u64,
    body: Body,
) -> Result<String> {
    let bucket = bucket(env)?;
    let upload = bucket.resume_multipart_upload(key, upload_id)?;
    let stored = upload.upload_part(part, fixed_length(body, size)).await.map_err(|err| match err.to_string() {
        // The upload was completed, aborted or expired: the uploader starts over.
        message if upload_gone(&message) => Error::NotFound,
        message => Error::bad_request(format!("R2 refused part {part} of `{key}`: {message}")),
    })?;
    if dev_base(env).is_some() {
        let marker = format!("{}{part}", dev_parts_prefix(upload_id));
        let metadata = [("etag".to_owned(), stored.etag()), ("size".to_owned(), size.to_string())];
        bucket.put(marker, Vec::new()).custom_metadata(metadata).execute().await?;
    }
    Ok(stored.etag())
}

/// A request body of `size` bytes as a stream R2 accepts (it needs the length up front).
fn fixed_length(body: Body, size: u64) -> FixedLengthStream {
    let chunks = body
        .into_data_stream()
        .map(|chunk| chunk.map(|bytes| bytes.to_vec()).map_err(|err| worker::Error::RustError(err.to_string())));
    FixedLengthStream::wrap(chunks, size)
}

/// A `PUT` of a presigned URL: a whole object, or a part of a multipart upload. Answers its `ETag`.
async fn dev_put(ctx: &Ctx, key: &str, query: &DevPut, headers: &HeaderMap, body: Body) -> Result<Response> {
    let env = ctx.env();
    let size: u64 = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse().ok())
        .ok_or_else(|| Error::bad_request("a PUT needs a Content-Length"))?;
    let etag = match (query.part_number, &query.upload_id) {
        (Some(part), Some(upload_id)) => upload_part(env, key, upload_id, part, size, body).await?,
        _ => {
            let content_type = headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).map(essence);
            let metadata = HttpMetadata { content_type, ..Default::default() };
            let object = bucket(env)?.put(key, fixed_length(body, size)).http_metadata(metadata).execute().await?;
            object.map(|object| object.etag()).unwrap_or_default()
        }
    };
    let mut response = StatusCode::OK.into_response();
    if let Ok(value) = HeaderValue::from_str(&format!("\"{}\"", etag.trim_matches('"'))) {
        response.headers_mut().insert(header::ETAG, value);
    }
    Ok(response)
}
