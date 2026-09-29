use super::*;

fn session(ids: &[&str]) -> Session {
    Session {
        logged_in: true,
        email: Some("a@b.c".into()),
        accounts: ids.iter().map(|id| Account { id: (*id).into(), name: format!("{id} name") }).collect(),
    }
}

#[test]
fn single_account_needs_no_account_id() {
    assert_eq!(pick_account(&session(&["a1"]), None).unwrap(), None);
    assert_eq!(pick_account(&session(&["a1"]), Some("a1")).unwrap(), Some("a1".into()));
}

#[test]
fn several_accounts_require_a_listed_choice() {
    let s = session(&["a1", "a2"]);
    assert_eq!(pick_account(&s, Some("a2")).unwrap(), Some("a2".into()));
    let err = pick_account(&s, None).unwrap_err();
    assert_eq!(err.hint.unwrap(), "pass --account-id with one of: a1 (a1 name), a2 (a2 name)");
    let err = pick_account(&s, Some("zz")).unwrap_err();
    assert!(err.message.contains("`zz`"));
}

#[test]
fn no_accounts_is_an_error() {
    assert!(pick_account(&session(&[]), None).is_err());
}

#[test]
fn parses_whoami_json() {
    let json = r#"{"loggedIn":true,"authType":"OAuth Token","email":"x@y.z","accounts":[{"id":"1","name":"One","type":"standard"}]}"#;
    let parsed: Session = serde_json::from_str(json).unwrap();
    assert_eq!(
        parsed,
        Session {
            logged_in: true,
            email: Some("x@y.z".into()),
            accounts: vec![Account { id: "1".into(), name: "One".into() }]
        }
    );
}
