use super::*;

const MIB: u64 = 1024 * 1024;
const VIDEO: Rules = Rules { max_bytes: 200 * 1024 * MIB * 1024, content_types: &["video/mp4"] };

#[test]
fn files_are_cut_in_parts_of_10_mib_or_larger_for_huge_files() {
    assert_eq!(part_layout("video", 10 * MIB, MAX_WORKER_PART).unwrap(), (PART_SIZE, 1));
    assert_eq!(part_layout("video", 10 * MIB + 1, MAX_WORKER_PART).unwrap(), (PART_SIZE, 2));
    // 5 GB: 500 parts of 10 MiB.
    assert_eq!(part_layout("video", 5 * 1024 * MIB, MAX_WORKER_PART).unwrap(), (PART_SIZE, 512));
    // 200 GiB would need 20,480 parts of 10 MiB: 21 MiB parts instead, under 10,000 of them.
    let (size, count) = part_layout("video", 200 * 1024 * MIB, MAX_PART).unwrap();
    assert_eq!((size, count), (21 * MIB, 9753));
    let err = part_layout("video", 2000 * 1024 * MIB, MAX_WORKER_PART).unwrap_err();
    assert_eq!(err.to_string(), "invalid: Video is too large to upload in parts of at most 95 MB");
}

#[test]
fn the_declared_file_is_checked_first() {
    assert_eq!(check_multipart("video", 3 * MIB, "video/mp4", &VIDEO, MAX_PART).unwrap(), (PART_SIZE, 1));
    let err = check_multipart("video", 3 * MIB, "image/png", &VIDEO, MAX_PART).unwrap_err();
    assert!(err.to_string().contains("unsupported type"), "{err}");
}

#[test]
fn parts_are_presigned_puts_numbered_1_to_10000() {
    let r2 = S3Endpoint::r2("acc", "bucket", "AKID", "secret");
    let urls = presign_parts(&r2, "uploads/v", "up+1", &[1, 10_000], 1_790_000_000, 3600).unwrap().urls;
    assert_eq!(urls.len(), 2);
    assert!(urls[&1].starts_with("https://acc.r2.cloudflarestorage.com/bucket/uploads/v?"), "{}", urls[&1]);
    assert!(urls[&10_000].contains("partNumber=10000&uploadId=up%2B1"), "{}", urls[&10_000]);
    for (parts, message) in [
        (vec![0], "part 0 is not between 1 and 10000"),
        (vec![10_001], "part 10001 is not between 1 and 10000"),
        (vec![1; 1001], "ask for at most 1000 parts at once"),
    ] {
        let err = presign_parts(&r2, "k", "u", &parts, 1_790_000_000, 3600).unwrap_err();
        assert_eq!(err.to_string(), format!("bad request: {message}"));
    }
}
