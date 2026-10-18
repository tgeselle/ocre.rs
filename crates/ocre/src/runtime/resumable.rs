//! [`multipart_uploads`]: large files sent in parts, resumable, straight to R2.

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    routing::{post, put},
};
use serde::Deserialize;
use worker::{Env, send::SendFuture};

use super::{Ctx, storage::bucket};
use crate::{
    ApiError, Error, Result,
    config::Environment,
    storage::{
        CompletedPart, DirectUploadRequest, FinishRequest, MAX_PART, MAX_WORKER_PART, MultipartUpload, PartUrls,
        PartsRequest, Rules, check_multipart, check_part_numbers, essence, new_key, presign_parts, r2_endpoint,
        sign_key, upload_secret, verify_key,
    },
};

/// How long a presigned part URL can be used: 24 hours, for slow connections.
const PART_URL_EXPIRES_IN: u64 = 24 * 3600;

/// Routes that upload large files to R2 in parts, resumably (S3 multipart uploads), for the `field` of a form.
///
/// The browser side is in [`DIRECT_UPLOAD_JS`](crate::storage::DIRECT_UPLOAD_JS):
/// `<input type="file" name="video" data-multipart-upload-url="/videos/uploads">`.
/// The routes, under `path`:
///
/// | Request | Does |
/// |---|---|
/// | `POST <path>` with `{"filename", "content_type", "size"}` | checks the file against `rules`, creates the upload of a new key under `prefix`, answers a [`MultipartUpload`] (`signed_key`, `upload_id`, `part_size`, `part_count`) |
/// | `POST <path>/parts` with `{"signed_key", "upload_id", "parts": [1, 2]}` | answers where to `PUT` each part ([`PartUrls`]) |
/// | `PUT <path>/parts/<n>?key=&upload_id=` | stores part `n` through the Worker, answers `{"part_number", "etag"}` |
/// | `POST <path>/complete` with `{"signed_key", "upload_id", "parts": [{"part_number", "etag"}]}` | assembles the object (204) |
/// | `POST <path>/abort` with `{"signed_key", "upload_id"}` | drops the parts (204) |
///
/// Where parts go: in a release build with the `R2_*` settings (see
/// [`presign_get`](crate::storage::presign_get)), straight to R2 through
/// presigned URLs, so the file never passes through the Worker (the bucket's
/// CORS rule must allow `PUT` and expose `ETag`). Otherwise (`ocre dev`, or
/// no `R2_*` settings) through the Worker, part by part (95 MB at most each,
/// under the 100 MB request limit). An interrupted upload resumes: the
/// script keeps the finished parts in `localStorage` and sends the others.
///
/// Once complete, the form submits the signed key like a direct upload:
/// [`attach_direct_upload`](crate::storage::attach_direct_upload) checks the
/// object and returns its [`Attachment`](crate::storage::Attachment).
/// Costs: one R2 class A operation per part, plus the create and complete.
///
/// # Examples
///
/// ```no_run
/// use axum::Router;
/// use ocre::{Ctx, storage::{self, Rules}};
///
/// static VIDEO: Rules = Rules { max_bytes: 20 * 1024 * 1024 * 1024, content_types: &["video/mp4", "video/quicktime"] };
///
/// fn routes() -> Router<Ctx> {
///     Router::new().merge(storage::multipart_uploads("/videos/uploads", "uploads/videos", "video", &VIDEO))
/// }
/// # let _ = routes;
/// ```
pub fn multipart_uploads(path: &str, prefix: &'static str, field: &'static str, rules: &'static Rules) -> Router<Ctx> {
    let base = path.trim_end_matches('/').to_owned();
    let parts_path = base.clone();
    Router::new()
        .route(
            &base,
            post(move |State(ctx): State<Ctx>, Json(request): Json<DirectUploadRequest>| {
                SendFuture::new(async move {
                    start(&ctx, prefix, field, rules, &request).await.map(Json).map_err(ApiError::from)
                })
            }),
        )
        .route(
            &format!("{base}/parts"),
            post(move |State(ctx): State<Ctx>, Json(request): Json<PartsRequest>| {
                let result = part_urls(&ctx, &parts_path, field, &request);
                async move { result.map(Json).map_err(ApiError::from) }
            }),
        )
        .route(
            &format!("{base}/parts/{{part}}"),
            put(
                move |State(ctx): State<Ctx>,
                      Path(part): Path<u16>,
                      Query(target): Query<PartTarget>,
                      headers: HeaderMap,
                      body: Body| {
                    SendFuture::new(async move {
                        upload_part(&ctx, field, part, &target, &headers, body).await.map(Json).map_err(ApiError::from)
                    })
                },
            ),
        )
        .route(
            &format!("{base}/complete"),
            post(move |State(ctx): State<Ctx>, Json(request): Json<FinishRequest>| {
                SendFuture::new(async move { finish(&ctx, field, &request, true).await.map_err(ApiError::from) })
            }),
        )
        .route(
            &format!("{base}/abort"),
            post(move |State(ctx): State<Ctx>, Json(request): Json<FinishRequest>| {
                SendFuture::new(async move { finish(&ctx, field, &request, false).await.map_err(ApiError::from) })
            }),
        )
}

/// The upload a part goes to, in the query of a part sent through the Worker.
#[derive(Deserialize)]
struct PartTarget {
    key: String,
    upload_id: String,
}

fn secret(env: &Env) -> Result<String> {
    upload_secret(&|name| env.var(name).ok().map(|value| value.to_string()))
}

/// Parts go straight to R2 in a release build with the `R2_*` settings.
fn direct(env: &Env) -> bool {
    !Environment::current().is_development() && r2_endpoint(&|name| env.var(name).ok().map(|v| v.to_string())).is_ok()
}

async fn start(
    ctx: &Ctx,
    prefix: &str,
    field: &str,
    rules: &Rules,
    request: &DirectUploadRequest,
) -> Result<MultipartUpload> {
    let env = ctx.env();
    let max_part = if direct(env) { MAX_PART } else { MAX_WORKER_PART };
    let content_type = essence(&request.content_type);
    let (part_size, part_count) = check_multipart(field, request.size, &content_type, rules, max_part)?;
    let key = new_key(prefix);
    let upload_id = super::s3::create(env, &key, content_type).await?;
    Ok(MultipartUpload { signed_key: sign_key(&secret(env)?, &key), upload_id, part_size, part_count })
}

fn part_urls(ctx: &Ctx, path: &str, field: &str, request: &PartsRequest) -> Result<PartUrls> {
    let env = ctx.env();
    let key = verify_key(field, &secret(env)?, &request.signed_key)?;
    if direct(env) {
        let endpoint = r2_endpoint(&|name| env.var(name).ok().map(|v| v.to_string()))?;
        return presign_parts(&endpoint, &key, &request.upload_id, &request.parts, crate::now(), PART_URL_EXPIRES_IN);
    }
    check_part_numbers(&request.parts)?;
    let query = serde_urlencoded::to_string([("key", request.signed_key.as_str()), ("upload_id", &request.upload_id)])
        .map_err(|err| Error::internal(err.to_string()))?;
    let urls = request.parts.iter().map(|&part| (part, format!("{path}/parts/{part}?{query}"))).collect();
    Ok(PartUrls { urls })
}

async fn upload_part(
    ctx: &Ctx,
    field: &str,
    part: u16,
    target: &PartTarget,
    headers: &HeaderMap,
    body: Body,
) -> Result<CompletedPart> {
    check_part_numbers(&[part])?;
    let env = ctx.env();
    let key = verify_key(field, &secret(env)?, &target.key)?;
    let size: u64 = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse().ok())
        .ok_or_else(|| Error::bad_request("a part needs a Content-Length"))?;
    if size > MAX_WORKER_PART {
        return Err(Error::PayloadTooLarge(format!(
            "a part sent through the Worker is at most {MAX_WORKER_PART} bytes"
        )));
    }
    let etag = super::s3::upload_part(env, &key, &target.upload_id, part, size, body).await?;
    Ok(CompletedPart { part_number: part, etag })
}

async fn finish(ctx: &Ctx, field: &str, request: &FinishRequest, complete: bool) -> Result<StatusCode> {
    let env = ctx.env();
    let key = verify_key(field, &secret(env)?, &request.signed_key)?;
    if !complete {
        super::s3::abort(env, &key, &request.upload_id).await?;
    } else if !super::s3::complete(env, &key, &request.upload_id, &request.parts).await? {
        // Completed by an earlier request whose answer was lost, aborted or expired.
        if bucket(env)?.head(key.clone()).await?.is_none() {
            return Err(Error::NotFound);
        }
    }
    Ok(StatusCode::NO_CONTENT)
}
