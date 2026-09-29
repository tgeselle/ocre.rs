use super::*;

fn secret(c: char) -> String {
    c.to_string().repeat(64)
}

fn encryptor() -> Encryptor {
    Encryptor::new(&secret('k'), &[]).unwrap()
}

/// Runs `f` on a fresh thread: nothing installed there.
fn on_fresh_thread<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| scope.spawn(f).join().unwrap())
}

#[test]
fn random_and_deterministic_encryption_round_trip() {
    let e = encryptor();
    let random = e.encrypt("123-45-6789");
    assert!(is_encrypted(&random) && random.len() > "v1:".len() + 16);
    assert_ne!(random, e.encrypt("123-45-6789"));
    assert_eq!(e.decrypt(&random).unwrap(), "123-45-6789");
    let fixed = e.encrypt_deterministic("ada@example.com");
    assert_eq!(fixed, e.encrypt_deterministic("ada@example.com"));
    assert_ne!(fixed, e.encrypt_deterministic("Ada@example.com"));
    assert_eq!(e.decrypt(&fixed).unwrap(), "ada@example.com");
    // Keys come from the secret: another secret gives other texts.
    assert_ne!(fixed, Encryptor::new(&secret('z'), &[]).unwrap().encrypt_deterministic("ada@example.com"));
}

#[test]
fn previous_keys_decrypt_and_give_lookup_candidates() {
    let old = Encryptor::new(&secret('o'), &[]).unwrap();
    let stored = old.encrypt("v");
    let looked_up = old.encrypt_deterministic("v");
    let rotated = Encryptor::new(&secret('n'), &[&secret('o')]).unwrap();
    assert_eq!(rotated.decrypt(&stored).unwrap(), "v");
    assert_eq!(rotated.deterministic_candidates("v"), [rotated.encrypt_deterministic("v"), looked_up]);
    assert_eq!(format!("{rotated:?}"), "Encryptor { previous_keys: 1, .. }");
}

#[test]
fn bad_values_and_secrets_fail_without_leaking() {
    let e = encryptor();
    let message = |text: &str| e.decrypt(text).unwrap_err().to_string();
    assert!(message("hello").ends_with("the value is not encrypted"));
    assert!(message("v1:***").ends_with("the value is not valid base64"));
    assert!(message("v1:AAAA").ends_with("the value is too short"));
    let other = Encryptor::new(&secret('x'), &[]).unwrap().encrypt("hidden");
    let wrong = message(&other);
    assert!(wrong.contains("wrong key or changed value") && !wrong.contains("hidden"), "{wrong}");
    let mut changed = e.encrypt("hidden");
    changed.push('A');
    assert!(e.decrypt(&changed).is_err());
    assert_eq!(e.decrypt_or_plaintext("plain").unwrap(), "plain");
    assert_eq!(e.decrypt_or_plaintext(&e.encrypt("x")).unwrap(), "x");
    assert!(Encryptor::new("short", &[]).is_err());
    assert!(Encryptor::new(&secret('k'), &["short"]).is_err());
}

#[test]
fn installed_keys_serve_column_types() {
    on_fresh_thread(|| {
        assert!(installed().unwrap_err().to_string().contains("SECRET_KEY_BASE is missing"));
        // Without keys, binding stores NULL rather than the plain value.
        assert_eq!(Encrypted::from("x").into_param(), None::<String>.into_param());
        assert_eq!(Deterministic::from("x").into_param(), None::<String>.into_param());
        let stored = encryptor().encrypt("x");
        assert!(serde_json::from_value::<Encrypted>(serde_json::json!(stored)).is_err());

        install(encryptor());
        let ssn: Encrypted = serde_json::from_value(serde_json::json!(stored)).unwrap();
        assert_eq!(
            (ssn.as_str(), ssn.to_string(), format!("{ssn:?}")),
            ("x", "x".to_owned(), "Encrypted(..)".to_owned())
        );
        assert_eq!(serde_json::to_string(&ssn).unwrap(), "\"x\"");
        assert!(serde_json::from_value::<Encrypted>(serde_json::json!("x")).is_err());
        assert!(serde_json::from_value::<Encrypted>(serde_json::json!(1)).is_err());
        let Param(crate::sql::Value::Text(text)) = Encrypted::from(String::from("y")).into_param() else {
            panic!("expected text")
        };
        assert_eq!(installed().unwrap().decrypt(&text).unwrap(), "y");

        let email: Deterministic = serde_json::from_value(serde_json::json!(encryptor().encrypt("a@b.co"))).unwrap();
        assert_eq!(
            (email.as_str(), email.to_string(), format!("{email:?}")),
            ("a@b.co", "a@b.co".to_owned(), "Deterministic(..)".to_owned())
        );
        assert_eq!(serde_json::to_string(&email).unwrap(), "\"a@b.co\"");
        assert_eq!(
            Deterministic::from(String::from("a@b.co")).into_param(),
            encryptor().encrypt_deterministic("a@b.co").into_param()
        );
    });
}

#[test]
fn keys_are_installed_once_from_the_worker_secrets() {
    on_fresh_thread(|| {
        ensure_installed(&|_| None);
        assert!(installed().is_err());
        let bad_previous =
            |name: &str| Some(if name == session::SECRET_KEY_BASE { secret('k') } else { "short".into() });
        ensure_installed(&bad_previous);
        assert!(installed().is_err());
        let secrets = |name: &str| (name == session::SECRET_KEY_BASE).then(|| secret('k'));
        ensure_installed(&secrets);
        let stored = installed().unwrap().encrypt_deterministic("v");
        assert_eq!(stored, encryptor().encrypt_deterministic("v"));
        // Already installed: later secrets are not read.
        ensure_installed(&|_| panic!("read again"));
    });
}
