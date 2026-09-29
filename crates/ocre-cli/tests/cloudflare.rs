use super::*;

fn session(ids: &[&str]) -> Session {
    Session {
        authenticated: true,
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
fn parses_cf_whoami_json() {
    let json = r#"{"authenticated":true,"authSource":"oauth","tokenValid":true,"email":"x@y.z","accounts":[{"id":"1","name":"One","type":"standard","settings":{},"created_on":"2020-01-01"}]}"#;
    let parsed: Session = serde_json::from_str(json).unwrap();
    assert_eq!(
        parsed,
        Session {
            authenticated: true,
            email: Some("x@y.z".into()),
            accounts: vec![Account { id: "1".into(), name: "One".into() }]
        }
    );
    let logged_out: Session = serde_json::from_str(r#"{"authenticated":false,"error":"Not logged in"}"#).unwrap();
    assert!(!logged_out.authenticated);
}

#[test]
fn reads_the_api_code_of_cf_error_boxes() {
    let stderr =
        "🍊☁️ cf · v1.0.0-beta.5\n┌ APIError\n│ [10006] The specified bucket does not exist.\n│ 404 Not Found\n└\n";
    assert_eq!(api_code(stderr), Some(10006));
    assert_eq!(error_box(stderr), "┌ APIError\n│ [10006] The specified bucket does not exist.\n│ 404 Not Found\n└");
    assert_eq!(api_code("┌ Error\n│ [not a code] and [42x]\n└"), None);
    assert_eq!(error_box("  plain failure \n"), "plain failure");
}

#[test]
fn pending_migrations_come_from_the_wrangler_table() {
    let output = "\n ⛅️ wrangler 4.144.0\n────────\nResource location: local \n\nMigrations to be applied:\n\
                  ┌───────────────────┐\n│ Name              │\n├───────────────────┤\n\
                  │ 0001_create_t.sql │\n├───────────────────┤\n│ 0002_x.sql        │\n└───────────────────┘\n";
    assert_eq!(pending_migrations(output), ["0001_create_t.sql", "0002_x.sql"]);
    assert!(pending_migrations("✅ No migrations to apply!\n").is_empty());
}
