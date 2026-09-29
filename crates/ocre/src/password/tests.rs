use super::*;
use crate::test_util::block_on;

#[test]
fn derive_matches_the_pbkdf2_hmac_sha256_test_vectors() {
    // RFC 7914 section 11 and the widely published "password"/"salt" vectors.
    let hex = |bytes: Vec<u8>| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    assert_eq!(
        hex(block_on(derive("password", b"salt", 1)).unwrap()),
        "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
    );
    assert_eq!(
        hex(block_on(derive("password", b"salt", 4096)).unwrap()),
        "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
    );
}

#[test]
fn hash_is_self_describing_and_salted() {
    let digest = block_on(hash("correct horse")).unwrap();
    let parts: Vec<&str> = digest.split('$').collect();
    assert_eq!(parts[..2], ["pbkdf2_sha256", "100000"]);
    assert_eq!(STANDARD_NO_PAD.decode(parts[2]).unwrap().len(), SALT_BYTES);
    assert_eq!(STANDARD_NO_PAD.decode(parts[3]).unwrap().len(), HASH_BYTES);
    assert_eq!(iterations(&digest), Some(ITERATIONS));
    assert!(block_on(verify("correct horse", &digest)).unwrap());
    assert!(!block_on(verify("correct horsE", &digest)).unwrap());
    assert!(!block_on(verify("", &digest)).unwrap());
}

#[test]
fn same_password_gets_a_new_salt_each_time() {
    let (a, b) = (block_on(hash_with("secret123", 2)).unwrap(), block_on(hash_with("secret123", 2)).unwrap());
    assert_ne!(a, b);
    assert!(block_on(verify("secret123", &a)).unwrap() && block_on(verify("secret123", &b)).unwrap());
}

#[test]
fn verify_uses_the_stored_iteration_count() {
    // Made with 1 iteration: still verifies after ITERATIONS changes.
    let digest = format!(
        "pbkdf2_sha256$1${}${}",
        STANDARD_NO_PAD.encode(b"salt"),
        STANDARD_NO_PAD.encode(block_on(derive("password", b"salt", 1)).unwrap())
    );
    assert_eq!(iterations(&digest), Some(1));
    assert!(block_on(verify("password", &digest)).unwrap());
    assert!(!block_on(verify("password", &digest.replace("$1$", "$2$"))).unwrap());
}

#[test]
fn verify_rejects_digests_in_another_format() {
    for digest in [
        "",
        "plaintext",
        "bcrypt$10$c2FsdA$aGFzaA",
        "pbkdf2_sha256",
        "pbkdf2_sha256$many$c2FsdA$aGFzaA",
        "pbkdf2_sha256$0$c2FsdA$aGFzaA",
        "pbkdf2_sha256$1",
        "pbkdf2_sha256$1$!!$aGFzaA",
        "pbkdf2_sha256$1$c2FsdA",
        "pbkdf2_sha256$1$c2FsdA$!!",
        "pbkdf2_sha256$1$c2FsdA$aGFzaA$extra",
        "pbkdf2_sha256$1$$aGFzaA",
        "pbkdf2_sha256$1$c2FsdA$",
    ] {
        assert_eq!(iterations(digest), None, "{digest}");
        let err = block_on(verify("password", digest)).unwrap_err();
        assert!(!err.to_string().contains("password\""), "no password in errors");
        assert!(matches!(err, Error::Internal(_)), "{digest}");
    }
}

#[test]
fn verifies_digests_made_by_other_pbkdf2_implementations() {
    // Python's hashlib.pbkdf2_hmac; the e2e test checks WebCrypto accepts it too.
    let digest = "pbkdf2_sha256$100000$b2NyZS1lMmUtc2FsdC0xNg$+YK85drz1qhfp9cYahe/+p4WE+lEWkjCL7PbkzXNN+A";
    assert!(block_on(verify("native digest", digest)).unwrap());
    assert!(!block_on(verify("native digesT", digest)).unwrap());
}
