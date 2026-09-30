//! File analysis: the real type of a file from its first bytes, and the
//! pixel size of an image from its header (Active Storage's analyzers,
//! without reading pixels).

use super::{Upload, essence};
use crate::Validator;

/// What [`analyze`] found in a file's bytes.
///
/// # Examples
///
/// ```
/// use ocre::storage::{Analysis, analyze};
///
/// assert_eq!(analyze(b"%PDF-1.7\n..."), Analysis { content_type: Some("application/pdf"), width: None, height: None });
/// assert_eq!(analyze(b"hello"), Analysis::default());
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Analysis {
    /// The type told by the file's signature (magic bytes), or `None` for other files (text, CSV, SVG...).
    pub content_type: Option<&'static str>,
    /// Width in pixels, for PNG, GIF, WebP and JPEG images whose header is in the bytes.
    pub width: Option<u32>,
    /// Height in pixels, like `width`.
    pub height: Option<u32>,
}

/// Types [`analyze`] recognizes, so a file declared as one of them must carry its signature.
const SNIFFED: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
    "application/pdf",
    "application/zip",
    "video/mp4",
    "video/quicktime",
    "audio/mp4",
];

/// Reads a file's signature and, for images, its width and height (Active Storage's `analyze`).
///
/// Recognized: PNG, JPEG, GIF, WebP, AVIF, PDF, ZIP (also `.docx`, `.xlsx`,
/// `.odt`...: they are ZIP files), MP4, M4A and QuickTime (MOV). Sizes come
/// from PNG, GIF and WebP (VP8, VP8L, VP8X) headers and from the JPEG
/// frame header (`SOF`), found by jumping from segment to segment. Text
/// formats (plain text, CSV, HTML, SVG, JSON) have no signature and give
/// `None`: telling them apart would need guesses that an attacker can steer.
///
/// Pure and bounded: it reads the first 32 bytes, plus a JPEG's segment
/// headers (a few dozen jumps, never the image data): microseconds of CPU
/// whatever the file size. Passing only the start of a file works (a few
/// KB; up to 256 KB for JPEGs with large EXIF blocks), for example the
/// first bytes of an R2 object from [`read_first`](crate::storage::read_first).
///
/// # Examples
///
/// ```
/// use ocre::storage::analyze;
///
/// // The first 24 bytes of a 640x480 PNG.
/// let png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\x02\x80\0\0\x01\xe0";
/// let analysis = analyze(png);
/// assert_eq!(analysis.content_type, Some("image/png"));
/// assert_eq!((analysis.width, analysis.height), (Some(640), Some(480)));
/// ```
pub fn analyze(bytes: &[u8]) -> Analysis {
    let size = |dimensions: Option<(u32, u32)>| match dimensions {
        Some((width, height)) => (Some(width), Some(height)),
        None => (None, None),
    };
    let (content_type, (width, height)) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ("image/png", size(png_size(bytes)))
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        ("image/jpeg", size(jpeg_size(bytes)))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        ("image/gif", size(bytes.get(6..10).map(|screen| (le16(screen, 0), le16(screen, 2)))))
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        ("image/webp", size(webp_size(bytes)))
    } else if bytes.starts_with(b"%PDF-") {
        ("application/pdf", (None, None))
    } else if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        ("application/zip", (None, None))
    } else if let Some(brand) = bytes.get(8..12).filter(|_| bytes.get(4..8) == Some(b"ftyp")) {
        let content_type = match brand {
            b"avif" | b"avis" => "image/avif",
            b"qt  " => "video/quicktime",
            b"M4A " => "audio/mp4",
            _ => "video/mp4",
        };
        (content_type, (None, None))
    } else {
        return Analysis::default();
    };
    Analysis { content_type: Some(content_type), width, height }
}

impl Validator {
    /// Checks that an upload's bytes match its declared content type (Active Storage's content-type identification).
    ///
    /// Adds "has content that does not match image/png" when the declared
    /// type is one [`analyze`] recognizes but the bytes do not carry its
    /// signature (a script renamed `.png`), or when the bytes are a
    /// recognized type other than the declared one (a PDF sent as
    /// `text/plain`). Text types without a signature pass. Use it after
    /// [`Validator::file`], which limits the declared type; costs
    /// microseconds (see [`analyze`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Validator, storage::Upload};
    ///
    /// let fake = Upload::new("cat.png", "image/png", "<script>alert(1)</script>");
    /// let err = Validator::new().file_content("photo", &fake).finish().unwrap_err();
    /// assert_eq!(err.to_string(), "invalid: Photo has content that does not match image/png");
    ///
    /// let text = Upload::new("notes.txt", "text/plain", "hello");
    /// assert!(Validator::new().file_content("notes", &text).finish().is_ok());
    /// ```
    pub fn file_content(&mut self, field: &str, upload: &Upload) -> &mut Self {
        let declared = essence(&upload.content_type);
        let sniffed = analyze(&upload.bytes).content_type;
        let mismatch = match sniffed {
            Some(found) => found != declared,
            None => SNIFFED.contains(&declared.as_str()),
        };
        self.check(field, mismatch, format!("has content that does not match {declared}"))
    }
}

fn le16(bytes: &[u8], at: usize) -> u32 {
    u32::from(bytes[at]) | u32::from(bytes[at + 1]) << 8
}

fn be16(bytes: &[u8], at: usize) -> u32 {
    u32::from(bytes[at]) << 8 | u32::from(bytes[at + 1])
}

fn le24(bytes: &[u8], at: usize) -> u32 {
    le16(bytes, at) | u32::from(bytes[at + 2]) << 16
}

/// Width and height of the `IHDR` chunk, which must come first.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let header = bytes.get(12..24).filter(|header| header.starts_with(b"IHDR"))?;
    let word = |at: usize| u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]);
    Some((word(4), word(8)))
}

/// Size of the first chunk: `VP8 ` (lossy), `VP8L` (lossless) or `VP8X` (extended).
fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let data = bytes.get(20..30)?;
    match &bytes[12..16] {
        b"VP8 " if data[3..6] == [0x9d, 0x01, 0x2a] => Some((le16(data, 6) & 0x3fff, le16(data, 8) & 0x3fff)),
        b"VP8L" if data[0] == 0x2f => {
            let bits = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
            Some(((bits & 0x3fff) + 1, (bits >> 14 & 0x3fff) + 1))
        }
        b"VP8X" => Some((le24(data, 4) + 1, le24(data, 7) + 1)),
        _ => None,
    }
}

/// Height and width of the first frame header (`SOF0`..`SOF15`, except
/// `DHT`, `JPG` and `DAC`), jumping over the other segments by their length.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        if *bytes.get(at)? != 0xff {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        match marker {
            // Fill byte before a marker.
            0xff => at += 1,
            // Markers without a segment.
            0x01 | 0xd0..=0xd8 => at += 2,
            // Start of scan or end of image: no frame header before the data.
            0xd9 | 0xda => return None,
            _ => {
                let is_frame = (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
                if is_frame {
                    let segment = bytes.get(at + 2..at + 9)?;
                    return Some((be16(segment, 5), be16(segment, 3)));
                }
                let length = be16(bytes.get(at + 2..at + 4)?, 0) as usize;
                if length < 2 {
                    return None;
                }
                at += 2 + length;
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/storage/analyze.rs"]
mod tests;
