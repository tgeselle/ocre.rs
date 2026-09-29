use super::*;

#[test]
fn tokens_are_random_url_safe_and_256_bits() {
    let (a, b) = (generate(), generate());
    assert_ne!(a, b);
    assert_eq!(a.len(), 43);
    assert!(a.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'), "{a}");
    assert_eq!(URL_SAFE_NO_PAD.decode(&a).unwrap().len(), TOKEN_BYTES);
}

#[test]
fn digest_is_sha256_hex() {
    assert_eq!(digest(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    let token = generate();
    assert_eq!(digest(&token), digest(&token));
    assert_ne!(digest(&token), digest(&generate()));
}

#[test]
fn constant_time_eq_compares_whole_values() {
    assert!(constant_time_eq(b"", b""));
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"xbc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abcd"));
}
