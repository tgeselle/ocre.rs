use axum::http::{HeaderMap, HeaderValue, StatusCode, header};

use super::*;
use crate::{Error, params, support::body_text};

fn attachment(filename: &str, content_type: &str, size: i64) -> Attachment {
    Attachment { key: "posts/avatar/k".into(), filename: filename.into(), content_type: content_type.into(), size }
}

fn upload(content_type: &str, bytes: &'static str) -> Upload {
    Upload { filename: "f".into(), content_type: content_type.into(), bytes: bytes.into() }
}

#[test]
fn prepare_generates_a_fresh_key_and_cleans_the_metadata() {
    let a = Attachment::prepare("posts/avatar/", "C:\\Users\\ada\\me.PNG", "Image/PNG; x=y", 42);
    let b = Attachment::prepare("posts/avatar", "me.png", "image/png", 42);
    assert!(a.key.starts_with("posts/avatar/") && a.key.len() == "posts/avatar/".len() + 22, "{}", a.key);
    assert_ne!(a.key, b.key, "keys are random");
    assert_eq!((a.filename.as_str(), a.content_type.as_str(), a.size), ("me.PNG", "image/png", 42));
    assert_eq!(Attachment::prepare("x", "f", "", u64::MAX).size, i64::MAX);
}

#[test]
fn keys_without_prefix_are_just_the_random_part() {
    let key = new_key("/");
    assert_eq!(key.len(), 22);
    assert!(key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{key}");
}

#[test]
fn human_sizes_use_powers_of_1024() {
    assert_eq!(human_size(0), "0 bytes");
    assert_eq!(human_size(1), "1 byte");
    assert_eq!(human_size(1023), "1023 bytes");
    assert_eq!(human_size(1024), "1 KB");
    assert_eq!(human_size(1536), "1.5 KB");
    assert_eq!(human_size(5 * 1024 * 1024), "5 MB");
    assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3 GB");
    assert_eq!(human_size(u64::MAX), "16777216 TB");
    assert_eq!(attachment("a", "b", 2048).human_size(), "2 KB");
    assert_eq!(attachment("a", "b", -1).human_size(), "0 bytes");
}

#[test]
fn upload_size_is_its_byte_count() {
    assert_eq!(upload("text/plain", "hello").size(), 5);
}

#[test]
fn rules_match_exact_types_ignoring_case_and_parameters() {
    let rules = Rules { max_bytes: 10, content_types: &["image/png", "text/plain"] };
    assert!(rules.allows("image/png"));
    assert!(rules.allows(" Text/Plain; charset=utf-8"));
    assert!(!rules.allows("image/svg+xml"));
    assert!(!rules.allows(""));
}

#[test]
fn file_validation_checks_size_and_type() {
    const RULES: Rules = Rules { max_bytes: 5, content_types: &["text/plain"] };
    let mut v = Validator::new();
    v.file("doc", &upload("text/plain", "12345"), &RULES);
    assert!(v.finish().is_ok(), "at the limit is fine");
    let Err(Error::Invalid(errors)) = Validator::new().file("doc", &upload("image/png", "123456"), &RULES).finish()
    else {
        panic!("expected field errors")
    };
    let messages: Vec<String> = errors.iter().map(|e| e.full_message()).collect();
    assert_eq!(
        messages,
        ["Doc is too large (maximum is 5 bytes)", "Doc has an unsupported type (allowed: text/plain)"]
    );
}

#[test]
fn columns_bind_the_four_values_or_nulls() {
    let file = attachment("a.png", "image/png", 3);
    assert_eq!(columns(Some(&file)).to_vec(), params!["posts/avatar/k", "a.png", "image/png", 3_i64]);
    assert_eq!(columns(None).to_vec(), params![None::<i64>, None::<i64>, None::<i64>, None::<i64>]);
}

#[test]
fn column_changes_start_with_whether_the_file_changes() {
    let file = attachment("a.png", "image/png", 3);
    assert_eq!(column_changes(None).to_vec(), params![false, None::<i64>, None::<i64>, None::<i64>, None::<i64>]);
    assert_eq!(column_changes(Some(None)).to_vec(), params![true, None::<i64>, None::<i64>, None::<i64>, None::<i64>]);
    assert_eq!(
        column_changes(Some(Some(&file))).to_vec(),
        params![true, "posts/avatar/k", "a.png", "image/png", 3_i64]
    );
}

#[test]
fn filenames_lose_directories_and_control_characters() {
    assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
    assert_eq!(sanitize_filename("C:\\fakepath\\photo.jpg"), "photo.jpg");
    assert_eq!(sanitize_filename(" re\u{7}port\n.pdf "), "report.pdf");
    assert_eq!(sanitize_filename("résumé été.txt"), "résumé été.txt");
    for empty in ["", "   ", "dir/", ".", ".."] {
        assert_eq!(sanitize_filename(empty), "file", "{empty:?}");
    }
}

#[test]
fn long_filenames_keep_their_extension() {
    let long = format!("{}.jpeg", "a".repeat(300));
    let kept = sanitize_filename(&long);
    assert_eq!(kept.chars().count(), 200);
    assert!(kept.ends_with("aaa.jpeg"));
    let odd = format!("{}.{}", "b".repeat(250), "c".repeat(20));
    assert_eq!(sanitize_filename(&odd), "b".repeat(200), "an extension over 16 characters is not kept");
    assert_eq!(sanitize_filename(&"é".repeat(201)), "é".repeat(200));
}

#[test]
fn essence_is_lowercase_without_parameters() {
    assert_eq!(essence("Text/HTML; charset=UTF-8"), "text/html");
    assert_eq!(essence("  "), "application/octet-stream");
}

#[test]
fn content_disposition_is_inline_only_for_safe_types() {
    assert_eq!(content_disposition(Disposition::Inline, "me.png", "image/png"), "inline; filename=\"me.png\"");
    assert_eq!(content_disposition(Disposition::Inline, "x.svg", "image/svg+xml"), "attachment; filename=\"x.svg\"");
    assert_eq!(content_disposition(Disposition::Inline, "x.html", "text/html"), "attachment; filename=\"x.html\"");
    assert_eq!(content_disposition(Disposition::Download, "me.png", "image/png"), "attachment; filename=\"me.png\"");
}

#[test]
fn content_disposition_encodes_non_ascii_names() {
    assert_eq!(
        content_disposition(Disposition::Download, "résumé \"final\".pdf", "application/pdf"),
        "attachment; filename=\"r_sum_ _final_.pdf\"; filename*=UTF-8''r%C3%A9sum%C3%A9%20%22final%22.pdf"
    );
    assert_eq!(
        content_disposition(Disposition::Download, "a\\b~!.txt", "text/plain"),
        "attachment; filename=\"a_b~!.txt\"; filename*=UTF-8''a%5Cb~!.txt"
    );
}

#[test]
fn byte_ranges_follow_rfc_9110() {
    let partial = |offset, length| ByteRange::Partial { offset, length };
    assert_eq!(byte_range(None, 100), ByteRange::Full);
    assert_eq!(byte_range(Some("bytes=0-9"), 100), partial(0, 10));
    assert_eq!(byte_range(Some(" bytes=90- "), 100), partial(90, 10));
    assert_eq!(byte_range(Some("bytes=50-500"), 100), partial(50, 50), "clamped to the end");
    assert_eq!(byte_range(Some("bytes=-10"), 100), partial(90, 10));
    assert_eq!(byte_range(Some("bytes=-500"), 100), partial(0, 100));
    assert_eq!(byte_range(Some("bytes=100-"), 100), ByteRange::Unsatisfiable);
    assert_eq!(byte_range(Some("bytes=0-0"), 0), ByteRange::Unsatisfiable);
    assert_eq!(byte_range(Some("bytes=-0"), 100), ByteRange::Unsatisfiable);
    assert_eq!(byte_range(Some("bytes=-5"), 0), ByteRange::Unsatisfiable);
    for ignored in ["items=0-1", "bytes=0-1,5-6", "bytes=5", "bytes=a-", "bytes=-z", "bytes=9-3", "bytes=1-x"] {
        assert_eq!(byte_range(Some(ignored), 100), ByteRange::Full, "{ignored}");
    }
}

#[test]
fn if_none_match_takes_the_first_tag_unquoted() {
    assert_eq!(if_none_match(Some("\"abc\"")), Some("abc".to_owned()));
    assert_eq!(if_none_match(Some("W/\"abc\", \"def\"")), Some("abc".to_owned()));
    assert_eq!(if_none_match(Some("*")), None);
    assert_eq!(if_none_match(Some("\"\"")), None);
    assert_eq!(if_none_match(None), None);
}

#[test]
fn fetch_reads_range_and_if_none_match() {
    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=2-3"));
    headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"e1\""));
    assert_eq!(
        Fetch::from_headers(&headers, 10),
        Fetch { range: ByteRange::Partial { offset: 2, length: 2 }, if_none_match: Some("e1".into()) }
    );
    headers.insert(header::IF_RANGE, HeaderValue::from_static("\"old\""));
    assert_eq!(Fetch::from_headers(&headers, 10).range, ByteRange::Full, "If-Range sends the whole file");
    assert_eq!(Fetch::from_headers(&HeaderMap::new(), -1), Fetch { range: ByteRange::Full, if_none_match: None });
}

fn header_of(response: &Response, name: header::HeaderName) -> &str {
    response.headers().get(name).map_or("", |value| value.to_str().unwrap())
}

#[test]
fn file_responses_carry_type_length_disposition_and_cache_headers() {
    let response = file_response(
        &attachment("me.png", "image/png", 5),
        Disposition::Inline,
        "\"e1\"",
        ByteRange::Full,
        "hello".into(),
    );
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, header::CONTENT_TYPE), "image/png");
    assert_eq!(header_of(&response, header::CONTENT_LENGTH), "5");
    assert_eq!(header_of(&response, header::CONTENT_DISPOSITION), "inline; filename=\"me.png\"");
    assert_eq!(header_of(&response, header::ACCEPT_RANGES), "bytes");
    assert_eq!(header_of(&response, header::CACHE_CONTROL), CACHE_CONTROL);
    assert_eq!(header_of(&response, header::ETAG), "\"e1\"");
    assert!(!response.headers().contains_key(header::CONTENT_RANGE));
    assert_eq!(body_text(response), "hello");
}

#[test]
fn partial_responses_are_206_with_content_range() {
    let range = ByteRange::Partial { offset: 1, length: 3 };
    let response =
        file_response(&attachment("a.txt", "text/plain", 5), Disposition::Download, "\"e\"", range, "ell".into());
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(header_of(&response, header::CONTENT_RANGE), "bytes 1-3/5");
    assert_eq!(header_of(&response, header::CONTENT_LENGTH), "3");
    assert_eq!(header_of(&response, header::CONTENT_DISPOSITION), "attachment; filename=\"a.txt\"");
}

#[test]
fn scriptable_types_are_served_as_binary_downloads() {
    let response = file_response(
        &attachment("x.svg", "image/svg+xml", -1),
        Disposition::Inline,
        "bad\netag",
        ByteRange::Full,
        "".into(),
    );
    assert_eq!(header_of(&response, header::CONTENT_TYPE), "application/octet-stream");
    assert_eq!(header_of(&response, header::CONTENT_DISPOSITION), "attachment; filename=\"x.svg\"");
    assert_eq!(header_of(&response, header::CONTENT_LENGTH), "0");
    assert_eq!(header_of(&response, header::ETAG), "application/octet-stream", "invalid header text is replaced");
}

#[test]
fn not_modified_and_unsatisfiable_have_no_body() {
    let response = not_modified("\"e1\"");
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(
        (header_of(&response, header::ETAG), header_of(&response, header::CACHE_CONTROL)),
        ("\"e1\"", CACHE_CONTROL)
    );
    assert_eq!(body_text(response), "");
    let response = unsatisfiable(5);
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(header_of(&response, header::CONTENT_RANGE), "bytes */5");
}

#[test]
fn a_missing_binding_names_the_config_entry() {
    let message = missing_binding("no such binding").to_string();
    assert!(message.contains("STORAGE: bindings.r2({ name: \"<app>-storage\" }),"), "{message}");
    assert!(message.contains("no such binding"));
}

#[test]
fn send_data_sends_generated_bytes_as_a_file() {
    let response = send_data("a,b\n", "../export.csv", "text/csv; charset=utf-8", Disposition::Inline);
    assert_eq!(header_of(&response, header::CONTENT_TYPE), "text/csv");
    assert_eq!(header_of(&response, header::CONTENT_DISPOSITION), "attachment; filename=\"export.csv\"");
    assert_eq!(header_of(&response, header::CONTENT_LENGTH), "4");
    assert_eq!(body_text(response), "a,b\n");
    let response = send_data(vec![1u8, 2], "chart.png", "image/png", Disposition::Inline);
    assert_eq!(header_of(&response, header::CONTENT_DISPOSITION), "inline; filename=\"chart.png\"");
    let response = send_data("<script>", "page.html", "text/html", Disposition::Inline);
    assert_eq!(header_of(&response, header::CONTENT_TYPE), "application/octet-stream");
}

#[test]
fn uploads_built_by_the_app_are_cleaned_up_like_a_browsers() {
    let upload = Upload::new("C:\\tmp\\report.pdf", " Application/PDF; q=1", vec![1u8, 2, 3]);
    assert_eq!(
        (upload.filename.as_str(), upload.content_type.as_str(), upload.size()),
        ("report.pdf", "application/pdf", 3)
    );
}

#[test]
fn stored_objects_become_attachments_under_a_clean_name() {
    let object = StoredObject {
        key: "uploads/k".into(),
        size: u64::MAX,
        content_type: "Image/PNG".into(),
        ..Default::default()
    };
    let attachment = object.attachment("../me.png");
    assert_eq!(
        attachment,
        Attachment {
            key: "uploads/k".into(),
            filename: "me.png".into(),
            content_type: "image/png".into(),
            size: i64::MAX
        }
    );
}

#[test]
fn public_urls_join_the_base_and_the_encoded_key() {
    let base = |url: &str| Some(url.to_owned());
    assert_eq!(
        join_public_url(base("https://files.example.com/"), "users/avatar/a b+é").unwrap(),
        "https://files.example.com/users/avatar/a%20b%2B%C3%A9"
    );
    assert_eq!(join_public_url(base(" https://pub-1.r2.dev "), "k").unwrap(), "https://pub-1.r2.dev/k");
    for missing in [None, base("  ")] {
        let message = join_public_url(missing, "k").unwrap_err().to_string();
        assert!(message.contains("STORAGE_PUBLIC_URL: bindings.text("), "{message}");
    }
}

#[test]
fn redirects_point_to_the_url_and_are_cached_for_half_its_life() {
    let response = redirect_response("https://acc.r2.cloudflarestorage.com/b/k?X-Amz-Signature=1", 3600);
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(header_of(&response, header::LOCATION), "https://acc.r2.cloudflarestorage.com/b/k?X-Amz-Signature=1");
    assert_eq!(header_of(&response, header::CACHE_CONTROL), "private, max-age=1800");
}

#[test]
fn stale_keys_are_those_uploaded_before_the_cutoff() {
    let object = |key: &str, uploaded_at| StoredObject { key: key.into(), uploaded_at, ..Default::default() };
    let objects = [object("old", 99), object("edge", 100), object("new", 101)];
    assert_eq!(stale_keys(&objects, 100), ["old"]);
}

#[test]
fn referenced_keys_sql_binds_every_key_and_refuses_odd_identifiers() {
    assert_eq!(
        referenced_keys_sql("lessons", "video_key", 3).unwrap(),
        "SELECT video_key AS key FROM lessons WHERE video_key IN (?1, ?2, ?3)"
    );
    for (table, column) in [("lessons; DROP TABLE users", "k"), ("t", ""), ("1t", "k"), ("t", "a-b")] {
        let message = referenced_keys_sql(table, column, 1).unwrap_err().to_string();
        assert!(message.contains("purge_unattached takes a table and a column name"), "{message}");
    }
}
