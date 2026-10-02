use super::*;
use crate::storage::Disposition;

const NOW: i64 = 1_790_724_341; // 2026-09-29T23:25:41Z

fn r2() -> S3Endpoint {
    S3Endpoint::r2("acc123", "blog-storage", "AKID", "SECRET/KEY")
}

fn signature(url: &str) -> &str {
    url.rsplit_once("X-Amz-Signature=").expect("signed URL").1
}

#[test]
fn presign_matches_the_aws_documentation_example() {
    // https://docs.aws.amazon.com/AmazonS3/latest/API/sigv4-query-string-auth.html
    let aws = S3Endpoint {
        host: "examplebucket.s3.amazonaws.com".into(),
        region: "us-east-1".into(),
        bucket: String::new(),
        access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
        secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
    };
    let url = aws.presign("GET", "test.txt", &[], &[], 1_369_353_600, 86_400).unwrap();
    assert_eq!(
        url,
        "https://examplebucket.s3.amazonaws.com/test.txt?X-Amz-Algorithm=AWS4-HMAC-SHA256\
         &X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request\
         &X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host\
         &X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
    );
}

#[test]
fn presigned_r2_get_with_response_overrides_matches_botocore() {
    // Signatures computed by botocore 1.42 (S3SigV4QueryAuth) for the same request.
    let query = [
        ("response-content-disposition", "attachment; filename=\"r\u{e9}sum\u{e9}.pdf\""),
        ("response-content-type", "application/pdf"),
    ];
    let url = r2().presign("get", "exports/report 1.csv", &[], &query, NOW, 300).unwrap();
    assert!(url.starts_with("https://acc123.r2.cloudflarestorage.com/blog-storage/exports/report%201.csv?"), "{url}");
    assert!(url.contains("X-Amz-Credential=AKID%2F20260929%2Fauto%2Fs3%2Faws4_request"), "{url}");
    assert!(url.contains("response-content-disposition=attachment%3B%20filename%3D%22r%C3%A9sum%C3%A9.pdf%22"));
    assert_eq!(signature(&url), "fc18edd98f9f2f1d5ee079d2688189183365ae53a6ecdf114b80b06f49c8022b");
}

#[test]
fn presigned_r2_put_signs_type_and_length_like_botocore() {
    let headers = [("Content-Type", " image/png "), ("Content-Length", "1234")];
    let url = r2().presign("PUT", "uploads/photos/abc", &headers, &[], NOW, 600).unwrap();
    assert!(url.contains("&X-Amz-SignedHeaders=content-length%3Bcontent-type%3Bhost&"), "{url}");
    assert_eq!(signature(&url), "a84f0c7803f65b66468fe49657b210296ccca815d3024515e562ed17e8ec1992");
}

#[test]
fn presign_refuses_lifetimes_outside_one_second_to_seven_days() {
    for expires_in in [0, MAX_EXPIRES_IN + 1] {
        let err = r2().presign("GET", "k", &[], &[], NOW, expires_in).unwrap_err();
        assert!(matches!(&err, Error::Internal(message) if message.contains("7 days")), "{err:?}");
    }
    assert!(r2().presign("GET", "k", &[], &[], NOW, MAX_EXPIRES_IN).is_ok());
}

#[test]
fn dates_before_1970_are_formatted_in_utc() {
    assert_eq!(amz_date(-1), ("19691231".to_owned(), "19691231T235959Z".to_owned()));
}

#[test]
fn debug_output_hides_the_secret() {
    let debug = format!("{:?}", r2());
    assert!(debug.contains("acc123.r2.cloudflarestorage.com") && debug.contains("[redacted]"), "{debug}");
    assert!(!debug.contains("SECRET/KEY"), "{debug}");
}

#[test]
fn r2_endpoint_reads_the_four_settings_and_names_the_missing_ones() {
    let all = |name: &str| Some(format!(" {name}-value "));
    let endpoint = r2_endpoint(&all).unwrap();
    assert_eq!(endpoint.host, "R2_ACCOUNT_ID-value.r2.cloudflarestorage.com");
    assert_eq!(endpoint.bucket, "R2_BUCKET-value");
    assert_eq!(endpoint.secret_access_key, "R2_SECRET_ACCESS_KEY-value");

    let some = |name: &str| (name == R2_BUCKET || name == R2_ACCESS_KEY_ID).then(|| "x".to_owned());
    let err = r2_endpoint(&some).unwrap_err().to_string();
    assert!(err.contains("need R2_ACCOUNT_ID, R2_SECRET_ACCESS_KEY (not set)"), "{err}");
    assert!(err.contains("ocre secrets push R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY"), "{err}");
    let blank = |_: &str| Some("  ".to_owned());
    assert!(r2_endpoint(&blank).unwrap_err().to_string().contains(R2_ACCOUNT_ID));
}

#[test]
fn presigned_gets_carry_a_safe_type_and_disposition() {
    let html = Attachment { key: "k".into(), filename: "page.html".into(), content_type: "text/html".into(), size: 1 };
    let url = presign_get_url(&r2(), &html, Disposition::Inline, NOW, 60).unwrap();
    assert!(url.contains("response-content-type=application%2Foctet-stream"), "{url}");
    assert!(url.contains("response-content-disposition=attachment%3B%20filename%3D%22page.html%22"), "{url}");

    let png = Attachment { content_type: "image/png".into(), filename: "a.png".into(), ..html };
    let url = presign_get_url(&r2(), &png, Disposition::Inline, NOW, 60).unwrap();
    assert!(url.contains("response-content-type=image%2Fpng"), "{url}");
    assert!(url.contains("response-content-disposition=inline%3B"), "{url}");
}

const PHOTO: Rules = Rules { max_bytes: 100, content_types: &["image/png"] };

fn request(content_type: &str, size: u64) -> DirectUploadRequest {
    DirectUploadRequest { filename: "a.png".into(), content_type: content_type.into(), size }
}

#[test]
fn direct_uploads_check_the_declaration_and_sign_a_new_key() {
    let upload = DirectUpload::sign(&r2(), "uploads/photos/", "image", &request("Image/PNG", 100), &PHOTO, NOW, 600);
    let upload = upload.unwrap();
    let key = verify_key("image", "SECRET/KEY", &upload.signed_key).unwrap();
    assert!(key.starts_with("uploads/photos/") && key.len() == "uploads/photos/".len() + 22, "{key}");
    assert!(upload.url.starts_with(&format!("https://acc123.r2.cloudflarestorage.com/blog-storage/{key}?")));
    assert!(upload.url.contains("X-Amz-SignedHeaders=content-length%3Bcontent-type%3Bhost"));
    assert_eq!(upload.headers, BTreeMap::from([("Content-Type".to_owned(), "image/png".to_owned())]));

    let err = DirectUpload::sign(&r2(), "uploads", "image", &request("image/gif", 101), &PHOTO, NOW, 600).unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid: Image is too large (maximum is 100 bytes), Image has an unsupported type (allowed: image/png)"
    );
}

#[test]
fn signed_keys_only_verify_with_the_same_secret_and_key() {
    let signed = sign_key("secret", "uploads/abc");
    assert_eq!(verify_key("file", "secret", &signed).unwrap(), "uploads/abc");
    for forged in [
        sign_key("other", "uploads/abc"),
        signed.replace("abc", "abd"),
        "uploads/abc".to_owned(),
        String::new(),
        format!("{signed}x"),
    ] {
        let err = verify_key("file", "secret", &forged).unwrap_err();
        assert_eq!(err.to_string(), "invalid: File is not a valid upload", "{forged}");
    }
}

#[test]
fn heads_become_attachments_once_they_pass_the_rules() {
    let object = StoredObject {
        key: "uploads/k".into(),
        size: 42,
        content_type: "image/png".into(),
        etag: "e".into(),
        uploaded_at: NOW,
        filename: None,
    };
    let attachment = attachment_from_head("image", Some(&object), "C:\\me.png", &PHOTO).unwrap();
    assert_eq!(
        attachment,
        Attachment { key: "uploads/k".into(), filename: "me.png".into(), content_type: "image/png".into(), size: 42 }
    );

    let missing = attachment_from_head("image", None, "me.png", &PHOTO).unwrap_err();
    assert_eq!(missing.to_string(), "invalid: Image was not uploaded");
    let big = StoredObject { size: 101, ..object };
    let err = attachment_from_head("image", Some(&big), "me.png", &PHOTO).unwrap_err();
    assert_eq!(err.to_string(), "invalid: Image is too large (maximum is 100 bytes)");
}

#[test]
fn upload_keys_are_signed_with_the_r2_secret_else_secret_key_base() {
    let vars = |pairs: &'static [(&str, &str)]| {
        move |name: &str| pairs.iter().find(|(n, _)| *n == name).map(|(_, v)| (*v).to_owned())
    };
    assert_eq!(upload_secret(&vars(&[("R2_SECRET_ACCESS_KEY", "r2"), ("SECRET_KEY_BASE", "base")])).unwrap(), "r2");
    assert_eq!(upload_secret(&vars(&[("R2_SECRET_ACCESS_KEY", " "), ("SECRET_KEY_BASE", "base")])).unwrap(), "base");
    assert!(upload_secret(&vars(&[])).unwrap_err().to_string().contains("R2_SECRET_ACCESS_KEY or SECRET_KEY_BASE"));
}
