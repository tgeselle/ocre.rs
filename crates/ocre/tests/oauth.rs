use super::*;

#[test]
fn providers_are_found_by_name_only() {
    assert_eq!(provider("github"), Some(&GITHUB));
    assert_eq!(provider("google"), Some(&GOOGLE));
    assert_eq!(provider("GitHub"), None);
    assert_eq!(provider(""), None);
}

#[test]
fn pkce_pairs_are_random_and_match_rfc_7636() {
    // RFC 7636, appendix B.
    assert_eq!(
        Pkce::challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    let pkce = Pkce::new();
    assert_eq!(pkce.challenge, Pkce::challenge_for(&pkce.verifier));
    // 43 characters is the minimum verifier length RFC 7636 allows.
    assert_eq!(pkce.verifier.len(), 43);
    assert_ne!(pkce, Pkce::new());
}

#[test]
fn authorize_urls_encode_every_parameter() {
    let url = authorize_url(&GITHUB, "id&x", "https://a.example/cb?next=/", "s t", "ch");
    assert_eq!(
        url,
        "https://github.com/login/oauth/authorize?response_type=code&client_id=id%26x\
         &redirect_uri=https%3A%2F%2Fa.example%2Fcb%3Fnext%3D%2F&scope=read%3Auser+user%3Aemail\
         &state=s+t&code_challenge=ch&code_challenge_method=S256"
    );
}

#[test]
fn token_requests_and_responses() {
    assert_eq!(
        token_request_body("id", "s&cret", "https://a.example/cb", "c", "v"),
        "grant_type=authorization_code&client_id=id&client_secret=s%26cret&redirect_uri=https%3A%2F%2Fa.example%2Fcb&code=c&code_verifier=v"
    );
    assert_eq!(parse_token_response(&GITHUB, 200, r#"{"access_token":"gho_1","scope":"x"}"#).unwrap(), "gho_1");
    // GitHub answers 200 with an error body for a bad code.
    let refused = r#"{"error":"bad_verification_code","error_description":"The code is incorrect."}"#;
    assert!(matches!(parse_token_response(&GITHUB, 200, refused), Err(Error::Unauthorized)));
    assert!(matches!(parse_token_response(&GOOGLE, 400, r#"{"error":"invalid_grant"}"#), Err(Error::Unauthorized)));
    assert!(matches!(parse_token_response(&GOOGLE, 500, r#"{"access_token":"x"}"#), Err(Error::Unauthorized)));
    assert!(matches!(parse_token_response(&GOOGLE, 502, "{}"), Err(Error::Unauthorized)));
    assert!(matches!(parse_token_response(&GOOGLE, 502, "<html>"), Err(Error::Internal(m)) if m.contains("502")));
}

#[test]
fn github_profiles_use_the_primary_verified_email() {
    let user = r#"{"id":583231,"login":"octocat","name":null}"#;
    let emails = r#"[{"email":"old@x.dev","primary":false,"verified":true},{"email":"Octo@GitHub.com","primary":true,"verified":true}]"#;
    let profile = parse_profile(&GITHUB, user, Some(emails)).unwrap();
    assert_eq!(
        profile,
        Profile {
            provider: "github",
            uid: "583231".into(),
            email: Some("octo@github.com".into()),
            name: Some("octocat".into()),
        }
    );
    let unverified = r#"[{"email":"a@x.dev","primary":true,"verified":false}]"#;
    assert_eq!(parse_profile(&GITHUB, user, Some(unverified)).unwrap().email, None);
    assert_eq!(parse_profile(&GITHUB, r#"{"id":1,"login":"l","name":"N"}"#, None).unwrap().name.as_deref(), Some("N"));
    assert!(matches!(parse_profile(&GITHUB, "{}", None), Err(Error::Internal(m)) if m.contains("github")));
}

#[test]
fn google_profiles_need_a_verified_email() {
    let user = r#"{"sub":"1099","email":"Ada@Example.com","email_verified":true,"name":"Ada"}"#;
    let profile = parse_profile(&GOOGLE, user, None).unwrap();
    assert_eq!((profile.uid.as_str(), profile.email.as_deref()), ("1099", Some("ada@example.com")));
    let unverified = r#"{"sub":"1","email":"a@example.com"}"#;
    assert_eq!(parse_profile(&GOOGLE, unverified, None).unwrap().email, None);
}
