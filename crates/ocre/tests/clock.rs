use super::*;

#[test]
fn now_is_unix_seconds() {
    let now = now();
    // 2026-01-01 and 2100-01-01: seconds, not milliseconds.
    assert!((1_767_225_600..4_102_444_800).contains(&now), "{now}");
}
