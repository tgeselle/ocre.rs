use super::*;

#[test]
fn list_parts_answers_parse_with_the_next_marker() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListPartsResult><Bucket>b</Bucket><Key>v/a.mp4</Key><UploadId>u1</UploadId>
<IsTruncated>true</IsTruncated><NextPartNumberMarker>2</NextPartNumberMarker>
<Part><PartNumber>1</PartNumber><ETag>&quot;e1&quot;</ETag><Size>5242880</Size></Part>
<Part><PartNumber>2</PartNumber><ETag>"e2"</ETag><Size>12</Size></Part>
<Part><PartNumber>x</PartNumber><ETag>"bad"</ETag><Size>1</Size></Part>
</ListPartsResult>"#;
    let (parts, next) = parse_parts(xml);
    assert_eq!(
        parts,
        [
            StoredPart { part_number: 1, etag: "e1".into(), size: 5_242_880 },
            StoredPart { part_number: 2, etag: "e2".into(), size: 12 },
        ]
    );
    assert_eq!(next.as_deref(), Some("2"));
    let (_, last) = parse_parts("<IsTruncated>false</IsTruncated><NextPartNumberMarker>9</NextPartNumberMarker>");
    assert_eq!(last, None, "a complete listing has no next page");
}

#[test]
fn list_uploads_answers_parse_with_both_markers() {
    let xml = "<ListMultipartUploadsResult><IsTruncated>true</IsTruncated>\
        <NextKeyMarker>v/b&amp;c</NextKeyMarker><NextUploadIdMarker>u2</NextUploadIdMarker>\
        <Upload><Key>v/a</Key><UploadId>u1</UploadId><Initiated>2026-10-18T10:00:00.000Z</Initiated></Upload>\
        <Upload><Key>v/b&amp;c</Key><UploadId>u2</UploadId></Upload>\
        <Upload><UploadId>no key</UploadId></Upload><Upload><Key>unclosed</Key>";
    let (uploads, next) = parse_uploads(xml);
    assert_eq!(
        uploads,
        [
            PendingUpload { key: "v/a".into(), upload_id: "u1".into(), initiated: "2026-10-18T10:00:00.000Z".into() },
            PendingUpload { key: "v/b&c".into(), upload_id: "u2".into(), initiated: String::new() },
        ]
    );
    assert_eq!(next, Some(("v/b&c".into(), "u2".into())));
    assert_eq!(parse_uploads("<IsTruncated>true</IsTruncated>").1, None, "markers are needed to go on");
}

#[test]
fn gone_uploads_are_recognized_in_r2_errors() {
    assert!(upload_gone("NoSuchUpload: The specified multipart upload does not exist."));
    assert!(upload_gone("Error: multipart upload does not exist (10024)"));
    assert!(!upload_gone("InvalidPart: one or more parts could not be found"));
}

#[test]
fn development_urls_stand_for_presigned_ones_on_the_app() {
    assert_eq!(
        dev_url("http://localhost:8787/ ", "videos/1/a b.mp4", &[("partNumber", "2"), ("uploadId", "u/1=")], 3600),
        "http://localhost:8787/ocre/dev/storage/videos/1/a%20b.mp4?partNumber=2&uploadId=u%2F1%3D&X-Amz-Expires=3600"
    );
}

#[test]
fn upload_markers_round_trip_keys_and_upload_ids() {
    let marker = dev_upload_marker("videos/1/a.mp4", "id/with=slash");
    assert_eq!(marker, ".ocre-dev-storage/uploads/videos/1/a.mp4/id%2Fwith%3Dslash");
    let upload = dev_upload_from_marker(&marker, "001".into()).unwrap();
    assert_eq!(
        (upload.key.as_str(), upload.upload_id.as_str(), upload.initiated.as_str()),
        ("videos/1/a.mp4", "id/with=slash", "001")
    );
    assert_eq!(dev_parts_prefix("id/x"), ".ocre-dev-storage/parts/id%2Fx/");
    assert!(dev_upload_from_marker(".ocre-dev-storage/parts/u/1", String::new()).is_none());
    assert!(dev_upload_from_marker("videos/a/u", String::new()).is_none());
    assert!(dev_upload_from_marker(".ocre-dev-storage/uploads/k/bad%zz", String::new()).is_none());
    assert!(dev_upload_from_marker(".ocre-dev-storage/uploads/k/%ff", String::new()).is_none(), "not UTF-8");
    assert!(dev_upload_from_marker(".ocre-dev-storage/uploads/k/%f", String::new()).is_none(), "truncated");
}

#[test]
fn stored_parts_become_completed_parts() {
    let part = StoredPart { part_number: 3, etag: "e".into(), size: 1 };
    assert_eq!(part.completed(), crate::storage::CompletedPart { part_number: 3, etag: "e".into() });
}
