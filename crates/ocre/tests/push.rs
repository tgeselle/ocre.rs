use p256::ecdsa::{VerifyingKey, signature::Verifier as _};

use super::*;

fn b64(text: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(text).unwrap()
}

/// RFC 8291, Appendix A.
#[test]
fn encryption_matches_the_rfc_8291_example() {
    let keys = SubscriptionKeys {
        p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4".into(),
        auth: "BTBZMqHH6r4Tts7J_aSIgg".into(),
    };
    let sender = SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
    let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
    let body = encrypt_with(&keys, b"When I grow up, I want to be a watermelon", &sender, salt).unwrap();
    assert_eq!(
        URL_SAFE_NO_PAD.encode(body),
        "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
    );
    // A random key and salt each time.
    assert_ne!(encrypt(&keys, b"hi").unwrap(), encrypt(&keys, b"hi").unwrap());
}

#[test]
fn bad_keys_and_long_messages_are_refused() {
    let keys = SubscriptionKeys { p256dh: "BAAA".into(), auth: "BTBZMqHH6r4Tts7J_aSIgg".into() };
    for (keys, payload) in [
        (keys.clone(), b"hi".to_vec()),
        (SubscriptionKeys { auth: "!".into(), ..keys.clone() }, b"hi".to_vec()),
        (SubscriptionKeys { p256dh: "!!".into(), ..keys.clone() }, b"hi".to_vec()),
        (keys, vec![0; MAX_PAYLOAD + 1]),
    ] {
        assert!(matches!(encrypt(&keys, &payload), Err(Error::BadRequest(_))));
    }
}

#[test]
fn vapid_tokens_are_es256_jwts_for_the_push_service_origin() {
    let keys = VapidKeys::generate();
    let header = vapid_authorization("https://fcm.googleapis.com/fcm/send/abc", "mailto:a@b.c", &keys, 1000).unwrap();
    let token = header.strip_prefix("vapid t=").unwrap().split(", k=").next().unwrap();
    let parts: Vec<&str> = token.split('.').collect();
    let claims: serde_json::Value = serde_json::from_slice(&b64(parts[1])).unwrap();
    assert_eq!(
        claims,
        serde_json::json!({"aud": "https://fcm.googleapis.com", "exp": 1000 + 12 * 3600, "sub": "mailto:a@b.c"})
    );
    let verifying = VerifyingKey::from_sec1_bytes(&b64(&keys.public_key)).unwrap();
    let signature = Signature::from_slice(&b64(parts[2])).unwrap();
    assert!(verifying.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature).is_ok());

    assert!(vapid_authorization("ftp://push/x", "mailto:a@b.c", &keys, 1000).is_err());
    assert!(vapid_authorization("http://localhost:8788/fake/1", "mailto:a@b.c", &keys, 1000).unwrap().contains(", k="));
    for private_key in ["short".to_owned(), URL_SAFE_NO_PAD.encode([0; 32])] {
        let broken = VapidKeys { private_key, ..keys.clone() };
        assert!(vapid_authorization("https://x/y", "mailto:a@b.c", &broken, 1000).is_err());
    }
    assert_eq!(message("T", "B", "/p")["title"], "T");
}

#[test]
fn debug_output_hides_the_private_key() {
    let keys = VapidKeys::generate();
    let debug = format!("{keys:?}");
    assert!(debug.contains(&keys.public_key) && !debug.contains(&keys.private_key) && debug.contains("[redacted]"));
}
