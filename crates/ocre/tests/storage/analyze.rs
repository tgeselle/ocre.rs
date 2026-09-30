use super::*;

fn padded(bytes: &[u8]) -> Vec<u8> {
    let mut bytes = bytes.to_vec();
    bytes.resize(32, 0);
    bytes
}

fn found(bytes: &[u8]) -> (Option<&'static str>, Option<u32>, Option<u32>) {
    let analysis = analyze(bytes);
    (analysis.content_type, analysis.width, analysis.height)
}

const PNG_640X480: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\x02\x80\0\0\x01\xe0\x08\x06\0\0\0";

#[test]
fn png_size_comes_from_ihdr() {
    assert_eq!(found(PNG_640X480), (Some("image/png"), Some(640), Some(480)));
    assert_eq!(found(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDX\0\0\x02\x80\0\0\x01\xe0"), (Some("image/png"), None, None));
    assert_eq!(found(b"\x89PNG\r\n\x1a\n"), (Some("image/png"), None, None));
}

#[test]
fn gif_size_comes_from_the_logical_screen() {
    assert_eq!(found(b"GIF89a\x80\x02\xe0\x01\0\0"), (Some("image/gif"), Some(640), Some(480)));
    assert_eq!(found(b"GIF87a\x01\0\x01\0"), (Some("image/gif"), Some(1), Some(1)));
    assert_eq!(found(b"GIF89a\x80\x02"), (Some("image/gif"), None, None));
}

#[test]
fn webp_size_comes_from_vp8_vp8l_or_vp8x() {
    let lossy = padded(b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\x10\x02\x00\x9d\x01\x2a\x80\x02\xe0\x01");
    assert_eq!(found(&lossy), (Some("image/webp"), Some(640), Some(480)));
    let lossless = padded(b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f\x7f\xc2\x77\x00");
    assert_eq!(found(&lossless), (Some("image/webp"), Some(640), Some(480)));
    let extended = padded(b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0\0\0\0\0\x7f\x02\x00\xdf\x01\x00");
    assert_eq!(found(&extended), (Some("image/webp"), Some(640), Some(480)));

    let bad_start_code = padded(b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\x10\x02\x00\x9d\x01\x2b");
    let bad_signature = padded(b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2e");
    let other_chunk = padded(b"RIFF\0\0\0\0WEBPALPH");
    for bytes in [bad_start_code, bad_signature, other_chunk, b"RIFF\0\0\0\0WEBPVP8X".to_vec()] {
        assert_eq!(found(&bytes), (Some("image/webp"), None, None));
    }
    assert_eq!(found(b"RIFF\0\0\0\0WAVEfmt "), (None, None, None), "RIFF but not WebP");
}

/// SOI, APP0 (16 bytes), a fill byte, DHT (not a frame), RST0, then SOF0 of a 640x480 image.
const JPEG_640X480: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF\0\x01\x01\0\0\x01\0\x01\0\0\xff\xff\xc4\x00\x04\0\0\
\xff\xd0\xff\x01\xff\xc0\x00\x11\x08\x01\xe0\x02\x80\x03";

#[test]
fn jpeg_size_comes_from_the_first_frame_header() {
    assert_eq!(found(JPEG_640X480), (Some("image/jpeg"), Some(640), Some(480)));
    let progressive = b"\xff\xd8\xff\xc2\x00\x11\x08\x00\x10\x00\x20\x03";
    assert_eq!(found(progressive), (Some("image/jpeg"), Some(32), Some(16)));
}

#[test]
fn jpeg_without_a_reachable_frame_header_has_no_size() {
    let cases: [&[u8]; 7] = [
        b"\xff\xd8\xff",                     // truncated marker
        b"\xff\xd8\xff\xe0\x00\x10JFIF",     // segment runs past the end
        b"\xff\xd8\xff\xda\x00\x08",         // start of scan first
        b"\xff\xd8\xff\xd9",                 // end of image
        b"\xff\xd8\xff\xe0\x00\x01",         // impossible segment length
        b"\xff\xd8\xff\xe0\x00\x02\x00",     // garbage where a marker should be
        b"\xff\xd8\xff\xc0\x00\x11\x08\x01", // frame header cut short
    ];
    for bytes in cases {
        assert_eq!(found(bytes), (Some("image/jpeg"), None, None), "{bytes:?}");
    }
    assert_eq!(found(b"\xff\xd8\xff\xe0\x00\x02"), (Some("image/jpeg"), None, None), "ends after a segment");
    assert_eq!(found(b"\xff\xd8\xff\xe0\x00"), (Some("image/jpeg"), None, None), "length cut short");
}

#[test]
fn other_signatures_give_a_type_without_size() {
    assert_eq!(found(b"%PDF-1.7\n"), (Some("application/pdf"), None, None));
    assert_eq!(found(b"PK\x03\x04\x14\0"), (Some("application/zip"), None, None));
    assert_eq!(found(b"PK\x05\x06\0\0"), (Some("application/zip"), None, None));
    let ftyp = |brand: &[u8]| [b"\0\0\0\x18ftyp".as_slice(), brand].concat();
    assert_eq!(found(&ftyp(b"avif")).0, Some("image/avif"));
    assert_eq!(found(&ftyp(b"avis")).0, Some("image/avif"));
    assert_eq!(found(&ftyp(b"qt  ")).0, Some("video/quicktime"));
    assert_eq!(found(&ftyp(b"M4A ")).0, Some("audio/mp4"));
    assert_eq!(found(&ftyp(b"isom")).0, Some("video/mp4"));
    assert_eq!(found(b"\0\0\0\x18ftyp"), (None, None, None), "brand missing");
}

#[test]
fn text_and_unknown_bytes_have_no_type() {
    for bytes in [&b""[..], b"hello", b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>", b"id,title\n1,a\n"] {
        assert_eq!(analyze(bytes), Analysis::default());
    }
}

#[test]
fn file_content_compares_the_bytes_with_the_declared_type() {
    let check = |content_type: &str, bytes: &'static [u8]| {
        Validator::new()
            .file_content("file", &Upload::new("f", content_type, bytes))
            .finish()
            .map_err(|e| e.to_string())
    };
    assert_eq!(check("image/png", PNG_640X480), Ok(()));
    assert_eq!(check("Image/PNG; x=y", PNG_640X480), Ok(()));
    assert_eq!(check("text/plain", b"hello"), Ok(()));
    assert_eq!(check("image/svg+xml", b"<svg/>"), Ok(()));
    assert_eq!(check("image/png", b"<script>"), Err("invalid: File has content that does not match image/png".into()));
    assert_eq!(
        check("image/jpeg", PNG_640X480),
        Err("invalid: File has content that does not match image/jpeg".into())
    );
    assert_eq!(
        check("text/plain", b"%PDF-1.7"),
        Err("invalid: File has content that does not match text/plain".into())
    );
}
