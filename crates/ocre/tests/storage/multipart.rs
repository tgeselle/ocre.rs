use axum::{
    body::Body,
    extract::FromRequest,
    http::{Request as HttpRequest, StatusCode},
};
use serde::Deserialize;

use super::*;
use crate::support::{block_on, body_text};

const BOUNDARY: &str = "----ocre7MA4YWxkTrZu0gW";

/// A browser-shaped body: text fields, then files `(name, filename, type, bytes)`.
fn body(fields: &[(&str, &str)], files: &[(&str, &str, &str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in fields {
        out.extend(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").bytes(),
        );
    }
    for (name, filename, content_type, bytes) in files {
        out.extend(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
            )
            .bytes(),
        );
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend(format!("--{BOUNDARY}--\r\n").bytes());
    out
}

fn parse(bytes: &[u8]) -> Result<MultipartForm> {
    MultipartForm::parse(Bytes::copy_from_slice(bytes), BOUNDARY)
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct PostForm {
    title: String,
    pages: i64,
    published: bool,
}

#[test]
fn parses_text_fields_and_files() {
    let png: &[u8] = b"\x89PNG\r\n\x1a\n\0\r\n--not-the-boundary\r\n";
    let bytes = body(&[("title", "Hello & bye"), ("pages", "12")], &[("avatar", "me.png", "image/png", png)]);
    let mut form = parse(&bytes).unwrap();
    assert_eq!(form.text("title"), Some("Hello & bye"));
    assert_eq!(form.text("missing"), None);
    assert_eq!(form.form::<PostForm>().unwrap(), PostForm { title: "Hello & bye".into(), pages: 12, published: false });
    let avatar = form.file("avatar").unwrap();
    assert_eq!((avatar.filename.as_str(), avatar.content_type.as_str()), ("me.png", "image/png"));
    assert_eq!(&avatar.bytes[..], png, "binary content, CRLF and dashes included, is kept byte for byte");
    assert_eq!(form.file("avatar"), None, "a file is taken once");
}

#[test]
fn an_unchosen_file_input_is_no_file() {
    let bytes = body(&[], &[("avatar", "", "application/octet-stream", b"")]);
    assert_eq!(parse(&bytes).unwrap().file("avatar"), None);
    let empty_file = body(&[], &[("avatar", "empty.txt", "text/plain", b"")]);
    assert_eq!(parse(&empty_file).unwrap().file("avatar").unwrap().size(), 0, "an empty file with a name is a file");
}

#[test]
fn file_metadata_is_normalized() {
    let raw = format!(
        "--{BOUNDARY}\r\ncontent-disposition: form-data; name=\"doc\"; filename=\"C:\\\\docs\\\\a \\\"b\\\".txt\"\r\n\r\nx\r\n--{BOUNDARY}--"
    );
    let upload = parse(raw.as_bytes()).unwrap().file("doc").unwrap();
    assert_eq!(upload.filename, "a \"b\".txt");
    assert_eq!(upload.content_type, "application/octet-stream", "no Content-Type header");
    let typed = body(&[], &[("doc", "a.txt", "Text/Plain; charset=UTF-8", b"x")]);
    assert_eq!(parse(&typed).unwrap().file("doc").unwrap().content_type, "text/plain");
}

#[test]
fn preamble_padding_and_headerless_parts_are_tolerated() {
    let raw = format!(
        "preamble\r\n--{BOUNDARY} \t\r\nContent-Disposition: form-data; name=title\r\nX-Other: 1\r\nbroken header\r\n\r\nHi\r\n--{BOUNDARY}\r\n\r\nno headers\r\n--{BOUNDARY}\r\nContent-Disposition: form-data\r\n\r\nno name\r\n--{BOUNDARY}--"
    );
    let form = parse(raw.as_bytes()).unwrap();
    assert_eq!(form.text("title"), Some("Hi"));
    assert_eq!(form, MultipartForm { fields: vec![("title".into(), "Hi".into())], files: vec![] });
}

#[test]
fn malformed_bodies_are_bad_requests() {
    let cases = [
        ("no boundary here".to_owned(), "the boundary never appears"),
        (format!("--{BOUNDARY}garbage"), "expected a line break after the boundary"),
        (format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=a"), "a part has no end of headers"),
        (
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=a\r\n\r\nvalue"),
            "the closing boundary is missing",
        ),
    ];
    for (raw, detail) in cases {
        let Err(Error::BadRequest(message)) = parse(raw.as_bytes()) else { panic!("{raw:?} parsed") };
        assert_eq!(message, format!("Malformed multipart/form-data body: {detail}"));
    }
    let latin1 = body(&[], &[]);
    let mut invalid = format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"t\"\r\n\r\n").into_bytes();
    invalid.extend_from_slice(b"\xff\r\n");
    invalid.extend_from_slice(&latin1);
    let Err(Error::BadRequest(message)) = parse(&invalid) else { panic!("invalid UTF-8 parsed") };
    assert_eq!(message, "Form field `t` is not valid UTF-8");
}

#[test]
fn form_errors_are_bad_requests() {
    let form = parse(&body(&[("pages", "many")], &[])).unwrap();
    let Err(Error::BadRequest(message)) = form.form::<PostForm>() else { panic!("parsed") };
    assert!(message.starts_with("Invalid form: "), "{message}");
}

#[test]
fn boundary_comes_from_the_content_type() {
    assert_eq!(boundary("multipart/form-data; boundary=abc"), Some("abc".into()));
    assert_eq!(boundary("Multipart/Form-Data; charset=utf-8; BOUNDARY=\"a;b\""), Some("a;b".into()));
    assert_eq!(boundary("multipart/form-data"), None);
    assert_eq!(boundary("multipart/form-data; boundary="), None);
    assert_eq!(boundary(&format!("multipart/form-data; boundary={}", "x".repeat(71))), None);
    assert_eq!(boundary("application/x-www-form-urlencoded"), None);
}

#[test]
fn quoted_values_unescape_backslashes() {
    assert_eq!(unquote("\"a \\\"b\\\"\""), "a \"b\"");
    assert_eq!(unquote("\"a\\\\b\\\""), "a\\b", "an escaped backslash; a dangling one is dropped");
    assert_eq!(unquote("plain"), "plain");
    assert_eq!(split_params("a; b=\"x;y\"; c=\"\\\";\""), ["a", " b=\"x;y\"", " c=\"\\\";\""]);
}

fn request(content_type: &str, accept: &str, length: Option<usize>, body: Vec<u8>) -> Request {
    let mut builder = HttpRequest::builder().method("POST").uri("/posts").header("content-type", content_type);
    if !accept.is_empty() {
        builder = builder.header("accept", accept);
    }
    if let Some(length) = length {
        builder = builder.header("content-length", length.to_string());
    }
    builder.body(Body::from(body)).unwrap()
}

fn multipart_type() -> String {
    format!("multipart/form-data; boundary={BOUNDARY}")
}

/// The extracted form, or the rejection's status and body.
fn extract<const LIMIT: usize>(req: Request) -> Result<MultipartForm, (StatusCode, String)> {
    match block_on(Multipart::<LIMIT>::from_request(req, &())) {
        Ok(Multipart(form)) => Ok(form),
        Err(response) => Err((response.status(), body_text(response))),
    }
}

#[test]
fn the_extractor_reads_the_body_within_its_limit() {
    let bytes = body(&[("title", "Hi")], &[("avatar", "a.txt", "text/plain", b"hello")]);
    let length = bytes.len();
    let mut form = extract::<1024>(request(&multipart_type(), "", Some(length), bytes)).unwrap();
    assert_eq!(form.text("title"), Some("Hi"));
    assert_eq!(&form.file("avatar").unwrap().bytes[..], b"hello");
}

#[test]
fn bodies_over_the_limit_are_413() {
    let bytes = body(&[("title", &"x".repeat(2000))], &[]);
    let (status, body) = extract::<1024>(request(&multipart_type(), "", Some(bytes.len()), bytes.clone())).unwrap_err();
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        body, r#"{"error":{"message":"The request is too large (maximum is 1 KB)","status":413}}"#,
        "JSON for API clients"
    );
    let (status, body) = extract::<1024>(request(&multipart_type(), "text/html,*/*", None, bytes)).unwrap_err();
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "counted while reading without Content-Length");
    assert!(body.contains("<p>The request is too large (maximum is 1 KB)</p>"), "HTML for browsers");
}

#[test]
fn other_content_types_are_400() {
    let (status, body) =
        extract::<1024>(request("application/x-www-form-urlencoded", "", None, b"a=b".to_vec())).unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("enctype=\\\"multipart/form-data\\\""));
}

#[test]
fn body_read_errors_are_400() {
    let stream = futures_util::stream::iter([Err::<Bytes, _>(std::io::Error::other("reset"))]);
    let req = HttpRequest::builder().header("content-type", multipart_type()).body(Body::from_stream(stream)).unwrap();
    let (status, body) = extract::<1024>(req).unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Could not read the request body: reset"));
}

#[test]
fn several_files_of_one_input_come_in_order() {
    let files: &[(&str, &str, &str, &[u8])] = &[
        ("photos", "a.png", "image/png", b"A"),
        ("cover", "c.png", "image/png", b"C"),
        ("photos[]", "b.png", "image/png", b"B"),
    ];
    let mut form = parse(&body(&[], files)).unwrap();
    let names: Vec<String> = form.files("photos").into_iter().map(|file| file.filename).collect();
    assert_eq!(names, ["a.png", "b.png"]);
    assert!(form.files("photos").is_empty(), "taken");
    assert_eq!(form.file("cover").unwrap().filename, "c.png", "other inputs stay");
}
