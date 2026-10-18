//! Multipart uploads beyond the browser (another service uploads the parts),
//! the S3 API calls the R2 binding lacks (ListParts, ListMultipartUploads),
//! and presigned URLs in development, served by the Worker from the local bucket.

use serde::{Deserialize, Serialize};

use crate::percent_encode;

/// Path of the development route serving presigned URLs from the local bucket: `/ocre/dev/storage/<key>`.
///
/// In debug builds (`ocre dev`, `ocre test`), [`presign_get`](crate::storage::presign_get),
/// [`presign_put`](crate::storage::presign_put), [`presign_part`](crate::storage::presign_part),
/// [`direct_upload`](crate::storage::direct_upload) and
/// [`serve_redirect`](crate::storage::serve_redirect) answer URLs on this
/// route of the app (`APP_URL`, else `http://localhost:8787`) instead of
/// R2's, and [`serve`](crate::serve) mounts it: `GET` (with `Range`), `PUT`
/// and `PUT ?partNumber=&uploadId=` work on the local bucket as on R2, so
/// development never touches the real bucket and needs no `R2_*` setting.
/// Release builds have no such route. The URLs are not signed: the route
/// exists only on a developer's machine.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::storage::DEV_STORAGE_PATH, "/ocre/dev/storage");
/// ```
pub const DEV_STORAGE_PATH: &str = "/ocre/dev/storage";

/// Marker objects of the development emulation of ListParts and ListMultipartUploads, under this prefix of the local bucket.
pub(crate) const DEV_MARKERS: &str = ".ocre-dev-storage/";

/// Default app address in development when `APP_URL` is not set.
pub(crate) const DEV_APP_URL: &str = "http://localhost:8787";

/// A part stored in a multipart upload, as [`list_parts`](crate::storage::list_parts) reports it.
///
/// # Examples
///
/// ```
/// let part = ocre::storage::StoredPart { part_number: 1, etag: "abc".into(), size: 5_242_880 };
/// assert_eq!(part.completed().etag, "abc");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPart {
    /// From 1.
    pub part_number: u16,
    /// The part's `ETag`, without quotes.
    pub etag: String,
    /// Bytes.
    pub size: u64,
}

impl StoredPart {
    /// The part as [`complete_multipart`](crate::storage::complete_multipart) takes it.
    pub fn completed(&self) -> super::CompletedPart {
        super::CompletedPart { part_number: self.part_number, etag: self.etag.clone() }
    }
}

/// A multipart upload not completed or aborted yet, as [`list_multipart_uploads`](crate::storage::list_multipart_uploads) reports it.
///
/// # Examples
///
/// ```
/// let upload = ocre::storage::PendingUpload {
///     key: "videos/1/source.mp4".into(),
///     upload_id: "u1".into(),
///     initiated: "2026-10-18T10:00:00.000Z".into(),
/// };
/// assert_eq!(upload.key, "videos/1/source.mp4");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingUpload {
    /// The object the upload will create.
    pub key: String,
    /// R2's id of the upload.
    pub upload_id: String,
    /// When it started, as text that sorts in time order (ISO 8601 on R2).
    pub initiated: String,
}

/// The text of each `<tag>...</tag>` of `xml`, in order (S3's flat answers: no attributes, no nesting of the same tag).
fn elements<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(&after[..end]);
        rest = &after[end + close.len()..];
    }
    found
}

/// The unescaped text of the first `<tag>` of `xml`.
fn element(xml: &str, tag: &str) -> Option<String> {
    elements(xml, tag).first().map(|text| {
        text.trim()
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    })
}

/// Whether a listing says more pages follow.
fn truncated(xml: &str) -> bool {
    element(xml, "IsTruncated").as_deref() == Some("true")
}

/// The parts of a ListParts answer, and the marker of the next page.
pub(crate) fn parse_parts(xml: &str) -> (Vec<StoredPart>, Option<String>) {
    let parts = elements(xml, "Part")
        .into_iter()
        .filter_map(|part| {
            Some(StoredPart {
                part_number: element(part, "PartNumber")?.parse().ok()?,
                etag: element(part, "ETag")?.trim_matches('"').to_owned(),
                size: element(part, "Size")?.parse().ok()?,
            })
        })
        .collect();
    let next = truncated(xml).then(|| element(xml, "NextPartNumberMarker")).flatten();
    (parts, next)
}

/// The uploads of a ListMultipartUploads answer, and the key and upload id markers of the next page.
pub(crate) fn parse_uploads(xml: &str) -> (Vec<PendingUpload>, Option<(String, String)>) {
    let uploads = elements(xml, "Upload")
        .into_iter()
        .filter_map(|upload| {
            Some(PendingUpload {
                key: element(upload, "Key")?,
                upload_id: element(upload, "UploadId")?,
                initiated: element(upload, "Initiated").unwrap_or_default(),
            })
        })
        .collect();
    let next = truncated(xml).then(|| element(xml, "NextKeyMarker").zip(element(xml, "NextUploadIdMarker"))).flatten();
    (uploads, next)
}

/// Whether an R2 error message says the multipart upload no longer exists (completed, aborted or expired).
pub(crate) fn upload_gone(message: &str) -> bool {
    message.contains("NoSuchUpload") || message.contains("does not exist")
}

/// The development URL standing for a presigned one: `<base>/ocre/dev/storage/<key>?<query>&X-Amz-Expires=<n>`.
///
/// `X-Amz-Expires` is not checked; it shows the lifetime as on R2.
pub(crate) fn dev_url(base: &str, key: &str, query: &[(&str, &str)], expires_in: u64) -> String {
    let expires = expires_in.to_string();
    let query: Vec<String> = query
        .iter()
        .chain(&[("X-Amz-Expires", expires.as_str())])
        .map(|(name, value)| format!("{}={}", percent_encode(name, false), percent_encode(value, false)))
        .collect();
    format!("{}{DEV_STORAGE_PATH}/{}?{}", base.trim().trim_end_matches('/'), percent_encode(key, true), query.join("&"))
}

/// The marker of a multipart upload in progress: `.ocre-dev-storage/uploads/<key>/<upload id>`.
pub(crate) fn dev_upload_marker(key: &str, upload_id: &str) -> String {
    format!("{DEV_MARKERS}uploads/{key}/{}", percent_encode(upload_id, false))
}

/// The upload of a marker made by [`dev_upload_marker`], if it is one.
pub(crate) fn dev_upload_from_marker(marker: &str, initiated: String) -> Option<PendingUpload> {
    let (key, upload_id) = marker.strip_prefix(DEV_MARKERS)?.strip_prefix("uploads/")?.rsplit_once('/')?;
    let upload_id = percent_decode(upload_id)?;
    Some(PendingUpload { key: key.to_owned(), upload_id, initiated })
}

/// The prefix of the part markers of an upload: `.ocre-dev-storage/parts/<upload id>/`.
pub(crate) fn dev_parts_prefix(upload_id: &str) -> String {
    format!("{DEV_MARKERS}parts/{}/", percent_encode(upload_id, false))
}

/// `%XX` sequences back to bytes; `None` when the result is not UTF-8 or a sequence is malformed.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The query of a `PUT` on the development route: a part of a multipart upload, or a whole object.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct DevPut {
    #[serde(rename = "partNumber")]
    pub(crate) part_number: Option<u16>,
    #[serde(rename = "uploadId")]
    pub(crate) upload_id: Option<String>,
}

/// The query of a `GET` on the development route: R2's response overrides.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct DevGet {
    #[serde(rename = "response-content-type")]
    pub(crate) content_type: Option<String>,
    #[serde(rename = "response-content-disposition")]
    pub(crate) content_disposition: Option<String>,
}

#[cfg(test)]
#[path = "../../tests/storage/s3.rs"]
mod tests;
