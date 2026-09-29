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

/// Stores `upload` in R2 under a new random key starting with `prefix`
/// (`posts/avatar/...`) and returns what to save with the record. One R2
/// write (class A operation).
///
/// ```ignore
/// let upload = form.file("avatar").or_404()?;
/// let avatar = ocre::storage::store(&ctx, "posts/avatar", upload).await?;
/// ```
pub fn store(ctx: &Ctx, prefix: &str, upload: Upload) -> impl Future<Output = Result<Attachment>> + Send + use<> {
    store_bytes(ctx, prefix, &upload.filename, &upload.content_type, Vec::from(upload.bytes))
}

/// Stores bytes built by the app (a generated report, an export) like
/// [`store`].
///
/// ```ignore
/// let csv = ocre::storage::store_bytes(&ctx, "exports", "posts.csv", "text/csv", rows.into_bytes()).await?;
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

/// Streams `body` (exactly `size` bytes, for example a request body with a
/// `Content-Length`) into R2 without holding it in memory. R2 needs the
/// length up front; a body of another length fails.
///
/// ```ignore
/// async fn upload(State(ctx): State<Ctx>, headers: HeaderMap, body: Body) -> ocre::Result<String> {
///     let size = headers.get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse().ok()).ok_or_else(|| Error::bad_request("Content-Length required"))?;
///     let attachment = ocre::storage::store_body(&ctx, "raw", "upload.bin", "application/octet-stream", size, body).await?;
///     Ok(attachment.key)
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

/// The whole object, or `None` when `key` does not exist. One R2 read
/// (class B operation). For sending a file to a browser use [`serve`], which
/// streams it.
///
/// ```ignore
/// let bytes = ocre::storage::read(&ctx, &post.avatar().key).await?.or_404()?;
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

/// Deletes the object at `key`; deleting a missing key is not an error. R2
/// deletes are free.
///
/// ```ignore
/// ocre::storage::delete(&ctx, &attachment.key).await?;
/// ```
pub fn delete(ctx: &Ctx, key: &str) -> impl Future<Output = Result<()>> + Send + use<> {
    let env = ctx.env().clone();
    let key = key.to_owned();
    SendFuture::new(async move { Ok(bucket(&env)?.delete(key).await?) })
}

/// Deletes the objects of every `Some` attachment in one R2 call. Generated
/// models use it when a record is deleted or its files replaced.
///
/// ```ignore
/// ocre::storage::delete_attachments(&ctx, &[Some(post.avatar()), post.doc()]).await?;
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

/// Streams a stored file to the client: `Content-Type`, `Content-Length`,
/// `Content-Disposition` (the original filename; `inline` only for types
/// safe to display, see [`Disposition`]), `ETag`, `Cache-Control`
/// ([`CACHE_CONTROL`](crate::storage::CACHE_CONTROL)), 304 for a matching
/// `If-None-Match`, and 206/416 for a single `Range` (video seeking,
/// resumed downloads). A missing object is a 404. One R2 read (class B).
///
/// Check that the user may see the record before calling it: the route is
/// the only protection.
///
/// ```ignore
/// async fn avatar(State(ctx): State<Ctx>, Path(id): Path<i64>, headers: HeaderMap) -> ocre::Result<Response> {
///     let post = post::find(&ctx, id).await?.or_404()?;
///     storage::serve(&ctx, &post.avatar(), &headers, Disposition::Inline).await
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
