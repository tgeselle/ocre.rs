//! Large files sent in parts, with resume: the requests and answers of
//! [`multipart_uploads`](crate::storage::multipart_uploads), how a file is
//! cut into parts, and the presigned `PUT` of each part.
//!
//! R2 multipart uploads take parts of the same size (at least 5 MiB, the
//! last one smaller) numbered 1 to 10,000. A part the browser sends again
//! replaces the previous one, so an interrupted upload resumes by sending
//! the parts it did not finish.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Rules, S3Endpoint};
use crate::{Error, Result, Validator};

/// Size of the parts: 10 MiB, or more for a file that would need over 10,000.
pub const PART_SIZE: u64 = 10 * MIB;

/// Largest part sent through the Worker (its request limit is 100 MB).
pub const MAX_WORKER_PART: u64 = 95 * MIB;

/// Largest part R2 accepts: 5 GiB.
pub const MAX_PART: u64 = 5 * 1024 * MIB;

/// Most parts in one upload.
pub const MAX_PARTS: u64 = 10_000;

const MIB: u64 = 1024 * 1024;

/// A started multipart upload, sent to the browser.
///
/// # Examples
///
/// ```
/// let upload: ocre::storage::MultipartUpload =
///     serde_json::from_str(r#"{"signed_key":"k.s","upload_id":"u","part_size":10485760,"part_count":3}"#).unwrap();
/// assert_eq!(upload.part_count, 3);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultipartUpload {
    /// The object key and its signature, as for a direct upload: the form submits it.
    pub signed_key: String,
    /// R2's id of the upload.
    pub upload_id: String,
    /// Bytes per part; the last part holds the rest.
    pub part_size: u64,
    /// Number of parts, numbered from 1.
    pub part_count: u16,
}

/// The browser asks where to send parts: `{"signed_key", "upload_id", "parts": [1, 2, 3]}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PartsRequest {
    /// From the [`MultipartUpload`].
    pub signed_key: String,
    /// From the [`MultipartUpload`].
    pub upload_id: String,
    /// Part numbers, from 1; at most [`MAX_PARTS_PER_REQUEST`].
    pub parts: Vec<u16>,
}

/// Most part URLs answered at once.
pub const MAX_PARTS_PER_REQUEST: usize = 1_000;

/// Where to `PUT` each part: `{"urls": {"1": "https://..."}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartUrls {
    /// Part number to URL: presigned on R2, or a route of the Worker.
    pub urls: BTreeMap<u16, String>,
}

/// A part R2 stored: its number and the `ETag` it answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletedPart {
    /// From 1.
    pub part_number: u16,
    /// The part's `ETag`, quoted or not.
    pub etag: String,
}

/// The browser finishes (`parts` set) or abandons an upload.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FinishRequest {
    /// From the [`MultipartUpload`].
    pub signed_key: String,
    /// From the [`MultipartUpload`].
    pub upload_id: String,
    /// Every part, as R2 answered them; empty to abort.
    #[serde(default)]
    pub parts: Vec<CompletedPart>,
}

/// How to cut `size` bytes into parts no larger than `max_part`: `(part_size, part_count)`.
///
/// # Errors
///
/// [`Error::Invalid`] on `field` when the file needs parts over `max_part`.
///
/// # Examples
///
/// ```
/// use ocre::storage::{MAX_WORKER_PART, PART_SIZE, part_layout};
///
/// assert_eq!(part_layout("video", 25 * 1024 * 1024, MAX_WORKER_PART).unwrap(), (PART_SIZE, 3));
/// assert_eq!(part_layout("video", 0, MAX_WORKER_PART).unwrap(), (PART_SIZE, 1));
/// ```
pub fn part_layout(field: &str, size: u64, max_part: u64) -> Result<(u64, u16)> {
    // Over 10,000 parts of 10 MiB: larger parts, in whole MiB.
    let part_size = PART_SIZE.max(size.div_ceil(MAX_PARTS).div_ceil(MIB) * MIB);
    Validator::new()
        .check(field, part_size > max_part, format!("is too large to upload in parts of at most {} MB", max_part / MIB))
        .finish()?;
    let count = size.div_ceil(part_size).max(1);
    Ok((part_size, u16::try_from(count).expect("at most 10,000 parts")))
}

/// Checks the declared file against `rules` and lays it out in parts, as
/// [`multipart_uploads`](crate::storage::multipart_uploads) does before
/// creating the upload.
///
/// # Errors
///
/// [`Error::Invalid`] on `field` when the size or type breaks `rules`, or
/// the file needs parts over `max_part`.
pub fn check_multipart(field: &str, size: u64, content_type: &str, rules: &Rules, max_part: u64) -> Result<(u64, u16)> {
    Validator::new().file_size_and_type(field, size, content_type, rules).finish()?;
    part_layout(field, size, max_part)
}

/// Presigned `PUT` URLs of `parts` of the upload `upload_id` of `key`, valid `expires_in` seconds.
///
/// # Errors
///
/// [`Error::BadRequest`] for a part number outside 1 to 10,000 or too many
/// parts at once; [`Error::Internal`] for an out-of-range `expires_in`.
///
/// # Examples
///
/// ```
/// use ocre::storage::{S3Endpoint, presign_parts};
///
/// let r2 = S3Endpoint::r2("acc", "blog-storage", "AKID", "secret");
/// let urls = presign_parts(&r2, "uploads/a", "up1", &[1, 2], 1_790_000_000, 3600).unwrap();
/// assert!(urls.urls[&2].contains("partNumber=2&uploadId=up1"));
/// ```
pub fn presign_parts(
    endpoint: &S3Endpoint,
    key: &str,
    upload_id: &str,
    parts: &[u16],
    now: i64,
    expires_in: u64,
) -> Result<PartUrls> {
    check_part_numbers(parts)?;
    let mut urls = BTreeMap::new();
    for &part in parts {
        let number = part.to_string();
        let query = [("partNumber", number.as_str()), ("uploadId", upload_id)];
        urls.insert(part, endpoint.presign("PUT", key, &[], &query, now, expires_in)?);
    }
    Ok(PartUrls { urls })
}

/// Part numbers are 1 to 10,000, at most [`MAX_PARTS_PER_REQUEST`] at once.
pub(crate) fn check_part_numbers(parts: &[u16]) -> Result<()> {
    if parts.len() > MAX_PARTS_PER_REQUEST {
        return Err(Error::bad_request(format!("ask for at most {MAX_PARTS_PER_REQUEST} parts at once")));
    }
    if let Some(part) = parts.iter().find(|&&part| part == 0 || u64::from(part) > MAX_PARTS) {
        return Err(Error::bad_request(format!("part {part} is not between 1 and {MAX_PARTS}")));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/storage/resumable.rs"]
mod tests;
